//! The S21 client's step/slide solver (r5apex_dx12.exe, decompiled for this port; the ds addresses are the 2019
//! server's from stepslide.md): StepSlide 0x140a47e30 (ds 0x140b9ad70), TryPlayerMoveStep 0x140a47a40
//! (ds 0x140b9a970), StepMove 0x140a5c640 (ds 0x140bb0610), TryPlayerMoveCore 0x140a5aba0 (ds 0x140baeba0) and
//! StepDownSlide 0x140a5b900 (ds 0x140baf900).

use super::{ground, upright, Ctx};
use crate::collision::{Collision, Trace};
use glam::Vec3;

/// StepSlide S21 0x140a47e30 = ds 0x140b9ad70 (stepslide.md 1, identical in S21): on a walkable hit, redirect the rest of the move along that surface
/// and re-trace; otherwise (or if that is blocked too) fall back to the step dispatcher. True = origin set.
pub(super) fn step_slide(c: &mut Ctx, tr: &mut Trace, keep_length: bool, dest: Option<Vec3>, force_fallback: bool) -> bool {
    let n = c.p.surface_normal;
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

/// TryPlayerMoveStep S21 0x140a47a40 = ds 0x140b9a970 (stepslide.md 2). The ground plane (pl+0x43dc) goes to
/// StepMove's plain move while on upright ground (S21 0x140b04b30 NotUpright false).
fn try_player_move_step(c: &mut Ctx, tr: &mut Trace) {
    let step_h = c.s.step_height;
    let mut pos = c.p.pos;
    let mut vel = c.p.vel;
    let q = if c.gnd() && upright(c.p.surface_normal) { Some(c.p.ground_normal) } else { None };
    let step_out = step_move(c, &mut pos, &mut vel, c.ft, step_h, tr, q);
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

/// StepMove S21 0x140a5c640 (ds 0x140bb0610, stepslide.md 3): plain move A, and a stepped move B (up stepH,
/// across, down); B is kept only when it got further along the horizontal velocity and landed sensibly. Returns
/// stepOut. S21 vs ds: the progress bound uses H.H (not hl*hl); the edge test and the non-walkable branch are
/// skipped when the step-down trace hit nothing, so in the air a raised move that clears the obstacle is accepted.
#[allow(clippy::too_many_arguments)]
fn step_move(c: &mut Ctx, pos: &mut Vec3, vel: &mut Vec3, dt: f32, step_h: f32, tr_in: &mut Trace, q: Option<Vec3>) -> Vec3 {
    let up = c.p.surface_normal;
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
    core(col, mn, mx, &mut a_pos, &mut a_vel, dt, Some(&*tr_in), q, None);
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
    let hh = ((hv.y * hv.y) + (hv.x * hv.x)) + (hv.z * hv.z);
    if hh == 0.0 {
        return Vec3::ZERO;
    }
    // step 4: move B from the raised position
    let mut b_pos = u;
    let mut b_vel = hv;
    core(col, mn, mx, &mut b_pos, &mut b_vel, dt, None, None, Some(up));
    // step 5: did B get further than A?
    let flat = |x: Vec3| x - up * up.dot(x);
    let a = flat(a_pos - p0);
    let b = flat(b_pos - p0);
    let wsum = a.length_squared() * hh;
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
        step_down_slide(col, mn, mx, up, b_pos, de - b_pos, &mut l);
    }
    // step 7: landing tests
    let n1 = l.normal;
    let e1 = l.endpos;
    let nu = up.dot(n1);
    let walkable = nu > 0.7;
    if l.fraction != 1.0 && (1.0 - nu) * ((mx.x - mn.x) * 0.5) > dd * l.fraction {
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
    if !walkable && (on_ground || l.fraction != 1.0) {
        // step 8: non-walkable landing, optional second step
        let mut f = e1 - a_pos;
        let q = flat(f).length_squared();
        if q <= 0.0 {
            return Vec3::ZERO;
        }
        let mut ok = false;
        let mut ee = e1;
        let mut nn = n1;
        let hl = hh.sqrt();
        let r = (((hl * dt) + (step_h * 1.0202)) as f64 + 0.03125) as f32;
        if r * r > q {
            let mut c_pos = u;
            let mut c_vel = hv;
            core(col, mn, mx, &mut c_pos, &mut c_vel, r / hl, None, None, Some(up));
            let de2 = c_pos - up * dd;
            let mut l2 = col.trace_hull(c_pos, de2, mn, mx);
            if 1.0 > l2.fraction {
                step_down_slide(col, mn, mx, up, c_pos, de2 - c_pos, &mut l2);
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
    // step 10: ACCEPT (the original velocity: a step does not clip it). Off upright (on a wall) stepOut is the
    // step along the landing normal, or nothing.
    *pos = e1;
    *vel = v0;
    let e = e1 - p0;
    if up.z != 1.0 {
        let s = ((n1.y * e.y) + (n1.x * e.x)) + (n1.z * e.z);
        return if s > 0.0 { n1 * s } else { Vec3::ZERO };
    }
    e
}

/// The S21 solver's push-out (DAT_1417f8080): a clipped velocity leaves the plane it hit at this speed, and each
/// bump's end point is moved this far off every listed plane, so no trace starts parallel to a plane it grazes.
const PUSH: f32 = 0.0078125;
/// DAT_14550f92c (BSS, set at runtime): the lower clamp of a length before dividing by it. Only a degenerate (zero)
/// vector ever reaches it.
const LEN_EPS: f32 = f32::MIN_POSITIVE;

/// The S21 bump loop's plane list: at most two planes.
struct Planes {
    p: [Vec3; 2],
    n: usize,
}

impl Planes {
    /// end += p * (1/128 - d.p) for every plane the planned displacement d does not leave by 1/128 (each against
    /// the same d, in list order)
    fn push_out(&self, end: &mut Vec3, d: Vec3) {
        for p in &self.p[..self.n] {
            let k = ((p.x * d.x) + (p.y * d.y)) + (p.z * d.z);
            if k < PUSH {
                let k = PUSH - k;
                *end += *p * k;
            }
        }
    }

    /// Adds the plane `n` after the clip `clipped`. Returns the new velocity, or None when a third plane matches
    /// neither of the two (the caller stops). `crease` builds the two-plane velocity.
    fn add(&mut self, n: Vec3, clipped: Vec3, crease: impl Fn(Vec3, Vec3) -> Vec3) -> Option<Vec3> {
        match self.n {
            0 => {
                self.p[0] = n;
                self.n = 1;
                return Some(clipped);
            }
            1 => {
                let p0 = self.p[0];
                if ((p0.x * clipped.x) + (p0.y * clipped.y)) + (p0.z * clipped.z) >= 0.0 {
                    // the clip already leaves the old plane: the new one replaces it
                    self.p[0] = n;
                    return Some(clipped);
                }
                self.p[1] = n;
                self.n = 2;
            }
            _ => {
                let (p0, p1) = (self.p[0], self.p[1]);
                let d0 = ((p0.y * n.y) + (p0.x * n.x)) + (p0.z * n.z);
                let d1 = ((p1.y * n.y) + (p1.x * n.x)) + (p1.z * n.z);
                let m = if d0 <= d1 { d1 } else { d0 };
                if m < 0.866 {
                    return None;
                }
                // replace the plane the new one is closer to
                if d0 < d1 {
                    self.p[1] = n;
                } else {
                    self.p[0] = n;
                }
            }
        }
        Some(crease(self.p[0], self.p[1]))
    }
}

/// p0 x p1 with the solver's operand order
fn cross_s21(p0: Vec3, p1: Vec3) -> Vec3 {
    Vec3::new((p1.z * p0.y) - (p1.y * p0.z), (p1.x * p0.z) - (p1.z * p0.x), (p1.y * p0.x) - (p1.x * p0.y))
}

/// v = o + n * (1/128 - o.n): the clip of the segment's starting velocity against a hit plane
fn clip_s21(o: Vec3, n: Vec3) -> Vec3 {
    let k = PUSH - (((n.y * o.y) + (n.x * o.x)) + (n.z * o.z));
    Vec3::new((n.x * k) + o.x, (n.y * k) + o.y, (n.z * k) + o.z)
}

/// TryPlayerMove S21 0x140a50cd0 (vtable+0x80 = 0x140a50c50): the plain bump move from the current origin with
/// the frametime, starting from the trace the caller already made. The trace is only read.
pub(super) fn try_player_move(c: &mut Ctx, tr: &mut Trace) {
    let (mn, mx) = c.p.hull();
    let mut pos = c.p.pos;
    let mut vel = c.p.vel;
    core(c.col, mn, mx, &mut pos, &mut vel, c.ft, Some(&*tr), None, None);
    c.p.pos = pos;
    c.p.vel = vel;
}

/// The plain TryPlayerMoveCore (no ground plane, no up vector) for callers outside this file.
pub(super) fn try_player_move_core(col: &Collision, mn: Vec3, mx: Vec3, pos: &mut Vec3, vel: &mut Vec3, dt: f32, tr_in: Option<&Trace>) {
    core(col, mn, mx, pos, vel, dt, tr_in, None, None);
}

/// TryPlayerMoveCore S21 0x140a5aba0 (ds 0x140baeba0), decompiled for this port. Up to 8 bumps. A 2-plane list
/// that is cleared after each 1+ unit of travel. Every clip starts from the velocity the segment began with and
/// leaves the plane at 1/128 u/s (`clip_s21`); each bump's end point is pushed 1/128 u off every listed plane; two
/// planes give the crease velocity plus a 1/128 push along their bisector; a third plane that matches neither
/// (cos < 0.866) stops the move. `tr_in` is the first bump's trace, already made by the caller (WalkMove /
/// AirMove / StepSlide), and is only read. With `q` (the ground plane, passed on the ground) a steep hit
/// (0 < n.z < 0.5) becomes a vertical wall; with `up` (StepMove's raised moves) a velocity going up along it is
/// clipped as if by a ceiling (-up) before tracing.
#[allow(clippy::too_many_arguments)]
fn core(col: &Collision, mn: Vec3, mx: Vec3, pos: &mut Vec3, vel: &mut Vec3, dt: f32, tr_in: Option<&Trace>, q: Option<Vec3>, up: Option<Vec3>) {
    let primal = *vel;
    let mut orig = *vel;
    let mut seg = *pos;
    let mut time = dt;
    let mut moved = false;
    let mut planes = Planes { p: [Vec3::ZERO; 2], n: 0 };
    let mut first = tr_in.copied();
    for _ in 0..8 {
        let (fraction, endpos, n);
        if let Some(t) = first.take() {
            (fraction, endpos, n) = (t.fraction, t.endpos, t.normal);
        } else if let Some(u) = up.filter(|u| ((u.y * vel.y) + (u.x * vel.x)) + (u.z * vel.z) > 0.0) {
            // moving up along `up`: a zero-length "hit" on the plane -up
            (fraction, endpos, n) = (0.0, *pos, -u);
        } else {
            let mut end = Vec3::new((time * vel.x) + pos.x, (time * vel.y) + pos.y, (time * vel.z) + pos.z);
            let d = end - *pos;
            if ((d.y * d.y) + (d.x * d.x)) + (d.z * d.z) < 0.0001 {
                break;
            }
            planes.push_out(&mut end, d);
            let t = col.trace_hull(*pos, end, mn, mx);
            let mut tn = t.normal;
            if let Some(q) = q {
                if 0.5 > tn.z && tn.z > 0.0 {
                    // on the ground a steep slope is a wall: its normal made horizontal relative to the most
                    // upward plane known (the ground plane or a listed one)
                    let mut pf = q;
                    for p in &planes.p[..planes.n] {
                        if p.z > pf.z {
                            pf = *p;
                        }
                    }
                    let x = (tn.x * pf.z) - (tn.z * pf.x);
                    let y = (tn.y * pf.z) - (tn.z * pf.y);
                    let k = 1.0 / (((x * x) + (y * y)) + 0.0).sqrt().max(LEN_EPS);
                    let w = Vec3::new(k * x, k * y, k * 0.0);
                    // ...unless a listed vertical plane already faces the same way: then the slope stays a slope
                    let dup = planes.p[..planes.n].iter().any(|p| p.z == 0.0 && ((w.y * p.y) + (w.x * p.x)) > 0.99);
                    if !dup {
                        tn = w;
                    }
                }
            }
            (fraction, endpos, n) = (t.fraction, t.endpos, tn);
        }
        if fraction > 0.0 {
            moved = true;
            *pos = endpos;
            if fraction == 1.0 {
                return;
            }
            let s = seg - *pos;
            if ((s.y * s.y) + (s.x * s.x)) + (s.z * s.z) > 1.0 {
                planes.n = 0;
                orig = *vel;
                seg = *pos;
            }
        }
        time -= fraction * time;
        // (the hit entity's vtable+0x6f0 can veto the clip; the world never does)
        let clipped = clip_s21(orig, n);
        let crease = |p0: Vec3, p1: Vec3| {
            let s = Vec3::new(p1.x + p0.x, p1.y + p0.y, p1.z + p0.z);
            let cr = cross_s21(p0, p1);
            let cc = (((cr.y * cr.y) + (cr.x * cr.x)) + (cr.z * cr.z)).max(LEN_EPS);
            let t = (((cr.y * orig.y) + (cr.x * orig.x)) + (cr.z * orig.z)) / cc;
            let ss = (((s.y * s.y) + (s.x * s.x)) + (s.z * s.z)).max(LEN_EPS);
            let k = (1.0 / ss).sqrt() * PUSH;
            Vec3::new((s.x * k) + (cr.x * t), (s.y * k) + (cr.y * t), (s.z * k) + (cr.z * t))
        };
        let Some(v) = planes.add(n, clipped, crease) else {
            *vel = Vec3::ZERO;
            return;
        };
        *vel = v;
        if ((primal.y * vel.y) + (primal.x * vel.x)) + (primal.z * vel.z) <= 0.0 {
            *vel = Vec3::ZERO;
            return;
        }
    }
    if !moved {
        *vel = Vec3::ZERO;
    }
}

/// StepDownSlide S21 0x140a5b900 (ds 0x140baf900), decompiled for this port: the same bump loop as `core`, run on
/// a displacement (time 1) and starting from the step-down trace `l`. The first trace whose normal faces `up`
/// (n.up > 0.1) becomes `l`; otherwise `l` stays as it was. Its two-plane case projects the displacement onto the
/// crease tilted off both planes by 1/128 (instead of `core`'s crease plus a push).
fn step_down_slide(col: &Collision, mn: Vec3, mx: Vec3, up: Vec3, start: Vec3, delta: Vec3, l: &mut Trace) {
    let primal = delta;
    let mut orig = delta;
    let mut pos = start;
    let mut seg = start;
    let mut dl = delta;
    let mut time = 1.0f32;
    let mut planes = Planes { p: [Vec3::ZERO; 2], n: 0 };
    for i in 0..8 {
        let t = if i == 0 {
            *l
        } else {
            let mut end = Vec3::new((time * dl.x) + pos.x, (time * dl.y) + pos.y, (time * dl.z) + pos.z);
            let d = end - pos;
            if ((d.y * d.y) + (d.x * d.x)) + (d.z * d.z) < 0.0001 {
                return;
            }
            planes.push_out(&mut end, d);
            col.trace_hull(pos, end, mn, mx)
        };
        if t.fraction > 0.0 {
            pos = t.endpos;
            if t.fraction == 1.0 {
                return;
            }
            let s = seg - pos;
            if ((s.y * s.y) + (s.x * s.x)) + (s.z * s.z) > 1.0 {
                planes.n = 0;
                orig = dl;
                seg = pos;
            }
        }
        let n = t.normal;
        if ((n.y * up.y) + (n.x * up.x)) + (n.z * up.z) > 0.1 {
            *l = t;
            return;
        }
        time -= t.fraction * time;
        let clipped = clip_s21(orig, n);
        let crease = |p0: Vec3, p1: Vec3| {
            let s = Vec3::new(p1.x + p0.x, p1.y + p0.y, p1.z + p0.z);
            let cr = cross_s21(p0, p1);
            let sign = 1.0f32.copysign(((orig.y * cr.y) + (orig.x * cr.x)) + (orig.z * cr.z));
            let r = (((cr.y * cr.y) + (cr.x * cr.x)) + (cr.z * cr.z)) / ((((s.y * s.y) + (s.x * s.x)) + (s.z * s.z)) + f32::MIN_POSITIVE);
            let k = (sign * r.sqrt()) * PUSH;
            let v = Vec3::new((k * s.x) + cr.x, (k * s.y) + cr.y, (k * s.z) + cr.z);
            let t = (((v.x * orig.x) + (orig.y * v.y)) + (orig.z * v.z)) / ((((v.x * v.x) + (v.y * v.y)) + (v.z * v.z)) + f32::MIN_POSITIVE);
            v * t
        };
        let Some(v) = planes.add(n, clipped, crease) else { return };
        dl = v;
        if ((primal.y * dl.y) + (primal.x * dl.x)) + (primal.z * dl.z) <= 0.0 {
            return;
        }
    }
}
