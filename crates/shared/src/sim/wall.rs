//! Wall climb: the S21 pilot's "wallrun" path (stock legends have `climbEnabled` and no `wallrun`, so every wall
//! they attach to is climbed). Decompiled from r5apex_ds.exe for this port (Ghidra headless + asm checks):
//!
//! - SurfaceTransition 0x140ba1990 and its classifier 0x140ba0a80 (attach to a wall the move was blocked by)
//! - WallProbe 0x140ba1690 (box trace toward the wall), MantleIntent 0x140b92e80, FacingWall 0x140bf0a40
//! - CanWallClimb 0x140bf0a90, WallTop 0x140b9f680, UpWallBoostScale 0x140ba2aa0, WallSpeedFrac 0x140b8b6b0
//! - WallRunUpdate 0x140b9f6e0 (when a climb ends), EndWallRun 0x140bf0ba0, WallSurfaceChange 0x140ba2680
//! - WallJump 0x140ba2c90 and RememberWall 0x140bacb10
//!
//! The on-wall motion itself is WalkMove's wall branch (walk.rs, walkmove.md 7W). While on a wall the wall is
//! the ground entity and `surface_normal` (pl+0x662c) is its normal.
//!
//! Checked against the S21 client r5apex_dx12.exe (ds -> dx12): SurfaceTransition 0x140a4f120, classifier
//! 0x140a4e220, WallProbe 0x140a4ee20, MantleIntent 0x140a40710, FacingWall 0x140b04bb0, CanWallClimb
//! 0x140b04c00, WallTop 0x140a4d470, UpWallBoostScale 0x140a4fed0, WallHangSlipScale 0x140a4ffa0, WallSpeedFrac
//! 0x140a34560, WallRunUpdate 0x140a4d4e0, EndWallRun 0x140b04df0, WallSurfaceChange 0x140a4fb00, WallJump
//! 0x140a50050, RememberWall 0x140a58f60, LedgeAssist 0x140a58ec0 (S21 player offsets: surface normal
//! pl+0x2ab0, attach origin 0x2ac8, start time 0x3624, ref height 0x2078). The same math for a stock pilot except
//! two S21 rules: crouch held never attaches (classify), and the wall probes run on command_number % 4
//! (wall_run_update; the ds build has that too, the first port assumed phase 0 every tick).

use super::{duck, ground, upright, Ctx, IN_DUCK};
use crate::collision::Trace;
use glam::Vec3;

/// pi/180 (0x1415bb344)
const DEG: f32 = 0.017453292;
/// pi/2 (0x1415bb980): the engine's inline cosines are its SSE sine polynomial at a + pi/2
const HALF_PI: f32 = 1.5707964;
const EPS: f32 = 1e-8;

/// cos(deg) the way the engine writes it: sin(deg*pi/180 + pi/2)
fn cos_deg(deg: f32) -> f32 {
    (deg * DEG + HALF_PI).sin()
}

/// FacingWall 0x140bf0a40(a, n): the flat direction `a` points into the wall within ~45.6 deg (cos^2 > 0.49).
fn facing_wall(a: Vec3, n: Vec3) -> bool {
    let d = (n.y * a.y) + (n.x * a.x);
    if 0.0 <= d {
        return false;
    }
    ((n.x * n.x) + (n.y * n.y)) * 0.49 < d * d
}

/// CanWallClimb 0x140bf0a90: climbEnabled; legends without wallrun always climb. (The wallrun-legend branch
/// gates on vertical speed and on the eye yaw facing the wall.)
pub(super) fn can_wall_climb(c: &Ctx) -> bool {
    if !c.s.climb_enabled {
        return false;
    }
    if !c.s.wallrun {
        return true;
    }
    let vz = c.p.vel.z; // pl+0x3e4 abs velocity z
    if vz < 10.0 {
        if vz < -200.0 {
            return false;
        }
        if 1.5 < c.now - c.p.wall.start_time {
            return false;
        }
    }
    let (s, co) = (c.yaw * DEG).sin_cos();
    let n = c.p.surface_normal;
    let d = (n.x * co) + (n.y * s);
    if 0.0 <= d {
        return false;
    }
    ((n.x * n.x) + (n.y * n.y)) * 0.49 < d * d
}

