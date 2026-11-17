//! Headless movement checker: runs the Apex movement port on a synthetic test world (and the real Firing Range
//! when the assets are present) and asserts concrete numbers. `cargo run --release -p shared --example movecheck`
//! Exit code 1 if any check fails.

use glam::Vec3;
use shared::collision::Collision;
use shared::settings::STOCK;
use shared::sim::{self, Input, MoveType, Player};
use std::time::Instant;

const TICK: f32 = 1.0 / 60.0;

struct Checks {
    fails: u32,
    n: u32,
}

impl Checks {
    fn near(&mut self, name: &str, got: f32, want: f32, tol: f32) {
        let ok = (got - want).abs() <= tol;
        self.report(name, ok, format!("{got:9.2}"), format!("{want} +-{tol}"));
    }
    fn range(&mut self, name: &str, got: f32, lo: f32, hi: f32) {
        let ok = got >= lo && got <= hi;
        self.report(name, ok, format!("{got:9.2}"), format!("[{lo}, {hi}]"));
    }
    fn truth(&mut self, name: &str, ok: bool, detail: String) {
        self.report(name, ok, detail, "true".into());
    }
    fn report(&mut self, name: &str, ok: bool, got: String, want: String) {
        self.n += 1;
        if !ok {
            self.fails += 1;
        }
        println!("{} {name:58} {got:>14}   want {want}", if ok { "PASS" } else { "FAIL" });
    }
}

// ---------------------------------------------------------------- test world

struct Geo {
    pos: Vec<[f32; 3]>,
    idx: Vec<u32>,
}

impl Geo {
    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        let i = self.pos.len() as u32;
        for v in [a, b, c, d] {
            self.pos.push(v.to_array());
        }
        self.idx.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
    }
    fn aabb(&mut self, mn: Vec3, mx: Vec3) {
        let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let (a, b) = (mn, mx);
        self.quad(v(a.x, a.y, b.z), v(b.x, a.y, b.z), v(b.x, b.y, b.z), v(a.x, b.y, b.z)); // top
        self.quad(v(a.x, a.y, a.z), v(a.x, b.y, a.z), v(b.x, b.y, a.z), v(b.x, a.y, a.z)); // bottom
        self.quad(v(a.x, a.y, a.z), v(b.x, a.y, a.z), v(b.x, a.y, b.z), v(a.x, a.y, b.z));
        self.quad(v(a.x, b.y, a.z), v(a.x, b.y, b.z), v(b.x, b.y, b.z), v(b.x, b.y, a.z));
        self.quad(v(a.x, a.y, a.z), v(a.x, a.y, b.z), v(a.x, b.y, b.z), v(a.x, b.y, a.z));
        self.quad(v(b.x, a.y, a.z), v(b.x, b.y, a.z), v(b.x, b.y, b.z), v(b.x, a.y, b.z));
    }
}

/// Flat floor at z 0, split into many triangles like a real mesh, plus test fixtures (all facing +X from x 200):
/// y 0: a 22-unit step; y 1000: a 23-unit step; y 2000: a 60-unit ledge (mantle); y 3000: a 100-unit cliff
/// (x -3000..-2000) to walk off; y 4200: a 300-unit wall (wall climb); y 5200: a 150-unit wall (climb, then
/// mantle the top).
fn test_world() -> Collision {
    let mut g = Geo { pos: vec![], idx: vec![] };
    let s = 500.0;
    let mut x = -4000.0;
    while x < 4000.0 {
        let mut y = -4000.0;
        while y < 6000.0 {
            g.quad(Vec3::new(x, y, 0.0), Vec3::new(x + s, y, 0.0), Vec3::new(x + s, y + s, 0.0), Vec3::new(x, y + s, 0.0));
            y += s;
        }
        x += s;
    }
    g.aabb(Vec3::new(200.0, -100.0, 0.0), Vec3::new(400.0, 100.0, 22.0));
    g.aabb(Vec3::new(200.0, 900.0, 0.0), Vec3::new(400.0, 1100.0, 23.0));
    g.aabb(Vec3::new(200.0, 1800.0, 0.0), Vec3::new(700.0, 2200.0, 60.0));
    g.aabb(Vec3::new(-3000.0, 2500.0, 0.0), Vec3::new(-2000.0, 3500.0, 100.0));
    g.aabb(Vec3::new(200.0, 3800.0, 0.0), Vec3::new(400.0, 4600.0, 300.0));
    g.aabb(Vec3::new(200.0, 4900.0, 0.0), Vec3::new(700.0, 5500.0, 150.0));
    Collision::build(g.pos, &g.idx)
}

fn inp(forward: f32, right: f32) -> Input {
    Input { forward, right, ..Default::default() }
}

fn hspeed(p: &Player) -> f32 {
    p.vel.truncate().length()
}

fn settle(col: &Collision, pos: Vec3) -> Player {
    let mut p = Player::new(pos);
    for _ in 0..10 {
        sim::simulate(&mut p, &Input::default(), TICK, col);
    }
    p
}

fn run(p: &mut Player, col: &Collision, ticks: usize, i: Input) {
    for _ in 0..ticks {
        sim::simulate(p, &i, TICK, col);
    }
}

/// Holding W into the 60u ledge's wall, then W+jump until the mantle starts. Returns ticks until it did.
fn start_mantle(col: &Collision, sprint_tap: bool) -> Player {
    let mut p = settle(col, Vec3::new(120.0, 2000.0, 0.1));
    if sprint_tap {
        sim::simulate(&mut p, &Input { sprint: true, forward: 1.0, ..Default::default() }, TICK, col);
    }
    run(&mut p, col, 30, inp(1.0, 0.0));
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, col);
    p
}

