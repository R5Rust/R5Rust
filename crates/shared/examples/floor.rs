//! floor heights along a line: cargo run --example floor -- x0,y,z x1 step
//! Uses collision.bin when present; RENDER=1 forces world.glb's render triangles.
use glam::Vec3;
use shared::{assets::Config, collision::Collision, collmesh, glb};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let p: Vec<f32> = a[0].split(',').map(|v| v.parse().unwrap()).collect();
    let (x1, step): (f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let col = if std::env::var_os("RENDER").is_some() {
        let w = glb::load_world(&Config::load().world_glb(), glb::Mode::Geometry, true).unwrap();
        Collision::build(w.positions.clone(), &collmesh::render_indices(&w))
    } else {
        let (col, src) = collmesh::build_player_collision(&Config::load()).unwrap();
        println!("collision: {src:?}, {} triangles", col.tri_count());
        col
    };
    let mut x = p[0];
    while x <= x1 {
        let o = Vec3::new(x, p[1], p[2] + 60.0);
        let down = col.ray(o, -Vec3::Z, 200.0).map(|t| o.z - t);
        let hs: Vec<String> = [2.0f32, 10.0, 20.0, 35.0, 50.0, 65.0, 72.0, 80.0].iter().map(|h| {
            let t = col.ray(Vec3::new(x, p[1], p[2] + h), Vec3::X, 64.0);
            format!("{h}:{}", t.map(|t| format!("{:.0}", t)).unwrap_or("-".into()))
        }).collect();
        let up = col.ray(Vec3::new(x, p[1], p[2] + 2.0), Vec3::Z, 400.0);
        println!("x {x:7.0}  floor {:?}  ceiling {:?}  ahead {}", down.map(|z| (z * 10.0).round() / 10.0), up.map(|t| t.round()), hs.join(" "));
        x += step;
    }
}