/// WallTop 0x140b9f680: the height the climb is measured from: the attach origin z, capped at
/// `verticalGainCutoff_wallrun + refHeight - climbFinalJumpUpHeight - climbHeight`.
pub(super) fn wall_top(c: &Ctx) -> f32 {
    let t = c.p.wall.attach_pos.z;
    let cut = c.s.vertical_gain_cutoff_wallrun;
    if 0.0 <= cut {
        let f = ((cut + c.p.wall.ref_height) - c.s.climb_final_jump_up_height) - c.s.climb_height;
        if f <= t {
            return f;
        }
    }
    t
}

/// UpWallBoostScale 0x140ba2aa0: 0 once spent or above the vertical-gain cutoff, else 1 (fading out between
/// wallrun_strengthLossStart and End walls, both 1000).
pub(super) fn up_wall_boost_scale(c: &Ctx) -> f32 {
    if c.p.wall.no_boost {
        return 0.0;
    }
    let cut = c.s.vertical_gain_cutoff_wallrun;
    if 0.0 <= cut && cut <= c.p.pos.z - c.p.wall.ref_height {
        return 0.0;
    }
    let k = c.p.wall.count as f32;
    let a = c.cv.wallrun_strength_loss_start;
    if k <= a {
        return 1.0;
    }
    let b = c.cv.wallrun_strength_loss_end;
    if b <= k {
        return 0.0;
    }
    (k - a) / (b - a)
}

/// WallHangSlipScale 0x140ba2ba0: 1 unless hanging. Hanging needs wallrunAdsType 1 (stock is 2), so 1 here.
fn hang_slip_scale(_c: &Ctx) -> f32 {
    1.0
}

/// WallSpeedFrac / WallGravityRamp 0x140b8b6b0(t = time on the wall): the wall gravity factor (AddGravity) and
/// the no-climb speed fraction `q` (WalkMove). 0 while climbing and pushing into the wall (input . n < -0.1);
/// otherwise ramps 1 - boost*(1 - t/wallrunGravityRampUpTime) up to 1.
pub(super) fn wall_speed_frac(c: &Ctx, t: f32) -> f32 {
    let mut tt = c.s.wallrun_gravity_ramp_up_time;
    if !(0.0 <= tt) {
        return 0.0;
    }
    let n = c.p.surface_normal;
    let w = c.wish;
    if can_wall_climb(c) && ((n.y * w.y) + (n.x * w.x)) + (n.z * w.z) < -0.1 {
        return 0.0;
    }
    if c.s.wallstick_enabled {
        tt = 100.0;
    }
    let t = if t <= 0.0 { 0.0 } else { t };
    if t < tt {
        let f = 1.0 - up_wall_boost_scale(c) * (1.0 - t / tt);
        hang_slip_scale(c) * f
    } else {
        hang_slip_scale(c)
    }
}

/// WallProbe 0x140ba1690: sweeps a cube of half-size `hw` from `p` toward the wall (flat -n) by
/// `h + dist + hw*0.2929`. 3 = wall within `dist` (with a correction for walls leaning back), else `miss`.
/// `out` gets the hit normal. Only the world is traced here, so the entity filters drop out.
#[allow(clippy::too_many_arguments)]
fn wall_probe(c: &Ctx, p: Vec3, h: f32, n: Vec3, hw: f32, dist: f32, out: Option<&mut Vec3>, miss: i32) -> i32 {
    let l = (h + dist) + hw * 0.2929;
    let e = Vec3::new(p.x - l * n.x, p.y - l * n.y, p.z);
    let t = c.col.trace_hull(p, e, Vec3::splat(-hw), Vec3::splat(hw));
    if t.fraction == 1.0 {
        return miss;
    }
    let mut d = t.fraction * l;
    if dist < d && 0.0 < t.normal.z && !t.startsolid {
        let f = (((t.normal.y * n.y) + (t.normal.x * n.x)) + 0.0).max(0.0).min(1.0);
        d = ((1.0 / f - 1.0) * hw + d) - (t.normal.z * (1.0 / f)) * h;
    }
    if let Some(o) = out {
        *o = t.normal;
    }
    if dist < d { miss } else { 3 }
}