fn main() {
    let mut c = Checks { fails: 0, n: 0 };
    let col = test_world();
    println!("test world: {} triangles", col.tri_count());

    // ------------------------------------------------------------ settings match the game files
    check_settings_file(&mut c);

    // ------------------------------------------------------------ ground speeds
    let start = Vec3::new(-1500.0, -2000.0, 0.1);
    let mut p = settle(&col, start);
    c.truth("spawn settles on the floor", p.on_ground && p.pos.z < 0.1, format!("z {:.3} ground {}", p.pos.z, p.on_ground));
    run(&mut p, &col, 180, inp(1.0, 0.0));
    c.near("walk speed (speed 173.5)", hspeed(&p), 173.5, 0.5);
    let mut p = settle(&col, start);
    let mut t200 = None;
    let mut t_sprint = None;
    for i in 0..240 {
        sim::simulate(&mut p, &Input { forward: 1.0, sprint: true, ..Default::default() }, TICK, &col);
        if t200.is_none() && hspeed(&p) >= 200.0 {
            t200 = Some(i as f32 * TICK);
        }
        if t_sprint.is_none() && hspeed(&p) >= 259.0 {
            t_sprint = Some(i as f32 * TICK);
        }
    }
    c.near("sprint speed (sprintspeed 260)", hspeed(&p), 260.0, 0.5);
    c.range("sprint: time 0 -> 200 u/s (lowAccel/accel/sprintAccel)", t200.unwrap_or(99.0), 0.2, 0.7);
    c.range("sprint: time 0 -> 259 u/s", t_sprint.unwrap_or(99.0), 0.8, 1.6);
    // sprint latch: one tap of shift, then W only
    let mut p = settle(&col, start);
    sim::simulate(&mut p, &Input { forward: 1.0, sprint: true, ..Default::default() }, TICK, &col);
    run(&mut p, &col, 180, inp(1.0, 0.0));
    c.near("sprint latches after one tap (W only)", hspeed(&p), 260.0, 0.5);
    run(&mut p, &col, 60, inp(0.0, 0.0));
    run(&mut p, &col, 180, inp(1.0, 0.0));
    c.near("stopping ends the latch", hspeed(&p), 173.5, 0.5);
    let mut p = settle(&col, start);
    run(&mut p, &col, 180, Input { forward: -1.0, sprint: true, ..Default::default() });
    c.near("no sprinting backwards", hspeed(&p), 173.5, 0.5);
    let mut p = settle(&col, start);
    p.speed_scale = 1.15;
    run(&mut p, &col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    c.near("holstered sprint (speed scale 1.15) = 299", hspeed(&p), 299.0, 0.5);
    // stopping distance
    let mut p = settle(&col, start);
    run(&mut p, &col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    let mut ts = 0.0;
    while hspeed(&p) > 0.0 && ts < 2.0 {
        sim::simulate(&mut p, &Input::default(), TICK, &col);
        ts += TICK;
    }
    c.range("sprint -> stop time (deceleration 1250)", ts, 0.15, 0.3);
    // crouch walk
    let mut p = settle(&col, start);
    run(&mut p, &col, 180, Input { forward: 1.0, crouch: true, ..Default::default() });
    c.near("crouch-walk speed (crouch speed 80)", hspeed(&p), 80.0, 0.5);
    c.truth("crouched hull 47 after 400 ms", p.ducked && p.hull().1.z == 47.0, format!("ducked {} z {}", p.ducked, p.hull().1.z));
    c.near("crouched eye height", p.eye().z - p.pos.z, 32.0, 0.01);
    let mut p2 = p;
    run(&mut p2, &col, 12, inp(1.0, 0.0));
    c.range("unduck: eye eases up (mid-transition)", p2.eye().z - p2.pos.z, 33.0, 59.0);
    run(&mut p2, &col, 30, inp(1.0, 0.0));
    c.near("unduck: standing eye height", p2.eye().z - p2.pos.z, 60.0, 0.01);
    // unduck blocked under a low ceiling: crouch under the 60 ledge? use a box: stand at y 2000 x 1000 is open;
    // instead check CanUnduck through the step's underside: none. (covered by the 47-hull check above)

    // ------------------------------------------------------------ jump
    let mut p = settle(&col, start);
    run(&mut p, &col, 60, Input::default());
    let z0 = p.pos.z;
    let mut peak = z0;
    sim::simulate(&mut p, &Input { jump: true, ..Default::default() }, TICK, &col);
    let vz_jump = p.vel.z;
    for _ in 0..90 {
        sim::simulate(&mut p, &Input { jump: true, ..Default::default() }, TICK, &col);
        peak = peak.max(p.pos.z);
    }
    c.near("jump: takeoff vz = sqrt(2*750*56) - 3 half gravities", vz_jump, (2.0f32 * 750.0 * 56.0).sqrt() - 3.0 * 0.5 * 750.0 * TICK, 0.1);
    c.range("jump apex height (jumpHeight 56)", peak - z0, 53.0, 56.5);
    c.truth("holding jump does not re-jump (no auto-hop)", p.on_ground && p.vel.z.abs() < 1.0, format!("ground {} vz {:.1}", p.on_ground, p.vel.z));
    // anti multi-jump: jump again right after landing
    let mut p = settle(&col, start);
    run(&mut p, &col, 60, Input::default());
    sim::simulate(&mut p, &Input { jump: true, ..Default::default() }, TICK, &col);
    while !p.on_ground {
        sim::simulate(&mut p, &Input::default(), TICK, &col);
    }
    let z0 = p.pos.z;
    let mut peak = z0;
    sim::simulate(&mut p, &Input { jump: true, ..Default::default() }, TICK, &col);
    for _ in 0..60 {
        sim::simulate(&mut p, &Input::default(), TICK, &col);
        peak = peak.max(p.pos.z);
    }
    c.range("instant re-jump: anti-multi-jump height (0.3 x 56)", peak - z0, 14.0, 17.5);
    run(&mut p, &col, 60, Input::default());
    let z0 = p.pos.z;
    let mut peak = z0;
    sim::simulate(&mut p, &Input { jump: true, ..Default::default() }, TICK, &col);
    for _ in 0..60 {
        sim::simulate(&mut p, &Input::default(), TICK, &col);
        peak = peak.max(p.pos.z);
    }
    c.range("re-jump after 1 s: full height", peak - z0, 53.0, 56.5);
    // bhop skip: land fast, jump within 1 s -> -100 u/s above sprint speed
    let mut p = settle(&col, start);
    p.pos.z = 30.0;
    p.vel = Vec3::new(500.0, 0.0, 0.0);
    while !p.on_ground {
        sim::simulate(&mut p, &Input::default(), TICK, &col);
    }
    let before = hspeed(&p);
    sim::simulate(&mut p, &Input { jump: true, ..Default::default() }, TICK, &col);
    c.near("bhop within 1 s of landing: speed - skip_speed_reduce 100", hspeed(&p), before - 100.0, 1.0);
    // coyote time
    let mut p = settle(&col, Vec3::new(-2100.0, 3000.0, 100.1));
    let mut left = None;
    for i in 0..200 {
        sim::simulate(&mut p, &inp(1.0, 0.0), TICK, &col);
        if !p.on_ground && p.pos.z > 50.0 {
            left = Some(i);
            break;
        }
    }
    let mut q = p;
    run(&mut q, &col, 6, inp(1.0, 0.0));
    sim::simulate(&mut q, &Input { jump: true, forward: 1.0, ..Default::default() }, TICK, &col);
    c.truth("coyote jump 0.1 s after walking off a ledge (adds 290 to vz)", left.is_some() && q.vel.z > 150.0, format!("vz {:.0}", q.vel.z));
    let mut q = p;
    run(&mut q, &col, 15, inp(1.0, 0.0));
    sim::simulate(&mut q, &Input { jump: true, forward: 1.0, ..Default::default() }, TICK, &col);
    c.truth("no jump 0.25 s after leaving the ledge", q.vel.z < 0.0, format!("vz {:.0}", q.vel.z));
    // landing slowdown from a big fall
    let mut p = settle(&col, start);
    p.pos.z = 900.0;
    p.vel = Vec3::new(200.0, 0.0, 0.0);
    sim::simulate(&mut p, &Input::default(), TICK, &col);
    while !p.on_ground {
        sim::simulate(&mut p, &Input::default(), TICK, &col);
    }
    c.near("hard landing (900u fall >= landSlowdownHeightMax): h-speed killed", hspeed(&p), 0.0, 1.0);
    let mut p = settle(&col, start);
    p.pos.z = 200.0;
    p.vel = Vec3::new(200.0, 0.0, 0.0);
    sim::simulate(&mut p, &Input::default(), TICK, &col);
    while !p.on_ground {
        sim::simulate(&mut p, &Input::default(), TICK, &col);
    }
    c.range("soft landing (200u fall < landSlowdownHeightMin 300): speed kept", hspeed(&p), 170.0, 200.0);

    // ------------------------------------------------------------ air control
    let mut p = settle(&col, start);
    run(&mut p, &col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, &col);
    let v0 = hspeed(&p);
    run(&mut p, &col, 20, inp(1.0, 0.0));
    c.near("air: holding W does not add speed past the 60 wish", hspeed(&p), v0, 0.5);
    // classic strafe: hold A, turn left 3 deg/tick
    let mut p = settle(&col, start);
    run(&mut p, &col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    let mut yaw = 0.0f32;
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, yaw, ..Default::default() }, TICK, &col);
    let v0 = hspeed(&p);
    for _ in 0..40 {
        yaw += 2.0;
        sim::simulate(&mut p, &Input { right: -1.0, yaw, ..Default::default() }, TICK, &col);
    }
    let gain = hspeed(&p) - v0;
    c.range("air strafe (A + turning 120 deg/s): speed gain in 0.67 s", gain, 5.0, 120.0);
    // tap-strafe: a new key press in the 0.5 s jump grace rotates the velocity, keeping speed (S21 jump_grace_*;
    // the same numbers as the 2019 keyboard grace at the default cvars)
    let mut p = settle(&col, start);
    run(&mut p, &col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, &col);
    run(&mut p, &col, 5, inp(1.0, 0.0));
    let before = p.vel;
    sim::simulate(&mut p, &inp(0.0, 1.0), TICK, &col);
    let ang = before.truncate().angle_to(p.vel.truncate()).to_degrees();
    c.range("tap-strafe at 0.1 s: velocity turned toward the key (deg)", -ang, 20.0, 60.0);
    c.range("tap-strafe: change capped at 0.7 x 260 per press (speed)", hspeed(&p), 200.0, 261.0);
    let mut p = settle(&col, start);
    run(&mut p, &col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, &col);
    run(&mut p, &col, 35, inp(1.0, 0.0));
    let before = p.vel;
    sim::simulate(&mut p, &inp(0.0, 1.0), TICK, &col);
    let ang = before.truncate().angle_to(p.vel.truncate()).to_degrees().abs();
    c.range("no jump grace 0.6 s after the jump (deg, air accel only)", ang, 0.0, 3.0);

    // ------------------------------------------------------------ slide
    let mut p = settle(&col, start);
    run(&mut p, &col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, ..Default::default() }, TICK, &col);
    c.truth("crouch while sprinting starts a slide", p.sliding && p.ducked, format!("sliding {} ducked {}", p.sliding, p.ducked));
    c.range("slide boost: 260 + 150 capped at 400", hspeed(&p), 390.0, 400.0);
    let slide_start = p;
    let mut speeds = vec![];
    let mut stop_t = None;
    for i in 0..240 {
        sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, ..Default::default() }, TICK, &col);
        if i == 29 || i == 59 {
            speeds.push(hspeed(&p));
        }
        if stop_t.is_none() && !p.sliding {
            stop_t = Some(i as f32 * TICK);
        }
    }
    c.range("slide speed after 0.5 s (decel 100 + 0.7^t decay)", speeds[0], 270.0, 330.0);
    c.range("slide speed after 1.0 s", speeds[1], 170.0, 230.0);
    c.range("slide ends below slideStopSpeed 125 (s)", stop_t.unwrap_or(99.0), 1.2, 3.0);
    c.near("then crouch-walk speed", hspeed(&p), 80.0, 1.0);
    // cooldown: a second slide within 2 s of the first gets no boost
    let mut p = slide_start;
    run(&mut p, &col, 30, Input { forward: 1.0, sprint: true, ..Default::default() }); // stand up, keep running
    run(&mut p, &col, 60, Input { forward: 1.0, sprint: true, ..Default::default() });
    let pre = hspeed(&p);
    sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, ..Default::default() }, TICK, &col);
    c.truth("slide again 1.5 s later: no boost (2 s cooldown)", p.sliding && hspeed(&p) <= pre + 1.0, format!("{pre:.0} -> {:.0}", hspeed(&p)));
    run(&mut p, &col, 150, Input { forward: 1.0, sprint: true, ..Default::default() });
    sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, ..Default::default() }, TICK, &col);
    c.range("slide after the cooldown: boosted again", hspeed(&p), 390.0, 400.0);
    // slide jump inside 0.4 s: boost refunded
    let mut p = slide_start;
    run(&mut p, &col, 5, Input { forward: 1.0, crouch: true, ..Default::default() });
    let pre = hspeed(&p);
    sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, jump: true, ..Default::default() }, TICK, &col);
    c.near("slide jump within 0.4 s: speed - the boost (140) given at slide start", hspeed(&p), pre - 140.0, 1.5);
    c.range("slide jump vz (slideJumpHeight 50: 273.9 - gravity)", p.vel.z, 254.0, 260.0);
    // slide jump later at 300..350 snaps to slideMaxJumpSpeed
    let mut p = settle(&col, start);
    p.speed_scale = 1.15; // holstered: the slide boost reaches 460, so the slide passes 300..350 after 0.4 s
    run(&mut p, &col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, ..Default::default() }, TICK, &col);
    let t_slide = p.time();
    let mut pre = 0.0;
    for _ in 0..240 {
        let s = hspeed(&p);
        let j = p.sliding && s < 345.0 && s > 300.0 && p.time() > t_slide + 0.42;
        pre = s;
        sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, jump: j, ..Default::default() }, TICK, &col);
        if j {
            break;
        }
    }
    println!("     (slide jump taken at {pre:.0} u/s)");
    c.near("slide jump at 300..350 u/s snaps to slideMaxJumpSpeed 350", hspeed(&p), 350.0, 0.5);
    // can't stand up from a fast slide
    let mut p = slide_start;
    run(&mut p, &col, 3, inp(1.0, 0.0));
    c.truth("releasing crouch above slideMaxStopSpeed keeps the slide", p.sliding, format!("speed {:.0}", hspeed(&p)));
    // walking speed is too slow to slide
    let mut p = settle(&col, start);
    run(&mut p, &col, 180, inp(1.0, 0.0));
    sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, ..Default::default() }, TICK, &col);
    c.truth("no slide from walk speed (< slideRequiredStartSpeed 200)", !p.sliding, format!("speed {:.0}", hspeed(&p)));

    // ------------------------------------------------------------ steps
    let mut p = settle(&col, Vec3::new(100.0, 0.0, 0.1));
    run(&mut p, &col, 60, inp(1.0, 0.0));
    c.truth("walks up a 22u step", p.pos.z > 21.9 && p.pos.x > 220.0 && hspeed(&p) > 173.0, format!("({:.0}, z {:.2})", p.pos.x, p.pos.z));
    c.range("step-up view smoothing: eye lags the 22u step, then catches up", p.step_smooth.z, -0.5, 0.0);
    let mut p = settle(&col, Vec3::new(100.0, 1000.0, 0.1));
    run(&mut p, &col, 120, inp(1.0, 0.0));
    c.truth("blocked by a 23u step", p.pos.z < 1.0 && p.pos.x < 184.1, format!("({:.1}, z {:.2})", p.pos.x, p.pos.z));
    let mut p = settle(&col, Vec3::new(300.0, 0.0, 22.1));
    run(&mut p, &col, 60, inp(1.0, 0.0));
    c.truth("walks down off the 22u step (StayOnGround)", p.on_ground && p.pos.z < 0.1 && p.pos.x > 420.0, format!("({:.0}, z {:.2})", p.pos.x, p.pos.z));

    // ------------------------------------------------------------ S21 lurch (jump grace) details
    lurch_checks(&mut c, &col, start);

    // ------------------------------------------------------------ mantle (native TraversalMove exits)
    mantle_checks(&mut c, &col);

    // ------------------------------------------------------------ wall climb
    wall_checks(&mut c, &col);

    // ------------------------------------------------------------ real map + perf
    real_map(&mut c);

    println!("\n{} / {} checks passed", c.n - c.fails, c.n);
    if c.fails > 0 {
        std::process::exit(1);
    }
}

