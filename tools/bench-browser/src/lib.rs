//! Load for `xtxp-muppet serve`: N lightweight viewers in one delivery mode,
//! and an editor on y-sweet changing the logo's cross at a fixed rate. Each
//! edit carries a number (`data-bench`), so a viewer knows which edit a patch
//! shows, and latency is measured on one clock, from the edit leaving the
//! editor to the patch reaching the viewer.

pub mod editor;
pub mod report;
pub mod viewers;

pub use report::Row;
use std::time::{Duration, Instant};

/// A delivery mode, as `?live=` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Refresh,
    Stream,
    Ws,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Refresh => "refresh",
            Mode::Stream => "stream",
            Mode::Ws => "ws",
        }
    }
}

/// One run.
#[derive(Clone, Debug)]
pub struct Config {
    /// `serve`'s base URL, such as `http://127.0.0.1:3000`.
    pub url: String,
    pub mode: Mode,
    /// `node` or `region`; ignored for refresh.
    pub grain: String,
    pub viewers: usize,
    pub edits_per_sec: f64,
    pub duration: Duration,
    /// y-sweet's address, for the editor.
    pub ysweet: String,
    pub doc: String,
}

/// Runs the viewers and the editor, and measures.
pub async fn run(cfg: &Config) -> Result<Row, String> {
    // After the last edit, viewers keep reading this long, for refresh's poll.
    let grace = Duration::from_secs(2);
    let before = server_stats(&cfg.url).await?;
    let wall = Instant::now();
    let until = wall + Duration::from_millis(300) + cfg.duration + grace;
    let viewers: Vec<_> = (0..cfg.viewers)
        .map(|_| {
            let (url, grain, mode) = (cfg.url.clone(), cfg.grain.clone(), cfg.mode);
            tokio::spawn(async move {
                match mode {
                    Mode::Refresh => viewers::refresh(&url, until).await,
                    Mode::Stream => viewers::stream(&url, &grain, until).await,
                    Mode::Ws => viewers::ws(&url, &grain, until).await,
                }
            })
        })
        .collect();
    // Let the viewers connect before the first edit.
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Edit numbers above any earlier run's, so a page from before can't count.
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64 * 1000)
        .unwrap_or(0);
    let sent = editor::run(&cfg.ysweet, &cfg.doc, base, cfg.edits_per_sec, cfg.duration).await?;
    let mut seen = Vec::new();
    for v in viewers {
        seen.push(v.await.map_err(|e| e.to_string())?);
    }
    let elapsed = wall.elapsed().as_secs_f64();
    let after = server_stats(&cfg.url).await?;

    let mut latencies = Vec::new();
    let mut missed = 0;
    for v in &seen {
        for (k, at) in &sent {
            match v.first_at_least(*k) {
                Some(when) => {
                    latencies.push(when.saturating_duration_since(*at).as_secs_f64() * 1000.0)
                }
                None => missed += 1,
            }
        }
    }
    let bytes: u64 = seen.iter().map(|v| v.bytes).sum();
    Ok(Row {
        mode: cfg.mode.name().into(),
        grain: if cfg.mode == Mode::Refresh {
            "page".into()
        } else {
            cfg.grain.clone()
        },
        viewers: cfg.viewers,
        rate: cfg.edits_per_sec,
        p50_ms: report::percentile(&latencies, 50.0),
        p95_ms: report::percentile(&latencies, 95.0),
        p99_ms: report::percentile(&latencies, 99.0),
        bytes_per_viewer_s: bytes as f64 / cfg.viewers.max(1) as f64 / elapsed,
        server_cpu_pct: (after.0.saturating_sub(before.0)) as f64 / (elapsed * 1000.0) * 100.0,
        server_rss_kb: after.1,
        errors: seen.iter().filter(|v| v.error.is_some()).count(),
        missed,
    })
}

/// The server's CPU time in ms and peak memory in KB, from `/metrics`.
async fn server_stats(url: &str) -> Result<(u64, u64), String> {
    let text = reqwest::get(format!("{url}/metrics"))
        .await
        .map_err(|e| format!("{url}/metrics: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let p = &json["process"];
    Ok((
        p["cpu_ms"].as_u64().unwrap_or(0),
        p["max_rss_kb"].as_u64().unwrap_or(0),
    ))
}
