//! Replays tools/r5f-probe/plan.json through R5Rust's sim on the game's own collision (collision.bin) and compares
//! every run against the real game's log (tools/r5f-probe/out/r5f_client.log = the S21 client's per-frame
//! prediction, else out/r5f.log = the server's samples).
//!   cargo run --release -p shared --example probecmp [-- run_name_filter]
//! With a real log, our sim steps with the real frame times and the real view yaw (the probe can turn with
//! +left/+right), starting from the real first sample. Without one it previews the plan at a fixed 60 Hz.
//! Per run it prints the worst horizontal-speed error (each real frame against our nearest of the same frame +-1,
//! so one frame of input jitter is not counted as a speed error), the worst vertical-speed error and the worst
//! position error. PROBE_TRACE=1 prints every tick.
use glam::Vec3;
use shared::{assets::Config, collision::Collision, collmesh, sim::{self, Input, Player}};
use std::collections::{BTreeMap, HashSet};

const TICK: f32 = 1.0 / 60.0;

#[derive(Clone, Copy, Default)]
struct S {
    t: f32,
    p: Vec3,
    v: Vec3,
    g: bool,
    yaw: f32,
    pitch: f32,
    /// real: 1 slide, 2 crouched, 4 traversing, 8 wallrunning (climb); ours: the same bits
    fl: u32,
    /// real: the client's game clock (curtime) since the run started, -1 in logs that predate it
    ct: f32,
}

fn load_log(path: &str, tag: &str) -> BTreeMap<String, Vec<S>> {
    let mut m: BTreeMap<String, Vec<S>> = BTreeMap::new();
    let Ok(txt) = std::fs::read_to_string(path) else { return m };
    for line in txt.lines() {
        let Some(i) = line.find(tag) else { continue };
        let f: Vec<&str> = line[i + tag.len()..].split_whitespace().collect();
        if f.len() < 9 {
            continue;
        }
        let n = |k: usize| f.get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        m.entry(f[0].to_string()).or_default().push(S {
            t: n(1),
            p: Vec3::new(n(2), n(3), n(4)),
            v: Vec3::new(n(5), n(6), n(7)),
            g: f[8] == "1",
            yaw: n(9),
            fl: n(10) as u32,
            pitch: n(11),
            ct: f.get(12).and_then(|v| v.parse::<f32>().ok()).unwrap_or(-1.0),
        });
    }
    m
}

fn flags(p: &Player) -> u32 {
    (p.sliding as u32) | (((p.duck_state == 2) as u32) << 1) | ((p.mantling() as u32) << 2) | ((p.on_wall() as u32) << 3)
}

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/r5f-probe");
    let plan: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/plan.json")).expect("plan.json")).unwrap();
    let mut real = load_log(&format!("{dir}/out/r5f_client.log"), "[RPROBE] c ");
    let src = if real.is_empty() {
        real = load_log(&format!("{dir}/out/r5f.log"), "[RPROBE] s ");
        "server"
    } else {
        "client"
    };
    println!("reference: R5Flowstate {src} log, {} runs", real.len());
    // The S21 client has no "+back" command (it is "+backward"). Logs recorded since the probe maps +back to
    // +backward carry this marker; older logs replay with "back" ignored, exactly as the game received them.
    let back_live = std::fs::read_to_string(format!("{dir}/out/r5f_client.log")).map(|t| t.contains("[RPROBE] backward-fix")).unwrap_or(false);
    let (col, csrc) = collmesh::build_player_collision(&Config::load()).expect("collision");
    println!("collision: {csrc:?}, {} triangles", col.tri_count());
    let csv_dir = format!("{dir}/out/cmp");
    let _ = std::fs::create_dir_all(&csv_dir);
    let mut table = vec![];

    for r in plan["runs"].as_array().unwrap() {
        let name = r["name"].as_str().unwrap();
        if !name.contains(&filter) {
            continue;
        }
        let spot = r.get("spot").unwrap_or(&plan["spot"]);
        let sp = |i: usize| spot[i].as_f64().unwrap() as f32;
        let (start, yaw0) = (Vec3::new(sp(0), sp(1), sp(2)), sp(3));
        // PROBE_NOREAL=1: preview the plan from its own spots, ignoring the log
        let rl = real.get(name).filter(|v| v.len() > 2 && std::env::var_os("PROBE_NOREAL").is_none());
        let (res, worst) = replay(&col, r, name, start, yaw0, rl, back_live);
        if let Some(w) = worst {
            table.push((name.to_string(), w));
        }
        std::fs::write(format!("{csv_dir}/{name}.csv"), res).ok();
    }
    if !table.is_empty() {
        println!("\n{:28} {:>8} {:>8} {:>8} {:>8}  (worst over the run; target hs <= 2, pos <= 10)", "run", "hs err", "hs mean", "vz err", "pos err");
        for (n, w) in &table {
            let ok = w[0] <= 2.0 && w[3] <= 10.0;
            println!("{n:28} {:8.1} {:8.2} {:8.1} {:8.1}  {}", w[0], w[1], w[2], w[3], if ok { "ok" } else { "" });
        }
        let ok = table.iter().filter(|(_, w)| w[0] <= 2.0 && w[3] <= 10.0).count();
        println!("{ok}/{} runs within target", table.len());
    }
}