/// Sprint along +X (speed scale 1: 260), jump, then `ticks` airborne ticks of `held`. The next simulate is
/// (ticks + 1) / 60 s after the jump.
fn airborne(col: &Collision, start: Vec3, ticks: usize, held: Input) -> Player {
    let mut p = settle(col, start);
    run(&mut p, col, 240, Input { forward: 1.0, sprint: true, ..Default::default() });
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, col);
    run(&mut p, col, ticks, held);
    p
}

/// One tick of `i` on `p`, and the same tick on a copy with the jump grace switched off (= the velocity the
/// lurch starts from: AirAccelerate has already run). Returns (without lurch, with lurch).
fn lurch_tick(p: &mut Player, col: &Collision, i: Input) -> (Vec3, Vec3) {
    let mut q = *p;
    q.cvars.jump_graceperiodmax = 0.0;
    sim::simulate(&mut q, &i, TICK, col);
    sim::simulate(p, &i, TICK, col);
    (q.vel, p.vel)
}

fn deg(a: Vec3, b: Vec3) -> f32 {
    a.truncate().angle_to(b.truncate()).to_degrees().abs()
}

/// S21 AirMove_TapStrafe 0x140a48a30 at its default cvars (strength 0.7, full until 0.2 s, fades to 0 at 0.5 s,
/// change capped at 0.7 x sprint speed, cutoff 1100), plus back presses, null inputs and multi-tap decay.
fn lurch_checks(c: &mut Checks, col: &Collision, start: Vec3) {
    let cap = 0.7 * 260.0;
    // strength vs time after the jump: a D press bends the velocity by atan(r / (1 - r)), r = 0.7 x ramp,
    // while the change stays under the cap
    for (ticks, label) in [(20usize, "0.35"), (26, "0.45")] {
        let el = (ticks + 1) as f32 * TICK;
        let mut p = airborne(col, start, ticks, Input::default());
        let (v1, v2) = lurch_tick(&mut p, col, inp(0.0, 1.0));
        // the push is +R, the velocity is v1 (not quite +X after AirAccelerate), so measure the bend from v1
        let u = v1.truncate().normalize();
        let w = glam::Vec2::new(0.0, -1.0);
        let r = ((el - 0.5) / (0.2 - 0.5)).clamp(0.0, 1.0) * 0.7;
        let want = u.angle_to(u + (w - u) * r).to_degrees().abs();
        c.near(&format!("lurch D at {label} s: bend toward D, r = 0.7 x ramp = {r:.3} (deg)"), deg(v1, v2), want, 0.05);
        c.near(&format!("lurch D at {label} s: speed kept (change under the cap)"), v2.truncate().length(), v1.truncate().length(), 0.05);
    }
    let mut p = airborne(col, start, 5, Input::default());
    let (v1, v2) = lurch_tick(&mut p, col, inp(0.0, 1.0));
    c.near("lurch D at 0.1 s: full strength, change = cap 0.7 x 260", (v2 - v1).truncate().length(), cap, 0.05);
    c.near("lurch D at 0.1 s: one tap counted", p.grace.taps as f32, 1.0, 0.0);
    let mut p = airborne(col, start, 30, Input::default());
    let (v1, v2) = lurch_tick(&mut p, col, inp(0.0, 1.0));
    c.near("lurch D at 0.517 s: window (0.5 s) over, no change", (v2 - v1).length(), 0.0, 1e-4);
    // a held key never re-fires
    let mut p = airborne(col, start, 5, Input::default());
    sim::simulate(&mut p, &inp(0.0, 1.0), TICK, col);
    let (v1, v2) = lurch_tick(&mut p, col, inp(0.0, 1.0));
    c.near("held D: no second lurch on the next tick", (v2 - v1).length(), 0.0, 1e-4);

    // back press (S): the push points against the velocity, so the change is the full cap straight backward:
    // speed drops by 182, direction kept (jump_grace_limit_to_90 = 0, the S21 default)
    let mut p = airborne(col, start, 5, Input::default());
    let (v1, v2) = lurch_tick(&mut p, col, inp(-1.0, 0.0));
    c.near("lurch S at 0.1 s: speed loss = the cap (182)", v1.truncate().length() - v2.truncate().length(), cap, 0.05);
    c.near("lurch S: direction kept (deg)", deg(v1, v2), 0.0, 0.01);
    let mut p = airborne(col, start, 5, Input::default());
    p.cvars.jump_grace_limit_to_90 = true;
    let (v1, v2) = lurch_tick(&mut p, col, inp(-1.0, 0.0));
    c.near("lurch S with jump_grace_limit_to_90 1: no change", (v2 - v1).length(), 0.0, 0.01);
    let mut a = airborne(col, start, 5, Input::default());
    a.cvars.jump_grace_limit_to_90 = true;
    let mut b = a;
    let (_, va) = lurch_tick(&mut a, col, inp(-1.0, 1.0));
    let (_, vb) = lurch_tick(&mut b, col, inp(0.0, 1.0));
    c.near("limit_to_90 1: S+D bends like D (deg)", deg(va, vb), 0.0, 1.5);

    // null inputs: holding A then pressing D (A+D cancel): no push, velocity untouched, but it is a tap;
    // A+D held + W pressed = a W lurch
    let mut p = airborne(col, start, 4, inp(0.0, -1.0));
    let taps = p.grace.taps; // the A press was a lurch of its own
    let (v1, v2) = lurch_tick(&mut p, col, Input { null_side: true, ..Default::default() });
    c.near("null input (A held, D pressed): velocity unchanged", (v2 - v1).length(), 0.0, 1e-3);
    c.near("null input: still counts as a tap", (p.grace.taps - taps) as f32, 1.0, 0.0);
    let mut w = airborne(col, start, 4, Input { yaw: 90.0, null_side: true, ..Default::default() });
    let mut f = airborne(col, start, 4, Input { yaw: 90.0, ..Default::default() });
    let (_, vw) = lurch_tick(&mut w, col, Input { forward: 1.0, yaw: 90.0, null_side: true, ..Default::default() });
    let (_, vf) = lurch_tick(&mut f, col, Input { forward: 1.0, yaw: 90.0, ..Default::default() });
    c.near("null input: A+D held + W pressed = a plain W lurch", (vw - vf).length(), 0.0, 1e-3);
    c.range("  (that W lurch, camera +Y, bends the +X velocity, deg)", deg(Vec3::X, vw), 30.0, 90.0);

    // wheel spam: 30 forward presses/s (every other tick) while turning 4 deg/tick. Default cvars have no decay
    // (multi_tap_decay 1, decay start 1000, velocity decay 0): every press in the 0.5 s window lurches at mult 1.
    let spam = |p: &mut Player| {
        let mut yaw = 0.0f32;
        for k in 0..30 {
            yaw += 4.0;
            let i = Input { forward: if k % 2 == 0 { 1.0 } else { 0.0 }, yaw, ..Default::default() };
            sim::simulate(p, &i, TICK, col);
        }
    };
    let mut p = airborne(col, start, 0, Input::default());
    spam(&mut p);
    // presses on ticks 2, 4 .. 28 after the jump (tick 0's notch is W still held from the jump tick; tick 30 is
    // at 0.5 s, past the window)
    c.near("wheel spam 30/s for 0.5 s while turning: 14 lurches", p.grace.taps as f32, 14.0, 0.0);
    c.near("wheel spam: default cvars never decay the strength", p.grace.mult, 1.0, 0.0);
    c.range("wheel spam: velocity follows the camera turn (deg of 116)", deg(Vec3::X, p.vel), 60.0, 116.0);
    let mut q = airborne(col, start, 0, Input::default());
    q.cvars.jump_grace_multi_tap_decay = 0.5;
    q.cvars.jump_grace_decay_start_tap_count = 2;
    spam(&mut q);
    c.near("multi_tap_decay 0.5 from tap 2: mult 0.5^13 after 14", q.grace.mult, 0.5f32.powi(13), 1e-7);
    c.truth("decayed spam turns less than full-strength spam", deg(Vec3::X, q.vel) < deg(Vec3::X, p.vel) - 10.0,
        format!("{:.1} vs {:.1} deg", deg(Vec3::X, q.vel), deg(Vec3::X, p.vel)));
    // velocity-based decay (easing 3 = linear): one lurch at speed v eats max_decay x v / max_speed of the mult
    let mut p = airborne(col, start, 5, Input::default());
    p.cvars.jump_grace_multi_tap_decay_velocity_based_max_decay = 0.5;
    p.cvars.jump_grace_multi_tap_decay_velocity_based_easing_type = 3;
    let (v1, _) = lurch_tick(&mut p, col, inp(0.0, 1.0));
    c.near("velocity decay (linear, 0.5 at 900): mult = 1 - 0.5 v/900", p.grace.mult, 1.0 - 0.5 * v1.truncate().length() / 900.0, 1e-4);
    // the cutoff: no lurch above 1100 u/s horizontal
    let mut p = airborne(col, start, 5, Input::default());
    p.vel.x = 1150.0;
    let (v1, v2) = lurch_tick(&mut p, col, inp(0.0, 1.0));
    c.near("no lurch above jump_grace_cutoff_speed 1100", (v2 - v1).length(), 0.0, 1e-4);
}

