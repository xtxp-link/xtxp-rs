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
struct Inner {
    refresh_ok: AtomicU64,
    refresh_not_modified: AtomicU64,
}

/// The server's counters. Cheap to clone.
#[derive(Clone, Default)]
pub struct Metrics(Arc<Inner>);

/// The counters at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetricsSnapshot {
    pub refresh_ok: u64,
    pub refresh_not_modified: u64,
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

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            refresh_ok: self.0.refresh_ok.load(Relaxed),
            refresh_not_modified: self.0.refresh_not_modified.load(Relaxed),
        }
    }
}
