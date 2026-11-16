//! StepSlide 0x140b9ad70, TryPlayerMoveStep 0x140b9a970, StepMove 0x140bb0610 (stepslide.md), and the two
//! callees the spec leaves as calls: TryPlayerMoveCore 0x140baeba0 and StepDownSlide 0x140baf900, rebuilt
//! as Source's TryPlayerMove bump loop (4 bumps, 5 clip planes, overbounce 1).

use super::{ground, Ctx, UP};
use crate::collision::{Collision, Trace};
use glam::Vec3;

/// StepSlide 0x140b9ad70 (stepslide.md 1): on a walkable hit, redirect the rest of the move along that surface
/// and re-trace; otherwise (or if that is blocked too) fall back to the step dispatcher. True = origin set.
pub(super) fn step_slide(c: &mut Ctx, tr: &mut Trace, keep_length: bool, dest: Option<Vec3>, force_fallback: bool) -> bool {
    let n = UP;
    let nn = tr.normal;
    let w = n.dot(nn);
    if !(w > 0.7) || force_fallback {
        try_player_move_step(c, tr);
        return false;
    }
    let s = tr.endpos;
    let along = |d: Vec3| -> (Vec3, f32, f32) {
        let dn = d.dot(nn);
        let cv = d * w - n * dn;
        (cv, d.length_squared(), cv.length_squared())
    };
    let leftover = (((1.0 - tr.fraction as f64) * c.ft as f64) as f32, c.p.vel);
    let k_vec = match (keep_length, dest) {
        // A
        (true, Some(dest)) => {
            let (cv, dd, cc) = along(dest - s);
            cv * (dd / cc).min(f32::MAX).sqrt()
        }
        // B
        (true, None) => {
            let (t, v) = leftover;
            let (cv, vv, cc) = along(v);
            cv * ((vv / cc).min(f32::MAX).sqrt() * t)
        }
        // C
        (false, Some(dest)) => {
            let (cv, dd, cc) = along(dest - s);
            let k = if (w * w) * dd > cc { 1.0 / w } else { (dd / cc).min(f32::MAX).sqrt() };
            cv * k
        }
        // D
        (false, None) => {
            let (t, v) = leftover;
            let (cv, vv, cc) = along(v);
            let k = if (w * w) * vv > cc { t / w } else { (vv / cc).min(f32::MAX).sqrt() * t };
            cv * k
        }
    };
    let e = Vec3::new(s.x + k_vec.x, s.y + k_vec.y, s.z + (k_vec.z + 0.01));
    let f0 = tr.fraction;
    *tr = c.trace(s, e);
    if tr.fraction == 1.0 {
        c.p.pos = e;
        return true;
    }
    tr.fraction = ((1.0 - f0) * tr.fraction) + f0;
    try_player_move_step(c, tr);
    false
}

/// TryPlayerMoveStep 0x140b9a970 (stepslide.md 2)
fn try_player_move_step(c: &mut Ctx, tr: &mut Trace) {
    let step_h = c.s.step_height;
    let mut pos = c.p.pos;
    let mut vel = c.p.vel;
    let step_out = step_move(c, &mut pos, &mut vel, c.ft, step_h, tr);
    c.p.pos = pos;
    c.p.vel = vel;
    // StepViewOffset 0x140b9ac40 = StepSmooth
    ground::step_smooth(c, step_out);
    // slide step speed loss: world z of the step
    if c.p.sliding && step_out.z > 0.0 {
        let v = c.p.vel;
        let sp = (v.x * v.x + v.y * v.y).sqrt();
        if sp > 0.0 {
            let r = (sp - step_out.z * c.cv.slide_step_velocity_reduction).max(0.0);
            let k = r / sp;
            c.p.vel.x *= k;
            c.p.vel.y *= k;
        }
    }
}

/// 0x140ba3a90 TryPlayerMove: the plain bump-and-slide move from the current origin with frametime.
pub(super) fn try_player_move(c: &mut Ctx, tr: &mut Trace) {
    let (mn, mx) = c.p.hull();
    let mut pos = c.p.pos;
    let mut vel = c.p.vel;
    try_player_move_core(c.col, mn, mx, &mut pos, &mut vel, c.ft, Some(tr));
    c.p.pos = pos;
    c.p.vel = vel;
}

