use clap::Parser;
use std::net::SocketAddr;
use std::process::ExitCode;
use xtxp_muppet::adapters::ysweet;

/// Serve a y-sweet document to browsers in three delivery modes.
#[derive(Debug, Parser)]
#[command(name = "xtxp-serve", version, long_version = concat!(env!("CARGO_PKG_VERSION"), " (Rust implementation)"))]
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

fn main() -> ExitCode {
    let args = Args::parse();
    let result = ysweet::check_doc(&args.doc)
        .and_then(|()| xtxp_serve::run(&args.server, &args.doc, args.listen));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtxp-serve: {e}");
            ExitCode::FAILURE
        }
    }
}
