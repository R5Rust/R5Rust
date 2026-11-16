//! Ground handling: CategorizePosition, SetGroundEntity, StayOnGround / GroundSnapTrace, step smoothing,
//! CheckStuck, LandingSpeedScale (playermove.md sections 4-8).

use super::{duck, Ctx, MoveType, UP};
use crate::collision::Trace;
use glam::Vec3;

/// CategorizePosition 0x140ba4f30 (playermove.md 4), forceVel = 0 at every caller here. The world is the only
/// ground entity, so base velocity / standable-entity / push-off branches drop out.
pub(super) fn categorize_position(c: &mut Ctx) {
    // 4.1
    c.p.surface_friction = 1.0;
    if c.p.move_type == MoveType::Traverse {
        return;
    }
    let n = UP;
    // 4.2 probe length: the step height while walking on ground (snaps down stairs/ramps), else 2
    let mut snap = c.p.move_type == MoveType::Walk && c.gnd();
    let dist = if snap { c.s.step_height } else { 2.0 };
    // 4.3
    let o0 = c.p.pos;
    let pend = o0 - n * dist;
    let vn = n.dot(c.p.vel);
    // 4.4 moving away from the ground (NON_JUMP_VELOCITY 140)
    if vn > 140.0 {
        // 4.13 + 4.15
        set_ground(c, None);
        c.p.ground_normal = Vec3::ZERO;
        return;
    }
    // 4.6
    let t = c.trace(o0, pend);
    let hit = t.fraction < 1.0;
    let mut tn = t.normal;
    let mut dn = tn.dot(n);
    let mut grounded = false;
    if hit {
        if dn >= 0.7 {
            // 4.9: standable; moving away from it faster than 140 leaves the ground entity unchanged
            if 140.0 > vn {
                grounded = true;
            } else {
                finish(c, &t, tn, snap);
                return;
            }
        } else {
            // 4.10 ledge re-probe: standing on an edge, look for the walkable face next to it
            if dn > 0.0 && c.gnd() {
                let r = tn - n * dn;
                let w = c.half_width();
                let s = (1.0 - dn * dn).sqrt();
                let cc = (s - 0.71414) * w;
                let mut a = (((1.0 - s) * w) as f64 + 0.03125) as f32;
                let mut h = (w * dn) + 0.03125;
                let g = (1.0 - t.fraction) * dist;
                if g > h {
                    a = (((g - h) * 0.5) + a).min((8.0 - cc) - 0.03125);
                    h = g;
                }
                h += 12.0;
                let a = a / s;
                let s2 = (r * a + t.endpos) + n * 12.0;
                let e2 = s2 - n * h;
                let t2 = c.trace(s2, e2);
                if !t2.startsolid && 1.0 > t2.fraction {
                    tn = t2.normal;
                    dn = t2.normal.dot(n);
                }
            }
            // 4.11
            grounded = dn >= 0.7;
        }
    }
    if grounded {
        // 4.12 (SurfaceFromTrace: default surfaces give friction 1.0)
        let mut tg = t;
        tg.normal = tn;
        set_ground(c, Some(&tg));
    } else {
        // airborne
        if hit && (c.gnd() || 0.1 > (c.now - c.p.near_ground_time)) {
            c.p.near_ground_time = c.now;
        }
        set_ground(c, None);
        if vn > 0.0 {
            c.p.surface_friction = 0.25;
        }
        snap = false;
    }
    finish(c, &t, tn, snap);
}

/// 4.14 snap + 4.15 ground plane
fn finish(c: &mut Ctx, t: &Trace, tn: Vec3, snap: bool) {
    if snap && !t.startsolid && t.fraction > 0.0 && 1.0 > t.fraction && c.ft != 0.0 {
        let d = t.endpos - c.p.pos;
        if d.length_squared() > 2.0 {
            step_smooth(c, d);
        }
        c.p.pos = t.endpos;
    }
    c.p.ground_normal = if c.gnd() && (1.0 > t.fraction || t.allsolid || t.startsolid) { tn } else { Vec3::ZERO };
}

/// SetGroundEntity 0x140ba46d0 (playermove.md 5). `tr` = the ground trace (normal used), None = leave the ground.
pub(super) fn set_ground(c: &mut Ctx, tr: Option<&Trace>) {
    let n = UP;
    let old = c.p.ground;
    let nw = tr.is_some();
    if old != nw {
        if let Some(tr) = tr {
            if !old {
                // LANDING: impact energy (as a fall height), landing time, hard-landing slowdown
                let vn = c.p.vel.dot(tr.normal);
                let r = ((c.now - c.p.last_land_time) / c.s.land_slowdown_duration).max(0.0).min(1.0);
                let e = ((vn * vn) * 0.5) / c.cv.sv_gravity;
                let x = (1.0 - r) * c.p.land_impact;
                c.p.land_impact = if x > e { x } else { e };
                c.p.last_land_time = c.now;
                let k = c.p.vel.dot(n);
                let q = n * k;
                let s = landing_speed_scale(c);
                c.p.vel = (c.p.vel - q) * s + q;
            }
            if c.p.duck_state == 1 || c.p.duck_state == 2 {
                duck::start_sliding(c, tr.normal, !old);
            }
        } else {
            c.p.air_from_jump = false;
        }
        c.p.ground = nw;
    }
    if nw {
        // ReplenishDoubleJump (skip_replenish_double_jump 1, upright): clears "jumped since leaving ground"
        c.p.jumped = false;
        let k = n.dot(c.p.vel);
        c.p.vel -= n * k;
    }
}

