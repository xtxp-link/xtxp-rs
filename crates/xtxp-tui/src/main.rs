use clap::Parser;
use xtxp_muppet::adapters::ysweet;

/// Edit a y-sweet document interactively in a terminal UI.
#[derive(Debug, Parser)]
#[command(name = "xtxp-tui", version)]
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

fn main() {
    let args = Args::parse();
    if let Err(e) = ysweet::check_doc(&args.doc) {
        eprintln!("xtxp-tui: {e}");
        std::process::exit(1);
    }
    if let Err(e) = xtxp_tui::run(&args.server, &args.doc, args.read_only) {
        eprintln!("xtxp-tui: {e}");
        std::process::exit(1);
    }
}
