//! Crouch-walk / bhop trace on a flat floor with auto-sprint on (sprint held), holstered speed scale.
//! cargo run --release -p shared --example crouchdbg -- [crouch|bhop]
use glam::Vec3;
use shared::collision::Collision;
use shared::sim::{self, Input, Player};
const TICK: f32 = 1.0 / 60.0;
fn main() {
    let s = 4000.0;
    let col = Collision::build(vec![[-s, -s, 0.0], [s, -s, 0.0], [s, s, 0.0], [-s, s, 0.0]], &[0, 1, 2, 0, 2, 3]);
    let mut p = Player::new(Vec3::new(0.0, 0.0, 0.1));
    p.speed_scale = 1.15;
    for _ in 0..30 { sim::simulate(&mut p, &Input::default(), TICK, &col); }
    let mode = std::env::args().nth(1).unwrap_or("crouch".into());
    for t in 0..150 {
        let i = match mode.as_str() {
            "crouch" => Input { forward: 1.0, sprint: true, crouch: true, ..Default::default() },
            _ => Input { forward: 1.0, sprint: true, jump: t == 90 || t == 137, ..Default::default() },
        };
        sim::simulate(&mut p, &i, TICK, &col);
        if t % 5 == 0 || (85..100).contains(&t) {
            println!("t{t:3} hs {:6.1} vz {:6.1} z {:6.2} {} duck {} sliding {} ground {}", p.vel.truncate().length(), p.vel.z, p.pos.z, p.state_name(), p.duck_state, p.sliding, p.on_ground);
        }
    }
}
