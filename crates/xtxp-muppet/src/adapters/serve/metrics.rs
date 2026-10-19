//! Counters for `/metrics`.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// Client samples kept per mode and grain.
const CLIENT_SAMPLES: usize = 10_000;

/// A batch from `metrics.js`.
#[derive(Debug, Deserialize)]
pub struct ClientBatch {
    pub mode: String,
    pub grain: String,
    pub samples: Vec<ClientSample>,
    #[serde(default)]
    pub jank: u64,
}

#[derive(Debug, Deserialize)]
pub struct ClientSample {
    #[serde(default)]
    pub seq: u64,
    pub latency_ms: f64,
    #[serde(default)]
    pub vt_ms: Option<f64>,
}

#[derive(Default)]
struct ClientAgg {
    latency_ms: Vec<u64>,
    vt_ms: Vec<u64>,
    jank: u64,
}

fn push_bounded(v: &mut Vec<u64>, x: u64) {
    if v.len() == CLIENT_SAMPLES {
        v.remove(0);
    }
    v.push(x);
}

/// The nearest-rank percentile `p` (0 to 100) of `values`, or `None` if empty.
pub fn percentile(values: &[u64], p: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    Some(sorted[rank.min(sorted.len()) - 1])
}

fn spread(values: &[u64]) -> Value {
    json!({ "p50": percentile(values, 50.0), "p95": percentile(values, 95.0) })
}

/// Peak resident memory in KB and CPU time in ms, for this process.
fn process_stats() -> (u64, u64) {
    // SAFETY: getrusage only writes the struct we pass it.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return (0, 0);
    }
    let maxrss = usage.ru_maxrss as u64;
    // macOS reports bytes, Linux kilobytes.
    let kb = if cfg!(target_os = "macos") {
        maxrss / 1024
    } else {
        maxrss
    };
    let ms = |t: libc::timeval| t.tv_sec as u64 * 1000 + t.tv_usec as u64 / 1000;
    (kb, ms(usage.ru_utime) + ms(usage.ru_stime))
}

/// A delivery mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Refresh,
    Stream,
    Ws,
}

#[derive(Default)]
struct Counters {
    open: AtomicU64,
    accepted: AtomicU64,
    patch_sets: AtomicU64,
    bytes: AtomicU64,
}

impl Counters {
    fn stats(&self) -> ModeStats {
        ModeStats {
            open: self.open.load(Relaxed),
            accepted: self.accepted.load(Relaxed),
            patch_sets: self.patch_sets.load(Relaxed),
            bytes: self.bytes.load(Relaxed),
        }
    }
}

#[derive(Default)]
struct Inner {
    refresh_ok: AtomicU64,
    refresh_not_modified: AtomicU64,
    modes: [Counters; 3],
    resyncs: AtomicU64,
    client: Mutex<BTreeMap<String, ClientAgg>>,
}

fn index(mode: Mode) -> usize {
    match mode {
        Mode::Refresh => 0,
        Mode::Stream => 1,
        Mode::Ws => 2,
    }
}

/// Counts a connection as open until it's dropped.
pub struct ConnGuard {
    metrics: Metrics,
    mode: Mode,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.metrics.0.modes[index(self.mode)]
            .open
            .fetch_sub(1, Relaxed);
    }
}

/// The server's counters. Cheap to clone.
#[derive(Clone, Default)]
pub struct Metrics(Arc<Inner>);

/// The counters at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetricsSnapshot {
    pub refresh_ok: u64,
    pub refresh_not_modified: u64,
    pub refresh: ModeStats,
    pub stream: ModeStats,
    pub ws: ModeStats,
    pub resyncs: u64,
}

/// One delivery mode's counters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModeStats {
    pub open: u64,
    pub accepted: u64,
    pub patch_sets: u64,
    pub bytes: u64,
}

impl Metrics {
    pub fn new() -> Metrics {
        Metrics::default()
    }

    /// A refresh-mode response: 200 or 304.
    pub fn refresh_status(&self, code: u16) {
        match code {
            304 => self.0.refresh_not_modified.fetch_add(1, Relaxed),
            _ => self.0.refresh_ok.fetch_add(1, Relaxed),
        };
    }

    /// A new connection in `mode`, open until the guard is dropped.
    pub fn opened(&self, mode: Mode) -> ConnGuard {
        let c = &self.0.modes[index(mode)];
        c.open.fetch_add(1, Relaxed);
        c.accepted.fetch_add(1, Relaxed);
        ConnGuard {
            metrics: self.clone(),
            mode,
        }
    }

    /// Bytes sent in `mode`; `patch_set` when they carry one.
    pub fn sent(&self, mode: Mode, bytes: usize, patch_set: bool) {
        let c = &self.0.modes[index(mode)];
        c.bytes.fetch_add(bytes as u64, Relaxed);
        if patch_set {
            c.patch_sets.fetch_add(1, Relaxed);
        }
    }

    /// A viewer fell behind and got the whole document instead.
    pub fn resync(&self) {
        self.0.resyncs.fetch_add(1, Relaxed);
    }

    /// Adds a batch of browser samples.
    pub fn client_batch(&self, batch: ClientBatch) {
        let mut client = self.0.client.lock().expect("metrics lock");
        let agg = client
            .entry(format!("{}/{}", batch.mode, batch.grain))
            .or_default();
        for s in batch.samples {
            push_bounded(&mut agg.latency_ms, s.latency_ms.max(0.0).round() as u64);
            if let Some(vt) = s.vt_ms {
                push_bounded(&mut agg.vt_ms, vt.max(0.0).round() as u64);
            }
        }
        agg.jank += batch.jank;
    }

    /// Everything, as `/metrics` returns it.
    pub fn to_json(&self, render_us: &[u64]) -> Value {
        let m = self.snapshot();
        let mode = |s: &ModeStats| json!({ "open": s.open, "accepted": s.accepted, "patch_sets": s.patch_sets, "bytes": s.bytes });
        let client: serde_json::Map<String, Value> = self
            .0
            .client
            .lock()
            .expect("metrics lock")
            .iter()
            .map(|(k, a)| {
                (
                    k.clone(),
                    json!({
                        "samples": a.latency_ms.len(),
                        "latency_ms": spread(&a.latency_ms),
                        "vt_ms": spread(&a.vt_ms),
                        "jank": a.jank,
                    }),
                )
            })
            .collect();
        let (max_rss_kb, cpu_ms) = process_stats();
        json!({
            "modes": { "refresh": mode(&m.refresh), "stream": mode(&m.stream), "ws": mode(&m.ws) },
            "refresh": { "ok": m.refresh_ok, "not_modified": m.refresh_not_modified },
            "render_us": spread(render_us),
            "resyncs": m.resyncs,
            "process": { "max_rss_kb": max_rss_kb, "cpu_ms": cpu_ms },
            "client": client,
        })
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            refresh_ok: self.0.refresh_ok.load(Relaxed),
            refresh_not_modified: self.0.refresh_not_modified.load(Relaxed),
            refresh: self.0.modes[0].stats(),
            stream: self.0.modes[1].stats(),
            ws: self.0.modes[2].stats(),
            resyncs: self.0.resyncs.load(Relaxed),
        }
    }
}