fn mantle_checks(c: &mut Checks, col: &Collision) {
    let native = |mut p: Player| {
        p.cvars.mantle_boost_enabled = false;
        p
    };
    // plain mantle onto the 60u ledge, W held (walk exit speed)
    let mut p = start_mantle(col, false);
    c.truth("W + jump at a 60u ledge starts a mantle", p.mantling(), format!("{:?} z {:.1}", p.move_type, p.pos.z));
    let t0 = p.time();
    let mut n = 0;
    while p.mantling() && n < 200 {
        sim::simulate(&mut p, &inp(1.0, 0.0), TICK, col);
        n += 1;
    }
    let dur = (p.time() - t0) as f32;
    c.near("mantle ends on the ledge (z)", p.pos.z, 60.0, 0.1);
    c.range("mantle duration (mantleDurationLevel 0.6 path)", dur, 0.35, 0.7);
    c.near("finish with W: exit speed = walk speed 173.5", hspeed(&p), 173.5, 0.5);
    c.truth("finish exit direction = input (+X)", p.vel.x > 170.0 && p.vel.y.abs() < 0.5, format!("{:.1?}", p.vel));
    // with the sprint latch the finish speed is sprint speed
    let mut p = start_mantle(col, true);
    while p.mantling() {
        sim::simulate(&mut p, &inp(1.0, 0.0), TICK, col);
    }
    c.near("finish with sprint latched: exit speed = 260", hspeed(&p), 260.0, 0.5);
    run(&mut p, col, 10, inp(1.0, 0.0));
    c.truth("standing on the ledge after the mantle", p.on_ground && p.pos.z > 59.9 && p.pos.x > 230.0, format!("({:.0}, z {:.2})", p.pos.x, p.pos.z));

    // native mantle jump past the corner: press jump with P > 0.5 and P >= C
    let mantle_until = |p: &mut Player, pred: &dyn Fn(&Player) -> bool, i: Input| {
        while p.mantling() && !pred(p) {
            sim::simulate(p, &i, TICK, col);
        }
    };
    let mut p = native(start_mantle(col, true));
    mantle_until(&mut p, &|p| p.traversal.p > 0.75 && p.traversal.p >= p.traversal.c, inp(1.0, 0.0));
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, col);
    c.truth("native mantle jump-off past the corner leaves the mantle", !p.mantling(), format!("{:?}", p.move_type));
    c.near("mantle jump: h-speed = 260 sprint cap + 30 stale contact-plane push", hspeed(&p), 290.0, 0.5);
    c.truth("mantle jump direction = input (+X)", p.vel.y.abs() < 0.01 && p.vel.x > 0.0, format!("{:.1?}", p.vel));
    c.near("mantle jump: vz = jump (56)", p.vel.z, (2.0f32 * 750.0 * 56.0).sqrt() - 0.5 * 750.0 * TICK, 0.5);
    let jumpoff = p;
    // superglide (native): crouch on the very next tick -> slide start with the stale ground normal + boost
    let mut p = jumpoff;
    sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, ..Default::default() }, TICK, col);
    c.truth("superglide (crouch the tick after the jump-off): slide starts in the air", p.sliding, format!("sliding {}", p.sliding));
    c.near("superglide exit h-speed (290 + slide boost 150, cap 400)", hspeed(&p), 400.0, 0.5);
    let mut p = jumpoff;
    run(&mut p, col, 1, inp(1.0, 0.0));
    sim::simulate(&mut p, &Input { forward: 1.0, crouch: true, ..Default::default() }, TICK, col);
    c.truth("crouch two ticks late: no superglide", !p.sliding && hspeed(&p) < 291.0, format!("sliding {} {:.0}", p.sliding, hspeed(&p)));
    // jump press on the vertical leg with W held is dropped (spec B6.1)
    let mut p = native(start_mantle(col, true));
    run(&mut p, col, 2, inp(1.0, 0.0));
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, col);
    c.truth("jump on the vertical leg with W held is ignored", p.mantling() && p.traversal.jump_time == 0.0, format!("{:?}", p.move_type));
    // hop-back: jump press below the corner without forward input -> wallrunJumpOutward/Up
    let mut p = native(start_mantle(col, false));
    run(&mut p, col, 2, inp(1.0, 0.0));
    sim::simulate(&mut p, &Input { jump: true, ..Default::default() }, TICK, col);
    let mut n = 0;
    while p.mantling() && n < 30 {
        sim::simulate(&mut p, &Input::default(), TICK, col);
        n += 1;
    }
    c.near("hop-back: away from the wall at wallrunJumpOutwardSpeed 188", -p.vel.x, 188.0, 0.5);
    c.near("hop-back: up at wallrunJumpUpSpeed 240", p.vel.z, 240.0, 0.5);
    c.near("hop-back fires after automantle_jumpoff_duration 0.1 s", n as f32 * TICK, 0.1, 0.02);
    // cancel: let go of W early -> traversal ends, you drop
    let mut p = native(start_mantle(col, false));
    run(&mut p, col, 2, inp(1.0, 0.0));
    sim::simulate(&mut p, &Input::default(), TICK, col);
    c.truth("releasing W below the rest fraction cancels the mantle", !p.mantling() && p.vel.length() < 1.0, format!("{:?} v {:.0}", p.move_type, p.vel.length()));
    let mut p = native(start_mantle(col, false));
    run(&mut p, col, 2, inp(1.0, 0.0));
    sim::simulate(&mut p, &inp(-1.0, 0.0), TICK, col);
    c.truth("pulling back (S) cancels the mantle", !p.mantling(), format!("{:?}", p.move_type));
    // past the rest fraction the climb finishes on its own
    let mut p = native(start_mantle(col, false));
    mantle_until(&mut p, &|p| p.traversal.p > 0.45, inp(1.0, 0.0));
    run(&mut p, col, 60, Input::default());
    c.truth("past the rest fraction the mantle completes without input", !p.mantling() && p.pos.z > 59.9, format!("z {:.1}", p.pos.z));
    // air mantle: jump at the wall from a few units back, no jump held while rising into it
    let mut p = settle(col, Vec3::new(150.0, 2000.0, 0.1));
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, col);
    let mut mantled = false;
    for _ in 0..60 {
        sim::simulate(&mut p, &inp(1.0, 0.0), TICK, col);
        mantled |= p.mantling();
    }
    c.truth("jumping into the ledge mantles automatically in the air", mantled && p.pos.z > 59.9, format!("z {:.1}", p.pos.z));

    // ------------------------------------------------------------ mantle boost (default on)
    let sweet = |p: &Player| p.traversal.rate > 0.0 && (1.0 - p.traversal.p) / p.traversal.rate <= 0.1 && p.traversal.p > 0.5;
    let mut p = start_mantle(col, false);
    mantle_until(&mut p, &sweet, inp(1.0, 0.0));
    let sweet_frac = p.mantle_boost.sweet_spot_frac;
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, col);
    c.truth("mantle boost: jump in the sweet spot arms state 4, climb continues", p.mantle_boost.state == 4 && p.mantling(), format!("state {} {:?}", p.mantle_boost.state, p.move_type));
    let mut p_done = p;
    while p_done.mantling() {
        sim::simulate(&mut p_done, &inp(1.0, 0.0), TICK, col);
    }
    c.near("mantle boost exit h-speed = 260 x 1.3", hspeed(&p_done), 338.0, 0.5);
    c.near("mantle boost vz = sqrt(2*750*90)", p_done.vel.z, (2.0f32 * 750.0 * 90.0).sqrt(), 0.5);
    c.truth("mantle boost: exit along the input (+X), slide granted", p_done.vel.x > 337.0 && p_done.sliding, format!("{:.1?} sliding {}", p_done.vel, p_done.sliding));
    c.range("sweet spot opens late in the climb (frac)", sweet_frac, 0.8, 0.98);
    // boost suppresses tap-strafes until landing
    let mut q = p_done;
    run(&mut q, col, 5, inp(1.0, 0.0));
    let mut r = q;
    r.mantle_boost.state = 0; // same flight without the suppression
    let before = q.vel;
    sim::simulate(&mut q, &inp(0.0, 1.0), TICK, col);
    sim::simulate(&mut r, &inp(0.0, 1.0), TICK, col);
    let ang = before.truncate().angle_to(q.vel.truncate()).to_degrees().abs();
    let ang_free = before.truncate().angle_to(r.vel.truncate()).to_degrees().abs();
    c.range("no tap-strafe after a mantle boost (deg, air accel only)", ang, 0.0, 3.0);
    c.range("  (same press without the boost state: tap-strafe turns, deg)", ang_free, 15.0, 60.0);
    // press outside the sweet spot: plain mantle jump (state 3)
    let mut p = start_mantle(col, false);
    mantle_until(&mut p, &|p| p.traversal.p > 0.3, inp(1.0, 0.0));
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, col);
    c.truth("jump outside the sweet spot latches state 3", p.mantle_boost.state == 3, format!("state {}", p.mantle_boost.state));
    while p.mantling() {
        sim::simulate(&mut p, &inp(1.0, 0.0), TICK, col);
    }
    c.near("state 3 exit h-speed = sprintspeed 260", hspeed(&p), 260.0, 0.5);
    c.near("state 3 vz = sqrt(2*750*56)", p.vel.z, (2.0f32 * 750.0 * 56.0).sqrt(), 0.5);
    c.truth("state 3: no slide granted", !p.sliding, format!("sliding {}", p.sliding));
    // no press: ordinary finish
    let mut p = start_mantle(col, false);
    while p.mantling() {
        sim::simulate(&mut p, &inp(1.0, 0.0), TICK, col);
    }
    c.truth("no press: plain finish, boost state 0", p.mantle_boost.state == 0 && (hspeed(&p) - 173.5).abs() < 0.5, format!("state {} {:.1}", p.mantle_boost.state, hspeed(&p)));
}

