//! Counters for `/metrics`.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

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
