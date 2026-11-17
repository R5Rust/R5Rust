//! Replays a reported stuck spot on the real collision: drop the player at x,y,z and push forward (sprint) at 8 yaws,
//! printing height and state over 2 s. cargo run --release -p shared --example stuckdbg -- x,y,z
//! TRACE=1 prints per-tick pos/vel for yaw 0 (TRACE_YAW=<deg> picks another yaw) and, on ticks where the player
//! did not move, every triangle that touches the hull's trace along the velocity. TRACE_T=<ticks> (default 24).
//! A start inside geometry is moved to the nearest free spot first (and says so).
use glam::Vec3;
use shared::{assets::Config, collmesh, sim::{self, Input, Player}};
fn main() {
    let v: Vec<f32> = std::env::args().nth(1).unwrap().split(',').map(|x| x.parse().unwrap()).collect();
    let (col, _) = collmesh::build_player_collision(&Config::load()).unwrap();
    let trace = std::env::var_os("TRACE").is_some();
    let start = nearest_free(&col, Vec3::new(v[0], v[1], v[2]));
    if start != Vec3::new(v[0], v[1], v[2]) {
        println!("start is inside geometry: moved {:.2} units to the nearest free spot {:.2?}", (start - Vec3::new(v[0], v[1], v[2])).length(), start);
    }
    let tyaw: f32 = std::env::var("TRACE_YAW").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    for k in 0..8 {
        let yaw = k as f32 * 45.0;
        let mut p = Player::new(start);
        p.speed_scale = 1.15;
        let mut out = vec![];
        for t in 0..std::env::var("TICKS").ok().and_then(|s| s.parse().ok()).unwrap_or(120) {
            let jump = t == 0;
            let (p0, v0) = (p.pos, p.vel);
            sim::simulate(&mut p, &Input { forward: 1.0, sprint: true, jump, jump_pressed: jump, yaw, ..Default::default() }, 1.0 / 60.0, &col);
            if t % 12 == 0 { out.push(format!("{:.0}{}", p.pos.z, &p.state_name()[..3])); }
            if trace && yaw == tyaw && t < std::env::var("TRACE_T").ok().and_then(|s| s.parse().ok()).unwrap_or(24) {
                println!("  t{t} pos {:?} vel {:?} {} ground {}", p.pos, p.vel, p.state_name(), p.on_ground);
                if p.pos == p0 {
                    let (mn, mx) = p.hull();
                    let tr = col.trace_hull(p0, p0 + v0 / 60.0, mn, mx);
                    println!("    trace along old vel {:?}: {:?}", v0, tr);
                    for (tri, kind, f, n) in col.trace_hull_detail(p0, p0 + v0 / 60.0, mn, mx) {
                        let tn = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize();
                        println!("    kind {kind} f {f:.4} n {n:.3} tri-normal {tn:.3} tri {:.2?}", tri);
                    }
                }
            }
        }
        println!("yaw {yaw:5}: {}", out.join(" "));
    }
}

/// Nearest spot where the standing hull is free (a real player can't be inside geometry; reported spots are rounded
/// to whole units and can land a hull a fraction of a unit inside a wall). Searches shells 0.25 units apart.
fn nearest_free(col: &shared::collision::Collision, p: Vec3) -> Vec3 {
    let (mn, mx) = (shared::sim::STOCK.stand.hull_min, shared::sim::STOCK.stand.hull_max);
    let free = |q: Vec3| !col.trace_hull(q, q, mn, mx).startsolid;
    if free(p) {
        return p;
    }
    let mut dirs = vec![];
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                if (x, y, z) != (0, 0, 0) {
                    dirs.push(Vec3::new(x as f32, y as f32, z as f32).normalize());
                }
            }
        }
    }
    for i in 1..=192 {
        let r = i as f32 * 0.25;
        for d in &dirs {
            if free(p + *d * r) {
                return p + *d * r;
            }
        }
    }
    p
}