/// Sprint from x -100 toward the wall face at x 200 (row `y`), view yaw `yaw`, jump at x 120, W held.
/// Returns the player on the tick it attached to the wall (None if it never did within 2 s).
fn climb_start(col: &Collision, y: f32, yaw: f32) -> Option<Player> {
    let mut p = settle(col, Vec3::new(-100.0, y, 0.1));
    let mut jumped = false;
    for _ in 0..120 {
        let jump = !jumped && p.pos.x > 120.0;
        jumped |= jump;
        sim::simulate(&mut p, &Input { forward: 1.0, sprint: true, yaw, jump, ..Default::default() }, TICK, col);
        if p.on_wall() {
            return Some(p);
        }
    }
    None
}

/// WallTop 0x140b9f680: min(attach z, refHeight + verticalGainCutoff_wallrun - climbFinalJumpUpHeight - climbHeight)
fn wall_top(p: &Player) -> f32 {
    let s = &p.settings;
    p.wall.attach_pos.z.min(((s.vertical_gain_cutoff_wallrun + p.wall.ref_height) - s.climb_final_jump_up_height) - s.climb_height)
}

fn wall_checks(c: &mut Checks, col: &Collision) {
    let g = STOCK;
    let w_in = Input { forward: 1.0, ..Default::default() };
    // ---- run + jump into the 300u wall, hold W
    let Some(p0) = climb_start(col, 4200.0, 0.0) else {
        c.truth("run + jump into a 300u wall attaches (wall climb)", false, "never attached".into());
        return;
    };
    c.truth("run + jump into a 300u wall attaches (wall climb)", p0.surface_normal.x < -0.99, format!("n {:.2?} z {:.1}", p0.surface_normal, p0.pos.z));
    c.range("attach: rising at most climbSpeedStart/2 (100)", p0.vel.z, -1000.0, 100.0);
    let top0 = wall_top(&p0);
    let mut p = p0;
    let mut ticks = 0;
    let mut prev = p;
    while p.on_wall() && ticks < 300 {
        prev = p;
        sim::simulate(&mut p, &w_in, TICK, col);
        ticks += 1;
    }
    let dur = ticks as f32 * TICK;
    c.near("climb rises climbHeight (100) above WallTop", prev.pos.z - top0, g.climb_height, 1.0);
    c.range("climb duration (200 -> 50 u/s over 100u, climbAccel 500)", dur, 0.9, 1.05);
    c.near("climb speed at the top = climbSpeedEnd", prev.vel.z, g.climb_speed_end, 2.0);
    // the top ends the climb with the final jump up: vz += sqrt(2 g (climbFinalJumpUpHeight + min(top - z, 10)))
    let h = ((top0 + g.climb_height) - prev.pos.z).min(10.0) + g.climb_final_jump_up_height;
    let want = prev.vel.z + (2.0 * 750.0 * h).sqrt() - 0.5 * 750.0 * TICK;
    c.near("top: final jump up (climbFinalJumpUpHeight 28)", p.vel.z, want, 0.05);
    c.near("top: pushed off by wallrun_fallAwaySpeed 70 (- one tick of air control)", p.vel.x, -70.0 + 500.0 * TICK, 0.05);
    c.near("top: reference height lowered by wallclimb_vertical_gain_reduction", p.wall.ref_height, p0.wall.ref_height - 128.0, 0.01);
    let mut peak = p.pos.z;
    let mut again = false;
    for _ in 0..240 {
        sim::simulate(&mut p, &w_in, TICK, col);
        peak = peak.max(p.pos.z);
        again |= p.on_wall() && p.pos.z > 20.0;
    }
    c.range("peak after the climb (feet z)", peak, 185.0, 200.0);
    c.truth("then drops to the floor (no re-climb of the same wall)", p.on_ground && !p.on_wall() && p.pos.z < 0.1 && !again, format!("z {:.2} {} again {again}", p.pos.z, p.state_name()));

    // ---- jump during the climb, W held and facing the wall
    let mut p = p0;
    run(&mut p, col, 20, w_in);
    let b = p;
    sim::simulate(&mut p, &Input { forward: 1.0, jump: true, ..Default::default() }, TICK, col);
    // WallJump: out = wallrunJumpOutwardSpeed * (1 - 0.8 (n.view)(n.input)) = 188 * 0.2; the same tick
    // WallRunUpdate sees the lowered reference height, ends the climb and adds wallrun_fallAwaySpeed 70
    c.truth("jump on the wall: off the wall", !p.on_wall() && !p.on_ground, p.state_name().to_string());
    c.near("wall jump (W held): out 188*0.2 + 70, minus one tick of air control", -p.vel.x, 188.0 * 0.2 + 70.0 - 500.0 * TICK, 0.05);
    c.near("wall jump: no up boost above the cutoff (keeps the climb vz)", p.vel.z, b.vel.z - 0.5 * 750.0 * TICK, 0.05);
    // without input: the full outward speed
    let mut p = p0;
    run(&mut p, col, 20, w_in);
    sim::simulate(&mut p, &Input { jump: true, ..Default::default() }, TICK, col);
    c.near("wall jump (no input): out 188 + 70", -p.vel.x, 188.0 + 70.0, 0.05);
    c.range("wall jump (no input): still rising", p.vel.z, 100.0, 140.0);

    // ---- let go of W: the climb stops, you slide and drop off
    let mut p = p0;
    run(&mut p, col, 20, w_in);
    let mut t = 0;
    while p.on_wall() && t < 300 {
        sim::simulate(&mut p, &Input::default(), TICK, col);
        t += 1;
    }
    c.range("let go of W: falls off within (s)", t as f32 * TICK, 0.5, 1.3);

    // ---- glancing approach: the view must face the wall within ~45.6 deg (cos^2 > 0.49)
    c.truth("approach at 50 deg: no climb", climb_start(col, 4200.0, 50.0).is_none(), String::new());
    c.truth("approach at 44 deg: climbs", climb_start(col, 4200.0, 44.0).is_some(), String::new());

    // ---- 150u wall: climb, then the mantle takes the ledge once it is within mantleHeight
    match climb_start(col, 5200.0, 0.0) {
        None => c.truth("150u wall: climb, then mantle onto the top", false, "never attached".into()),
        Some(mut p) => {
            let mut mantled = false;
            for _ in 0..180 {
                sim::simulate(&mut p, &w_in, TICK, col);
                mantled |= p.mantling();
                if mantled && !p.mantling() {
                    break;
                }
            }
            c.truth("150u wall: climb, then mantle onto the top", mantled && p.on_ground && (p.pos.z - 150.0).abs() < 0.1, format!("mantled {mantled} z {:.2}", p.pos.z));
        }
    }

    // ---- the 60u ledge from a running jump still mantles (no climb)
    let mut p = settle(col, Vec3::new(-100.0, 2000.0, 0.1));
    let (mut jumped, mut mantled, mut climbed) = (false, false, false);
    for _ in 0..180 {
        let jump = !jumped && p.pos.x > 120.0;
        jumped |= jump;
        sim::simulate(&mut p, &Input { forward: 1.0, sprint: true, jump, ..Default::default() }, TICK, col);
        mantled |= p.mantling();
        climbed |= p.on_wall();
    }
    c.truth("60u ledge from a running jump: mantles, no wall climb", mantled && !climbed && (p.pos.z - 60.0).abs() < 0.1, format!("mantled {mantled} climbed {climbed} z {:.2}", p.pos.z));
}