/// MantleIntent 0x140b92e80: pushing into the wall within automantle_maxangle_push and looking at it within
/// automantle_maxangle_view (times mantleAngleScale). Classify uses it to leave walls with a ledge in reach to
/// the mantle.
fn mantle_intent(c: &Ctx, dir: Vec3, nrm: Vec3) -> bool {
    let w = c.wish;
    let f = -(((w.x * nrm.x) + (nrm.y * w.y)) + (nrm.z * w.z));
    if 0.0 > f {
        return false;
    }
    let cp = cos_deg(c.cv.automantle_maxangle_push);
    if f * f < ((((w.x * w.x) + (w.y * w.y)) + (w.z * w.z)) * cp) * cp {
        return false;
    }
    let s = c.s.mantle_angle_scale;
    let fv = -(((nrm.y * dir.y) + (nrm.x * dir.x)) + (nrm.z * dir.z));
    if upright(c.p.surface_normal) {
        if fv * s < cos_deg(c.cv.automantle_maxangle_view) {
            return false;
        }
    } else {
        if fv * s < cos_deg(c.cv.automantle_wallrun_maxangle_view) {
            return false;
        }
        let v = c.p.vel;
        if ((nrm.y * v.y) + (nrm.x * v.x)) + (nrm.z * v.z) < -100.0 {
            return false;
        }
    }
    true
}

