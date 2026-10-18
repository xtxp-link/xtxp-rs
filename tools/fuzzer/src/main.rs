//! `xtxp-fuzzer [--single-round] <seed>` prints the generated vector as JSON.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let single = args.iter().any(|a| a == "--single-round");
    let seed = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            eprintln!("usage: xtxp-fuzzer [--single-round] <seed>");
            std::process::exit(2);
        });
    let vector = if single {
        xtxp_fuzzer::generate_single_round(seed)
    } else {
        xtxp_fuzzer::generate(seed)
    };
    println!("{}", vector.to_json());
}
