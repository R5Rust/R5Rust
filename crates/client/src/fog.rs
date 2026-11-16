//! Height fog from the map's env_fog_controller entity, mapped onto the parameters of Apex's fog formula
//! (docs/apex-lighting.md section 7). The engine-side mapping isn't visible in the shaders, so this is our reading
//! of the entity keys:
//!   k0 = (density at fogzbottom, density above fogztop, density change across [bottom, top], max opacity)
//!   k1 = (fog colour * strength, -fogdistoffset)       k2 = (dir colour * strength, cos(dirhalfangle))
//!   k3 = (direction towards the fog lobe, 1 / (1 - cos))  k4 = (fogzbottom, fogztop, 1 / (top - bottom), 0)
//! Half-distance keys become extinction: density = ln2 / halfdist.

use std::path::Path;

pub struct Fog {
    pub k: [[f32; 4]; 5],
}

fn srgb(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn parse_ent(text: &str, class: &str) -> Option<std::collections::HashMap<String, String>> {
    for blk in text.split('{').skip(1) {
        let blk = blk.split('}').next().unwrap_or("");
        let mut kv = std::collections::HashMap::new();
        for line in blk.lines() {
            let parts: Vec<&str> = line.split('"').collect();
            if parts.len() >= 4 {
                kv.insert(parts[1].to_string(), parts[3].to_string());
            }
        }
        if kv.get("classname").map(|c| c == class).unwrap_or(false) {
            return Some(kv);
        }
    }
    None
}

impl Fog {
    pub fn none() -> Self {
        Fog { k: [[0.0; 4]; 5] }
    }

    pub fn from_map(bsp_dir: &Path, sun_dir: [f32; 3]) -> Self {
        let mut text = String::new();
        if let Ok(rd) = std::fs::read_dir(bsp_dir) {
            for e in rd.flatten() {
                if e.path().extension().map(|x| x == "ent").unwrap_or(false) {
                    text.push_str(&String::from_utf8_lossy(&std::fs::read(e.path()).unwrap_or_default()));
                }
            }
        }
        let Some(kv) = parse_ent(&text, "env_fog_controller") else {
            log::info!("fog: no env_fog_controller");
            return Fog::none();
        };
        let f = |k: &str, d: f32| kv.get(k).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(d);
        let col = |k: &str| -> [f32; 3] {
            let v: Vec<f32> = kv.get(k).map(|s| s.split_whitespace().filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
            [0, 1, 2].map(|i| srgb(v.get(i).copied().unwrap_or(255.0) / 255.0))
        };
        if f("fogenable", 1.0) == 0.0 {
            return Fog::none();
        }
        let ln2 = std::f32::consts::LN_2;
        let (zb, zt) = (f("fogzbottom", 0.0), f("fogztop", 10000.0));
        let d_bottom = ln2 / f("foghalfdistbottom", 10000.0).max(1.0);
        let d_top = ln2 / f("foghalfdisttop", 10000.0).max(1.0);
        let fc = col("fogcolor").map(|c| c * f("fogcolorstrength", 1.0));
        let dc = col("fogdircolor").map(|c| c * f("fogdircolorstrength", 0.0));
        let cos_h = f("fogdirhalfangle", 90.0).to_radians().cos();
        let max_op = f("fogdensity", 1.0).clamp(0.0, 1.0);
        let k = [
            [d_bottom, d_top, d_top - d_bottom, max_op],
            [fc[0], fc[1], fc[2], -f("fogdistoffset", 0.0)],
            [dc[0], dc[1], dc[2], cos_h],
            [sun_dir[0], sun_dir[1], sun_dir[2], 1.0 / (1.0 - cos_h).max(1e-3)],
            [zb, zt, 1.0 / (zt - zb).max(1.0), 0.0],
        ];
        log::info!("fog: colour {fc:?} dir {dc:?} half-dist {}/{} offset {} max {max_op}", f("foghalfdistbottom", 0.0), f("foghalfdisttop", 0.0), f("fogdistoffset", 0.0));
        Fog { k }
    }
}
