//! Which render geometry blocks the sun from a point? Casts toward the sun (the BSP's sun worldlight) and lists the
//! materials hit, nearest first. cargo run --release -p shared --example sunray -- x,y,z [x,y,z ...]
use glam::Vec3;
use shared::{assets::Config, bsplight::BspLighting, collision::Collision, glb};
fn main() {
    let cfg = Config::load();
    let w = glb::load_world(&cfg.world_glb(), glb::Mode::Geometry, false).unwrap();
    let bsp = BspLighting::load(&cfg.bsp_dir()).unwrap();
    let sun = bsp.lights.iter().find(|l| l.emit_type == 3).expect("sun");
    let to_sun = -Vec3::from(sun.normal).normalize();
    println!("to_sun {to_sun:?}");
    let pts: Vec<Vec3> = std::env::args().skip(1).map(|a| { let v: Vec<f32> = a.split(',').map(|x| x.parse().unwrap()).collect(); Vec3::new(v[0], v[1], v[2]) }).collect();
    // one BVH per material
    let mut by_mat: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
    for d in &w.draws {
        by_mat.entry(d.material).or_default().extend_from_slice(&w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize]);
    }
    let cols: Vec<(u32, Collision)> = by_mat.into_iter().map(|(m, idx)| (m, Collision::build(w.positions.clone(), &idx))).collect();
    for p in pts {
        let o = p + Vec3::Z * 8.0;
        let mut hits: Vec<(f32, String)> = cols.iter().filter_map(|(m, c)| c.ray(o, to_sun, 60000.0).map(|t| (t, format!("{} (key {:?})", w.materials[*m as usize].name, w.materials[*m as usize].solid())))).collect();
        hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        println!("from {p:?}:");
        for (t, n) in hits.iter().take(6) { println!("   {t:8.0}  {n}"); }
    }
}
