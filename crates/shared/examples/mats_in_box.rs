//! materials with triangles inside a box: cargo run --example mats_in_box -- x0,y0,z0 x1,y1,z1
use shared::{assets::Config, glb};
fn main() {
    let a: Vec<Vec<f32>> = std::env::args().skip(1).map(|s| s.split(',').map(|v| v.parse().unwrap()).collect()).collect();
    let w = glb::load_world(&Config::load().world_glb(), glb::Mode::Geometry, false).unwrap();
    let mut m = std::collections::BTreeMap::new();
    for d in &w.draws {
        for t in w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].chunks_exact(3) {
            let p = w.positions[t[0] as usize];
            if (0..3).all(|k| p[k] >= a[0][k] && p[k] <= a[1][k]) {
                *m.entry(w.materials[d.material as usize].name.clone()).or_insert(0) += 1;
            }
        }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (n, c) in v { println!("{c:6} {n}"); }
}
