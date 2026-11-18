//! Jump while pushing into a wall (the probe's jump_back): prints z / velocity per tick, to check that a rising
//! hull keeps its vertical motion while the horizontal part is clipped by the wall.
//!   cargo run --release -p shared --example airdbg -- [x,y,z] [yaw] [forward] [right] [dt]
use glam::Vec3;
use shared::{assets::Config, collmesh, sim::{self, Input, Player}};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let p0: Vec<f32> = a.first().map(|s| s.split(',').map(|v| v.parse().unwrap()).collect()).unwrap_or(vec![-12211.0, 384.0, 1580.1]);
    let yaw: f32 = a.get(1).map(|v| v.parse().unwrap()).unwrap_or(0.0);
    let fwd: f32 = a.get(2).map(|v| v.parse().unwrap()).unwrap_or(-1.0);
    let right: f32 = a.get(3).map(|v| v.parse().unwrap()).unwrap_or(0.0);
    let dt: f32 = a.get(4).map(|v| v.parse().unwrap()).unwrap_or(1.0 / 60.0);
    let (col, _) = collmesh::build_player_collision(&Config::load()).unwrap();
    let mut p = Player::new(sim::find_free(&col, Vec3::new(p0[0], p0[1], p0[2])));
    p.speed_scale = 1.15;
    for k in 0..(2.0 / dt) as usize {
        let jump = k == (1.0 / dt) as usize;
        let inp = Input { forward: fwd, right, jump, jump_pressed: jump, sprint: true, yaw, ..Default::default() };
        sim::simulate(&mut p, &inp, dt, &col);
        if k + 3 >= (1.0 / dt) as usize {
            println!("k{k:3} pos ({:.2},{:.2},{:.2}) vel ({:.1},{:.1},{:.1}) {}", p.pos.x, p.pos.y, p.pos.z, p.vel.x, p.vel.y, p.vel.z, p.state_name());
        }
    }
}
