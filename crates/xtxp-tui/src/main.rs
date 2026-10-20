use clap::Parser;
use std::process::ExitCode;
use xtxp_muppet::adapters::ysweet;

/// Edit a y-sweet document interactively in a terminal UI.
#[derive(Debug, Parser)]
#[command(name = "xtxp-tui", version, long_version = concat!(env!("CARGO_PKG_VERSION"), " (Rust implementation)"))]
struct Args {
    /// Document name
    doc: String,
    /// y-sweet's address
    #[arg(long, default_value = "localhost:8080")]
    server: String,
    /// Watch without editing (a viewer, not access control)
    #[arg(long)]
    read_only: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let result = ysweet::check_doc(&args.doc)
        .and_then(|()| xtxp_tui::run(&args.server, &args.doc, args.read_only));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtxp-tui: {e}");
            ExitCode::FAILURE
        }
    }
}
