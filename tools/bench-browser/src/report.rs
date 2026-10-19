//! One run's results, as a CSV row and a Markdown table.

use std::fmt::Write;

/// The results of one run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub mode: String,
    pub grain: String,
    pub viewers: usize,
    pub rate: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub bytes_per_viewer_s: f64,
    pub server_cpu_pct: f64,
    pub server_rss_kb: u64,
    pub errors: usize,
    pub missed: usize,
}

/// The nearest-rank percentile `p` (0 to 100) of `values`; 0 if empty.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

pub const CSV_HEADER: &str = "mode,grain,viewers,rate,p50_ms,p95_ms,p99_ms,bytes_per_viewer_s,server_cpu_pct,server_rss_kb,errors,missed";

impl Row {
    pub fn csv(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            self.mode,
            self.grain,
            self.viewers,
            self.rate,
            round(self.p50_ms),
            round(self.p95_ms),
            round(self.p99_ms),
            round(self.bytes_per_viewer_s),
            round(self.server_cpu_pct),
            self.server_rss_kb,
            self.errors,
            self.missed
        )
    }
}

/// A Markdown table of rows.
pub fn markdown(rows: &[Row]) -> String {
    let mut out = String::from(
        "| mode | grain | viewers | edits/s | p50 ms | p95 ms | p99 ms | bytes/viewer/s | server CPU % | server max RSS KB | errors | missed |\n\
         | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for r in rows {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.mode,
            r.grain,
            r.viewers,
            r.rate,
            round(r.p50_ms),
            round(r.p95_ms),
            round(r.p99_ms),
            round(r.bytes_per_viewer_s),
            round(r.server_cpu_pct),
            r.server_rss_kb,
            r.errors,
            r.missed
        );
    }
    out
}

/// One decimal place.
fn round(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_percentiles() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), 50.0);
        assert_eq!(percentile(&v, 95.0), 95.0);
        assert_eq!(percentile(&v, 99.0), 99.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn csv_and_markdown() {
        let row = Row {
            mode: "ws".into(),
            grain: "node".into(),
            viewers: 10,
            rate: 5.0,
            p50_ms: 1.5,
            missed: 2,
            ..Row::default()
        };
        assert_eq!(CSV_HEADER.split(',').count(), row.csv().split(',').count());
        assert!(row.csv().starts_with("ws,node,10,5,1.5,"), "{}", row.csv());
        let md = markdown(&[row]);
        assert!(md.starts_with("| mode | grain | viewers |"), "{md}");
        assert!(md.contains("| ws | node | 10 |"), "{md}");
    }
}
