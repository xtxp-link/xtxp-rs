use clap::Parser;
use std::net::SocketAddr;
use xtxp_muppet::adapters::ysweet;

/// Serve a y-sweet document to browsers in three delivery modes.
#[derive(Debug, Parser)]
#[command(name = "xtxp-serve", version)]
struct Args {
    /// Document name
    doc: String,
    /// y-sweet's address
    #[arg(long, default_value = "localhost:8080")]
    server: String,
    /// Address to listen on
    #[arg(long, default_value = "127.0.0.1:3000")]
    listen: SocketAddr,
}

fn main() {
    let args = Args::parse();
    if let Err(e) = ysweet::check_doc(&args.doc) {
        eprintln!("xtxp-serve: {e}");
        std::process::exit(1);
    }
    if let Err(e) = xtxp_serve::run(&args.server, &args.doc, args.listen) {
        eprintln!("xtxp-serve: {e}");
        std::process::exit(1);
    }
}
