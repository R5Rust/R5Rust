//! Replays tools/r5f-probe/plan.json through R5Rust's sim on the real Firing Range collision and compares every run
//! against the real game's log (tools/r5f-probe/out/r5f_client.log = the S21 client's per-frame prediction, else
//! out/r5f.log = the server's samples). cargo run --release -p shared --example probecmp [-- run_name_filter]
use glam::Vec3;
use shared::{assets::Config, collision::Collision, glb, sim::{self, Input, Player}};
use std::collections::{BTreeMap, HashSet};

const TICK: f32 = 1.0 / 60.0;

#[derive(Clone, Copy, Default)]
struct S { t: f32, p: Vec3, v: Vec3, g: bool }

fn load_log(path: &str, tag: &str) -> BTreeMap<String, Vec<S>> {
    let mut m: BTreeMap<String, Vec<S>> = BTreeMap::new();
    let Ok(txt) = std::fs::read_to_string(path) else { return m };
    for line in txt.lines() {
        let Some(i) = line.find(tag) else { continue };
        let f: Vec<&str> = line[i + tag.len()..].split_whitespace().collect();
        if f.len() < 9 { continue }
        let n = |k: usize| f[k].parse::<f32>().unwrap_or(0.0);
        m.entry(f[0].to_string()).or_default().push(S { t: n(1), p: Vec3::new(n(2), n(3), n(4)), v: Vec3::new(n(5), n(6), n(7)), g: f[8] == "1" });
    }
    m
}

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/r5f-probe");
    let plan: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/plan.json")).expect("plan.json")).unwrap();
    let mut real = load_log(&format!("{dir}/out/r5f_client.log"), "[RPROBE] c ");
    let src = if real.is_empty() { real = load_log(&format!("{dir}/out/r5f.log"), "[RPROBE] s "); "server" } else { "client" };
    println!("reference: R5Flowstate {src} log, {} runs", real.len());
    let w = glb::load_world(&Config::load().world_glb(), glb::Mode::Geometry, true).unwrap();
    let idx: Vec<u32> = w.draws.iter().filter(|d| w.materials[d.material as usize].solid()).flat_map(|d| w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].iter().copied()).collect();
    let col = Collision::build(w.positions.clone(), &idx);
    let spot = &plan["spot"];
    let start = Vec3::new(spot[0].as_f64().unwrap() as f32, spot[1].as_f64().unwrap() as f32, spot[2].as_f64().unwrap() as f32);
    let yaw = spot[3].as_f64().unwrap() as f32;
    let csv_dir = format!("{dir}/out/cmp");
    let _ = std::fs::create_dir_all(&csv_dir);

    for r in plan["runs"].as_array().unwrap() {
        let name = r["name"].as_str().unwrap();
        if !name.contains(&filter) { continue }
        // settle like the probe's Settle(): drop onto the floor and stand still
        let mut p = Player::new(sim::find_free(&col, start));
        for _ in 0..120 {
            sim::simulate(&mut p, &Input { yaw, ..Default::default() }, TICK, &col);
        }
        // the real run starts wherever the real player settled; align to its first sample
        let rl = real.get(name);
        if let Some(rs) = rl.and_then(|v| v.first()) {
            p.pos = sim::find_free(&col, rs.p);
            for _ in 0..30 {
                sim::simulate(&mut p, &Input { yaw, ..Default::default() }, TICK, &col);
            }
        }
        let p0 = p.pos;
        // event list -> per-tick held set + one-frame presses
        let mut evs: Vec<(f32, String)> = r["ev"].as_array().unwrap().iter().map(|e| (e["t"].as_f64().unwrap() as f32 / 1000.0, e["c"].as_str().unwrap().to_string())).collect();
        evs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let end = evs.iter().find(|e| e.1 == "END").map(|e| e.0).unwrap_or(4.0);
        let mut held: HashSet<String> = HashSet::new();
        let mut spam: Vec<(String, usize, u32)> = vec![]; // (cmd, start tick, presses)
        let mut ours: Vec<S> = vec![];
        let ticks = (end / TICK).ceil() as usize;
        let mut ei = 0;
        for k in 0..=ticks {
            let t = k as f32 * TICK;
            while ei < evs.len() && evs[ei].0 <= t + 1e-4 {
                let c = &evs[ei].1;
                if let Some(x) = c.strip_prefix('+') { held.insert(x.to_string()); }
                else if let Some(x) = c.strip_prefix('-') { held.remove(x); }
                else if let Some(n) = c.strip_prefix("JUMP:") { spam.push(("jump".into(), k, n.parse().unwrap())); }
                else if let Some(n) = c.strip_prefix("TAP:") { spam.push(("forward".into(), k, n.parse().unwrap())); }
                ei += 1;
            }
            let mut now = held.clone();
            for (c, k0, n) in &spam {
                let d = k - k0;
                if d < 2 * *n as usize && d % 2 == 0 { now.insert(c.clone()); }
            }
            let h = |s: &str| now.contains(s);
            let input = Input {
                forward: (h("forward") as i32 - h("back") as i32) as f32,
                right: (h("moveright") as i32 - h("moveleft") as i32) as f32,
                jump: h("jump"),
                sprint: h("speed"),
                crouch: h("duck"),
                yaw,
                ..Default::default()
            };
            ours.push(S { t, p: p.pos - p0, v: p.vel, g: p.on_ground });
            sim::simulate(&mut p, &input, TICK, &col);
        }
        // compare
        let mut csv = String::from("t,ours_x,ours_y,ours_z,ours_vx,ours_vy,ours_vz,real_x,real_y,real_z,real_vx,real_vy,real_vz\n");
        let Some(rs) = rl else { println!("{name:28} (no real samples)"); continue };
        let r0 = rs[0].p;
        let at = |t: f32| -> S {
            let i = rs.partition_point(|s| s.t < t).min(rs.len() - 1);
            let s = rs[i];
            S { p: s.p - r0, ..s }
        };
        let mut worst = 0.0f32;
        let mut summary = vec![];
        for o in &ours {
            let rr = at(o.t);
            worst = worst.max((o.p - rr.p).length());
            csv += &format!("{:.4},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}\n", o.t, o.p.x, o.p.y, o.p.z, o.v.x, o.v.y, o.v.z, rr.p.x, rr.p.y, rr.p.z, rr.v.x, rr.v.y, rr.v.z);
        }
        for tt in [0.25f32, 0.5, 1.0, 1.5, 1.6, 1.75, 2.0, 2.5, 3.0] {
            if tt > end { break }
            let o = ours[((tt / TICK) as usize).min(ours.len() - 1)];
            let rr = at(tt);
            summary.push(format!("t{tt}: hs {:.0}/{:.0} vz {:.0}/{:.0} d {:.0}", o.v.truncate().length(), rr.v.truncate().length(), o.v.z, rr.v.z, (o.p - rr.p).length()));
        }
        std::fs::write(format!("{csv_dir}/{name}.csv"), csv).ok();
        println!("{name:28} max pos diff {worst:7.1}u   ours/real: {}", summary.join(" | "));
    }
}
