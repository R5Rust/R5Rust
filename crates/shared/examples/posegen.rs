//! Camera poses spread over the Firing Range for render side-by-sides: floor points with head room, far apart,
//! each looking along its longest open line of sight. Writes tools/r5f-probe/poses.json ([name, x, y, z_feet, pitch, yaw]).
//! cargo run --release -p shared --example posegen -- [count]
use glam::Vec3;
use shared::{assets::Config, collision::Collision, glb};
fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(24);
    let w = glb::load_world(&Config::load().world_glb(), glb::Mode::Geometry, true).unwrap();
    let idx: Vec<u32> = w.draws.iter().filter(|d| w.materials[d.material as usize].solid()).flat_map(|d| w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].iter().copied()).collect();
    let col = Collision::build(w.positions.clone(), &idx);
    // candidates: walkable floor between z -400 and 2000 (the range + spawn decks), 3 m+ head room
    let mut cand = vec![];
    let mut y = -9000.0f32;
    while y < 9000.0 {
        let mut x = -16000.0f32;
        while x < 2000.0 {
            if let Some(t) = col.ray(Vec3::new(x, y, 2200.0), -Vec3::Z, 2600.0) {
                let z = 2200.0 - t;
                if z > -400.0 && z < 2000.0 && col.ray(Vec3::new(x, y, z + 2.0), Vec3::Z, 150.0).is_none() {
                    cand.push(Vec3::new(x, y, z));
                }
            }
            x += 200.0;
        }
        y += 200.0;
    }
    // farthest-point sampling from the user's reference shot
    let mut picks = vec![Vec3::new(-6724.0, 55.0, -32.0)];
    while picks.len() < n && !cand.is_empty() {
        let (i, _) = cand.iter().enumerate().map(|(i, c)| (i, picks.iter().map(|p| (*p - *c).truncate().length()).fold(f32::MAX, f32::min))).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
        picks.push(cand.swap_remove(i));
    }
    let mut out = vec![];
    for (k, p) in picks.iter().enumerate() {
        let eye = *p + Vec3::Z * 60.0;
        let (mut best, mut by) = (0.0f32, 0.0f32);
        for a in 0..16 {
            let yaw = a as f32 * 22.5;
            let d = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), -0.08).normalize();
            let len = col.ray(eye, d, 6000.0).unwrap_or(6000.0);
            if len > best { best = len; by = yaw; }
        }
        let (yaw, pitch) = if k == 0 { (44.65, 0.0) } else { (by, 5.0) };
        out.push(format!("[\"map{k:02}\", {:.0}, {:.0}, {:.1}, {pitch}, {yaw}]", p.x, p.y, p.z));
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/r5f-probe/poses.json");
    std::fs::write(path, format!("[\n{}\n]\n", out.join(",\n"))).unwrap();
    println!("{} poses -> tools/r5f-probe/poses.json (from {} candidates)", out.len(), cand.len() + picks.len());
    let f = col.ray(Vec3::new(-6724.0, 55.0, 200.0), -Vec3::Z, 600.0).map(|t| 200.0 - t);
    println!("floor under the reference shot: {f:?}");
}
