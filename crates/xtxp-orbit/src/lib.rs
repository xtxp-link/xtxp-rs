//! A three-body physics engine that owns the space and drives a muppet.
//!
//! The engine simulates the logo's cross and two outer X's under mutual softened
//! gravity: velocity-Verlet at a fixed timestep (an accumulator, so the
//! simulation is the same however often it is sampled). Each body also spins
//! about its own centre at a constant rate. A separate, slower publish loop
//! samples the interpolated poses and sends one `set-attr transform` per body
//! through the muppet's command channel.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use xmesh::{IdNode, IdNodeKind, NodeId};
use xtxp_muppet::core::Command;
use xtxp_muppet::ports::{Outcome, Screen, View};

pub type Vec2 = (f64, f64);

/// Where a body is and how far it has turned, in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: Vec2,
    pub angle: f64,
}

/// Longest wall-clock frame the simulation will catch up on, in seconds.
const MAX_FRAME: f64 = 0.25;

#[derive(Clone, Debug)]
pub struct Body {
    pub mass: f64,
    pub pos: Vec2,
    pub vel: Vec2,
    /// Degrees turned about the body's own centre, unwrapped.
    pub angle: f64,
    /// Degrees per second, clockwise as drawn.
    pub spin: f64,
    acc: Vec2,
    prev: Vec2,
    prev_angle: f64,
}

impl Body {
    pub fn new(mass: f64, pos: Vec2, vel: Vec2) -> Self {
        Self {
            mass,
            pos,
            vel,
            angle: 0.0,
            spin: 0.0,
            acc: (0.0, 0.0),
            prev: pos,
            prev_angle: 0.0,
        }
    }

    pub fn with_spin(mut self, degrees_per_second: f64) -> Self {
        self.spin = degrees_per_second;
        self
    }
}

pub struct Engine {
    bodies: Vec<Body>,
    gravity: f64,
    softening: f64,
    dt: f64,
    pending: f64,
}

impl Engine {
    pub fn new(bodies: Vec<Body>, gravity: f64, softening: f64, physics_hz: f64) -> Self {
        let mut engine = Self {
            bodies,
            gravity,
            softening,
            dt: 1.0 / physics_hz,
            pending: 0.0,
        };
        let acc = engine.accelerations();
        for (body, a) in engine.bodies.iter_mut().zip(acc) {
            body.acc = a;
        }
        engine
    }

    fn accelerations(&self) -> Vec<Vec2> {
        let eps2 = self.softening * self.softening;
        self.bodies
            .iter()
            .enumerate()
            .map(|(i, bi)| {
                let mut a = (0.0, 0.0);
                for (j, bj) in self.bodies.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    let (dx, dy) = (bj.pos.0 - bi.pos.0, bj.pos.1 - bi.pos.1);
                    let d2 = dx * dx + dy * dy + eps2;
                    let k = self.gravity * bj.mass / (d2 * d2.sqrt());
                    a.0 += k * dx;
                    a.1 += k * dy;
                }
                a
            })
            .collect()
    }

    /// One velocity-Verlet step of `dt`.
    pub fn step(&mut self) {
        let dt = self.dt;
        for b in &mut self.bodies {
            b.prev = b.pos;
            b.prev_angle = b.angle;
            b.angle += b.spin * dt;
            b.pos.0 += b.vel.0 * dt + 0.5 * b.acc.0 * dt * dt;
            b.pos.1 += b.vel.1 * dt + 0.5 * b.acc.1 * dt * dt;
        }
        let next = self.accelerations();
        for (b, a) in self.bodies.iter_mut().zip(next) {
            b.vel.0 += 0.5 * (b.acc.0 + a.0) * dt;
            b.vel.1 += 0.5 * (b.acc.1 + a.1) * dt;
            b.acc = a;
        }
    }

    /// Advances by `frame` seconds of wall-clock time in whole fixed steps; the
    /// remainder carries over and shows up as interpolation in `poses`.
    pub fn advance(&mut self, frame: f64) {
        self.pending += frame.clamp(0.0, MAX_FRAME);
        while self.pending >= self.dt {
            self.step();
            self.pending -= self.dt;
        }
    }

    /// Each body's pose now, interpolated between its last two steps.
    pub fn poses(&self) -> Vec<Pose> {
        let alpha = self.pending / self.dt;
        self.bodies
            .iter()
            .map(|b| Pose {
                pos: (
                    b.prev.0 + (b.pos.0 - b.prev.0) * alpha,
                    b.prev.1 + (b.pos.1 - b.prev.1) * alpha,
                ),
                angle: b.prev_angle + (b.angle - b.prev_angle) * alpha,
            })
            .collect()
    }

    pub fn set_velocity(&mut self, body: usize, vel: Vec2) {
        self.bodies[body].vel = vel;
    }

    /// The speed that keeps `body` on a circle about `about`, given the pull on it now.
    pub fn circular_speed(&self, body: usize, about: Vec2) -> f64 {
        let b = &self.bodies[body];
        let r = ((b.pos.0 - about.0).powi(2) + (b.pos.1 - about.1).powi(2)).sqrt();
        (b.acc.0.hypot(b.acc.1) * r).sqrt()
    }
}