/// The surface-transition classifier 0x140ba0a80. 3 = attach to the wall `nrm` (hit at `pos`), 1/2 = retry in
/// wallrun_retry_interval (the head / feet probe missed), 0 = no.
fn classify(c: &Ctx, pos: Vec3, nrm: Vec3, tr: Option<&Trace>, vel: Vec3) -> i32 {
    if !c.cv.wallrun_enable {
        return 0;
    }
    let up = upright(c.p.surface_normal);
    // on the ground, or near it within 0.5 s (a jump zeroes that time)
    if (c.gnd() || c.now - c.p.near_ground_time < 0.5) && up {
        return 0;
    }
    // pl+0x69b0 slip triggers: none
    let s = &c.s;
    if !s.wallrun {
        if !s.climb_enabled {
            return 0;
        }
        if !facing_wall(c.flat_f, nrm) {
            return 0;
        }
        // rising faster than half of climbSpeedStart
        if s.climb_speed_start * 0.5 < c.p.vel.z {
            return 0;
        }
        // above the vertical-gain cutoff
        if ((s.vertical_gain_cutoff_wallrun + c.p.wall.ref_height) - s.climb_final_jump_up_height) < pos.z {
            return 0;
        }
        // pl+0x6ecc: never set here
    } else if s.climb_enabled && facing_wall(c.flat_f, nrm) && s.climb_speed_start * 0.5 < c.p.vel.z {
        return 0;
    }
    // zipline (0x140bf5480) / status effect 0x16951af34: none
    if 0.7 < nrm.z {
        return 0; // a floor
    }
    let cl = s.wallrun_ceiling_limit.max(0.0).min(1.0);
    if nrm.z < -cl {
        return 0; // a ceiling
    }
    // grapple: none. The hit: start-solid refuses; contents 0x100000 / players / non-solid entities can't
    // happen (world only).
    if let Some(t) = tr {
        if t.startsolid {
            return 0;
        }
    }
    // S21 0x140a4e650 (new since ds): crouch held (mv+0x24 & 0x4000004, duck / toggle duck) never attaches
    if c.buttons & IN_DUCK != 0 {
        return 0;
    }
    // crouched and can't stand up here (pl+0x1dfc, the S21 slide flag that replaced ds slide state 3, never set)
    if c.p.duck_state != 0 && !duck::can_unduck(c) {
        return 0;
    }
    // the last wall left (one record): not the same wall again at or above where you attached to it
    if !s.wallstick_enabled && c.p.wall.last_count > 0 {
        let hl = (((nrm.x * nrm.x) + (nrm.y * nrm.y)) + 0.0).sqrt().max(EPS);
        let k = 1.0 / hl;
        let ln = c.p.wall.last_normal;
        let lim = c.p.wall.attach_pos.z + s.wallrun_same_wall_height;
        if c.cv.wallrun_same_wall_dot < (((k * nrm.y) * ln.y) + ((k * nrm.x) * ln.x)) + ((k * 0.0) * ln.z) {
            if !s.wallrun_same_wall_allowed {
                return 0;
            }
            if 0.0 <= s.wallrun_same_wall_height {
                let o = c.p.pos;
                if lim <= o.z {
                    let r = c.cv.wallrun_same_wall_dist;
                    let dx = o.x - c.p.wall.attach_pos.x;
                    let dy = o.y - c.p.wall.attach_pos.y;
                    let d2 = (dy * dy) + (dx * dx);
                    if d2 <= r * r {
                        return 0;
                    }
                    if (d2.sqrt() - r) * c.cv.wallrun_same_wall_slope <= o.z - lim {
                        return 0;
                    }
                }
            }
        }
    }
    let o = c.p.pos;
    if up {
        // moving into the wall (wallrun_minAngle_air 180: any velocity with a component into it)
        let vl = (((vel.x * vel.x) + (vel.y * vel.y)) + (vel.z * vel.z)).sqrt().max(EPS);
        let k = 1.0 / vl;
        let th = ((c.cv.wallrun_min_angle_air * 0.5) * DEG + HALF_PI).sin() * -1.0;
        if th <= (((vel.y * k) * nrm.y) + ((vel.x * k) * nrm.x)) + ((vel.z * k) * nrm.z) {
            return 0;
        }
    }
    // the probes start at the hit point unless the origin is behind the hit plane
    let f23 = (((o.y - pos.y) * nrm.y) + ((o.x - pos.x) * nrm.x)) + ((o.z - pos.z) * nrm.z);
    let mut b = if -0.01 <= f23 { pos } else { o };
    // crouched in the air: where the standing hull would be (CanUnduck's offset)
    if c.p.duck_state != 0 && c.p.ducked && !c.gnd() {
        if let Some(d) = duck::air_hull_delta(c) {
            b += d;
        }
    }
    let (mn, mx) = c.p.hull();
    let hh = mx.z - mn.z; // mv+0xc8
    let hw = c.half_width(); // mv+0xd0
    let dist = s.wallrun_allowed_wall_distance;
    // feet probe (box z..z+32) and middle probe (box centred hull/2 + hw up)
    let pts = [Vec3::new(b.x, b.y, b.z + 16.0), Vec3::new(b.x, b.y, ((hh * 0.5) + hw) + b.z)];
    let mut all_floor = true;
    for (i, p) in pts.iter().enumerate() {
        let mut out = Vec3::ZERO;
        let r = wall_probe(c, *p, p.z - b.z, nrm, hw, dist, Some(&mut out), if i == 0 { 2 } else { 1 });
        if r != 3 {
            return r;
        }
        if out.z < 0.7 {
            all_floor = false;
        }
    }
    if f23 < -0.01 && all_floor {
        return 0;
    }
    if up {
        // pushing and looking into it like a mantle: the head probe must find the wall too, or the ledge is the
        // mantle's
        let (sn, co) = (c.yaw * DEG).sin_cos();
        if mantle_intent(c, Vec3::new(co, sn, 0.0), nrm) {
            let p = Vec3::new(b.x, b.y, (hh - hw) + b.z);
            let r = wall_probe(c, p, hh, nrm, hw, dist, None, 0);
            if r != 3 {
                return r;
            }
        }
    }
    3
}

