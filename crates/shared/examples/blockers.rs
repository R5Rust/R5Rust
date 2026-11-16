//! cargo run --release -p shared --example blockers -- x,y,z : materials of triangles touching a player capsule at feet pos.
use glam::Vec3;
use shared::assets::Config;
use shared::collision::{AXIS_LEN, RADIUS};
use shared::glb;

fn closest_on_seg(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    a + ab * ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
}
fn closest_on_tri(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    // Ericson
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 { return a; }
    let bp = p - b; let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 { return b; }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 { return a + ab * (d1 / (d1 - d3)); }
    let cp = p - c; let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 { return c; }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 { return a + ac * (d2 / (d2 - d6)); }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 { return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6))); }
    let den = 1.0 / (va + vb + vc);
    a + ab * (vb * den) + ac * (vc * den)
}

fn main() {
    let p: Vec<f32> = std::env::args().nth(1).unwrap().split(',').map(|v| v.parse().unwrap()).collect();
    let feet = Vec3::new(p[0], p[1], p[2]);
    let margin: f32 = std::env::args().nth(2).map(|v| v.parse().unwrap()).unwrap_or(4.0);
    let w = glb::load_world(&Config::load().world_glb(), glb::Mode::Geometry, false).unwrap();
    let (s0, s1) = (feet + Vec3::Z * RADIUS, feet + Vec3::Z * (RADIUS + AXIS_LEN));
    let mut hits = std::collections::BTreeMap::new();
    for d in &w.draws {
        let m = &w.materials[d.material as usize];
        for t in w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(w.positions[i as usize]));
            // iterate: closest point between segment and triangle
            let mut q = closest_on_seg(s0, s1, (a + b + c) / 3.0);
            let mut tp = closest_on_tri(q, a, b, c);
            for _ in 0..4 { q = closest_on_seg(s0, s1, tp); tp = closest_on_tri(q, a, b, c); }
            let dist = (q - tp).length();
            if dist < RADIUS + margin {
                let e = hits.entry(format!("{} (alpha {})", m.name, m.alpha_mode)).or_insert((0, f32::MAX, Vec3::ZERO));
                e.0 += 1;
                if dist < e.1 { e.1 = dist; e.2 = tp; }
            }
        }
    }
    for (k, (n, d, tp)) in hits { println!("{n:5} tris  min dist {d:6.1}  at ({:.0},{:.0},{:.0})  {k}", tp.x, tp.y, tp.z); }
}