/// Settings consts vs the game's own file (tolerant line regex `"key": value`; poseSettings blocks in order).
fn check_settings_file(c: &mut Checks) {
    let cfg = shared::assets::Config::load();
    let path = cfg.assets_root().join("data/settings/player/mp/pilot_survival_firesupport.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        println!("SKIP settings file check ({} not found)", path.display());
        return;
    };
    let mut stances: Vec<Vec<(String, String)>> = vec![];
    let mut top: Vec<(String, String)> = vec![];
    let mut in_pose = false;
    let mut cur = vec![];
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("\"poseSettings\"") {
            in_pose = true;
            continue;
        }
        if in_pose && l.starts_with(']') {
            in_pose = false;
            continue;
        }
        if in_pose && l.starts_with('}') {
            stances.push(std::mem::take(&mut cur));
            continue;
        }
        if let Some((k, v)) = l.split_once(':') {
            let k = k.trim().trim_matches('"').to_string();
            let v = v.trim().trim_end_matches(',').trim_matches('"').to_string();
            if in_pose { cur.push((k, v)) } else { top.push((k, v)) }
        }
    }
    let num = |kv: &[(String, String)], k: &str| -> f32 {
        kv.iter().find(|(a, _)| a == k).map(|(_, v)| v.trim_start_matches('<').split(',').last().unwrap().trim_end_matches('>').parse().unwrap_or(f32::NAN)).unwrap_or(f32::NAN)
    };
    let s = STOCK;
    let mut bad = vec![];
    for (i, st) in [s.stand, s.crouch].iter().enumerate() {
        let kv = &stances[i];
        for (k, v) in [
            ("hull_max", st.hull_max.z),
            ("viewheight", st.viewheight),
            ("lowSpeed", st.low_speed),
            ("speed", st.speed),
            ("sprintspeed", st.sprintspeed),
            ("acceleration", st.acceleration),
            ("deceleration", st.deceleration),
            ("lowAcceleration", st.low_acceleration),
            ("sprintAcceleration", st.sprint_acceleration),
            ("sprintDeceleration", st.sprint_deceleration),
        ] {
            if num(kv, k) != v {
                bad.push(format!("pose{i}.{k}"));
            }
        }
    }
    for (k, v) in [
        ("stepHeight", s.step_height),
        ("airSpeed", s.air_speed),
        ("airAcceleration", s.air_acceleration),
        ("jumpHeight", s.jump_height),
        ("slideJumpHeight", s.slide_jump_height),
        ("antiMultiJumpHeightFrac", s.anti_multi_jump_height_frac),
        ("antiMultiJumpTimeMin", s.anti_multi_jump_time_min),
        ("antiMultiJumpTimeMax", s.anti_multi_jump_time_max),
        ("landSlowdownDuration", s.land_slowdown_duration),
        ("landSlowdownHeightMin", s.land_slowdown_height_min),
        ("landSlowdownHeightMax", s.land_slowdown_height_max),
        ("landSlowdownNoSprintFrac", s.land_slowdown_no_sprint_frac),
        ("mantleHeight", s.mantle_height),
        ("mantleDurationBelow", s.mantle_duration_below),
        ("mantleDurationLevel", s.mantle_duration_level),
        ("mantleDurationAbove", s.mantle_duration_above),
        ("mantleDurationHigh", s.mantle_duration_high),
        ("player_slideBoostCooldown", s.slide_boost_cooldown),
        ("slideDecel", s.slide_decel),
        ("slideWantToStopDecel", s.slide_want_to_stop_decel),
        ("slideVelocityDecay", s.slide_velocity_decay),
        ("slideAccel", s.slide_accel),
        ("slideRequiredStartSpeed", s.slide_required_start_speed),
        ("slideRequiredStartSpeedAir", s.slide_required_start_speed_air),
        ("slideStopSpeed", s.slide_stop_speed),
        ("slideMaxStopSpeed", s.slide_max_stop_speed),
        ("slideMaxJumpSpeed", s.slide_max_jump_speed),
        ("slideSpeedBoost", s.slide_speed_boost),
        ("slideSpeedBoostCap", s.slide_speed_boost_cap),
        ("wallrunJumpUpSpeed", s.wallrun_jump_up_speed),
        ("wallrunJumpOutwardSpeed", s.wallrun_jump_outward_speed),
        ("wallrunJumpInputDirSpeed", s.wallrun_jump_input_dir_speed),
        ("climbHeight", s.climb_height),
        ("climbFinalJumpUpHeight", s.climb_final_jump_up_height),
        ("climbSpeedStart", s.climb_speed_start),
        ("climbSpeedEnd", s.climb_speed_end),
        ("climbAccel", s.climb_accel),
        ("climbDecel", s.climb_decel),
        ("climbHorzSpeed", s.climb_horz_speed),
        ("climbHorzAccel", s.climb_horz_accel),
        ("climbHorzDecel", s.climb_horz_decel),
        ("verticalGainCutoff_wallrun", s.vertical_gain_cutoff_wallrun),
        ("wallrun_timeLimit", s.wallrun_time_limit),
        ("wallrunCeilingLimit", s.wallrun_ceiling_limit),
        ("wallrunAllowedWallDistance", s.wallrun_allowed_wall_distance),
        ("wallrunUpWallBoost", s.wallrun_up_wall_boost),
        ("wallrunMaxSpeedVertical", s.wallrun_max_speed_vertical),
        ("wallrunMaxSpeedHorizontal", s.wallrun_max_speed_horizontal),
        ("wallrunMaxSpeedHorizontalBackward", s.wallrun_max_speed_horizontal_backward),
        ("wallrunAccelerateVertical", s.wallrun_accelerate_vertical),
        ("wallrunDecelerateVertical", s.wallrun_decelerate_vertical),
        ("wallrunAccelerateHorizontal", s.wallrun_accelerate_horizontal),
        ("wallrunDecelerateHorizontal", s.wallrun_decelerate_horizontal),
        ("wallrunGravityRampUpTime", s.wallrun_gravity_ramp_up_time),
        ("wallrunSameWallHeight", s.wallrun_same_wall_height),
        ("wallrunAdsType", s.wallrun_ads_type as f32),
        ("mantleAngleScale", s.mantle_angle_scale),
        ("gravityScale", s.gravity_scale),
    ] {
        if num(&top, k) != v {
            bad.push(k.to_string());
        }
    }
    c.truth("settings consts == pilot_survival_firesupport.json", bad.is_empty(), if bad.is_empty() { "all match".into() } else { bad.join(",") });
}

