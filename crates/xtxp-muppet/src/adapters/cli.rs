//! The one-shot commands (`init`, `edit`, `show`, `replay`) and the entry point.

use crate::adapters::{ws, ysweet};
use crate::core::Mode;
use crate::ops_words::parse_op_words;
use crate::ports::{Outcome, Screen, View};
use clap::{Parser, Subcommand};
use rand::{Rng, SeedableRng};
use std::path::PathBuf;
use xmesh::vector::Vector;
use xmesh::{IdNode, IdNodeKind, Op, Xmesh};

/// A local XTXP muppet for a stock y-sweet (M1-y-sweet).
#[derive(Debug, Parser)]
#[command(name = "xtxp-muppet", version)]
pub struct Cli {
    /// y-sweet's address.
    #[arg(long, global = true, default_value = "localhost:8080")]
    pub server: String,
    #[command(subcommand)]
    pub sub: Sub,
}

#[derive(Debug, Subcommand)]
pub enum Sub {
    /// Create a document and push a drawing into it as its seed.
    Init {
        file: PathBuf,
        /// Document name; defaults to the file's stem.
        #[arg(long)]
        doc: Option<String>,
    },
    /// Edit interactively in a terminal UI.
    Open {
        doc: String,
        /// Watch without editing (a viewer, not access control).
        #[arg(long)]
        read_only: bool,
    },
    /// Apply one batch of ops and exit once y-sweet has it. Separate ops with `,`.
    Edit {
        doc: String,
        #[arg(required = true, num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
        ops: Vec<String>,
    },
    /// Sync, print and exit.
    Show {
        doc: String,
        #[arg(long)]
        canonical: bool,
        #[arg(long)]
        tree: bool,
        #[arg(long)]
        hashes: bool,
    },
    /// Replay one peer of a single-round vector (for tests).
    Replay {
        vector: PathBuf,
        #[arg(long)]
        peer: String,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Document name; defaults to the vector's name.
        #[arg(long)]
        doc: Option<String>,
    },
}

/// Splits op words on `,` and parses each op.
pub fn parse_edit_ops(words: &[String]) -> Result<Vec<Op>, String> {
    words
        .split(|w| w == ",")
        .filter(|op| !op.is_empty())
        .map(parse_op_words)
        .collect::<Result<Vec<_>, _>>()
        .and_then(|ops| {
            if ops.is_empty() {
                Err("no ops given".into())
            } else {
                Ok(ops)
            }
        })
}

