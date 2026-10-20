//! Physics engine that drives a muppet: owns the geometry and motion of orbiting elements.
//!
//! Implements a three-body system (the two outer X elements and the cross) with
//! velocity-Verlet integration and softened gravity. Two loops: physics at a fixed
//! timestep (240 Hz default) and rendering at a variable rate (30 Hz default) that
//! sends `set-attr transform` commands to move the SVG elements.

use xtxp_muppet::core::Command;
use xtxp_muppet::ports::{Outcome, Screen, View};
use std::time::{Duration, Instant};
use xmesh::NodeId;

/// A body in the orbital system.
#[derive(Clone, Debug)]
pub struct Body {
    /// SVG path to this element (e.g., "2/1" for the left X).
    pub path: String,
    /// 2D position in pixels.
    pub pos: (f64, f64),
    /// 2D velocity in pixels per second.
    pub vel: (f64, f64),
    /// 2D acceleration (recalculated each step).
    pub acc: (f64, f64),
    /// Mass (unused for now, but can drive physics).
    pub mass: f64,
    /// Old position for velocity-Verlet.
    pub prev_pos: (f64, f64),
}

impl Body {
    fn new(path: &str, pos: (f64, f64), vel: (f64, f64)) -> Self {
        Self {
            path: path.to_string(),
            pos,
            vel,
            acc: (0.0, 0.0),
            mass: 1.0,
            prev_pos: pos,
        }
    }
}

/// Physics engine: updates positions with velocity-Verlet and softened gravity.
pub struct Engine {
    /// The three bodies: left X, cross, right X.
    bodies: Vec<Body>,
    /// Physics timestep in seconds (default: 1/240).
    pub physics_dt: f64,
    /// Gravity strength (softened to avoid singularities).
    pub gravity: f64,
    /// Softening parameter (minimum distance before gravity saturates).
    pub softening: f64,
    /// Central body position (center of mass).
    pub center: (f64, f64),
}

impl Engine {
    /// Create a new engine with three bodies orbiting the center.
    pub fn new(
        center: (f64, f64),
        left_offset: (f64, f64),
        left_velocity: (f64, f64),
        right_offset: (f64, f64),
        right_velocity: (f64, f64),
    ) -> Self {
        let left_pos = (center.0 + left_offset.0, center.1 + left_offset.1);
        let right_pos = (center.0 + right_offset.0, center.1 + right_offset.1);

        Self {
            bodies: vec![
                Body::new("2/1", left_pos, left_velocity),  // left X
                Body::new("2/0", center, (0.0, 0.0)),        // cross (stationary)
                Body::new("2/2", right_pos, right_velocity), // right X
            ],
            physics_dt: 1.0 / 240.0,
            gravity: 5000.0,  // pixels²/s² per unit mass
            softening: 50.0,
            center,
        }
    }

    /// Step the physics forward by one timestep using velocity-Verlet.
    pub fn step(&mut self) {
        let dt = self.physics_dt;

        // Store old positions for velocity-Verlet.
        for body in &mut self.bodies {
            body.prev_pos = body.pos;
        }

        // Compute accelerations (gravity towards center, softened).
        for i in 0..self.bodies.len() {
            let mut ax = 0.0;
            let mut ay = 0.0;

            if i == 1 {
                // The cross doesn't move.
                self.bodies[i].acc = (0.0, 0.0);
                continue;
            }

            // Gravity towards the center.
            let dx = self.center.0 - self.bodies[i].pos.0;
            let dy = self.center.1 - self.bodies[i].pos.1;
            let dist_sq = dx * dx + dy * dy;
            let dist = (dist_sq + self.softening * self.softening).sqrt();
            let accel_mag = self.gravity / (dist * dist);
            ax += accel_mag * (dx / dist);
            ay += accel_mag * (dy / dist);

            self.bodies[i].acc = (ax, ay);
        }

        // Velocity-Verlet: x = 2*x - x_prev + a*dt²
        for body in &mut self.bodies {
            let new_x = 2.0 * body.pos.0 - body.prev_pos.0 + body.acc.0 * dt * dt;
            let new_y = 2.0 * body.pos.1 - body.prev_pos.1 + body.acc.1 * dt * dt;
            body.vel = (
                (new_x - body.pos.0) / dt,
                (new_y - body.pos.1) / dt,
            );
            body.pos = (new_x, new_y);
        }
    }

