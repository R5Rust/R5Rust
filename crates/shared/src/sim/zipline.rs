//! Zipline movement, ported from R5F S30 `r5apex_dx12_s30.exe` (md5 9fcca108...; addresses below are S30).
//!
//! States (player `m_ziplineState`, +0x2fbc): 0 none, 1 mount requested, 2 mounting, 3 riding. The movement loop
//! (0x140b25d60) runs the zipline code instead of the normal move while it is non-zero, writing origin and velocity
//! directly (no collision move): mount begin 0x1407c65c0, mounting 0x1407c71e0, riding 0x1407c8200.
//! Tuning: retail player settings (`settings/player/mp/pilot_survival_firesupport`, stgs export of the retail
//! common.rpak): ziplineSpeed 600, ziplineAcceleration 400, mountZiplineTime 0.5, useZiplineCooldown 0.4,
//! ziplineJumpOffSpeed 400, ziplineJumpOffDampScale 1, ziplineDetachSpeed 600, ziplineMaxVerticalDetachSpeed 1500.
//! Convar `zipline_use_range` 120. Not ported: ziprails (curved), the grapple mount, vine (end-detached) lines,
//! the player-in-front slowdown (single player), the reattach cooldown escalation (gated by a playlist var whose
//! default isn't known), `zipline_slope_offset_correction` (default off).

use super::*;
use crate::zipline::Zipline;

pub const ZIPLINE_SPEED: f32 = 600.0;
pub const ZIPLINE_ACCELERATION: f32 = 400.0;
pub const MOUNT_ZIPLINE_TIME: f32 = 0.5;
pub const USE_ZIPLINE_COOLDOWN: f32 = 0.4;
pub const ZIPLINE_JUMP_OFF_SPEED: f32 = 400.0;
pub const ZIPLINE_JUMP_OFF_DAMP_SCALE: f32 = 1.0;
pub const ZIPLINE_DETACH_SPEED: f32 = 600.0;
pub const ZIPLINE_MAX_VERTICAL_DETACH_SPEED: f32 = 1500.0;
/// convar zipline_use_range
pub const ZIPLINE_USE_RANGE: f32 = 120.0;

/// Sound / animation cue for the client (CodeCallback_ZiplineMount / Start / Stop order).
pub const ZIP_EV_MOUNT: u8 = 1;
pub const ZIP_EV_START: u8 = 2;
pub const ZIP_EV_STOP: u8 = 4;

#[derive(Clone, Copy, Debug, Default)]
pub struct ZipState {
    /// 0 none, 1 mount requested, 2 mounting, 3 riding
    pub state: u8,
    /// index into Collision::ziplines
    pub line: usize,
    /// m_mountingZiplineAlpha 0..1
    pub mount_alpha: f32,
    pub src_pos: Vec3,
    pub src_vel: Vec3,
    /// m_mountingZiplineTargetPosition: the origin target (cable point - hull height)
    pub target: Vec3,
    /// m_ziplineUsePosition: the attach point on the cable
    pub use_pos: Vec3,
    /// m_slidingZiplineAlpha: arc-length fraction along the cable
    pub alpha: f32,
    /// m_ziplineReverse: riding end -> start
    pub reverse: bool,
    pub last_line: Option<usize>,
    pub last_detach_time: f32,
    pub start_time: f32,
    /// cues since the client last read them (ZIP_EV_*)
    pub events: u8,
}

impl ZipState {
    pub fn active(&self) -> bool {
        self.state != 0
    }
    pub fn riding(&self) -> bool {
        self.state == 3
    }
}

fn hull_h(p: &Player) -> f32 {
    let (mn, mx) = p.hull();
    mx.z - mn.z
}

/// eye -> hull-top gap e = max(0, h - view height)
fn eye_gap(p: &Player) -> f32 {
    (hull_h(p) - p.view_z).max(0.0)
}

/// HullTrace 0x1407b6d60: clear only with fraction 1 and no start/all solid.
fn hull_clear(p: &Player, col: &Collision, from: Vec3, to: Vec3) -> bool {
    let (mn, mx) = p.hull();
    let t = col.trace_hull(from, to, mn, mx);
    t.fraction >= 1.0 && !t.startsolid && !t.allsolid
}