/// One line per node: its path and a short description.
pub fn tree_lines(tree: &[IdNode]) -> Vec<String> {
    fn walk(n: &IdNode, path: &str, depth: usize, out: &mut Vec<String>) {
        let what = match &n.node {
            IdNodeKind::Text(t) => format!("{t:?}"),
            IdNodeKind::Element { name, attrs, .. } => {
                let (_, local) = xmesh::names::split(name);
                let id = attrs
                    .iter()
                    .find(|(k, _)| k == "id")
                    .map(|(_, v)| format!(" id={v:?}"))
                    .unwrap_or_default();
                format!("<{local}{id}>")
            }
        };
        out.push(format!("{path:<10} {}{what}", "  ".repeat(depth)));
        if let IdNodeKind::Element { children, .. } = &n.node {
            for (i, c) in children.iter().enumerate() {
                let p = if path == "/" {
                    i.to_string()
                } else {
                    format!("{path}/{i}")
                };
                walk(c, &p, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for n in tree {
        match &n.node {
            IdNodeKind::Element { name, .. } if !xmesh::names::is_reserved(name) => {
                walk(n, "/", 0, &mut out)
            }
            _ => {}
        }
    }
    out
}

/// A peer's updates in a single-round vector, made exactly as the vector
/// defines them: its batches applied to its own replica of the seed, with its
/// own client ID, seeing no other peer's edits.
pub fn peer_updates(v: &Vector, peer: &str) -> Result<Vec<Vec<u8>>, String> {
    let batches = v.rounds[0]
        .get(peer)
        .ok_or_else(|| format!("no peer {peer:?} in {}", v.name))?;
    let client = *v
        .peers
        .get(peer)
        .ok_or_else(|| format!("no client ID for {peer:?} in {}", v.name))?;
    let replica = Xmesh::new(client).map_err(|e| e.to_string())?;
    replica
        .apply(&Xmesh::seed(&v.seed).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    batches
        .iter()
        .map(|b| replica.edit(b).map_err(|e| e.to_string()))
        .collect()
}

/// One-shot view: nothing to show while running; statuses go to stderr.
struct Quiet;

impl View for Quiet {
    fn show(&mut self, _: &Screen) {}
    fn status(&mut self, line: &str) {
        eprintln!("{line}");
    }
    fn finished(&mut self, _: &Outcome) {}
}

fn hashes_json(o: &Outcome) -> String {
    serde_json::json!({
        "doc_hash": o.doc_hash,
        "snapshot_hash": o.snapshot_hash,
        "updates_before_synced": o.updates_before_synced,
    })
    .to_string()
}

fn finish(server: &str, outcome: &Outcome) -> Result<(), String> {
    if outcome.ok {
        return Ok(());
    }
    let message = outcome.message.clone().unwrap_or_else(|| "failed".into());
    if message.starts_with("could not reach y-sweet") {
        Err(format!(
            "could not reach y-sweet at {server} within 10 s; is it running?"
        ))
    } else {
        Err(message)
    }
}

/// Refuses a name y-sweet wouldn't take, before trying to connect.
fn check_doc(name: &str) -> Result<(), String> {
    if ysweet::valid_doc_name(name) {
        Ok(())
    } else {
        Err(format!(
            "{name:?} isn't a valid document name (letters, digits, - and _)"
        ))
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let server = cli.server.clone();
    match cli.sub {
        Sub::Init { file, doc } => {
            let xml =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let seed = Xmesh::seed(&xml).map_err(|e| format!("{}: {e}", file.display()))?;
            let name = doc.unwrap_or_else(|| {
                file.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            check_doc(&name)?;
            ysweet::create_doc(&server, &name)?;
            let outcome = ws::run(Mode::Init { seed }, &server, &name, Quiet, None);
            finish(&server, &outcome)?;
            println!("{}", hashes_json(&outcome));
        }
        Sub::Edit { doc, ops } => {
            check_doc(&doc)?;
            let ops = parse_edit_ops(&ops)?;
            let outcome = ws::run(Mode::Edit(ops), &server, &doc, Quiet, None);
            finish(&server, &outcome)?;
            println!("{}", hashes_json(&outcome));
        }
        Sub::Show {
            doc,
            canonical,
            tree,
            hashes,
        } => {
            check_doc(&doc)?;
            let outcome = ws::run(Mode::Show, &server, &doc, Quiet, None);
            finish(&server, &outcome)?;
            if hashes {
                println!("{}", hashes_json(&outcome));
            } else if tree {
                let doc = Xmesh::new(1).expect("valid client ID");
                // The canonical XML is enough to rebuild the tree for printing.
                doc.apply(&Xmesh::seed(&outcome.canonical).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                for line in tree_lines(&doc.tree_with_ids()) {
                    println!("{line}");
                }
            } else {
                let _ = canonical;
                println!("{}", outcome.canonical);
            }
        }
        Sub::Replay {
            vector,
            peer,
            seed,
            doc,
        } => {
            if let Some(name) = &doc {
                check_doc(name)?;
            }
            let json = std::fs::read_to_string(&vector)
                .map_err(|e| format!("{}: {e}", vector.display()))?;
            let v = Vector::from_json(&json).map_err(|e| e.to_string())?;
            if v.rounds.len() != 1 {
                return Err(format!(
                    "replay needs a single-round vector; {} has {} rounds",
                    v.name,
                    v.rounds.len()
                ));
            }
            let updates = peer_updates(&v, &peer)?;
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let gaps_ms = updates.iter().map(|_| rng.gen_range(0..=50)).collect();
            let name = doc.unwrap_or(v.name.clone());
            check_doc(&name)?;
            let outcome = ws::run(
                Mode::Replay { updates, gaps_ms },
                &server,
                &name,
                Quiet,
                None,
            );
            finish(&server, &outcome)?;
            println!("{}", hashes_json(&outcome));
        }
        Sub::Open { doc, read_only } => {
            check_doc(&doc)?;
            crate::adapters::tui::run(&server, &doc, read_only)?
        }
    }
    Ok(())
}

/// The binary's entry point; returns the exit status.
pub fn main() -> i32 {
    match run(Cli::parse()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("xtxp-muppet: {e}");
            1
        }
    }
}
