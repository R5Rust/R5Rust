//! Tap-strafe trace on a flat floor: sprint, jump, then turn the view while flicking forward on the wheel.
//! cargo run --release -p shared --example tapdbg
use glam::Vec3;
use shared::collision::Collision;
use shared::sim::{self, Input, Player};
const TICK: f32 = 1.0 / 60.0;
fn main() {
    let s = 4000.0;
    let pos = vec![[-s, -s, 0.0], [s, -s, 0.0], [s, s, 0.0], [-s, s, 0.0]];
    let col = Collision::build(pos, &[0, 1, 2, 0, 2, 3]);
    let mut p = Player::new(Vec3::new(0.0, 0.0, 0.1));
    let mut yaw = 0.0f32;
    for i in 0..90 {
        sim::simulate(&mut p, &Input { forward: 1.0, sprint: i == 0, yaw, ..Default::default() }, TICK, &col);
    }
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, jump_pressed: true, yaw, ..Default::default() }, TICK, &col);
    let mode = std::env::args().nth(1).unwrap_or("wheel".into());
    for t in 0..40 {
        yaw += 4.0; // turning left
        // nothing held; the wheel gives a forward press every other tick
        let fwd = if mode == "wheel" { if t % 2 == 0 { 1.0 } else { 0.0 } } else { 0.0 };
        sim::simulate(&mut p, &Input { forward: fwd, yaw, ..Default::default() }, TICK, &col);
        let vyaw = p.vel.y.atan2(p.vel.x).to_degrees();
        println!("t{t:2} view {yaw:6.1}  vel yaw {vyaw:7.1}  speed {:6.1}  z {:6.1} {}", p.vel.truncate().length(), p.pos.z, p.state_name());
    }
}
