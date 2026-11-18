//! Finds movement-test spots near a point on the real map (the game's own collision): sprints forward from a grid of
//! starts in 4 directions and reports where the run hits a mantle ledge, a step, a ledge to walk off, a slope or a
//! tall wall (climbable), with the approach length. Used to place tools/r5f-probe runs on real geometry.
//!   cargo run --release -p shared --example featscan -- cx,cy,cz radius [step]
use glam::Vec3;
use shared::{assets::Config, collmesh, sim::{self, Input, Player}};

const TICK: f32 = 1.0 / 60.0;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c: Vec<f32> = a[0].split(',').map(|v| v.parse().unwrap()).collect();
    let r: f32 = a[1].parse().unwrap();
    let step: f32 = a.get(2).map(|v| v.parse().unwrap()).unwrap_or(128.0);
    let (col, _) = collmesh::build_player_collision(&Config::load()).unwrap();
    let mut hits: Vec<(f32, String)> = vec![];
    let mut y = c[1] - r;
    while y <= c[1] + r {
        let mut x = c[0] - r;
        while x <= c[0] + r {
            let o = Vec3::new(x, y, c[2] + 300.0);
            if let Some(t) = col.ray(o, -Vec3::Z, 800.0) {
                let fz = o.z - t;
                for (yaw, jump) in [0.0f32, 90.0, 180.0, 270.0].into_iter().flat_map(|y| [(y, false), (y, true)]) {
                    let mut p = Player::new(sim::find_free(&col, Vec3::new(x, y, fz + 2.0)));
                    p.speed_scale = 1.15;
                    for _ in 0..30 {
                        sim::simulate(&mut p, &Input { yaw, ..Default::default() }, TICK, &col);
                    }
                    if !p.on_ground || (p.pos.z - fz).abs() > 4.0 {
                        continue;
                    }
                    let p0 = p.pos;
                    let inp = Input { forward: 1.0, sprint: true, jump, yaw, ..Default::default() };
                    let mut last = p;
                    let mut maxhs = 0.0f32;
                    let mut slope_run = 0;
                    let tag = |kind: &str, extra: String, p: &Player| {
                        let d = (p.pos - p0).truncate().length();
                        (((p0 - Vec3::from_slice(&c)).truncate().length()), format!("{kind:8}{} start {:.0},{:.0},{:.1} yaw {yaw:3.0} after {d:4.0}u  {extra}", if jump { "J" } else { " " }, p0.x, p0.y, p0.z))
                    };
                    for k in 0..200 {
                        sim::simulate(&mut p, &inp, TICK, &col);
                        let hs = p.vel.truncate().length();
                        maxhs = maxhs.max(hs);
                        if p.mantling() {
                            // finish the mantle and report the ledge height
                            for _ in 0..90 {
                                sim::simulate(&mut p, &inp, TICK, &col);
                                if !p.mantling() { break }
                            }
                            hits.push(tag("MANTLE", format!("ledge +{:.1}", p.pos.z - last.pos.z), &p));
                            break;
                        }
                        if p.on_wall() {
                            hits.push(tag("CLIMB", String::new(), &p));
                            break;
                        }
                        if jump && !p.mantling() && !p.on_wall() { last = p; continue }
                        if p.on_ground && last.on_ground && p.pos.z - last.pos.z > 3.0 {
                            hits.push(tag("STEP", format!("+{:.1}", p.pos.z - last.pos.z), &p));
                            break;
                        }
                        if !p.on_ground && last.on_ground && p.vel.z <= 0.0 {
                            // walked off: how far down?
                            let mut q = p;
                            for _ in 0..120 {
                                sim::simulate(&mut q, &inp, TICK, &col);
                                if q.on_ground { break }
                            }
                            if p.pos.z - q.pos.z > 24.0 && q.on_ground {
                                hits.push(tag("DROP", format!("-{:.0}", p.pos.z - q.pos.z), &p));
                            }
                            break;
                        }
                        if p.on_ground && last.on_ground && k > 20 {
                            let g = (p.pos.z - last.pos.z) / (p.pos - last.pos).truncate().length().max(0.01);
                            if g.abs() > 0.18 && g.abs() < 0.9 { slope_run += 1 } else { slope_run = 0 }
                            if slope_run == 15 {
                                hits.push(tag("SLOPE", format!("grade {:.2} ({:.0} deg)", g, g.atan().to_degrees()), &p));
                                break;
                            }
                        }
                        if k > 30 && maxhs > 250.0 && hs < 30.0 && p.on_ground {
                            // stopped by a wall: how tall is it?
                            let f = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
                            let top = Vec3::new(p.pos.x + f.x * 30.0, p.pos.y + f.y * 30.0, p.pos.z + 1500.0);
                            let h = col.ray(top, -Vec3::Z, 1500.0).map(|t| 1500.0 - t).unwrap_or(1500.0);
                            if h > 180.0 {
                                hits.push(tag("WALL", format!("height {h:.0}"), &p));
                            }
                            break;
                        }
                        last = p;
                    }
                }
            }
            x += step;
        }
        y += step;
    }
    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for kind in ["MANTLE", "STEP", "DROP", "SLOPE", "WALL", "CLIMB"] {
        println!("== {kind}");
        for (_, h) in hits.iter().filter(|h| h.1.starts_with(kind)).take(25) {
            println!("  {h}");
        }
    }
}