/// StepMove 0x140bb0610 (stepslide.md 3): plain move A, and a stepped move B (up stepH, across, down);
/// B is kept only when it got further along the horizontal velocity and landed sensibly. Returns stepOut.
fn step_move(c: &mut Ctx, pos: &mut Vec3, vel: &mut Vec3, dt: f32, step_h: f32, tr_in: &mut Trace) -> Vec3 {
    let up = UP;
    let (mn, mx) = c.p.hull();
    let col = c.col;
    // step 0
    if vel.length_squared() == 0.0 {
        return Vec3::ZERO;
    }
    let p0 = *pos;
    let v0 = *vel;
    // step 1: plain move A
    let mut a_pos = p0;
    let mut a_vel = v0;
    try_player_move_core(col, mn, mx, &mut a_pos, &mut a_vel, dt, Some(tr_in));
    *pos = a_pos;
    *vel = a_vel;
    // step 2: trace up
    let h = (step_h as f64 + 0.03125) as f32;
    let l = col.trace_hull(p0, p0 + up * h, mn, mx);
    let (u, up_dist) = if !l.startsolid && !l.allsolid { (l.endpos, l.fraction * h) } else { (p0, 0.0) };
    // step 3: horizontal velocity
    let d = up.dot(v0);
    let k = (d * 0.999).max(d * 1.001) + 0.0078125;
    let hv = v0 - up * k;
    let hl = hv.length();
    if hl == 0.0 {
        return Vec3::ZERO;
    }
    // step 4: move B from the raised position
    let mut b_pos = u;
    let mut b_vel = hv;
    try_player_move_core(col, mn, mx, &mut b_pos, &mut b_vel, dt, None);
    // step 5: did B get further than A?
    let flat = |x: Vec3| x - up * up.dot(x);
    let a = flat(a_pos - p0);
    let b = flat(b_pos - p0);
    let wsum = a.length_squared() * (hl * hl);
    let pb = hv.dot(b);
    if pb <= 0.0 {
        return Vec3::ZERO;
    }
    let t = (pb as f64 * 0.999) as f32;
    if wsum >= t * t {
        return Vec3::ZERO;
    }
    // step 6: step down
    let on_ground = c.p.ground;
    let mut dd = (up_dist as f64 + 0.03125) as f32;
    if on_ground {
        dd += step_h;
    }
    let de = b_pos - up * dd;
    let mut l = col.trace_hull(b_pos, de, mn, mx);
    if 1.0 > l.fraction {
        step_down_slide(col, mn, mx, b_pos, de - b_pos, &mut l);
    }
    // step 7: landing tests
    let n1 = l.normal;
    let e1 = l.endpos;
    let nu = up.dot(n1);
    let walkable = nu > 0.7;
    if (1.0 - nu) * ((mx.x - mn.x) * 0.5) > dd * l.fraction {
        return Vec3::ZERO;
    }
    let e = e1 - p0;
    let pe = flat(e).dot(hv);
    if pe <= 0.0 {
        return Vec3::ZERO;
    }
    let t = (pe as f64 * 0.999) as f32;
    if wsum >= t * t {
        return Vec3::ZERO;
    }
    if !walkable {
        // step 8: non-walkable landing, optional second step
        let mut f = e1 - a_pos;
        let q = flat(f).length_squared();
        if q <= 0.0 {
            return Vec3::ZERO;
        }
        let mut ok = false;
        let mut ee = e1;
        let mut nn = n1;
        let r = (((hl * dt) + (step_h * 1.0202)) as f64 + 0.03125) as f32;
        if r * r > q {
            let mut c_pos = u;
            let mut c_vel = hv;
            try_player_move_core(col, mn, mx, &mut c_pos, &mut c_vel, r / hl, None);
            let de2 = c_pos - up * dd;
            let mut l2 = col.trace_hull(c_pos, de2, mn, mx);
            if 1.0 > l2.fraction {
                step_down_slide(col, mn, mx, c_pos, de2 - c_pos, &mut l2);
            }
            ee = l2.endpos;
            nn = l2.normal;
            f = ee - a_pos;
            ok = up.dot(nn) > 0.7;
        }
        // step 9: wall-or-ledge tests
        let nu2 = nn.dot(up);
        if nu2 > 0.0 && on_ground {
            let g = f.dot(up);
            if g <= 0.0 {
                ok = true;
            } else {
                let fp = f - up * g;
                let s = fp.length_squared();
                if s / ((g * g) + s) > 0.49 {
                    ok = true;
                }
            }
        }
        let hp = p0.dot(up);
        let he = ee.dot(up);
        let h1 = e1.dot(up);
        let m = ((he - hp).abs() * 2.0).max(step_h * 0.25);
        if !((h1 - hp).abs() <= m) {
            return Vec3::ZERO;
        }
        if !ok {
            return Vec3::ZERO;
        }
        if 0.7 > nu2 {
            let cn = nn.dot(n1);
            if cn > 0.9 {
                let dz = (ee - e1).dot(n1);
                if 5.0 > dz.abs() {
                    return Vec3::ZERO;
                }
            }
        }
    }
    // step 10: ACCEPT (the original velocity: a step does not clip it)
    *pos = e1;
    *vel = v0;
    e1 - p0
}

