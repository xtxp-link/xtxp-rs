//! xtxp-orbit: animates the XTXP logo with a three-body physics engine.

use clap::Parser;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::mpsc::unbounded_channel;
use xtxp_muppet::adapters::{ws, ysweet};
use xtxp_muppet::core::Mode;
use xtxp_orbit::{drive, Body, Engine, Finder, Vec2};

fn pair(s: &str) -> Result<Vec2, String> {
    let (x, y) = s.split_once(',').ok_or("expected X,Y")?;
    let n = |v: &str| v.trim().parse::<f64>().map_err(|e| format!("{v:?}: {e}"));
    Ok((n(x)?, n(y)?))
}

/// Drive the logo in a y-sweet document with a three-body physics engine.
///
/// Positions are in the logo's own units: the left X at 0,0, the cross at 32,0
/// and the right X at 64,0. By default the two outer X's start on a circular
/// orbit about the cross, one moving up and one down.
#[derive(Parser, Debug)]
#[command(name = "xtxp-orbit", version, long_version = concat!(env!("CARGO_PKG_VERSION"), " (Rust implementation)"))]
struct Args {
    /// Document name (the XTXP logo)
    doc: String,

    /// y-sweet's address
    #[arg(long, default_value = "localhost:8080")]
    server: String,

    /// Physics steps per second
    #[arg(long, default_value_t = 240.0)]
    physics_hz: f64,

    /// Transforms published per second
    #[arg(long, default_value_t = 30.0)]
    render_hz: f64,

    /// Gravitational constant
    #[arg(long, default_value_t = 800.0)]
    gravity: f64,

    /// Distance below which gravity stops growing
    #[arg(long, default_value_t = 4.0)]
    softening: f64,

    /// Mass of the cross
    #[arg(long, default_value_t = 100.0)]
    cross_mass: f64,

    /// Mass of each outer X
    #[arg(long, default_value_t = 1.0)]
    outer_mass: f64,

    /// Left X's start position, X,Y
    #[arg(long, value_parser = pair, allow_hyphen_values = true, default_value = "0,0")]
    left_pos: Vec2,

    /// Cross's start position, X,Y
    #[arg(long, value_parser = pair, allow_hyphen_values = true, default_value = "32,0")]
    cross_pos: Vec2,

    /// Right X's start position, X,Y
    #[arg(long, value_parser = pair, allow_hyphen_values = true, default_value = "64,0")]
    right_pos: Vec2,

    /// Left X's velocity, X,Y (default: circular, moving up)
    #[arg(long, value_parser = pair, allow_hyphen_values = true)]
    left_vel: Option<Vec2>,

    /// Cross's velocity, X,Y
    #[arg(long, value_parser = pair, allow_hyphen_values = true, default_value = "0,0")]
    cross_vel: Vec2,

    /// Right X's velocity, X,Y (default: circular, moving down)
    #[arg(long, value_parser = pair, allow_hyphen_values = true)]
    right_vel: Option<Vec2>,

    /// Left X's spin about its own centre, degrees per second (clockwise)
    #[arg(long, allow_hyphen_values = true, default_value_t = 180.0)]
    left_spin: f64,

    /// Cross's spin about its own centre, degrees per second (clockwise)
    #[arg(long, allow_hyphen_values = true, default_value_t = 0.0)]
    cross_spin: f64,

    /// Right X's spin about its own centre, degrees per second (clockwise)
    #[arg(long, allow_hyphen_values = true, default_value_t = 180.0)]
    right_spin: f64,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if let Err(e) = ysweet::check_doc(&args.doc) {
        eprintln!("xtxp-orbit: {e}");
        return ExitCode::FAILURE;
    }
    if args.physics_hz <= 0.0 || args.render_hz <= 0.0 {
        eprintln!("xtxp-orbit: --physics-hz and --render-hz must be positive");
        return ExitCode::FAILURE;
    }

    let bodies = vec![
        Body::new(args.outer_mass, args.left_pos, (0.0, 0.0)).with_spin(args.left_spin),
        Body::new(args.cross_mass, args.cross_pos, args.cross_vel).with_spin(args.cross_spin),
        Body::new(args.outer_mass, args.right_pos, (0.0, 0.0)).with_spin(args.right_spin),
    ];
    let mut engine = Engine::new(bodies, args.gravity, args.softening, args.physics_hz);
    let left_v = engine.circular_speed(0, args.cross_pos);
    let right_v = engine.circular_speed(2, args.cross_pos);
    engine.set_velocity(0, args.left_vel.unwrap_or((0.0, -left_v)));
    engine.set_velocity(2, args.right_vel.unwrap_or((0.0, right_v)));

    let targets = Arc::new(Mutex::new(None));
    let (tx, rx) = unbounded_channel();
    let render_hz = args.render_hz;
    let shared = Arc::clone(&targets);
    thread::spawn(move || drive(engine, shared, tx, render_hz));

    eprintln!("orbit: driving {} on {}", args.doc, args.server);
    let outcome = ws::run(
        Mode::Interactive,
        &args.server,
        &args.doc,
        Finder(targets),
        Some(rx),
    );
    if outcome.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