fn real_map(c: &mut Checks) {
    use shared::{assets::Config, collmesh};
    let cfg = Config::load();
    // The game's collision.bin when exported, else world.glb's render triangles.
    let Ok((col, src)) = collmesh::build_player_collision(&cfg) else {
        println!("SKIP real map (no collision.bin or world.glb)");
        return;
    };
    println!("real map: {src:?} collision, {} triangles, BVH {:.0} ms", col.tri_count(), col.build_ms);
    let spawn = sim::find_free(&col, Vec3::new(-11740.0, 512.0, 1584.0));
    let mut p = Player::new(spawn);
    run(&mut p, &col, 30, Input::default());
    c.truth("real map: spawn lands on the floor", p.on_ground, format!("z {:.2}", p.pos.z));
    let mut q = p;
    run(&mut q, &col, 600, Input { forward: 1.0, sprint: true, ..Default::default() });
    let moved = (q.pos - p.pos).truncate().length();
    c.range("real map: 10 s sprint down the spawn corridor + ramp (horizontal units)", moved, 2200.0, 2600.0);
    c.truth("real map: on the ground at the end", q.on_ground, format!("({:.0},{:.0},{:.0})", q.pos.x, q.pos.y, q.pos.z));
    let z0 = p.pos.z;
    let t = Instant::now();
    let mut worst = 0.0f64;
    let ticks = 600;
    for i in 0..ticks {
        let jump = i % 50 == 25;
        let crouch = (i / 120) % 2 == 1 && i % 120 < 30;
        let ti = Instant::now();
        sim::simulate(&mut p, &Input { forward: 1.0, sprint: true, jump, crouch, right: if (i / 90) % 2 == 0 { 0.0 } else { 0.3 }, ..Default::default() }, TICK, &col);
        worst = worst.max(ti.elapsed().as_secs_f64());
    }
    let avg_us = t.elapsed().as_secs_f64() * 1e6 / ticks as f64;
    c.truth("real map: still above the floor (no fall-through)", p.pos.z > z0 - 200.0 && p.move_type == MoveType::Walk, format!("z {:.0} (start {:.0})", p.pos.z, z0));
    c.range("real map: avg tick cost (us)", avg_us as f32, 0.0, 1000.0);
    println!("     real map tick: avg {avg_us:.1} us, worst {:.1} us, ended at ({:.0},{:.0},{:.0}) {}", worst * 1e6, p.pos.x, p.pos.y, p.pos.z, p.state_name());
}