/// LandingSpeedScale 0x140bf5200 (disassembled for this port): 1 unless within landSlowdownDuration of a landing
/// whose impact height exceeded landSlowdownHeightMin.
pub(super) fn landing_speed_scale(c: &Ctx) -> f32 {
    let s = &c.s;
    let e = c.now - c.p.last_land_time;
    let d = s.land_slowdown_duration;
    if !(e < d) {
        return 1.0;
    }
    let h = c.p.land_impact;
    let mn = s.land_slowdown_height_min;
    if mn >= h {
        return 1.0;
    }
    let mx = s.land_slowdown_height_max;
    let hf = if h >= mx { 1.0 } else { (h - mn) / (mx - mn) };
    let r = (e / d).max(0.0).powf(s.land_slowdown_time_power);
    1.0 - ((1.0 - r) * hf) * (1.0 - s.land_slowdown_frac)
}

/// StayOnGround 0x140b8b9f0 (6.1): keep the feet on the floor down slopes and stairs.
pub(super) fn stay_on_ground(c: &mut Ctx) -> bool {
    let o = c.p.pos;
    let Some(t) = ground_snap_trace(c, o, c.s.step_height, UP) else { return false };
    let d = t.endpos - o;
    if d.length_squared() > 2.0 {
        step_smooth(c, d);
    }
    c.p.pos = t.endpos;
    true
}

/// GroundSnapTrace 0x140b8b7c0 (6.2)
fn ground_snap_trace(c: &Ctx, start: Vec3, dist: f32, dir: Vec3) -> Option<Trace> {
    let mut s = start;
    let e = s - dir * dist;
    let mut t = c.trace(s, e);
    if t.startsolid {
        s += dir;
        t = c.trace(s, e);
        if t.startsolid {
            return None;
        }
    }
    let f = t.fraction;
    if !(0.0 < f) {
        return Some(t);
    }
    if f >= 1.0 {
        return None;
    }
    if 0.7 > dir.dot(t.normal) {
        return None;
    }
    let off = (t.endpos - start).dot(dir);
    if off.abs() <= 0.015625 {
        t.endpos = start;
    }
    Some(t)
}

/// StepSmooth 0x140b9ac40 (6.3): the camera lags a step by its height, then DecayStepSmooth eases it back.
pub(super) fn step_smooth(c: &mut Ctx, d: Vec3) {
    let n = UP;
    let dn = n.dot(d);
    let s = if c.s.step_height < dn { c.s.step_height } else { dn };
    c.p.step_smooth -= n * s;
}

/// DecayStepSmooth 0x140be80c0
pub(super) fn decay_step_smooth(c: &mut Ctx) {
    let w = c.p.step_smooth;
    if w == Vec3::ZERO {
        return;
    }
    let l = w.length();
    let k = (c.ft.max(0.0) * -10.0).exp();
    let m = (c.s.step_height * 2.0).min(l) * k;
    c.p.step_smooth = if m > 0.0 && l > 0.0 { w * (m / l) } else { Vec3::ZERO };
}

/// Source's 54-entry stuck table (CreateStuckTable); Apex has a 54-entry table filled the same way at runtime.
fn stuck_table() -> [Vec3; 54] {
    let mut t = [Vec3::ZERO; 54];
    let mut i = 0;
    let mut push = |v: Vec3| {
        t[i] = v;
        i += 1;
    };
    for z in [-0.125f32, 0.0, 0.125] {
        push(Vec3::new(0.0, 0.0, z));
    }
    for y in [-0.125f32, 0.0, 0.125] {
        push(Vec3::new(0.0, y, 0.0));
    }
    for x in [-0.125f32, 0.0, 0.125] {
        push(Vec3::new(x, 0.0, 0.0));
    }
    for x in [-0.125f32, 0.125] {
        for y in [-0.125f32, 0.125] {
            for z in [-0.125f32, 0.125] {
                push(Vec3::new(x, y, z));
            }
        }
    }
    let zi = [0.0f32, 1.0, 6.0];
    for z in zi {
        push(Vec3::new(0.0, 0.0, z));
    }
    for y in [-2.0f32, 0.0, 2.0] {
        push(Vec3::new(0.0, y, 0.0));
    }
    for x in [-2.0f32, 0.0, 2.0] {
        push(Vec3::new(x, 0.0, 0.0));
    }
    for z in zi {
        for x in [-2.0f32, 0.0, 2.0] {
            for y in [-2.0f32, 0.0, 2.0] {
                push(Vec3::new(x, y, z));
            }
        }
    }
    t
}

/// CheckStuck 0x140ba42d0 (8.1): one nudge from the stuck table per try, retried every 0.05 s.
/// Returns 0 when free (possibly after moving the origin).
pub(super) fn check_stuck(c: &mut Ctx) -> i32 {
    let o = c.p.pos;
    if !c.trace(o, o).startsolid {
        c.p.stuck_last = 0;
        return 0;
    }
    if c.p.stuck_check_time >= c.now - 0.05 {
        return 1;
    }
    c.p.stuck_check_time = c.now;
    let i = (c.p.stuck_last % 54) as usize;
    c.p.stuck_last += 1;
    let p = o + stuck_table()[i];
    if c.trace(p, p).startsolid {
        return 1;
    }
    c.p.stuck_last = 0;
    c.p.pos = p;
    0
}