/// Closest approach of a ray (o, unit d) to segment [a, b]; None when that point is behind the ray start.
/// Returns (distance, point on the segment). FUN_140baa490 (semantics per the spec, a guess).
fn ray_segment(o: Vec3, d: Vec3, a: Vec3, b: Vec3) -> Option<(f32, Vec3)> {
    let u = b - a;
    let w = a - o;
    let (aa, bb, cc, dd, ee) = (u.dot(u), u.dot(d), d.dot(d), u.dot(w), d.dot(w));
    let den = aa * cc - bb * bb;
    let mut s = if den.abs() > 1e-9 { ((bb * ee - cc * dd) / den).clamp(0.0, 1.0) } else { 0.0 };
    let t = (bb * s + ee) / cc.max(1e-9);
    if t < 0.0 {
        return None;
    }
    s = ((t * bb - dd) / aa.max(1e-9)).clamp(0.0, 1.0);
    let q = a + u * s;
    let r = o + d * ((q - o).dot(d)).max(0.0);
    Some((q.distance(r), q))
}

/// Direction choice 0x1407b75b0 + mount reverse distance 0x1407b7150.
fn choose_reverse(z: &Zipline, q: Vec3, f: Vec3, h: f32) -> bool {
    let (first, last) = (z.points[0], *z.points.last().unwrap());
    let dir = (last - first).normalize_or(Vec3::X);
    let mut reverse = if z.vertical {
        if dir.z <= 0.0 { dir.dot(f) < 0.5 } else { dir.dot(f) < -0.5 }
    } else {
        dir.dot(f) < 0.0
    };
    let rev_dist = if z.mount_reverse_distance != 0.0 { z.mount_reverse_distance } else { z.auto_detach_distance.max(0.01) };
    let dmin = |e: Vec3| [q, q + Vec3::Z * h, q - Vec3::Z * h].iter().map(|p| p.distance_squared(e)).fold(f32::MAX, f32::min);
    let (ds, de) = (dmin(first), dmin(last));
    let r2 = rev_dist * rev_dist;
    if !reverse && de <= r2 && de < ds {
        reverse = true;
    }
    if reverse && ds <= r2 && ds < de {
        reverse = false;
    }
    reverse
}

/// Use check 0x1407bfa40: the line in reach whose cable passes closest to the view ray. Returns (line, attach point,
/// reverse) when the hull can reach the hang point.
pub fn find_mount(p: &Player, col: &Collision, view_f: Vec3, now: f32) -> Option<(usize, Vec3, bool)> {
    let top = p.pos + Vec3::Z * 72.0;
    let r = ZIPLINE_USE_RANGE;
    let ray_o = top + view_f * 7.0;
    let mut best: Option<(f32, usize, Vec3)> = None;
    for (i, z) in col.ziplines.iter().enumerate() {
        if z.points.len() < 2 {
            continue;
        }
        if p.zip.last_line == Some(i) && now - p.zip.last_detach_time <= USE_ZIPLINE_COOLDOWN {
            continue;
        }
        if z.closest(top).2 >= r {
            continue;
        }
        let rr = r + 10.0;
        for s in 1..z.points.len() {
            let (a, b) = (z.points[s - 1], z.points[s]);
            // clip the segment to the sphere (top, rr)
            let u = b - a;
            let m = a - top;
            let (qa, qb, qc) = (u.dot(u), 2.0 * u.dot(m), m.dot(m) - rr * rr);
            let disc = qb * qb - 4.0 * qa * qc;
            if qa < 1e-9 || disc < 0.0 {
                continue;
            }
            let sq = disc.sqrt();
            let (t0, t1) = (((-qb - sq) / (2.0 * qa)).max(0.0), ((-qb + sq) / (2.0 * qa)).min(1.0));
            if t0 > t1 {
                continue;
            }
            if let Some((d, q)) = ray_segment(ray_o, view_f, a + u * t0, a + u * t1) {
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, i, q));
                }
            }
        }
    }
    let (_, i, q) = best?;
    let h = hull_h(p);
    if !hull_clear(p, col, p.pos, q - Vec3::Z * h) {
        return None;
    }
    Some((i, q, choose_reverse(&col.ziplines[i], q, view_f, h)))
}

/// ProjectAlpha 0x1407b88a0: arc-length fraction of the cable point closest to `p`.
fn project_alpha(z: &Zipline, p: Vec3) -> f32 {
    let l = z.length().max(1e-6);
    (z.closest(p).0 / l).clamp(0.0, 1.0)
}