/// SurfaceTransition 0x140ba1990(pos, normal, trace, velocity): called by AirMove / WalkMove when the move was
/// blocked (and by AirMove's deferred retry). Attaches to a wall (sets the surface normal), changes walls while
/// on one, or ends the wall state when the move hit a floor.
pub(super) fn surface_transition(c: &mut Ctx, pos: Vec3, nrm: Vec3, tr: Option<&Trace>, vel: Vec3) {
    c.p.wall.retry_time = 0.0;
    let r = classify(c, pos, nrm, tr, vel);
    if r == 3 {
        if upright(c.p.surface_normal) {
            // attach
            c.p.surface_normal = nrm;
            // 0x140bf49e0: one-handed weapon / sprint view flags: not ported
            c.p.wall.count += 1;
            if c.s.wallrun {
                // up-wall boost: wallrunUpWallBoost along the wall's up direction (wallrun legends only)
                let boost = c.s.wallrun_up_wall_boost * up_wall_boost_scale(c);
                let n = c.p.surface_normal;
                let l = (n.x * n.x + n.y * n.y).sqrt();
                let (ux, uy, uz) = if l == 0.0 { (0.0, 0.0, 1.0) } else { let k = 1.0 / l; (-(n.z * n.x * k), -(n.z * n.y * k), l) };
                let v = c.p.vel;
                let d = ((uy * v.y) + (ux * v.x)) + (uz * v.z);
                if d < boost {
                    let d = if d <= 0.0 { 0.0 } else { d };
                    let a = boost - d;
                    c.p.vel = Vec3::new(v.x + ux * a, v.y + uy * a, v.z + uz * a);
                }
            }
            // sounds; the same wall as the last one gives no boost
            let ln = c.p.wall.last_normal;
            if c.p.wall.last_count > 0 && c.cv.wallrun_same_wall_dot < ((nrm.y * ln.y) + (nrm.x * ln.x)) + (nrm.z * ln.z) {
                c.p.wall.no_boost = true;
            }
            // re-attach view punch after 0.25 s: not ported
            c.p.wall.attach_pos = c.p.pos;
            c.p.wall.start_time = c.now;
            c.p.wall.push_away_time = 0.0;
            // pl+0x6699 hang, 0x6944/0x6945 dodge flags, toggle crouch: cleared (never set here)
            duck::stop_sliding(c);
            // WallClimbUpdate 0x140bf0e90 (traversal_anim 1): the wall_climb animation only
        } else {
            wall_surface_change(c, nrm);
        }
        return;
    }
    let nz = nrm.z;
    if r != 0 && !c.gnd() && nz <= 0.7 {
        c.p.wall.retry_time = c.cv.wallrun_retry_interval + c.now;
        c.p.wall.retry_pos = c.p.pos;
        c.p.wall.retry_normal = nrm;
    }
    if nz <= 0.7 {
        // on a wall, slow, and the feet are over a floor within a step: the climb is over
        if !upright(c.p.surface_normal) && c.p.vel.z <= 140.0 && (r & !2) == 0 {
            let s = c.s.step_height; // pl+0x5b00
            let o = c.p.pos;
            let e = Vec3::new(o.x + s * 0.0, o.y + s * 0.0, o.z + s * -1.0);
            let t = c.trace(o, e);
            if t.fraction < 1.0 && 0.7 < t.normal.z {
                end_wall_run(c);
            }
        }
    } else {
        let o = c.p.pos;
        if 0.0 <= (((o.y - pos.y) * nrm.y) + ((o.x - pos.x) * nrm.x)) + ((o.z - pos.z) * nrm.z) {
            end_wall_run(c);
        }
    }
}

/// WallSurfaceChange 0x140ba2680: turn the surface normal toward a new wall face, at most
/// wallrun_rotateMaxRate radians per second.
pub(super) fn wall_surface_change(c: &mut Ctx, nn: Vec3) {
    let a = c.cv.wallrun_rotate_max_rate * c.ft;
    let co = (a + HALF_PI).sin();
    let n = c.p.surface_normal;
    if co < ((n.x * nn.x) + (n.y * nn.y)) + (n.z * nn.z) {
        c.p.surface_normal = nn;
        return;
    }
    // t = (n x nn) x n: the part of nn perpendicular to n
    let cx = Vec3::new(n.y * nn.z - n.z * nn.y, n.z * nn.x - n.x * nn.z, n.x * nn.y - n.y * nn.x);
    let mut t = Vec3::new(n.z * cx.y - n.y * cx.z, n.x * cx.z - n.z * cx.x, n.y * cx.x - n.x * cx.y);
    if ((t.z * t.z) + (t.x * t.x)) + (t.y * t.y) == 0.0 {
        if ((nn.x * nn.x) + (nn.y * nn.y)) + (nn.z * nn.z) == 0.0 {
            return;
        }
        t = Vec3::new(n.y, -n.x, 0.0);
    }
    let si = (a + 0.0).sin() / (((t.x * t.x) + (t.y * t.y)) + (t.z * t.z)).sqrt();
    let r = Vec3::new(n.x * co + t.x * si, n.y * co + t.y * si, n.z * co + t.z * si);
    let l = (((r.y * r.y) + (r.x * r.x)) + (r.z * r.z)).sqrt().max(EPS);
    let k = 1.0 / l;
    c.p.surface_normal = r * k;
}