    /// Get the current state of all bodies.
    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }
}

/// Orbital app: drives a muppet, renders positions as transform attributes.
pub struct Orbit {
    pub doc: String,
    engine: Engine,
    last_render: Instant,
    render_dt: Duration,
    status: String,
    /// Cached node IDs for the three bodies (left, cross, right).
    node_ids: [Option<NodeId>; 3],
}

impl Orbit {
    pub fn new(doc: &str, engine: Engine, render_hz: f64) -> Self {
        Self {
            doc: doc.to_string(),
            engine,
            last_render: Instant::now(),
            render_dt: Duration::from_secs_f64(1.0 / render_hz),
            status: String::new(),
            node_ids: [None; 3],
        }
    }

    /// Update node IDs from the current screen (call when you receive a new screen).
    pub fn sync_node_ids(&mut self, screen: &Screen) {
        // Find node IDs for the three bodies: path 2/1, 2/0, 2/2.
        // Walk the tree to find these elements.
        fn find_at_path(tree: &[xmesh::IdNode], path: &[usize]) -> Option<xmesh::NodeId> {
            if path.is_empty() {
                return None;
            }
            if path.len() == 1 {
                return tree.get(path[0]).map(|n| n.id);
            }
            match tree.get(path[0]).map(|n| &n.node) {
                Some(xmesh::IdNodeKind::Element { children, .. }) => {
                    find_at_path(children, &path[1..])
                }
                _ => None,
            }
        }

        self.node_ids[0] = find_at_path(&screen.tree, &[2, 1]); // left X
        self.node_ids[1] = find_at_path(&screen.tree, &[2, 0]); // cross
        self.node_ids[2] = find_at_path(&screen.tree, &[2, 2]); // right X

        if self.node_ids[0].is_some() && self.node_ids[1].is_some() && self.node_ids[2].is_some() {
            self.status = "orbit: found all three bodies".into();
        }
    }

    /// Run the orbit engine: step physics, and periodically send render commands.
    pub fn update(&mut self) -> Vec<Command> {
        self.engine.step();

        let mut commands = Vec::new();

        // Check if it's time to render.
        if self.last_render.elapsed() >= self.render_dt {
            self.last_render = Instant::now();

            // Generate commands for each body (if we have its node ID).
            for (idx, body) in self.engine.bodies().iter().enumerate() {
                if let Some(node_id) = self.node_ids[idx] {
                    let transform = if body.path == "2/0" {
                        // Cross keeps its rotate(30).
                        "rotate(30)".to_string()
                    } else {
                        // Outer X's: translate based on position, and rotate.
                        let dx = body.pos.0 - self.engine.center.0;
                        let dy = body.pos.1 - self.engine.center.1;
                        let angle = dy.atan2(dx).to_degrees();
                        format!("translate({}, {}) rotate({})", dx, dy, angle)
                    };
                    commands.push(Command::SetAttr {
                        node: node_id,
                        name: "transform".into(),
                        value: transform,
                    });
                }
            }
        }

        commands
    }
}

impl View for Orbit {
    fn show(&mut self, screen: &Screen) {
        // Sync node IDs when we get a new screen from the muppet.
        self.sync_node_ids(screen);
    }

    fn status(&mut self, line: &str) {
        self.status = line.to_string();
    }

    fn finished(&mut self, outcome: &Outcome) {
        if outcome.ok {
            eprintln!("orbit: done");
        } else {
            eprintln!(
                "orbit: {}",
                outcome.message.clone().unwrap_or_else(|| "failed".into())
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_steps_without_panic() {
        let mut engine = Engine::new((100.0, 100.0), (-50.0, 0.0), (0.0, 50.0), (50.0, 0.0), (0.0, -50.0));
        for _ in 0..10 {
            engine.step();
        }
        assert!(engine.bodies[0].pos.0.is_finite());
        assert!(engine.bodies[0].pos.1.is_finite());
    }

    #[test]
    fn cross_stays_at_center() {
        let mut engine = Engine::new((100.0, 100.0), (-50.0, 0.0), (0.0, 50.0), (50.0, 0.0), (0.0, -50.0));
        engine.step();
        assert_eq!(engine.bodies[1].pos, (100.0, 100.0));
    }
}
