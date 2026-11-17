//! Ray-casts the game collision (collision.bin) and the render triangles (world.glb) along the same lines and prints
//! both hit distances, to spot misplaced/rotated prop collision. cargo run --release -p shared --example collvsrender -- x,y,z dx,dy,dz [steps step]
use glam::Vec3;
use shared::{assets::Config, collision::Collision, collmesh, glb};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let v = |s: &str| { let f: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect(); Vec3::new(f[0], f[1], f[2]) };
    let (o, d) = (v(&a[0]), v(&a[1]).normalize());
    let steps: i32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(9);
    let step: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(8.0);
    let cfg = Config::load();
    let w = glb::load_world(&cfg.world_glb(), glb::Mode::Geometry, true).unwrap();
    let idx: Vec<u32> = w.draws.iter().filter(|d| w.materials[d.material as usize].solid()).flat_map(|d| w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].iter().copied()).collect();
    let render = Collision::build(w.positions.clone(), &idx);
    let (game, _src) = collmesh::build_player_collision(&cfg).expect("collision.bin");
    // sweep the ray sideways (perpendicular, horizontal) to map the outline
    let side = d.cross(Vec3::Z).normalize_or(Vec3::X);
    for k in -steps / 2..=steps / 2 {
        let p = o + side * (k as f32 * step);
        let r = render.ray(p, d, 400.0);
        let g = game.ray(p, d, 400.0);
        println!("offset {:6.0}: render {:>8}  game {:>8}", k as f32 * step, r.map(|t| format!("{t:.1}")).unwrap_or("-".into()), g.map(|t| format!("{t:.1}")).unwrap_or("-".into()));
    }
}