/// The auto-detach distance of the end being ridden towards (0x1407c6550).
fn auto_det(z: &Zipline, reverse: bool) -> f32 {
    (if reverse { z.auto_detach_distance } else { z.end_auto_detach_distance }).max(0.01)
}

/// Advance 0x1407b92f0: (cable point, velocity, alpha', endAlpha).
fn advance(z: &Zipline, alpha: f32, vel: Vec3, reverse: bool, e: f32, accel: f32, max_speed: f32, dt: f32) -> (Vec3, Vec3, f32, f32) {
    let l = z.length().max(1e-6);
    let d = (auto_det(z, reverse) - e).max(0.0);
    let end_alpha = if !reverse { (l - d).max(0.0) / l } else { 1.0 - (l - d).max(0.0) / l };
    let (_, mut seg) = z.at(alpha * l);
    if end_alpha < alpha {
        seg = -seg;
    }
    let speed = (seg.dot(vel).max(0.01) + accel * dt).min(max_speed);
    let da = speed * dt / l;
    let a2 = if alpha <= end_alpha { (alpha + da).min(end_alpha) } else { (alpha - da).max(end_alpha) };
    let (cable, mut nd) = z.at(a2 * l);
    if reverse {
        nd = -nd;
    }
    (cable, nd * speed, a2, end_alpha)
}

/// End auto-detach 0x1407c6ac0: Some(exit velocity) when the rider reached the end.
fn end_detach(p: &Player, z: &Zipline, vel: Vec3, end_alpha: f32, view_f: Vec3) -> Option<Vec3> {
    let (first, last) = (z.points[0], *z.points.last().unwrap());
    let reverse = p.zip.reverse;
    let (target, from) = if reverse { (first, last) } else { (last, first) };
    if p.zip.alpha != end_alpha {
        let h = hull_h(p);
        let d2 = target.distance_squared(p.pos).min(target.distance_squared(p.pos + Vec3::Z * h));
        let ad = auto_det(z, reverse);
        if d2 > ad * ad {
            return None;
        }
    }
    if !z.vertical {
        let n = z.points.len();
        let dir = if reverse { z.points[0] - z.points[1] } else { z.points[n - 1] - z.points[n - 2] }.normalize_or(Vec3::X);
        let s = dir.dot(vel).max(ZIPLINE_SPEED).min(ZIPLINE_DETACH_SPEED);
        Some(dir * s)
    } else if target.z < from.z {
        Some(vel)
    } else {
        let f = Vec3::new(view_f.x, view_f.y, 0.0).normalize_or(Vec3::X);
        let mut v = f * ZIPLINE_JUMP_OFF_SPEED;
        if z.preserve_velocity {
            v += vel;
        }
        if v.length() > ZIPLINE_MAX_VERTICAL_DETACH_SPEED {
            v = v.normalize() * ZIPLINE_MAX_VERTICAL_DETACH_SPEED;
        }
        Some(v)
    }
}

/// Input detach 0x1407b9a50 (jump / crouch press): Some(exit velocity).
fn input_detach(p: &Player, z: &Zipline, pressed: u32, view_f: Vec3) -> Option<Vec3> {
    if pressed & (IN_JUMP | IN_DUCK) == 0 {
        return None;
    }
    let j = ZIPLINE_JUMP_OFF_SPEED;
    let k = ZIPLINE_JUMP_OFF_DAMP_SCALE;
    let jump = pressed & IN_JUMP != 0;
    let mut v = if z.vertical {
        Vec3::new(view_f.x, view_f.y, 0.0).normalize_or(Vec3::X) * j
    } else {
        let to = if jump { view_f * j } else { Vec3::new(0.0, 0.0, -j) };
        let mut v = to.lerp(p.vel, k);
        if v.length() > j {
            v = v.normalize() * j;
        }
        v
    };
    if jump {
        v += Vec3::Z * (2.0 * p.cvars.sv_gravity * p.settings.jump_height).sqrt();
    }
    Some(v)
}

/// Common detach 0x1407c8a10.
fn detach(p: &mut Player, vel: Vec3, now: f32) {
    p.zip.last_line = Some(p.zip.line);
    p.zip.last_detach_time = now;
    p.zip.state = 0;
    p.zip.mount_alpha = 0.0;
    p.zip.events |= ZIP_EV_STOP;
    p.vel = vel;
    p.on_ground = false;
}

