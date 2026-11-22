//! Headless zipline check: for each map zipline, hang the player under a point 30% along the cable, aim at it,
//! press use for one tick, then ride: prints the state changes, speed, where it detached and the exit velocity.
//!     cargo run --release -p shared --example zipcheck [line index]

use glam::Vec3;
use shared::sim::{self, zipline as z, Input, Player};
use shared::{assets::Config, collmesh};

const TICK: f32 = 1.0 / 60.0;

fn main() {
    let cfg = Config::load();
    let (mut col, _) = collmesh::build_player_collision(&cfg).expect("collision");
    col.ziplines = shared::zipline::load(&cfg.bsp_dir(), &cfg.map);
    let a: Vec<String> = std::env::args().collect();
    if a.get(1).map(String::as_str) == Some("probe") {
        // probe x y z yaw pitch: stand there (settled), report every line's use-search steps
        let v: Vec<f32> = a[2..7].iter().map(|s| s.parse().unwrap()).collect();
        let mut p = Player::new(sim::find_free(&col, Vec3::new(v[0], v[1], v[2])));
        for _ in 0..60 {
            sim::simulate(&mut p, &Input::default(), TICK, &col);
        }
        let f = sim::forward_vec(v[3], v[4]);
        let top = p.pos + Vec3::Z * 72.0;
        println!("player {:.1} on_ground {} eye {:.1}", p.pos, p.on_ground, p.eye());
        for (li, line) in col.ziplines.iter().enumerate() {
            let (_, cq, cd) = line.closest(top);
            if cd > 300.0 {
                continue;
            }
            println!("  line {li}: closest {cq:.0} dist from hull top {cd:.0} (range 120)");
        }
        println!("  find_mount: {:?}", z::find_mount(&p, &col, f, 10.0));
        return;
    }
    if a.get(1).map(String::as_str) == Some("crouch") {
        // crouch, mount line 3 while crouched, let go, ride a second, jump off, land: must come down standing
        let mut p = Player::new(sim::find_free(&col, Vec3::new(-8138.0, 809.0, 385.0)));
        let ci = Input { crouch: true, ..Default::default() };
        for _ in 0..40 {
            sim::simulate(&mut p, &ci, TICK, &col);
        }
        println!("crouched: ducked {} view_z {:.1}", p.ducked, p.view_z);
        let up = Input { yaw: 100.0, pitch: -80.0, use_pressed: true, crouch: true, ..Default::default() };
        sim::simulate(&mut p, &up, TICK, &col);
        println!("after use: zip state {} ducked {} view_z {:.1}", p.zip.state, p.ducked, p.view_z);
        for _ in 0..60 {
            sim::simulate(&mut p, &Input { yaw: 100.0, ..Default::default() }, TICK, &col);
        }
        println!("riding: zip state {} view_z {:.1}", p.zip.state, p.view_z);
        sim::simulate(&mut p, &Input { yaw: 100.0, jump: true, jump_pressed: true, ..Default::default() }, TICK, &col);
        for _ in 0..240 {
            sim::simulate(&mut p, &Input { yaw: 100.0, ..Default::default() }, TICK, &col);
        }
        println!("landed: on_ground {} zip {} ducked {} duck_state {} view_z {:.1} (stand {:.0})", p.on_ground, p.zip.state, p.ducked, p.duck_state, p.view_z, p.settings.stand.viewheight);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("spots") {
        // a standing spot in reach of each line: drop under points along the cable, land, check the use search
        for (li, line) in col.ziplines.iter().enumerate() {
            for k in 1..20 {
                let (q, _) = line.at(line.length() * k as f32 / 20.0);
                let mut p = Player::new(sim::find_free(&col, q - Vec3::new(0.0, 0.0, 90.0)));
                for _ in 0..90 {
                    sim::simulate(&mut p, &Input::default(), TICK, &col);
                }
                if !p.on_ground {
                    continue;
                }
                let d = (q - p.eye()).normalize();
                let (yaw, pitch) = (d.y.atan2(d.x).to_degrees(), -d.z.asin().to_degrees());
                if z::find_mount(&p, &col, sim::forward_vec(yaw, pitch), 10.0).is_some() {
                    println!("line {li:2}: --pos {:.0},{:.0},{:.0} --yaw {yaw:.1} --pitch {pitch:.1}", p.pos.x, p.pos.y, p.pos.z);
                    break;
                }
            }
        }
        return;
    }
    let only: Option<usize> = std::env::args().nth(1).and_then(|a| a.parse().ok());
    println!("{} ziplines", col.ziplines.len());
    for li in 0..col.ziplines.len() {
        if only.is_some_and(|o| o != li) {
            continue;
        }
        let line = col.ziplines[li].clone();
        let (q, _) = line.at(line.length() * 0.3);
        let mut p = Player::new(q - Vec3::new(0.0, 0.0, 110.0));
        let eye = p.eye();
        let d = (q - eye).normalize();
        let yaw = d.y.atan2(d.x).to_degrees();
        let pitch = -d.z.asin().to_degrees();
        let mut inp = Input { yaw, pitch, use_pressed: true, ..Default::default() };
        let mut last_state = 255u8;
        let mut t_ride = 0.0;
        let mut max_speed = 0.0f32;
        let start = p.pos;
        for tick in 0..60 * 30 {
            sim::simulate(&mut p, &inp, TICK, &col);
            inp.use_pressed = false;
            if p.zip.events & z::ZIP_EV_STOP != 0 && last_state != 0 {
                println!("  line {li:2}: DETACH at t {:.2}s pos {:.0} vel {:.0} |v| {:.0} (rode {:.0}u, max {:.0} u/s)", tick as f32 * TICK, p.pos, p.vel, p.vel.length(), (p.pos - start).length(), max_speed);
            }
            p.zip.events = 0;
            if p.zip.state != last_state {
                if p.zip.state != 0 || last_state == 255 {
                    println!("  line {li:2}: state {} at t {:.2}s pos {:.0} vel |{:.0}| rev {} vertical {} len {:.0} speedScale {}", p.zip.state, tick as f32 * TICK, p.pos, p.vel.length(), p.zip.reverse, line.vertical, line.length(), line.speed_scale);
                }
                last_state = p.zip.state;
                if p.zip.state == 3 {
                    t_ride = tick as f32 * TICK;
                }
            }
            if p.zip.riding() {
                max_speed = max_speed.max(p.vel.length());
            }
            if p.zip.state == 0 && tick > 2 && last_state == 0 {
                if t_ride == 0.0 {
                    println!("  line {li:2}: NO MOUNT (cable {:.0} from player top)", (q - (start + Vec3::Z * 72.0)).length());
                }
                break;
            }
        }
    }
}
