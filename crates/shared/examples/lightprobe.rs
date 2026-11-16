//! cargo run --release -p shared --example lightprobe : dumps lighting-lump stats + debug PNG-ready raw.
use shared::assets::Config;
use shared::bsplight::BspLighting;
fn main() {
    let cfg = Config::load();
    let l = BspLighting::load(&cfg.bsp_dir()).unwrap();
    println!("atlas {}x{}  lights {}  probes {}  tree {}  refs {}  cubemaps {}  rtl_index {}", l.sky0.width, l.sky0.height, l.lights.len(), l.probes.len(), l.probe_tree.len(), l.probe_refs.len(), l.cubemaps.len(), l.rtl_index.len());
    let spawn = [-11740.0, 512.0, 1600.0];
    println!("spawn leaf {:?}", l.probe_leaf(spawn).iter().map(|r| (r.origin, r.probe)).collect::<Vec<_>>());
    if let Some(p) = l.nearest_probe(spawn) { println!("probe {:?}", l.probes[p as usize]); }
    let out = std::env::args().nth(1).unwrap_or(".".into());
    for (name, a, bpp) in [("sky0", &l.sky0, 4), ("sky1", &l.sky1, 4), ("rtl_b", &l.rtl_b, 4)] {
        let _ = bpp;
        std::fs::write(format!("{out}/{name}_{}x{}.rgba", a.width, a.height), &a.rgba).unwrap();
    }
    std::fs::write(format!("{out}/rtl_a_{}x{}.r16", l.rtl_a.width, l.rtl_a.height), &l.rtl_a.rgba).unwrap();
    let mut hist = std::collections::BTreeMap::new();
    for c in l.rtl_a.rgba.chunks_exact(2) { *hist.entry(u16::from_le_bytes([c[0], c[1]])).or_insert(0u32) += 1; }
    println!("rtl_a distinct values {} first: {:?}", hist.len(), hist.iter().take(12).collect::<Vec<_>>());
    println!("rtl_index {:?}", &l.rtl_index[..l.rtl_index.len().min(70)]);
}
