//! Lists every triangle under (or around) a point in both collision sources: the game's
//! collision.bin (contents mask + source, including what players pass through) and world.glb's
//! render triangles (material, solid or not). For working out why the two disagree somewhere.
//!
//!   cargo run --release -p shared --example collprobe -- x,y [zmin zmax] [radius] [collision.bin]
use glam::{Vec2, Vec3};
use shared::{assets::Config, collmesh, glb};

/// Height of the triangle's plane at (x, y) if (x, y) is inside its XY projection.
fn height_at(t: [Vec3; 3], p: Vec2) -> Option<f32> {
    let (a, b, c) = (t[0], t[1], t[2]);
    let d = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
    if d.abs() < 1e-9 {
        return None;
    }
    let w0 = ((b.y - c.y) * (p.x - c.x) + (c.x - b.x) * (p.y - c.y)) / d;
    let w1 = ((c.y - a.y) * (p.x - c.x) + (a.x - c.x) * (p.y - c.y)) / d;
    let w2 = 1.0 - w0 - w1;
    (w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4).then_some(w0 * a.z + w1 * b.z + w2 * c.z)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let xy: Vec<f32> = a.first().expect("x,y").split(',').map(|v| v.parse().unwrap()).collect();
    let p = Vec2::new(xy[0], xy[1]);
    let zmin: f32 = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(-1e9);
    let zmax: f32 = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(1e9);
    let radius: f32 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let cfg = Config::load();
    let bin = a.get(4).map(std::path::PathBuf::from).unwrap_or_else(|| cfg.collision_bin());
    let names = ["world poly", "world hull", "static prop", "prop physics"];

    // radius 0: the plane height right under the point; radius > 0: the highest corner of every
    // triangle whose box comes within `radius` (what a hull of that half-width could stand on).
    let pick = |v: [Vec3; 3]| -> Option<f32> {
        if radius > 0.0 {
            let (lo, hi) = (v[0].min(v[1]).min(v[2]), v[0].max(v[1]).max(v[2]));
            let near = p.x >= lo.x - radius && p.x <= hi.x + radius && p.y >= lo.y - radius && p.y <= hi.y + radius;
            near.then_some(hi.z)
        } else {
            height_at(v, p)
        }
    };
    let m = collmesh::CollMesh::load(&bin).expect("collision.bin");
    println!("== game collision ({}) at ({:.0},{:.0}) r {radius}:", bin.display(), p.x, p.y);
    let mut rows = Vec::new();
    for (i, t) in m.tris.iter().enumerate() {
        let v = t.map(|k| Vec3::from(m.positions[k as usize]));
        if let Some(z) = pick(v).filter(|z| *z >= zmin && *z <= zmax) {
            rows.push((z, m.contents[i], m.source[i], m.player_solid(i)));
        }
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (z, c, s, ps) in rows.iter().take(40) {
        println!("  z {z:9.2}  contents {c:#010x}  {:<13}  {}", names.get(*s as usize).unwrap_or(&"?"), if *ps { "blocks players" } else { "players pass" });
    }

    let w = glb::load_world(&cfg.world_glb(), glb::Mode::Geometry, false).expect("world.glb");
    println!("== render triangles:");
    let mut rows = Vec::new();
    for d in &w.draws {
        let mat = &w.materials[d.material as usize];
        for t in w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].chunks_exact(3) {
            let v = [0, 1, 2].map(|k| Vec3::from(w.positions[t[k] as usize]));
            if let Some(z) = pick(v).filter(|z| *z >= zmin && *z <= zmax) {
                rows.push((z, mat.name.clone(), mat.solid()));
            }
        }
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (z, n, s) in rows.iter().take(40) {
        println!("  z {z:9.2}  {}  {n}", if *s { "solid" } else { "soft " });
    }
}
