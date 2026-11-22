//! Viewmodel sway + bob, 1:1 with retail Apex (S21 r5apex_dx12.exe FUN_1406afc40 simulate, FUN_1406b0e60 matrix;
//! notes in D:\R5Rust\_work_retail\anim_re\viewmodel_motion.md section 3). The item's `sway_*` / `bob_*` settings
//! (weapon.json) drive a per-frame target (move + turn + bob) that the viewmodel eases toward; the result is one
//! matrix in viewmodel-local space (Source axes: x forward, y left, z up), applied after the eye: Final = Eye * M.
//!
//! Not done: the SWAY_ROTATE pivot attachment (RSX can't dump v21 attachments yet), so rotation is about the
//! viewmodel origin, the engine's own fallback for a model without that attachment.

use glam::{Affine3A, EulerRot, Quat, Vec3};

/// One block of settings (unzoomed, or the `_zoomed` copy). Field order follows the engine's block (0x54..0x1a0).
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub min_t: Vec3,
    pub max_t: Vec3,
    /// pitch, yaw, roll (degrees)
    pub min_r: Vec3,
    pub max_r: Vec3,
    pub translate_gain: f32,
    pub rotate_gain: f32,
    pub bob_cycle_time: f32,
    pub bob_min_speed: f32,
    pub bob_max_speed: f32,
    pub bob_vert_dist: f32,
    pub bob_horz_dist: f32,
    /// pitch, yaw, roll
    pub bob_ang: Vec3,
    pub bob_gain: f32,
    /// per direction: (translate xyz, rotate pitch/yaw/roll)
    pub move_forward: [f32; 6],
    pub move_back: [f32; 6],
    pub move_left: [f32; 6],
    pub move_right: [f32; 6],
    pub move_up: [f32; 6],
    pub move_down: [f32; 6],
    pub turn_left: [f32; 6],
    pub turn_right: [f32; 6],
    pub turn_up: [f32; 6],
    pub turn_down: [f32; 6],
}

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub u: Block,
    pub z: Block,
    /// sway_enable_zoomed (default on)
    pub enable_zoomed: bool,
    /// sway_rotate_scale_unzoomed / _zoomed (native vars, default 1)
    pub scale_u: f32,
    pub scale_z: f32,
}

/// A settings value: retail .txt prints small negative integers as their u64 wrap (18446744073709551613 = -3).
fn num(v: &serde_json::Value) -> Option<f32> {
    let s = v.as_str().map(str::trim).map(str::to_owned).or_else(|| v.as_f64().map(|f| f.to_string()))?;
    let f: f64 = s.parse().ok()?;
    Some(if f >= 9.2e18 { (f - 18446744073709551616.0) as f32 } else { f as f32 })
}

impl Settings {
    /// From weapon.json "settings" (missing keys take the S3 parser defaults).
    pub fn from_json(s: &serde_json::Value) -> Settings {
        let block = |sfx: &str| -> Block {
            let zoomed = !sfx.is_empty();
            let f = |k: &str, d: f32| num(&s[format!("{k}{sfx}")]).unwrap_or(d);
            let v3 = |a: &str, b: &str, c: &str| Vec3::new(f(a, 0.0), f(b, 0.0), f(c, 0.0));
            let six = |dir: &str| -> [f32; 6] {
                let k = |what: &str| f(&format!("sway_{dir}_{what}"), 0.0);
                [k("translate_x"), k("translate_y"), k("translate_z"), k("rotate_pitch"), k("rotate_yaw"), k("rotate_roll")]
            };
            Block {
                min_t: v3("sway_min_x", "sway_min_y", "sway_min_z"),
                max_t: v3("sway_max_x", "sway_max_y", "sway_max_z"),
                min_r: v3("sway_min_pitch", "sway_min_yaw", "sway_min_roll"),
                max_r: v3("sway_max_pitch", "sway_max_yaw", "sway_max_roll"),
                translate_gain: f("sway_translate_gain", if zoomed { 24.0 } else { 12.0 }),
                rotate_gain: f("sway_rotate_gain", if zoomed { 24.0 } else { 12.0 }),
                bob_cycle_time: f("bob_cycle_time", 0.0),
                bob_min_speed: f("bob_min_speed", 0.1),
                bob_max_speed: f("bob_max_speed", 0.2),
                bob_vert_dist: f("bob_vert_dist", 0.0),
                bob_horz_dist: f("bob_horz_dist", 0.0),
                bob_ang: v3("bob_pitch", "bob_yaw", "bob_roll"),
                bob_gain: f("bob_gain", 12.0),
                move_forward: six("move_forward"),
                move_back: six("move_back"),
                move_left: six("move_left"),
                move_right: six("move_right"),
                move_up: six("move_up"),
                move_down: six("move_down"),
                turn_left: six("turn_left"),
                turn_right: six("turn_right"),
                turn_up: six("turn_up"),
                turn_down: six("turn_down"),
            }
        };
        let flag = |k: &str| num(&s[k]).map(|v| v != 0.0).unwrap_or(true);
        Settings {
            u: block(""),
            z: block("_zoomed"),
            enable_zoomed: flag("sway_enable_zoomed"),
            scale_u: num(&s["sway_rotate_scale_unzoomed"]).unwrap_or(1.0).clamp(0.0, 1.0),
            scale_z: num(&s["sway_rotate_scale_zoomed"]).unwrap_or(1.0).clamp(0.0, 1.0),
        }
    }
}

