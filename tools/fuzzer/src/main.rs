//! `xtxp-fuzzer <seed>` prints the generated vector as JSON.

fn main() {
    let seed = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            eprintln!("usage: xtxp-fuzzer <seed>");
            std::process::exit(2);
        });
    println!("{}", xtxp_fuzzer::generate(seed).to_json());
}