/// Mount start 0x1407c52e0 (state 1). The script's CodeCallback_ZiplineMount forces the standing stance.
pub fn start_mount(p: &mut Player, line: usize, q: Vec3, reverse: bool) {
    p.zip.state = 1;
    p.zip.line = line;
    p.zip.src_pos = p.pos;
    p.zip.src_vel = p.vel;
    p.zip.use_pos = q;
    p.zip.reverse = reverse;
    p.zip.events |= ZIP_EV_MOUNT;
    // the whole stand: hull, duck state, timer and eye height (the duck code doesn't run while on the line, so a
    // crouched eye used to stay low through the ride and you landed looking out from crouch height)
    p.ducked = false;
    p.duck_state = 0;
    p.duck_timer_ms = 0;
    p.duck_in_air = false;
    p.view_z = p.settings.stand.viewheight;
    p.sliding = false;
    p.sprinting = false;
}

/// One zipline tick (state 1, 2 or 3). `pressed` = buttons pressed this tick.
pub fn tick(p: &mut Player, col: &Collision, pressed: u32, view_f: Vec3, dt: f32, now: f32) {
    let Some(z) = col.ziplines.get(p.zip.line).cloned() else {
        p.zip.state = 0;
        return;
    };
    let h = hull_h(p);
    let e = eye_gap(p);
    let max_speed = ZIPLINE_SPEED * z.speed_scale.max(0.0);
    let accel = ZIPLINE_ACCELERATION;
    p.on_ground = false;
    match p.zip.state {
        1 => {
            // mount begin 0x1407c65c0
            p.zip.state = 2;
            p.zip.mount_alpha = 0.0;
            p.zip.start_time = now;
            p.zip.target = p.zip.use_pos - Vec3::Z * h;
            p.pos = p.zip.src_pos;
            p.vel = p.zip.src_vel;
            p.zip.alpha = project_alpha(&z, p.zip.src_pos);
        }
        2 => {
            // mounting 0x1407c71e0
            if let Some(v) = input_detach(p, &z, pressed, view_f) {
                detach(p, v, now);
                return;
            }
            p.zip.mount_alpha = (p.zip.mount_alpha + dt / MOUNT_ZIPLINE_TIME.max(0.01)).clamp(0.0, 1.0);
            let o = p.pos;
            let dir = (*z.points.last().unwrap() - z.points[0]).normalize_or(Vec3::X);
            let alpha = project_alpha(&z, p.zip.target + Vec3::Z * h);
            let v0dir = if p.zip.reverse { -dir } else { dir };
            let speed0 = (p.zip.mount_alpha * 0.5 * max_speed).max(v0dir.dot(p.zip.src_vel));
            let (cable, vel, a2, end_a) = advance(&z, alpha, v0dir * speed0, p.zip.reverse, e, accel, max_speed, dt);
            if let Some(v) = end_detach(p, &z, vel, end_a, view_f) {
                detach(p, v, now);
                return;
            }
            let nt = cable - Vec3::Z * h;
            if hull_clear(p, col, o, nt) {
                p.zip.target = nt;
            } else if !hull_clear(p, col, o, p.zip.target) {
                let v = p.vel;
                detach(p, v, now);
                return;
            }
            p.zip.alpha = a2;
            p.vel = vel;
            p.pos = o + (p.zip.target - o) * p.zip.mount_alpha;
            if p.zip.mount_alpha >= 1.0 {
                p.zip.state = 3;
                p.zip.mount_alpha = 0.0;
                p.zip.events |= ZIP_EV_START;
            }
        }
        _ => {
            // riding 0x1407c8200
            if let Some(v) = input_detach(p, &z, pressed, view_f) {
                detach(p, v, now);
                return;
            }
            let o = p.pos;
            let (cable, vel, a2, end_a) = advance(&z, p.zip.alpha, p.vel, p.zip.reverse, e, accel, max_speed, dt);
            let no = cable - Vec3::Z * h;
            if hull_clear(p, col, o, no) {
                p.zip.alpha = a2;
                p.vel = vel;
                p.pos = no;
            } else {
                let v = p.vel;
                detach(p, v, now);
                return;
            }
            if let Some(v) = end_detach(p, &z, p.vel, end_a, view_f) {
                detach(p, v, now);
            }
        }
    }
}