/// Source ClipVelocity
fn clip_velocity(v: Vec3, n: Vec3, overbounce: f32) -> Vec3 {
    let backoff = v.dot(n) * overbounce;
    let mut out = v - n * backoff;
    let adjust = out.dot(n);
    if adjust < 0.0 {
        out -= n * adjust;
    }
    out
}

/// TryPlayerMoveCore 0x140baeba0 (not specified bit-exact yet): Source 2013's TryPlayerMove bump loop.
/// Writes the last trace into `tr_out` when given. Returns the blocked flags (1 floor, 2 wall).
pub(super) fn try_player_move_core(col: &Collision, mn: Vec3, mx: Vec3, pos: &mut Vec3, vel: &mut Vec3, dt: f32, tr_out: Option<&mut Trace>) -> i32 {
    let mut planes = [Vec3::ZERO; 5];
    let mut numplanes = 0usize;
    let primal = *vel;
    let mut original = *vel;
    let mut time_left = dt;
    let mut all_fraction = 0.0;
    let mut blocked = 0;
    let mut last = Trace { fraction: 1.0, endpos: *pos, normal: Vec3::ZERO, startsolid: false, allsolid: false };
    for _ in 0..4 {
        if vel.length_squared() == 0.0 {
            break;
        }
        let end = *pos + *vel * time_left;
        let t = col.trace_hull(*pos, end, mn, mx);
        last = t;
        all_fraction += t.fraction;
        if t.allsolid {
            *vel = Vec3::ZERO;
            blocked = 4;
            break;
        }
        if t.fraction > 0.0 {
            *pos = t.endpos;
            original = *vel;
            numplanes = 0;
        }
        if t.fraction == 1.0 {
            break;
        }
        if t.normal.z > 0.7 {
            blocked |= 1;
        }
        if t.normal.z == 0.0 {
            blocked |= 2;
        }
        time_left -= time_left * t.fraction;
        if numplanes >= planes.len() {
            *vel = Vec3::ZERO;
            break;
        }
        planes[numplanes] = t.normal;
        numplanes += 1;
        let mut i = 0;
        while i < numplanes {
            let nv = clip_velocity(original, planes[i], 1.0);
            let mut j = 0;
            while j < numplanes {
                if j != i && nv.dot(planes[j]) < 0.0 {
                    break;
                }
                j += 1;
            }
            if j == numplanes {
                *vel = nv;
                break;
            }
            i += 1;
        }
        if i == numplanes {
            if numplanes != 2 {
                *vel = Vec3::ZERO;
                break;
            }
            let dir = planes[0].cross(planes[1]).normalize_or_zero();
            *vel = dir * dir.dot(*vel);
        }
        if vel.dot(primal) <= 0.0 {
            *vel = Vec3::ZERO;
            break;
        }
    }
    if all_fraction == 0.0 {
        *vel = Vec3::ZERO;
    }
    if let Some(o) = tr_out {
        *o = last;
    }
    blocked
}

/// StepDownSlide 0x140baf900 (only described as a call in the spec): slide a displacement along what it hits,
/// up to 8 times; when it lands on something facing up (normal.up > 0.1) that trace becomes `l`.
fn step_down_slide(col: &Collision, mn: Vec3, mx: Vec3, start: Vec3, delta: Vec3, l: &mut Trace) {
    let mut pos = start;
    let mut dl = delta;
    let mut t = *l;
    for i in 0..8 {
        if i > 0 {
            t = col.trace_hull(pos, pos + dl, mn, mx);
        }
        if t.fraction < 1.0 && t.normal.dot(UP) > 0.1 {
            *l = t;
            return;
        }
        if t.fraction >= 1.0 {
            return;
        }
        pos = t.endpos;
        dl = clip_velocity(dl * (1.0 - t.fraction), t.normal, 1.0);
        if dl.length_squared() < 1e-6 {
            return;
        }
    }
}
