//! `chaos-proxy --listen A --upstream B [--seed N] [--delay-ms D] [--dup P] [--drop Q]`

use chaos_proxy::{serve, Chaos};

fn arg<T: std::str::FromStr>(args: &[String], name: &str) -> Option<T> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(listen), Some(upstream)) = (arg(&args, "--listen"), arg(&args, "--upstream")) else {
        eprintln!("usage: chaos-proxy --listen HOST:PORT --upstream HOST:PORT [--seed N] [--delay-ms D] [--dup P] [--drop Q]");
        std::process::exit(2);
    };
    let chaos = Chaos {
        seed: arg(&args, "--seed").unwrap_or(0),
        delay_ms: arg(&args, "--delay-ms").unwrap_or(0),
        dup: arg(&args, "--dup").unwrap_or(0.0),
        drop: arg(&args, "--drop").unwrap_or(0.0),
    };
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = runtime.block_on(serve(listen, upstream, chaos)) {
        eprintln!("chaos-proxy: {e}");
        std::process::exit(1);
    }
}