/// The server tick the engine scales turn sway by (CGlobalVars+0x44, Apex runs 20 Hz).
const TICK_INTERVAL: f32 = 1.0 / 20.0;

fn wrap180(a: f32) -> f32 {
    let a = a % 360.0;
    if a > 180.0 {
        a - 360.0
    } else if a < -180.0 {
        a + 360.0
    } else {
        a
    }
}

fn angle_diff(a: f32, b: f32) -> f32 {
    wrap180(a - b)
}

/// The simulation state (vm+0x1d40).
#[derive(Clone, Debug, Default)]
pub struct Sway {
    trans: Vec3,
    rot: Vec3,
    prev_angles: Option<Vec3>,
    ring: [Vec3; 5],
    idx: usize,
    phase: f32,
    sin_v: f32,
    sin_h: f32,
    vert: f32,
    horz: f32,
}

/// What the simulation reads each frame.
pub struct Frame {
    pub dt: f32,
    /// player velocity, world units/s (Source axes)
    pub vel: Vec3,
    /// eye angles: pitch (down +), yaw (left +), roll, degrees
    pub angles: Vec3,
    /// zoom fraction 0..1
    pub ads: f32,
    pub on_ground: bool,
}

impl Sway {
    /// Forget the last eye angles (after a teleport / item swap so the turn sway doesn't spike).
    pub fn reset(&mut self) {
        *self = Sway::default();
    }