/// EndWallRun 0x140bf0ba0: back to world up (written as (-0, -0, 1)), stamps the end time.
pub(super) fn end_wall_run(c: &mut Ctx) {
    // script callbacks
    if !upright(c.p.surface_normal) && c.p.wall.end_time != c.now {
        c.p.wall.end_time = c.now;
    }
    c.p.surface_normal = Vec3::new(-0.0, -0.0, 1.0);
    // pl+0x6699 hang off; 0x665d / 0x638d one-handed and sprint view flags; 0x140bf1500 climb animation off
    c.p.wall.push_away_time = 0.0;
}

/// RememberWall 0x140bacb10: the last wall's flat normal (the same-wall rule in the classifier).
fn remember_wall(c: &mut Ctx, n: Vec3) {
    if !c.cv.wallrun_repel_enable {
        return;
    }
    let l = (((n.x * n.x) + (n.y * n.y)) + 0.0).sqrt().max(EPS);
    let k = 1.0 / l;
    c.p.wall.last_normal = Vec3::new(n.x * k, n.y * k, k * 0.0);
    c.p.wall.last_time = c.now;
    c.p.wall.last_count = 1;
}

/// WallRunUpdate 0x140b9f6e0 (FullWalkMove step 3), the on-wall half: decides when a climb ends. Ends on
/// crouch, looking away from the wall, slowing under 10 u/s up after 1 s, reaching climbHeight above WallTop
/// (with the final jump up), pulling away for 0.05 s, or the wall no longer within reach.
pub(super) fn wall_run_update(c: &mut Ctx) {
    let n = c.p.surface_normal;
    if upright(n) {
        // the airborne wallrun start (predicted trace + classifier) needs S[wallrun]; stock legends return here
        return;
    }
    let mut fall = false;
    if c.s.wallrun_duck_causes_fall_off {
        // buttons & 0x4000004 (duck, toggle duck); pl+0x5aac slide states 2/3 never happen on a wall here
        fall = c.buttons & IN_DUCK != 0;
        if fall {
            c.p.jumped = true; // pl+0x6230
        }
    }
    // status effect 0x16951af34: none
    let mut climb = can_wall_climb(c);
    let mut top = false;
    if climb {
        if !c.s.wallrun {
            if !facing_wall(c.flat_f, n) {
                fall = true;
            }
            if c.p.vel.z < 10.0 && 1.0 < c.now - c.p.wall.start_time {
                fall = true;
            }
        }
        if wall_top(c) + c.s.climb_height <= c.p.pos.z {
            top = true;
            if !c.s.wallrun || 0.0 <= c.s.vertical_gain_cutoff_wallrun {
                fall = true;
            } else {
                climb = false;
            }
        }
    }
    // hang (wallrunAdsType 1 + ADS): not reachable, stock is 2
    let w = c.wish;
    if ((n.y * w.y) + (n.x * w.x)) + (n.z * w.z) <= 0.7 {
        c.p.wall.push_away_time = 0.0;
    } else {
        if c.p.wall.push_away_time == 0.0 {
            c.p.wall.push_away_time = c.now;
        }
        let e = c.now - c.p.wall.push_away_time;
        if c.cv.wallrun_push_away_fall_off_time < e {
            fall = true;
        }
    }
    let mut timeout = false;
    let tl = c.s.wallrun_time_limit;
    if !(climb || tl < 0.0 || c.now - c.p.wall.start_time <= tl) {
        fall = true;
        timeout = true;
    }
    // The wall probes run on a 4-command cycle: `**(int**)(pl+0x34b8) % 4` is the current usercmd's
    // command_number (S21 0x140a4dc3a; ds 0x140ba04xx has the same code at pl+0x6578).
    let o = c.p.pos;
    let dist = c.s.wallrun_allowed_wall_distance;
    match (c.p.cmd_number as i32) % 4 {
        // the wall must still be within wallrunAllowedWallDistance of a probe 16 up, or you fall off
        0 => {
            let r = wall_probe(c, Vec3::new(o.x, o.y, o.z + 16.0), 16.0, n, c.half_width(), dist, None, 0);
            if r == 3 && !fall {
                return;
            }
        }
        // rising: a probe 32 up that misses the wall (its top is passing the chest) slows the climb by
        // 3 * ft * wallrun_avoid_wall_top_decel (3000), floored at 0
        2 => {
            if 0.0 < c.p.vel.z {
                let r = wall_probe(c, Vec3::new(o.x, o.y, o.z + 32.0), 16.0, n, c.half_width(), dist, None, 0);
                if r != 3 {
                    let vz = c.p.vel.z - (c.ft * 3.0) * c.cv.wallrun_avoid_wall_top_decel;
                    c.p.vel.z = if vz <= 0.0 { 0.0 } else { vz };
                }
            }
            if !fall {
                return;
            }
        }
        _ => {
            if !fall {
                return;
            }
        }
    }
    // fall off. LedgeAssist 0x140baca70 (jump assist) is not ported, so there is never an assist target and the
    // player is always pushed off the wall by wallrun_fallAwaySpeed.
    c.p.vel += n * c.cv.wallrun_fall_away_speed;
    remember_wall(c, n);
    end_wall_run(c);
    c.p.wall.no_boost = true;
    ground::set_ground(c, None);
    // timeout: view punch only (not ported)
    let _ = timeout;
    if top {
        // the final jump up past the climb height: up to climbFinalJumpUpHeight (+ the overshoot, capped at 10)
        let mut h = (wall_top(c) + c.s.climb_height) - c.p.pos.z;
        if 10.0 <= h {
            h = 10.0;
        }
        h += c.s.climb_final_jump_up_height;
        if 0.0 < h {
            let g = c.cv.sv_gravity;
            c.p.vel.x += 0.0;
            c.p.vel.z = ((g + g) * h).sqrt() + c.p.vel.z;
            c.p.vel.y += 0.0;
        }
    }
    if climb {
        c.p.wall.ref_height -= c.cv.wallclimb_vertical_gain_reduction;
    }
}