/// Replays one run; returns the CSV and, with real samples, [max hs err, mean hs err, max vz err, max pos err].
fn replay(col: &Collision, r: &serde_json::Value, name: &str, start: Vec3, yaw0: f32, rl: Option<&Vec<S>>, back_live: bool) -> (String, Option<[f32; 4]>) {
    let yaw_start = rl.map(|v| v[0].yaw).unwrap_or(yaw0);
    let at0 = rl.map(|v| v[0].p).unwrap_or(start);
    let mut p = Player::new(sim::find_free(col, at0));
    p.speed_scale = 1.15; // holstered, as the probe's client
    for _ in 0..90 {
        sim::simulate(&mut p, &Input { yaw: yaw_start, ..Default::default() }, TICK, col);
    }
    let p0 = p.pos;
    let mut evs: Vec<(f32, String)> = r["ev"].as_array().unwrap().iter().map(|e| (e["t"].as_f64().unwrap() as f32 / 1000.0, e["c"].as_str().unwrap().to_string())).collect();
    evs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let end = evs.iter().find(|e| e.1 == "END").map(|e| e.0).unwrap_or(4.0);
    // frame times: the real ones, or a fixed 60 Hz
    let times: Vec<f32> = match rl {
        Some(v) => v.iter().map(|s| s.t).collect(),
        None => (0..=((end / TICK).ceil() as usize)).map(|k| k as f32 * TICK).collect(),
    };
    // each usercmd's frametime: from the client's game clock when the log has it (exact), else the sample spacing
    let clock: Vec<f32> = match rl {
        Some(v) if v.iter().all(|s| s.ct >= 0.0) => v.iter().map(|s| s.ct).collect(),
        _ => times.clone(),
    };
    let mut held: HashSet<String> = HashSet::new();
    let mut prev: HashSet<String> = HashSet::new();
    let mut spam: Vec<(String, usize, u32)> = vec![]; // (cmd, start frame, presses)
    let mut ours: Vec<S> = vec![];
    let mut ei = 0;
    let mut yaw = yaw_start;
    let mut yawspeed = 210.0f32;
    for k in 0..times.len() {
        let t = times[k];
        let dt = clock.get(k + 1).map(|n| n - clock[k]).unwrap_or(TICK).clamp(1.0 / 300.0, 0.1);
        while ei < evs.len() && evs[ei].0 <= t + 1e-4 {
            let c = &evs[ei].1;
            if let Some(x) = c.strip_prefix('+') {
                held.insert(x.to_string());
            } else if let Some(x) = c.strip_prefix('-') {
                held.remove(x);
            } else if let Some(n) = c.strip_prefix("JUMP:") {
                spam.push(("jump".into(), k, n.parse().unwrap()));
            } else if let Some(n) = c.strip_prefix("TAP:") {
                spam.push(("forward".into(), k, n.parse().unwrap()));
            } else if let Some(n) = c.strip_prefix("mantle_boost_input_setting ") {
                // the SDK hook's input (0 = off); R5Rust expresses off as mantle_boost_enabled = false
                let n: i32 = n.trim().parse().unwrap_or(0);
                p.cvars.mantle_boost_enabled = n != 0;
                if n != 0 {
                    p.cvars.mantle_boost_input_setting = n;
                }
            } else if let Some(n) = c.strip_prefix("cl_yawspeed ") {
                yawspeed = n.trim().parse().unwrap_or(yawspeed);
            } else if c == "JUMP" || c == "TAP" {
                println!("  {name}: bare {c} (no count): the probe script errors there, run is invalid");
            }
            ei += 1;
        }
        let mut now = held.clone();
        for (c, k0, n) in &spam {
            let d = k - k0;
            if d < 2 * *n as usize && d % 2 == 0 {
                now.insert(c.clone());
            }
        }
        let h = |s: &str| now.contains(s);
        // the client's KeyState (S21 0x140900e40): a key that went down this frame counts 0.5, held 1.0
        let ks = |s: &str| if !now.contains(s) { 0.0 } else if prev.contains(s) { 1.0 } else { 0.5 };
        // view yaw for this frame's usercmd: the real one (the next sample shows the angles the cmd used), else
        // the +left/+right turn rate
        match rl {
            Some(v) => yaw = v.get(k + 1).unwrap_or(&v[k]).yaw,
            None => yaw += (ks("left") - ks("right")) * yawspeed * dt,
        }
        let pitch = rl.map(|v| v.get(k + 1).unwrap_or(&v[k]).pitch).unwrap_or(0.0);
        let input = Input {
            forward: ks("forward") - ks("backward").max(if back_live { ks("back") } else { 0.0 }),
            right: ks("moveright") - ks("moveleft"),
            jump: h("jump"),
            // Kainz's R5F profile has player_setting_autosprint 1: the S21 cmd builder's rule (sim::auto_sprint)
            sprint: sim::auto_sprint(&p),
            crouch: h("duck"),
            yaw,
            pitch,
            ..Default::default()
        };
        ours.push(S { t, p: p.pos - p0, v: p.vel, g: p.on_ground, yaw, pitch, fl: flags(&p), ct: -1.0 });
        if std::env::var_os("PROBE_TRACE").is_some() {
            let rr = rl.map(|v| v[k]);
            let rs = rr.map(|s| format!(" | real hs {:6.1} vz {:7.1} d {:5.1} g{} f{}", s.v.truncate().length(), s.v.z, (s.p - v_at(rl, 0).p - (p.pos - p0)).length(), s.g as u8, s.fl)).unwrap_or_default();
            println!("  {name} t{t:6.3} hs {:6.1} vz {:7.1} z {:8.2} {:6} yaw {yaw:6.1} at {:.1},{:.1},{:.1}{rs}", p.vel.truncate().length(), p.vel.z, p.pos.z - p0.z, p.state_name(), p.pos.x, p.pos.y, p.pos.z);
        }
        sim::simulate(&mut p, &input, dt, col);
        prev = now;
    }
    let mut csv = String::from("t,ours_x,ours_y,ours_z,ours_vx,ours_vy,ours_vz,ours_fl,real_x,real_y,real_z,real_vx,real_vy,real_vz,real_fl\n");
    let Some(rs) = rl else {
        let last = ours.last().unwrap();
        println!("{name:28} (no real samples)  ours: end hs {:.0} vz {:.0} at {:?}", last.v.truncate().length(), last.v.z, last.p);
        for o in &ours {
            csv += &format!("{:.4},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{}\n", o.t, o.p.x, o.p.y, o.p.z, o.v.x, o.v.y, o.v.z, o.fl);
        }
        return (csv, None);
    };
    let r0 = rs[0].p;
    let (mut hs_max, mut hs_sum, mut vz_max, mut pos_max) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut n = 0;
    let mut summary = vec![];
    let mut marks = [0.25f32, 0.5, 1.0, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0].into_iter().peekable();
    for (k, o) in ours.iter().enumerate() {
        let rr = rs[k];
        if rr.t > end + 1e-3 {
            break;
        }
        let hs_r = rr.v.truncate().length();
        let near = |f: &dyn Fn(&S) -> f32, want: f32| (k.saturating_sub(1)..=(k + 1).min(ours.len() - 1)).map(|j| (f(&ours[j]) - want).abs()).fold(f32::MAX, f32::min);
        let e_hs = near(&|s: &S| s.v.truncate().length(), hs_r);
        let e_vz = near(&|s: &S| s.v.z, rr.v.z);
        let e_p = (o.p - (rr.p - r0)).length();
        hs_max = hs_max.max(e_hs);
        vz_max = vz_max.max(e_vz);
        pos_max = pos_max.max(e_p);
        hs_sum += e_hs;
        n += 1;
        if marks.peek().is_some_and(|m| rr.t >= *m) {
            let m = marks.next().unwrap();
            summary.push(format!("t{m}: hs {:.0}/{:.0} vz {:.0}/{:.0} d {:.0}", o.v.truncate().length(), hs_r, o.v.z, rr.v.z, e_p));
        }
        csv += &format!(
            "{:.4},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{}\n",
            o.t, o.p.x, o.p.y, o.p.z, o.v.x, o.v.y, o.v.z, o.fl, rr.p.x - r0.x, rr.p.y - r0.y, rr.p.z - r0.z, rr.v.x, rr.v.y, rr.v.z, rr.fl
        );
    }
    println!("{name:28} hs {hs_max:6.1} pos {pos_max:6.1}   ours/real: {}", summary.join(" | "));
    (csv, Some([hs_max, hs_sum / n.max(1) as f32, vz_max, pos_max]))
}

fn v_at(rl: Option<&Vec<S>>, k: usize) -> S {
    rl.map(|v| v[k]).unwrap_or_default()
}