    /// One frame of FUN_1406afc40; returns M (viewmodel-local, Source axes) for FUN_1406b0e60.
    pub fn update(&mut self, st: &Settings, f: &Frame) -> Affine3A {
        let dt = f.dt.max(1e-5);
        let zoomed = f.ads > 0.0 && st.enable_zoomed;
        let b = if zoomed { &st.z } else { &st.u };
        let sc = if zoomed { st.scale_z } else { st.scale_u };
        // velocity into eye space (Source AngleVectors)
        let q = Quat::from_euler(EulerRot::ZYX, f.angles.y.to_radians(), f.angles.x.to_radians(), f.angles.z.to_radians());
        let (fwd, left, up) = (q * Vec3::X, q * Vec3::Y, q * Vec3::Z);
        let (vf, vl, vu) = (f.vel.dot(fwd), f.vel.dot(left), f.vel.dot(up));

        // ---- bob ----
        let cycle = b.bob_cycle_time;
        let fb = if f.on_ground && cycle > 0.0 {
            let spd = (vf * vf + vl * vl).sqrt();
            let span = b.bob_max_speed - b.bob_min_speed;
            if span.abs() < 1e-6 {
                if spd >= b.bob_max_speed { 1.0 } else { 0.0 }
            } else {
                ((spd - b.bob_min_speed) / span).clamp(0.0, 1.0)
            }
        } else {
            0.0
        };
        let e_b = (-b.bob_gain * dt).exp();
        let (sin_v, sin_h, tv, th);
        if fb > 0.0 {
            self.phase = (fb * dt + self.phase) % (2.0 * cycle);
            sin_v = (std::f32::consts::TAU * (self.phase % cycle) / cycle).sin();
            sin_h = (std::f32::consts::TAU * self.phase / (2.0 * cycle)).sin();
            tv = sin_v * b.bob_vert_dist;
            th = sin_h * b.bob_horz_dist;
        } else {
            if cycle > 0.0 {
                let tgt = if self.phase > 1.5 * cycle { 2.0 * cycle } else if self.phase < 0.5 * cycle { 0.0 } else { cycle };
                self.phase = (1.0 - e_b) * tgt + e_b * self.phase;
            }
            (sin_v, sin_h, tv, th) = (0.0, 0.0, 0.0, 0.0);
        }
        let lerp = |a: f32, b: f32, e: f32| (1.0 - e) * a + e * b;
        self.sin_v = lerp(sin_v, self.sin_v, e_b);
        self.sin_h = lerp(sin_h, self.sin_h, e_b);
        self.vert = lerp(tv, self.vert, e_b);
        self.horz = lerp(th, self.horz, e_b);
        let bob_t = Vec3::new(0.0, self.horz, self.vert);

        // ---- move sway: a per-frame impulse (scales with the frame time, as in the game) ----
        let mut acc = [0.0f32; 6];
        let mut add = |k: f32, c: &[f32; 6]| {
            for i in 0..6 {
                acc[i] += k * c[i];
            }
        };
        add((vf * dt).abs(), if vf >= 0.0 { &b.move_forward } else { &b.move_back });
        add((vl * dt).abs(), if vl >= 0.0 { &b.move_left } else { &b.move_right });
        add((vu * dt).abs(), if vu >= 0.0 { &b.move_up } else { &b.move_down });

        // ---- turn sway: the last 5 frames' angular velocity ----
        let prev = self.prev_angles.unwrap_or(f.angles);
        let w = Vec3::new(angle_diff(f.angles.x, prev.x), angle_diff(f.angles.y, prev.y), angle_diff(f.angles.z, prev.z)) / dt;
        self.idx = (self.idx + 1) % 5;
        self.ring[self.idx] = w;
        self.prev_angles = Some(f.angles);
        let avg = self.ring.iter().copied().sum::<Vec3>() * 0.2 * TICK_INTERVAL;
        add(avg.x.abs(), if avg.x < 0.0 { &b.turn_up } else { &b.turn_down });
        add(avg.y.abs(), if avg.y < 0.0 { &b.turn_right } else { &b.turn_left });

        // ---- clamp + ease ----
        let t = Vec3::new(acc[0], acc[1], acc[2]).clamp(b.min_t.min(b.max_t), b.max_t.max(b.min_t));
        let mut r = Vec3::new(wrap180(acc[3]), wrap180(acc[4]), wrap180(acc[5]));
        r = r.max(sc * b.min_r).min(sc * b.max_r);
        let e_t = (-b.translate_gain * dt).exp();
        self.trans = t * (1.0 - e_t) + self.trans * e_t;
        let e_r = (-b.rotate_gain * dt).exp();
        self.rot += Vec3::new(angle_diff(r.x, self.rot.x), angle_diff(r.y, self.rot.y), angle_diff(r.z, self.rot.z)) * (1.0 - e_r);

        // ---- ADS: rotation limits + bob angles blend over the first 45% of the zoom ----
        let bob_ang = if st.enable_zoomed {
            let k = (2.2 * f.ads).clamp(0.0, 1.0);
            let lo = (st.scale_u * st.u.min_r).lerp(st.scale_z * st.z.min_r, k);
            let hi = (st.scale_u * st.u.max_r).lerp(st.scale_z * st.z.max_r, k);
            self.rot = self.rot.max(lo).min(hi);
            let a = st.u.bob_ang.lerp(st.z.bob_ang, k);
            Vec3::new(a.x * self.sin_v, a.y * self.sin_h, a.z * self.sin_h)
        } else {
            Vec3::new(b.bob_ang.x * self.sin_v, b.bob_ang.y * self.sin_h, b.bob_ang.z * self.sin_h)
        };

        // M = AngleMatrix(rot + bob) about the (identity) pivot, translated by sway + bob
        let a = self.rot + bob_ang;
        let rq = Quat::from_euler(EulerRot::ZYX, a.y.to_radians(), a.x.to_radians(), a.z.to_radians());
        Affine3A::from_rotation_translation(rq, self.trans + bob_t)
    }
}
