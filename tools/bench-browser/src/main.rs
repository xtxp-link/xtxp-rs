//! `bench-browser`: one run against `xtxp-muppet serve`, appended to a CSV.

use bench_browser::report::{markdown, Row, CSV_HEADER};
use bench_browser::{run, Config, Mode};
use clap::{Parser, ValueEnum};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModeArg {
    Refresh,
    Stream,
    Ws,
}

/// Load for `xtxp-muppet serve`: N viewers in one mode, an editor on y-sweet.
#[derive(Debug, Parser)]
#[command(name = "bench-browser")]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:3000")]
    url: String,
    #[arg(long, value_enum, default_value = "ws")]
    mode: ModeArg,
    #[arg(long, default_value = "node")]
    grain: String,
    #[arg(long, default_value_t = 10)]
    viewers: usize,
    #[arg(long, default_value_t = 5.0)]
    edits_per_sec: f64,
    #[arg(long, default_value_t = 30)]
    seconds: u64,
    #[arg(long, default_value = "localhost:8080")]
    ysweet: String,
    #[arg(long, default_value = "logo")]
    doc: String,
    /// Appends the row to this CSV (writing the header first if it's new).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Prints a Markdown table of an existing CSV, and exits.
    #[arg(long)]
    table: Option<PathBuf>,
}

fn parse_row(line: &str) -> Option<Row> {
    let f: Vec<&str> = line.split(',').collect();
    Some(Row {
        mode: f.first()?.to_string(),
        grain: f.get(1)?.to_string(),
        viewers: f.get(2)?.parse().ok()?,
        rate: f.get(3)?.parse().ok()?,
        p50_ms: f.get(4)?.parse().ok()?,
        p95_ms: f.get(5)?.parse().ok()?,
        p99_ms: f.get(6)?.parse().ok()?,
        bytes_per_viewer_s: f.get(7)?.parse().ok()?,
        server_cpu_pct: f.get(8)?.parse().ok()?,
        server_rss_kb: f.get(9)?.parse().ok()?,
        errors: f.get(10)?.parse().ok()?,
        missed: f.get(11)?.parse().ok()?,
    })
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    if let Some(csv) = args.table {
        let text = std::fs::read_to_string(&csv).map_err(|e| format!("{}: {e}", csv.display()))?;
        let rows: Vec<Row> = text.lines().skip(1).filter_map(parse_row).collect();
        print!("{}", markdown(&rows));
        return Ok(());
    }
    let cfg = Config {
        url: args.url.trim_end_matches('/').to_string(),
        mode: match args.mode {
            ModeArg::Refresh => Mode::Refresh,
            ModeArg::Stream => Mode::Stream,
            ModeArg::Ws => Mode::Ws,
        },
        grain: args.grain,
        viewers: args.viewers,
        edits_per_sec: args.edits_per_sec,
        duration: Duration::from_secs(args.seconds),
        ysweet: args.ysweet,
        doc: args.doc,
    };
    let row = tokio::runtime::Runtime::new()
        .map_err(|e| e.to_string())?
        .block_on(run(&cfg))?;
    if let Some(path) = args.out {
        let new = !path.exists();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if new {
            writeln!(f, "{CSV_HEADER}").map_err(|e| e.to_string())?;
        }
        writeln!(f, "{}", row.csv()).map_err(|e| e.to_string())?;
    }
    print!("{}", markdown(&[row]));
    Ok(())
}
