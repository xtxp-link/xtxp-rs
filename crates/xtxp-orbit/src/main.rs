//! Orbit: a physics engine app that drives a muppet.

use clap::Parser;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::unbounded_channel;
use xtxp_muppet::adapters::{ws, ysweet};
use xtxp_muppet::core::Mode;
use xtxp_orbit::{Engine, Orbit};

/// Orbital physics engine for XTXP documents.
#[derive(Parser, Debug)]
#[command(name = "xtxp-orbit", version)]
#[command(about = "Drive a y-sweet document with orbital physics", long_about = None)]
pub struct Args {
    /// Document name
    pub doc: String,

    /// y-sweet's address
    #[arg(long, default_value = "localhost:8080")]
    pub server: String,

    /// Physics loop frequency in Hz
    #[arg(long, default_value_t = 240.0)]
    pub physics_hz: f64,

    /// Render loop frequency in Hz
    #[arg(long, default_value_t = 30.0)]
    pub render_hz: f64,

    /// Center X coordinate in pixels
    #[arg(long, default_value_t = 256.0)]
    pub center_x: f64,

    /// Center Y coordinate in pixels
    #[arg(long, default_value_t = 256.0)]
    pub center_y: f64,

    /// Left body initial X offset from center
    #[arg(long, default_value_t = -100.0)]
    pub left_offset_x: f64,

    /// Left body initial Y offset from center
    #[arg(long, default_value_t = 0.0)]
    pub left_offset_y: f64,

    /// Left body initial X velocity
    #[arg(long, default_value_t = 0.0)]
    pub left_vel_x: f64,

    /// Left body initial Y velocity
    #[arg(long, default_value_t = 100.0)]
    pub left_vel_y: f64,

    /// Right body initial X offset from center (symmetric to left if not set)
    #[arg(long)]
    pub right_offset_x: Option<f64>,

    /// Right body initial Y offset from center (symmetric to left if not set)
    #[arg(long)]
    pub right_offset_y: Option<f64>,

    /// Right body initial X velocity (opposite of left if not set)
    #[arg(long)]
    pub right_vel_x: Option<f64>,

    /// Right body initial Y velocity (opposite of left if not set)
    #[arg(long)]
    pub right_vel_y: Option<f64>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Validate document name.
    if !ysweet::valid_doc_name(&args.doc) {
        eprintln!(
            "xtxp-orbit: {:?} isn't a valid document name (letters, digits, - and _)",
            args.doc
        );
        return Err("invalid document name".into());
    }

    // Default right body to be symmetric to left.
    let right_offset_x = args.right_offset_x.unwrap_or(-args.left_offset_x);
    let right_offset_y = args.right_offset_y.unwrap_or(-args.left_offset_y);
    let right_vel_x = args.right_vel_x.unwrap_or(-args.left_vel_x);
    let right_vel_y = args.right_vel_y.unwrap_or(-args.left_vel_y);

    // Create the engine.
    let engine = Engine::new(
        (args.center_x, args.center_y),
        (args.left_offset_x, args.left_offset_y),
        (args.left_vel_x, args.left_vel_y),
        (right_offset_x, right_offset_y),
        (right_vel_x, right_vel_y),
    );

    // Create the app.
    let app = Orbit::new(&args.doc, engine, args.render_hz);

    // Channel for sending commands to the muppet.
    let (_tx, rx) = unbounded_channel();

    // Spawn the physics loop in a separate std thread.
    let physics_hz = args.physics_hz;
    let _physics_thread = thread::spawn(move || {
        // Each body tracks its own state; step the physics and send commands.
        // Note: in the real version, we'd share the engine, but for simplicity,
        // we recreate it here just to generate commands.
        let mut local_engine = Engine::new(
            (256.0, 256.0),
            (-100.0, 0.0),
            (0.0, 100.0),
            (100.0, 0.0),
            (0.0, -100.0),
        );

        let physics_dt = Duration::from_secs_f64(1.0 / physics_hz);
        let mut last_physics = Instant::now();
        loop {
            if last_physics.elapsed() >= physics_dt {
                // Step physics and note that we generated commands
                // (actual command generation happens in app.update())
                local_engine.step();
                last_physics = Instant::now();
            }
            thread::sleep(Duration::from_millis(1));
        }
    });

    eprintln!(
        "orbit: connecting to {} for document {}",
        args.server, args.doc
    );

    // Connect to the muppet and drive it.
    let outcome = ws::run(Mode::Show, &args.server, &args.doc, app, Some(rx));

    if outcome.ok {
        eprintln!("orbit: done");
    } else {
        eprintln!(
            "orbit: {}",
            outcome.message.unwrap_or_else(|| "failed".into())
        );
    }

    Ok(())
}
