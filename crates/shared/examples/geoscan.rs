//! Finds clean test geometry for the probe runs on the game's own collision, from floor-height profiles along
//! straight lines: LEDGE (a flat run-up, a vertical rise of 30..100u, a flat top), WALL (a flat run-up into a
//! vertical wall at least 500u tall), SLOPE (a constant grade over 300u+), DROP (a flat run-up to a drop).
//!   cargo run --release -p shared --example geoscan -- cx,cy,cz radius [step]
use glam::Vec3;
use shared::{assets::Config, collmesh};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a[0] == "trace" {
        // trace x,y,z dx,dy,dz [crouch]: the standing (or crouched) hull swept by d; every triangle it touches
        let (col, _) = collmesh::build_player_collision(&Config::load()).unwrap();
        let p: Vec<f32> = a[1].split(',').map(|v| v.parse().unwrap()).collect();
        let d: Vec<f32> = a[2].split(',').map(|v| v.parse().unwrap()).collect();
        let h = if a.get(3).is_some() { 47.0 } else { 72.0 };
        let (s, e) = (Vec3::new(p[0], p[1], p[2]), Vec3::new(p[0] + d[0], p[1] + d[1], p[2] + d[2]));
        let t = col.trace_hull(s, e, Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, h));
        println!("fraction {:.4} normal {:?} startsolid {}", t.fraction, t.normal, t.startsolid);
        for (tri, src, f, n) in col.trace_hull_detail(s, e, Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, h)) {
            println!("  frac {f:.4} src {src} n ({:.3},{:.3},{:.3}) tri {:?}", n.x, n.y, n.z, tri.map(|v| [v.x.round(), v.y.round(), v.z.round()]));
        }
        return;
    }
    if a[0] == "profile" {
        // profile x,y,z yaw len: floor height every 16u (ray from 200 above the last floor) and walls at knee height
        let (col, _) = collmesh::build_player_collision(&Config::load()).unwrap();
        let c: Vec<f32> = a[1].split(',').map(|v| v.parse().unwrap()).collect();
        let yaw: f32 = a[2].parse().unwrap();
        let len: f32 = a[3].parse().unwrap();
        let d = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
        let mut zc = c[2];
        let mut i = 0.0;
        while i <= len {
            let q = Vec3::new(c[0], c[1], 0.0) + d * i;
            let f = col.ray(Vec3::new(q.x, q.y, zc + 200.0), -Vec3::Z, 1000.0).map(|t| zc + 200.0 - t);
            let wall = col.ray(Vec3::new(q.x, q.y, f.unwrap_or(zc) + 20.0), d, 16.0).is_some();
            println!("{i:5.0}  ({:.0},{:.0})  floor {}{}", q.x, q.y, f.map(|z| format!("{z:8.1}")).unwrap_or("    none".into()), if wall { "  WALL" } else { "" });
            if let Some(z) = f { zc = z }
            i += 16.0;
        }
        return;
    }
    let c: Vec<f32> = a[0].split(',').map(|v| v.parse().unwrap()).collect();
    let r: f32 = a[1].parse().unwrap();
    let step: f32 = a.get(2).map(|v| v.parse().unwrap()).unwrap_or(64.0);
    let (col, _) = collmesh::build_player_collision(&Config::load()).unwrap();
    let floor = |x: f32, y: f32, top: f32| col.ray(Vec3::new(x, y, top), -Vec3::Z, 400.0).map(|t| top - t);
    let mut out: Vec<(f32, String)> = vec![];
    let ds = 8.0;
    let mut y = c[1] - r;
    while y <= c[1] + r {
        let mut x = c[0] - r;
        while x <= c[0] + r {
            let Some(z0) = floor(x, y, c[2] + 200.0) else { x += step; continue };
            if col.ray(Vec3::new(x, y, z0 + 2.0), Vec3::Z, 120.0).is_some() {
                x += step;
                continue;
            }
            for yaw in [0.0f32, 90.0, 180.0, 270.0] {
                let d = Vec3::new(yaw.to_radians().cos().round(), yaw.to_radians().sin().round(), 0.0);
                let dist0 = (Vec3::new(x, y, 0.0) - Vec3::new(c[0], c[1], 0.0)).length();
                // floor profile, following the floor (ray from 150 above the last height)
                let mut prof = vec![z0];
                let mut zc = z0;
                let mut wall_at = None;
                for i in 1..120 {
                    let px = x + d.x * i as f32 * ds;
                    let py = y + d.y * i as f32 * ds;
                    // a wall: a horizontal ray at knee and chest height hits within one step
                    let prev = Vec3::new(px - d.x * ds, py - d.y * ds, zc);
                    let knee = col.ray(prev + Vec3::Z * 20.0, d, ds).is_some() && col.ray(prev + Vec3::Z * 120.0, d, ds).is_some();
                    if knee {
                        wall_at = Some(i);
                        break;
                    }
                    match floor(px, py, zc + 150.0) {
                        Some(z) => {
                            prof.push(z);
                            zc = z;
                        }
                        None => break,
                    }
                }
                let flat = |a: usize, b: usize, z: f32| (a..b.min(prof.len())).all(|i| (prof[i] - z).abs() < 2.5) && b <= prof.len();
                let tag = |kind: &str, extra: String| (dist0, format!("{kind:6} start {x:.0},{y:.0},{z0:.1} yaw {yaw:3.0} {extra}"));
                // LEDGE: flat for >= 200u, then a rise of 30..100 within one step, then flat for >= 120u
                for i in 25..prof.len().saturating_sub(16) {
                    let h = prof[i] - prof[i - 1];
                    if (std::env::var("RISE_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(30.0)..100.0).contains(&h) && flat(0, i, z0) && flat(i, i + 16, prof[i]) {
                        // headroom over the top
                        let top = Vec3::new(x + d.x * (i as f32 * ds + 32.0), y + d.y * (i as f32 * ds + 32.0), prof[i] + 2.0);
                        if col.ray(top, Vec3::Z, 100.0).is_none() {
                            out.push(tag("LEDGE", format!("rise {h:.1} at {:.0}u", i as f32 * ds)));
                        }
                        break;
                    }
                    if (prof[i] - prof[i - 1]).abs() > 2.5 {
                        break;
                    }
                }
                // DROP: flat >= 150u then a fall of 24..300
                for i in 19..prof.len() {
                    let h = prof[i - 1] - prof[i];
                    if h > 24.0 && flat(0, i, z0) {
                        out.push(tag("DROP", format!("fall {h:.0} at {:.0}u", i as f32 * ds)));
                        break;
                    }
                    if (prof[i] - prof[i - 1]).abs() > 2.5 {
                        break;
                    }
                }
                // WALL: flat >= 250u then a wall >= 500u tall
                if let Some(i) = wall_at {
                    if i > 31 && flat(0, i, z0) {
                        let base = Vec3::new(x + d.x * (i - 1) as f32 * ds, y + d.y * (i - 1) as f32 * ds, z0);
                        let tall = (1..=25).all(|k| col.ray(base + Vec3::Z * (k as f32 * 20.0), d, ds + 4.0).is_some());
                        if tall {
                            out.push(tag("WALL", format!("at {:.0}u", i as f32 * ds)));
                        }
                    }
                }
                // SLOPE: from the start, a steady grade over >= 300u (38 steps)
                if prof.len() > 40 {
                    let g = (prof[38] - prof[0]) / (38.0 * ds);
                    let steady = (1..39).all(|i| ((prof[i] - prof[i - 1]) / ds - g).abs() < 0.08);
                    if steady && g.abs() > 0.15 {
                        out.push(tag("SLOPE", format!("grade {g:.2} ({:.0} deg) over {:.0}u", g.atan().to_degrees(), 38.0 * ds)));
                    }
                }
            }
            x += step;
        }
        y += step;
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for kind in ["LEDGE", "DROP", "WALL", "SLOPE"] {
        println!("== {kind}");
        for (_, h) in out.iter().filter(|h| h.1.starts_with(kind)).take(20) {
            println!("  {h}");
        }
    }
}