/// WallJump 0x140ba2c90 (CheckJumpButton on a wall): out from the wall by wallrunJumpOutwardSpeed, scaled by
/// (1 - 0.8 * (n . view) * (n . input)) when both face the wall, plus the climb's final jump up
/// (sqrt(2 g climbFinalJumpUpHeight)) scaled by the up-wall boost (0 above the vertical-gain cutoff).
pub(super) fn wall_jump(c: &mut Ctx) {
    let u = c.wish; // mv+0x78
    // hanging: not reachable
    let climb = can_wall_climb(c);
    let mut o = c.s.wallrun_jump_outward_speed;
    let (inp, up) = if !climb {
        (c.s.wallrun_jump_input_dir_speed, c.s.wallrun_jump_up_speed)
    } else {
        let g = c.cv.sv_gravity;
        let up = ((g + g) * c.s.climb_final_jump_up_height).sqrt();
        c.p.wall.ref_height -= c.cv.wallclimb_vertical_gain_reduction;
        (0.0, up)
    };
    let mut a = Vec3::new(u.x * inp, u.y * inp, u.z * inp);
    let n = c.p.surface_normal;
    let f = c.flat_f; // mv+0x60
    let mut d1 = ((n.y * f.y) + (n.x * f.x)) + (n.z * f.z);
    let mut d2 = ((u.y * n.y) + (n.x * u.x)) + (u.z * n.z);
    if 0.0 <= d1 {
        d1 = 0.0;
    }
    if 0.0 <= d2 {
        d2 = 0.0;
    }
    o *= 1.0 - (d1 * d2) * 0.8;
    let dd = ((a.x * n.x) + (n.y * a.y)) + (a.z * n.z);
    if dd < o {
        let k = o - dd;
        a = Vec3::new(n.x * k + a.x, n.y * k + a.y, n.z * k + a.z);
    }
    remember_wall(c, n);
    c.p.vel += a;
    c.p.wall.no_boost = false;
    let up = up * up_wall_boost_scale(c);
    let vz = c.p.vel.z;
    let mut nz = vz;
    if vz < up {
        nz = vz + up * 1.5;
        if up < nz {
            nz = up;
        }
    }
    c.p.vel.z = nz;
    // LedgeAssist 0x140baca70: not ported
}
