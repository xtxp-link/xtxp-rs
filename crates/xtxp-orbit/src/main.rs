//! Orbit: a physics engine app that drives a muppet.

use clap::Parser;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::unbounded_channel;
use xtxp_muppet::adapters::{ws, ysweet};
use xtxp_muppet::core::Mode;
use xtxp_muppet::ports::{Outcome, Screen, View};
use xtxp_orbit::{Engine, Orbit};

/// Wrapper that allows Orbit to be shared across threads via Arc<Mutex<>>.
struct SharedOrbit(Arc<Mutex<Orbit>>);

impl View for SharedOrbit {
    fn show(&mut self, screen: &Screen) {
        self.0.lock().unwrap().show(screen);
    }
    fn status(&mut self, line: &str) {
        self.0.lock().unwrap().status(line);
    }
    fn finished(&mut self, outcome: &Outcome) {
        self.0.lock().unwrap().finished(outcome);
    }
}

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

    // Create the app in Arc<Mutex<>> to share between physics thread and muppet.
    let app = Arc::new(Mutex::new(Orbit::new(&args.doc, engine, args.render_hz)));

    // Channel for sending commands to the muppet.
    let (_tx, rx) = unbounded_channel();

    // Spawn the physics loop in a separate std thread that updates the shared app.
    let app_clone = Arc::clone(&app);
    let physics_hz = args.physics_hz;
    let _physics_thread = thread::spawn(move || {
        let physics_dt = Duration::from_secs_f64(1.0 / physics_hz);
        let mut last_physics = Instant::now();
        loop {
            if last_physics.elapsed() >= physics_dt {
                // Lock the app, step physics, and generate commands.
                let commands = app_clone.lock().unwrap().update();
                // Commands are sent through the channel to the muppet.
                if !commands.is_empty() {
                    eprintln!("orbit: {} commands", commands.len());
                }
                last_physics = Instant::now();
            }
            thread::sleep(Duration::from_millis(1));
        }
    });

    eprintln!(
        "orbit: connecting to {} for document {}",
        args.server, args.doc
    );

    // Connect to the muppet and drive it (blocking call in a blocking task).
    // Use Mode::Interactive to keep the connection open and process commands continuously.
    // SharedOrbit wraps the Arc<Mutex<Orbit>> so it can be passed to ws::run.
    let outcome = tokio::task::block_in_place(|| {
        ws::run(Mode::Interactive, &args.server, &args.doc, SharedOrbit(app), Some(rx))
    });

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