/// A node the engine moves, and what follows our `translate(..) rotate(..)` in its transform.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub id: NodeId,
    pub suffix: String,
}

/// The logo's left X, cross and right X, in that order.
pub type Targets = [Target; 3];

const CLASSES: [&str; 3] = ["xl-l", "xl-c", "xl-r"];

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Drops a leading `rotate(A)` that orbit itself wrote: its angle always has
/// exactly three decimals, which no hand-written transform in the logo has.
fn strip_own_rotation(s: &str) -> &str {
    let s = s.trim_start();
    let Some(rest) = s.strip_prefix("rotate(") else {
        return s;
    };
    match rest.split_once(')') {
        Some((n, after)) if n.split_once('.').is_some_and(|(_, d)| d.len() == 3) => after,
        _ => s,
    }
}

fn find_class(tree: &[IdNode], class: &str) -> Option<Target> {
    for n in tree {
        if let IdNodeKind::Element {
            attrs, children, ..
        } = &n.node
        {
            if attr(attrs, "class").is_some_and(|c| c.split_whitespace().any(|t| t == class)) {
                let transform = attr(attrs, "transform").unwrap_or("").trim();
                let suffix = match transform.strip_prefix("translate(") {
                    Some(rest) => {
                        strip_own_rotation(rest.split_once(')').map_or("", |(_, after)| after))
                    }
                    None => transform,
                };
                return Some(Target {
                    id: n.id,
                    suffix: if suffix.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", suffix.trim())
                    },
                });
            }
            if let Some(found) = find_class(children, class) {
                return Some(found);
            }
        }
    }
    None
}

/// Finds the logo's three moving parts by their classes. A transform the
/// document already has keeps everything after its leading `translate(..)` (and
/// the `rotate(..)` orbit wrote after it), so the cross keeps its own rotation
/// and scale.
pub fn find_targets(tree: &[IdNode]) -> Option<Targets> {
    Some([
        find_class(tree, CLASSES[0])?,
        find_class(tree, CLASSES[1])?,
        find_class(tree, CLASSES[2])?,
    ])
}

