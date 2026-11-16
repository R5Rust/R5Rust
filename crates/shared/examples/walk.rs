//! walk forward from spawn with the real sim; report where it stops and what pushes back.
use glam::Vec3;
use shared::{assets::Config, collmesh, sim};
fn main() {
    let (col, src) = collmesh::build_player_collision(&Config::load()).unwrap();
    println!("collision: {src:?}, {} triangles", col.tri_count());
    let mut p = sim::Player::new(sim::find_free(&col, Vec3::new(-11740.0, 512.0, 1584.0)));
    let idle = sim::Input::default();
    for _ in 0..30 { sim::simulate(&mut p, &idle, 1.0 / 60.0, &col); }
    let a: Vec<f32> = std::env::args().skip(1).map(|v| v.parse().unwrap()).collect();
    let (yaw, fwd, right, ticks) = (a.first().copied().unwrap_or(0.0), a.get(1).copied().unwrap_or(1.0), a.get(2).copied().unwrap_or(0.0), a.get(3).copied().unwrap_or(300.0) as usize);
    let inp = sim::Input { forward: fwd, right, sprint: right == 0.0, yaw, ..Default::default() };
    let mut last = p.pos;
    for t in 0..ticks {
        sim::simulate(&mut p, &inp, 1.0 / 60.0, &col);
        if t % 60 == 0 || (p.pos - last).length() < 0.5 {
            println!("t{t:3} pos ({:.1},{:.1},{:.2}) vel ({:.0},{:.0},{:.0}) ground {}", p.pos.x, p.pos.y, p.pos.z, p.vel.x, p.vel.y, p.vel.z, p.on_ground);
        }
        if (p.pos - last).length() < 0.05 && t > 20 && std::env::var("NOSTOP").is_err() {
            println!("STUCK. contacts:");
            for off in [2.0f32, 4.0, 5.0, 8.0] { println!(" +{off}:"); for (tri, push) in col.contacts(p.pos + Vec3::new(off, 0.0, 0.0)) {
                println!("  push ({:.2},{:.2},{:.2})  tri {:?}", push.x, push.y, push.z, tri.map(|v| [v.x.round(), v.y.round(), v.z.round()])); }
            }
            let mut q = p.pos; let mut v = Vec3::new(300.0, 0.0, 0.0);
            let c = col.move_capsule(&mut q, Vec3::new(5.0, 0.0, 0.0), &mut v);
            println!("move_capsule +5x: {:?} -> {:?} vel {:?} ground {} wall {} ceil {}", p.pos, q, v, c.ground, c.wall, c.ceiling);
            for (tri, push) in col.contacts(p.pos) { println!("  at pos: push {:?} tri {:?}", push, tri); }
            let mut q2 = p.pos + Vec3::new(5.0, 0.0, 0.0); let mut v2 = Vec3::ZERO;
            let c2 = col.resolve(&mut q2, &mut v2);
            println!("resolve(pos+5): -> {:?} ground {} wall {}", q2, c2.ground, c2.wall);
            break;
        }
        last = p.pos;
    }
}
