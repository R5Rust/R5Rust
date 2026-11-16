//! Finds open flat floor for movement tests: cargo run --release -p shared --example flatspot -- cx,cy radius
//! Prints spots where a 1000-unit run along +X / -X / +Y / -Y is flat (+-2u) with 200u of headroom and no wall.
use glam::Vec3;
use shared::{assets::Config, collision::Collision, glb};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c: Vec<f32> = a[0].split(',').map(|v| v.parse().unwrap()).collect();
    let r: f32 = a[1].parse().unwrap();
    let w = glb::load_world(&Config::load().world_glb(), glb::Mode::Geometry, true).unwrap();
    let idx: Vec<u32> = w.draws.iter().filter(|d| w.materials[d.material as usize].solid()).flat_map(|d| w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].iter().copied()).collect();
    let col = Collision::build(w.positions.clone(), &idx);
    let floor = |x: f32, y: f32, top: f32| { let o = Vec3::new(x, y, top); col.ray(o, -Vec3::Z, 6000.0).map(|t| o.z - t) };
    let mut found = 0;
    let mut y = c[1] - r;
    while y <= c[1] + r && found < 30 {
        let mut x = c[0] - r;
        while x <= c[0] + r && found < 30 {
            if let Some(z0) = floor(x, y, c[2]).filter(|z| *z > c[2] - 300.0) {
                for (dx, dy, name) in [(1.0, 0.0, "+X yaw 0"), (-1.0, 0.0, "-X yaw 180"), (0.0, 1.0, "+Y yaw 90"), (0.0, -1.0, "-Y yaw 270")] {
                    let ok = (0..=20).all(|i| {
                        let (px, py) = (x + dx * i as f32 * 50.0, y + dy * i as f32 * 50.0);
                        let f = floor(px, py, z0 + 100.0);
                        let head = col.ray(Vec3::new(px, py, z0 + 2.0), Vec3::Z, 200.0).is_none();
                        let side = [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y].iter().all(|d| col.ray(Vec3::new(px, py, z0 + 30.0), *d, 120.0).is_none());
                        f.is_some_and(|f| (f - z0).abs() < 2.0) && head && side
                    });
                    if ok {
                        println!("{x:.0},{y:.0},{z0:.1}  {name}");
                        found += 1;
                    }
                }
            }
            x += 256.0;
        }
        y += 256.0;
    }
}