/// Rounds to the three decimals we publish, and never prints `-0.000`.
fn coord(v: f64) -> f64 {
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// One `set-attr transform` per body: moved to the pose's position, then
/// turned about its own centre.
pub fn transform_commands(targets: &Targets, poses: &[Pose]) -> Vec<Command> {
    targets
        .iter()
        .zip(poses)
        .map(|(t, p)| Command::SetAttr {
            node: t.id,
            name: "transform".into(),
            value: format!(
                "translate({:.3} {:.3}) rotate({:.3}){}",
                coord(p.pos.0),
                coord(p.pos.1),
                coord(p.angle.rem_euclid(360.0)),
                t.suffix
            ),
        })
        .collect()
}

pub type SharedTargets = Arc<Mutex<Option<Targets>>>;

/// The muppet's view of the document, used only to learn the node IDs.
pub struct Finder(pub SharedTargets);

impl View for Finder {
    fn show(&mut self, screen: &Screen) {
        let mut slot = self.0.lock().unwrap();
        if slot.is_none() {
            *slot = find_targets(&screen.tree);
        }
    }

    fn status(&mut self, line: &str) {
        eprintln!("orbit: {line}");
    }

    fn finished(&mut self, outcome: &Outcome) {
        if !outcome.ok {
            eprintln!("orbit: {}", outcome.message.as_deref().unwrap_or("failed"));
        }
    }
}

/// The publish loop: waits for the targets, then at `render_hz` advances the
/// simulation and sends the bodies' transforms. Returns when the muppet is gone.
pub fn drive(
    mut engine: Engine,
    targets: SharedTargets,
    commands: UnboundedSender<Command>,
    render_hz: f64,
) {
    let period = Duration::from_secs_f64(1.0 / render_hz);
    let mut last = Instant::now();
    let mut next = last + period;
    loop {
        let now = Instant::now();
        if now < next {
            thread::sleep(next - now);
        }
        let now = Instant::now();
        let found = targets.lock().unwrap().clone();
        let Some(found) = found else {
            last = now;
            next = now + period;
            continue;
        };
        engine.advance(now.duration_since(last).as_secs_f64());
        last = now;
        for command in transform_commands(&found, &engine.poses()) {
            if commands.send(command).is_err() {
                return;
            }
        }
        next += period;
        if next < now {
            next = now + period;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmesh::Xmesh;

    const LOGO: &str = include_str!("../../xmesh/tests/fixtures/xtxp-logo.svg");

    /// The logo's layout: left X at 0, cross at 32, right X at 64, the cross heavy.
    fn logo_engine() -> Engine {
        let bodies = vec![
            Body::new(1.0, (0.0, 0.0), (0.0, 0.0)),
            Body::new(100.0, (32.0, 0.0), (0.0, 0.0)),
            Body::new(1.0, (64.0, 0.0), (0.0, 0.0)),
        ];
        let mut e = Engine::new(bodies, 800.0, 4.0, 240.0);
        let v = e.circular_speed(0, (32.0, 0.0));
        e.set_velocity(0, (0.0, -v));
        e.set_velocity(2, (0.0, v));
        e
    }

    fn logo_tree() -> Vec<IdNode> {
        let d = Xmesh::new(1).unwrap();
        d.apply(&Xmesh::seed(LOGO).unwrap()).unwrap();
        d.tree_with_ids()
    }

    fn class_of(tree: &[IdNode], id: NodeId) -> Option<String> {
        for n in tree {
            if let IdNodeKind::Element {
                attrs, children, ..
            } = &n.node
            {
                if n.id == id {
                    return attr(attrs, "class").map(String::from);
                }
                if let Some(c) = class_of(children, id) {
                    return Some(c);
                }
            }
        }
        None
    }

    #[test]
    fn initial_velocity_moves_the_body() {
        let mut e = logo_engine();
        let start: Vec<Vec2> = e.bodies.iter().map(|b| b.pos).collect();
        e.step();
        let now: Vec<Vec2> = e.bodies.iter().map(|b| b.pos).collect();
        assert!(now[0].1 < start[0].1, "left X moves up");
        assert!(now[2].1 > start[2].1, "right X moves down");
        assert!(now[0].0 > start[0].0, "and is pulled towards the cross");
    }

    #[test]
    fn symmetric_start_is_a_steady_circular_orbit() {
        let mut e = logo_engine();
        for _ in 0..(240 * 10) {
            e.advance(1.0 / 240.0);
        }
        let p: Vec<Vec2> = e.poses().iter().map(|q| q.pos).collect();
        let r = |i: usize| (p[i].0 - p[1].0).hypot(p[i].1 - p[1].1);
        assert!((r(0) - 32.0).abs() < 0.5, "left radius {}", r(0));
        assert!((r(2) - 32.0).abs() < 0.5, "right radius {}", r(2));
        assert!(
            (p[1].0 - 32.0).abs() < 0.1 && p[1].1.abs() < 0.1,
            "cross stays: {:?}",
            p[1]
        );
        assert!(p[0].0.hypot(p[0].1) > 1.0, "the left X has left its start");
    }

    #[test]
    fn sampling_rate_does_not_change_the_simulation() {
        let (mut a, mut b) = (logo_engine(), logo_engine());
        for _ in 0..240 {
            a.advance(1.0 / 240.0);
        }
        for _ in 0..8 {
            b.advance(1.0 / 8.0);
        }
        let (pa, pb) = (a.poses(), b.poses());
        assert!((pa[0].pos.0 - pb[0].pos.0).abs() < 1e-6);
        assert!((pa[0].pos.1 - pb[0].pos.1).abs() < 1e-6);
        assert!((pa[0].angle - pb[0].angle).abs() < 1e-6);
    }

    #[test]
    fn finds_the_three_parts_of_the_logo() {
        let tree = logo_tree();
        let t = find_targets(&tree).expect("logo has all three");
        for (target, class) in t.iter().zip(CLASSES) {
            assert_eq!(class_of(&tree, target.id).as_deref(), Some(class));
        }
        assert_eq!(t[0].suffix, "");
        assert_eq!(t[1].suffix, " rotate(45) scale(.606)");
        assert_eq!(t[2].suffix, "");
    }

    #[test]
    fn a_document_orbit_already_moved_gives_the_same_targets() {
        let moved = LOGO
            .replace(
                "translate(32 0) rotate(45)",
                "translate(31.5 -2.25) rotate(45)",
            )
            .replace("translate(64 0)", "translate(10.125 3)");
        let d = Xmesh::new(1).unwrap();
        d.apply(&Xmesh::seed(&moved).unwrap()).unwrap();
        let t = find_targets(&d.tree_with_ids()).unwrap();
        assert_eq!(t[1].suffix, " rotate(45) scale(.606)");
        assert_eq!(t[2].suffix, "");
    }

    #[test]
    fn negative_zero_is_not_published() {
        let t = find_targets(&logo_tree()).unwrap();
        let pose = Pose {
            pos: (-0.0001, 0.0),
            angle: 0.0,
        };
        let Command::SetAttr { value, .. } = &transform_commands(&t, &[pose; 3])[0] else {
            unreachable!()
        };
        assert_eq!(value, "translate(0.000 0.000) rotate(0.000)");
    }

    #[test]
    fn an_empty_document_has_no_targets() {
        assert!(find_targets(&[]).is_none());
    }

    #[test]
    fn transforms_keep_the_crosss_rotation_and_scale() {
        let t = find_targets(&logo_tree()).unwrap();
        let pose = |x, y, angle| Pose { pos: (x, y), angle };
        let cmds = transform_commands(
            &t,
            &[
                pose(1.0, 2.0, 12.5),
                pose(32.0, 0.0, 0.0),
                pose(63.5, -4.25, 725.0),
            ],
        );
        let values: Vec<_> = cmds
            .iter()
            .map(|c| match c {
                Command::SetAttr { name, value, .. } => {
                    assert_eq!(name, "transform");
                    value.clone()
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(values[0], "translate(1.000 2.000) rotate(12.500)");
        assert_eq!(
            values[1],
            "translate(32.000 0.000) rotate(0.000) rotate(45) scale(.606)"
        );
        assert_eq!(values[2], "translate(63.500 -4.250) rotate(5.000)");
    }

    fn spinning_logo_engine(spin: f64) -> Engine {
        let mut e = logo_engine();
        e.bodies[0].spin = spin;
        e.bodies[2].spin = spin;
        e
    }

    #[test]
    fn spin_turns_a_body_about_its_own_centre() {
        let mut e = spinning_logo_engine(90.0);
        for _ in 0..240 {
            e.advance(1.0 / 240.0);
        }
        let poses = e.bodies.iter().map(|b| b.angle).collect::<Vec<_>>();
        assert!((poses[0] - 90.0).abs() < 1e-6, "one second at 90 deg/s");
        assert_eq!(poses[1], 0.0, "the cross has no spin by default");
        assert!((poses[2] - 90.0).abs() < 1e-6);
    }

    #[test]
    fn spin_does_not_change_the_orbit() {
        let (mut a, mut b) = (logo_engine(), spinning_logo_engine(300.0));
        for _ in 0..(240 * 3) {
            a.advance(1.0 / 240.0);
            b.advance(1.0 / 240.0);
        }
        for (pa, pb) in a.poses().iter().zip(b.poses()) {
            assert_eq!(pa.pos, pb.pos);
        }
    }

    #[test]
    fn angle_is_interpolated_between_steps() {
        let mut e = spinning_logo_engine(240.0);
        e.advance(1.5 / 240.0);
        let a = e.poses()[0].angle;
        assert!(
            (a - 0.5).abs() < 1e-9,
            "halfway through the step from 0 to 1 degree: {a}"
        );
    }

    #[test]
    fn a_document_orbit_already_spun_does_not_compound_the_rotation() {
        let spun = LOGO
            .replace(
                "translate(32 0) rotate(45)",
                "translate(31.5 -2.25) rotate(30.000) rotate(45)",
            )
            .replace("translate(64 0)", "translate(10.125 3) rotate(271.250)");
        let d = Xmesh::new(1).unwrap();
        d.apply(&Xmesh::seed(&spun).unwrap()).unwrap();
        let t = find_targets(&d.tree_with_ids()).unwrap();
        assert_eq!(t[1].suffix, " rotate(45) scale(.606)");
        assert_eq!(t[2].suffix, "");
    }
}
