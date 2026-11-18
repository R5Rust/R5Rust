//! Slide on the real hall floor, printing z and speed per tick (seam / step-loss debugging).
//!   cargo run --release -p shared --example slidedbg -- [x,y,z] [yaw]
use glam::Vec3;
use shared::{assets::Config, collmesh, sim::{self, Input, Player}};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let p0: Vec<f32> = a.first().map(|s| s.split(',').map(|v| v.parse().unwrap()).collect()).unwrap_or(vec![-12211.0, 384.0, 1580.1]);
    let yaw: f32 = a.get(1).map(|v| v.parse().unwrap()).unwrap_or(0.0);
    let (col, _) = collmesh::build_player_collision(&Config::load()).unwrap();
    let mut p = Player::new(sim::find_free(&col, Vec3::new(p0[0], p0[1], p0[2])));
    p.speed_scale = 1.15;
    let dt = 1.0 / 60.0;
    for k in 0..300 {
        let crouch = k >= 90;
        let inp = Input { forward: 1.0, sprint: true, crouch, yaw, ..Default::default() };
        let (z0, h0) = (p.pos.z, p.vel.truncate().length());
        sim::simulate(&mut p, &inp, dt, &col);
        let h = p.vel.truncate().length();
        if k >= 88 && (p.pos.z != z0 || k % 10 == 0) {
            println!("k{k:3} x {:9.3} z {:9.5} dz {:+.5} hs {h:7.2} dhs {:+6.2} {}", p.pos.x, p.pos.z, p.pos.z - z0, h - h0, p.state_name());
        }
        if !p.sliding && k > 100 { break }
    }
}
