//! Mantling, ported from the S21 client r5apex_dx12.exe:
//! CheckAutoMantle 0x140a43060 (ds 0x140b95a40) with its ledge probe 0x140a40c90 and angle test 0x140a40710,
//! StartTraversal 0x140a41af0 (ds 0x140b940a0), TraversalMove 0x140a45380 (ds 0x140b981b0, movemodes.md part B),
//! FinishTraversal 0x140a46e40 (ds 0x140b99b60), and the mantle boost (R5Flowstate SDK hook,
//! engine/s21-r5sdk/src/game/client/mantle_boost.cpp; the S21 binary has no mantle-boost code).
//!
//! S21 runs traversal_anim 1: the first-person animation's root motion drives the origin. We have no animation
//! data, so the origin follows the two-segment path (the engine's traversal_anim 0 code). Everything else the
//! animation path does that needs no animation data is kept: the state choice, and the smoothed cycle
//! pl+0x2234 that TraversalMove's END test and the boost's frac gate read.

use super::{duck, ground, upright, walk, wall, Ctx, MoveType, Traversal, IN_DUCK, IN_JUMP, UP};
use crate::collision::Trace;
use glam::Vec3;

/// Remaining mantle time in which a press counts as the mantle-boost sweet spot. The SDK measures
/// |traversal camera pitch - eye pitch| < 1.5 deg (bridge_mantle_boost_sweet_spot_angle) on the animation and
/// notes "1.5 opens about the last 0.10-0.13 s of a mantle"; without the animation we use the middle of that.
pub const MANTLE_BOOST_SWEET_SPOT_TIME: f32 = 0.115;

/// TraversalPathPos 0x140b054a0 (ds 0x140bf15d0, B4): up the wall to the corner, then forward onto the ledge.
fn path_pos(t: &Traversal, p: f32) -> Vec3 {
    let (a, b, f) = if t.c > p { (t.start, t.corner, p / t.c) } else { (t.corner, t.end, (p - t.c) / (1.0 - t.c)) };
    (b - a) * f + a
}

/// TraceHull with the crouched hull: the ledge probe forces pl+0x2a70 = 2 and FL_DUCKING around its traces.
fn trace_crouch(c: &Ctx, a: Vec3, b: Vec3) -> Trace {
    c.col.trace_hull(a, b, c.s.crouch.hull_min, c.s.crouch.hull_max)
}

/// sin(deg * 0.017453292 + 1.5707964), the way the exe takes a cosine
fn cos_deg(deg: f32) -> f32 {
    (deg * 0.017453292 + 1.5707964).sin()
}

/// 0x140a40710: the wall must face the push (input within automantle_maxangle_push of the wall's inward normal; no
/// input passes) and the view (yaw within automantle_maxangle_view, scaled by mantleAngleScale). On a wall
/// (surface normal z < 0.999) the view limit is automantle_wallrun_maxangle_view and the velocity into the wall
/// must not exceed 100.
fn angle_ok(c: &Ctx, yaw: Vec3, n: Vec3) -> bool {
    let i = c.wish; // mv+0x78
    let push = -(i.x * n.x + n.y * i.y + n.z * i.z);
    if !(0.0 <= push) {
        return false;
    }
    let cp = cos_deg(c.cv.automantle_maxangle_push);
    if !((i.x * i.x + i.y * i.y + i.z * i.z) * cp * cp <= push * push) {
        return false;
    }
    let scale = c.s.mantle_angle_scale;
    let fv = -(n.y * yaw.y + n.x * yaw.x + n.z * yaw.z);
    if upright(c.p.surface_normal) {
        cos_deg(c.cv.automantle_maxangle_view) <= fv * scale
    } else {
        let v = c.p.vel;
        cos_deg(c.cv.automantle_wallrun_maxangle_view) <= fv * scale && -100.0 <= n.y * v.y + n.x * v.x + n.z * v.z
    }
}

/// What the ledge probe hands back when it finds a ledge (its return value 2).
struct Ledge {
    /// horizontal wall normal (points out of the wall, at the player)
    n: Vec3,
    /// the down trace onto the ledge (param_7)
    tr: Trace,
    /// its start height (the probe top)
    top: f32,
    /// how far over the lip the probe reached (param_10)
    dist: f32,
    /// solid wall `automantle_dangle_required_space - 5` below the lip (param_11)
    can_hang: bool,
}

/// The ledge probe 0x140a40c90 (`param_9` = 1). `wall` is the yaw trace that hit the wall. None = its return 0/1.
fn find_ledge(c: &Ctx, yaw: Vec3, o: Vec3, wall: &Trace) -> Option<Ledge> {
    // contents & 0x100000 and the entity test 0x140a40b70: the world always passes
    let n = Vec3::new(wall.normal.x, wall.normal.y, 0.0).normalize_or_zero();
    let d0 = (o.y - wall.endpos.y) * n.y + (o.x - wall.endpos.x) * n.x + (o.z - wall.endpos.z) * n.z;
    if !angle_ok(c, yaw, n) {
        return None;
    }
    let cv = &c.cv;
    let fwd_reach = cv.automantle_forwarddist + d0; // local_5f4
    let mut reach = cv.automantle_forwarddist;
    if reach <= cv.automantle_mindist {
        reach = cv.automantle_mindist;
    }
    let mh = 0.0 + c.s.mantle_height; // param_5 (0) + mantleHeight
    reach += d0;
    let s = mh / 80.0;
    let dv = mh as f64 + 0.03125;
    let mut frac_cap = fwd_reach / reach;
    // the probe height cycles with the usercmd number: one height per tick
    let idx = c.p.cmd_number % 7;
    let h = match idx {
        0 | 4 => dv as f32,
        1 => s * 66.5,
        2 => s * 26.5,
        3 => s * 16.5,
        5 => s * 34.5,
        // 6: FUN_140668c20 searches "GetWindowHintForPath" entities along the input direction; maps here have none
        _ => return None,
    };
    let top = h + o.z;
    // 1) crouched hull at the probe height, straight into the wall
    let a = Vec3::new(o.x, o.y, top);
    let b = Vec3::new(n.x * -reach + o.x, n.y * -reach + o.y, n.z * -reach + top);
    let t1 = trace_crouch(c, a, b);
    let moved = t1.fraction * reach;
    if !(cv.automantle_mindist <= moved - d0) {
        return None;
    }
    if t1.fraction <= frac_cap {
        frac_cap = t1.fraction;
    }
    // 2) down onto the ledge from where it stopped
    let p = Vec3::new((b.x - a.x) * frac_cap + a.x, (b.y - a.y) * frac_cap + a.y, (b.z - top) * frac_cap + top);
    let mut t2 = trace_crouch(c, p, Vec3::new(p.x, p.y, (p.z as f64 - dv) as f32));
    if t2.normal.z < 0.7 {
        // not walkable: again, mindist into the wall from the origin
        let md = cv.automantle_mindist;
        let q = Vec3::new(o.x - md * n.x, o.y - md * n.y, top - md * n.z);
        t2 = trace_crouch(c, q, Vec3::new(q.x, q.y, (q.z as f64 - dv) as f32));
        if t2.normal.z < 0.7 {
            return None;
        }
    }
    let rise = t2.endpos.z - o.z;
    if !(0.03125 <= rise) || t2.startsolid {
        return None;
    }
    // 3) head room: crouched, from the origin straight up to the probe height
    let t3 = trace_crouch(c, o, Vec3::new(o.x, o.y, top));
    if !(1.0 <= t3.fraction) {
        return None;
    }
    let v = c.p.vel;
    let into = v.y * n.y + v.x * n.x + v.z * n.z < -10.0;
    // a step you can walk up (pl+0x1de8, the step size) is not a mantle while walking into it on flat ground,
    // unless the standing hull does not fit on it
    if (rise as f64) < c.s.step_height as f64 + 0.03125 && upright(c.p.surface_normal) && into {
        if c.p.duck_state == 2 {
            return None;
        }
        let t = c.trace(t2.endpos, t2.endpos);
        if !t.startsolid {
            return None;
        }
    }
    // a low obstacle with no wall at half its height within 0.35 of the reach is a slope or stairs
    if rise < moved + moved && into {
        let a = Vec3::new(o.x, o.y, o.z + rise * 0.5);
        let k = moved * 0.35;
        let b = Vec3::new(a.x - moved * n.x * 0.35, a.y - moved * n.y * 0.35, a.z - k * n.z);
        let t = trace_crouch(c, a, b);
        if t.fraction == 1.0 && ((0.1 < wall.normal.z.abs()) == (0.1 < t.normal.z.abs())) {
            return None;
        }
    }
    // can hang: a ray at the lip minus (dangle space - 5), 2 half-widths into the wall, must hit. The
    // disable-hang player attribute (0x140abdc20) is 0 for a stock pilot.
    let hw = c.half_width();
    let rs = Vec3::new(o.x, o.y, t2.endpos.z - (cv.automantle_dangle_required_space - 5.0));
    let rd = -n * (hw + hw);
    let rl = rd.length();
    let hit = rl > 0.0 && c.col.ray(rs, rd / rl, rl).is_some();
    let can_hang = hit && !cv.automantle_disable_hang;
    let dist = if fwd_reach <= moved { fwd_reach } else { moved };
    Some(Ledge { n, tr: t2, top, dist, can_hang })
}

/// CheckAutoMantle 0x140a43060.
pub(super) fn check_auto_mantle(c: &mut Ctx) -> bool {
    // automantle_enable, pl+0x3268 bit 11, settings `automantle`, 0x140b134e0, pushers pl+0x3078, the base/mover
    // velocity 0x140b18e50 <= 0.01: the last four never block a stock pilot on static geometry
    if !c.cv.automantle_enable || !c.s.automantle || c.p.move_type != MoveType::Walk {
        return false;
    }
    let jump_held = c.buttons & IN_JUMP != 0;
    let on_wall = !upright(c.p.surface_normal); // S21_NotUpright 0x140b04b30
    // (no ground, on a wall, or jump held) and (0.25 s since pl+0x362c, or jump held); grapple and pl+0x322c
    // refusals not modelled; automantle_cooldown since the last traversal start
    if !(!c.gnd() || on_wall || jump_held) {
        return false;
    }
    if !(0.25 <= c.now - c.p.near_ground_time || jump_held) {
        return false;
    }
    if !(c.cv.automantle_cooldown <= c.now - c.p.traversal.start_time) {
        return false;
    }
    let mh = c.s.mantle_height;
    let o = c.p.pos;
    let yaw = c.flat_f; // (sin(yaw + pi/2), sin(yaw), 0)
    // mv+0x30 forwardmove >= 0.3, or the grapple_autoMantle grace (no grapple here)
    let push = c.fwd >= 0.3;
    let along = yaw.y * c.p.vel.y + yaw.x * c.p.vel.x + c.p.vel.z * 0.0;
    if !(push || on_wall || 0.01 < along) {
        return false;
    }
    let mut dist = c.cv.automantle_searchdist - 0.5;
    if dist < 0.5 {
        dist = 0.5;
    }
    let fwd_move = along.max(0.0) * c.ft;
    if dist <= fwd_move {
        dist = fwd_move;
    }
    let end = Vec3::new(yaw.x * dist + o.x, yaw.y * dist + o.y, dist * 0.0 + o.z);
    let tr = c.trace(o, end);
    let nup = tr.normal.z;
    // (a floor-like first hit is retraced along the slope; that only feeds the airborne weapon-lowering branch,
    // which sets pl+0x2bb0 = 7 and never mantles, so it is not ported)
    if 1.0 <= tr.fraction || 0.7 <= nup || tr.startsolid {
        return false;
    }
    let Some(l) = find_ledge(c, yaw, o, &tr) else {
        return false;
    };
    // pl+0x1d3c == 2 within 0.3 s of pl+0x2cc8 sets velocity = pl+0x2ab0 * 30000 instead of mantling: state not
    // identified, not modelled
    if !(l.can_hang || push) {
        return false;
    }
    let n = l.n;
    let lt = &l.tr;
    let probe_top = l.top; // the down trace's start z
    // walk the ledge back toward the origin column to find the corner height
    let (mut te, mut tn, mut tf) = (lt.endpos, lt.normal, lt.fraction);
    let mut fz = o.z;
    let mut iter = 0;
    let corner_z = loop {
        let mut step = false;
        if 0.7 <= tn.z {
            fz = ((te.y - o.y) * tn.y + (te.x - o.x) * tn.x + (te.z - fz) * tn.z) / tn.z + fz + 0.015625;
            let d = fz - te.z;
            if 6.0 <= d {
                step = true;
            } else if 0.0 < d || {
                fz = te.z;
                tf != 0.0
            } || iter < 1
            {
                if probe_top < fz {
                    break probe_top;
                }
            } else {
                break probe_top;
            }
        } else {
            step = true;
        }
        if step {
            fz = te.z + 4.0;
            if probe_top < fz {
                break probe_top;
            }
            te.z = fz; // (and the normal set to up; the retrace below overwrites it)
        }
        // crouched, from the ledge point back over the origin column at that height
        let t = trace_crouch(c, te, Vec3::new(o.x, o.y, fz));
        te = t.endpos;
        tn = t.normal;
        tf = t.fraction;
        if tf == 1.0 {
            break fz;
        }
        iter += 1;
        if iter >= 1000 {
            return false;
        }
    };
    let cz = corner_z;
    let mut o = o;
    // no room standing at the corner height (0.99 of the reach into the ledge): FinishDuck 0x140a53d50
    if c.p.duck_state != 2 {
        let f = l.dist * 0.99;
        let a = Vec3::new(o.x, o.y, cz);
        let t = c.trace(a, Vec3::new(o.x - f * n.x, o.y - f * n.y, cz - f * n.z));
        if t.startsolid || t.fraction < 1.0 {
            c.p.ducked = true;
            c.p.duck_state = 2;
            c.p.view_z = c.s.crouch.viewheight;
            // the hull mins do not move (both stances start at z 0), so the origin stays
            ground::categorize_position(c);
            o = c.p.pos;
        }
    }
    let start = Vec3::new(o.x, o.y, (cz as f64 - (mh as f64 + 0.03125)) as f32);
    let start_z = if o.z < start.z { start.z } else { o.z };
    let mut cz2 = cz;
    let ln = lt.normal;
    if 0.0 < n.x * ln.x + ln.y * n.y + ln.z * n.z {
        cz2 = ((lt.endpos.y - o.y) * ln.y + (lt.endpos.x - o.x) * ln.x + (lt.endpos.z - cz) * ln.z) / ln.z + cz;
    }
    let hw = c.half_width(); // mv+0xd0
    cz2 -= hw * 0.5;
    if cz2 <= start_z {
        cz2 = start_z;
    }
    // how far the hull gets toward the wall just under the corner
    let a = Vec3::new(o.x, o.y, cz2);
    let t = c.trace(a, Vec3::new(o.x - n.x * l.dist, o.y - n.y * l.dist, cz2 - n.z * l.dist));
    let mut f20 = t.fraction * l.dist;
    let lim = l.dist - c.cv.automantle_mindist;
    if lim <= f20 {
        f20 = lim;
    }
    let f26 = f20 + hw;
    let corner = Vec3::new(o.x, o.y, cz);
    let end_u = Vec3::new(o.x - n.x * f26, o.y - f26 * n.y, cz - f26 * n.z);
    let t = c.trace(corner, end_u);
    let end = (end_u - corner) * t.fraction + corner;
    let ledge_point = end_u;
    // 0x140a44120 refines the ledge point to the lip with ray traces and decides "can rest" (needs
    // traversal_hand_required_width 6 of lip); taken as found, at this point
    let cc = mh / (f26 + mh);
    let mut can_rest = true;
    if t.fraction * mh < c.cv.automantle_dangle_required_space || !l.can_hang {
        can_rest = false;
    } else if f20 + hw + 4.0 < (o.y - ledge_point.y) * n.y + (o.x - ledge_point.x) * n.x + (cz - ledge_point.z) * n.z {
        can_rest = false;
    }
    let start_origin = Vec3::new(o.x, o.y, start_z);
    start_traversal(c, start, corner, end, cc, -n, ledge_point, start_origin, can_rest);
    let mut f = (start_z - start.z) / mh;
    if 1.0 < f {
        f = 1.0;
    }
    c.p.traversal.p = f * cc;
    c.p.traversal.cycle = 0.0;
    // minimum target: how much of the vertical leg is blocked below (the floor under a standing start)
    let t = c.trace(corner, start);
    c.p.traversal.min_target = (1.0 - t.fraction) * cc;
    true
}

/// StartTraversal 0x140a41af0 (type 0, automantle)
#[allow(clippy::too_many_arguments)]
fn start_traversal(c: &mut Ctx, start: Vec3, corner: Vec3, end: Vec3, cc: f32, fwd: Vec3, ledge_point: Vec3, origin: Vec3, can_rest: bool) {
    let now = c.now;
    // the ledge point is quantised to 1/32 (floorf(x * 32) * 0.03125)
    let q = |x: f32| ((x * 32.0).floor() as i32) as f32 * 0.03125;
    c.p.traversal = Traversal {
        start,
        corner,
        end,
        c: cc,
        p: 0.0,
        min_target: 0.0,
        state: 0,
        jump_time: 0.0,
        drop: false,
        fwd_latch: false,
        can_rest,
        start_time: now,
        end_time: c.p.traversal.end_time,
        ledge_forward: fwd,
        ledge_point: Vec3::new(q(ledge_point.x), q(ledge_point.y), q(ledge_point.z)),
        rate: 0.0,
        cycle: 0.0,
    };
    c.p.sliding = false;
    c.p.air_from_jump = false;
    c.p.pos = origin;
    c.p.ground = true; // the ledge becomes the ground entity
    c.p.vel = Vec3::ZERO;
    c.p.move_type = MoveType::Traverse;
    // StartTraversal ends a wall climb (0x140b04df0)
    wall::end_wall_run(c);
}

/// FinishTraversal 0x140a46e40: origin at the stored progress, cycle 0, back to WALK, CategorizePosition.
fn finish_traversal(c: &mut Ctx) {
    c.p.pos = path_pos(&c.p.traversal, c.p.traversal.p);
    c.p.traversal.cycle = 0.0;
    c.p.move_type = MoveType::Walk;
    ground::categorize_position(c);
}

/// EndTraversal = FinishTraversal + the traversal_cooldown stamp pl+0x2b88
fn end_traversal(c: &mut Ctx) {
    finish_traversal(c);
    c.p.traversal.end_time = c.now;
}

/// MantleBoost_ActivationButtonMask: 1 = jump, 2 = crouch, 3 = a custom mask (default +dodge, which a pilot has
/// not got here), 0 = off
fn boost_activation(c: &Ctx) -> bool {
    match c.cv.mantle_boost_input_setting {
        1 => c.pressed & IN_JUMP != 0,
        2 => c.pressed & IN_DUCK != 0,
        _ => false,
    }
}

/// Hook_TraversalMove (SDK mantle_boost.cpp): on the first activation press of a climb (not on its first tick,
/// not while dropping or after a native jump request) the FSM latches 4 (inside the sweet spot and past
/// mantle_boost_min_valid_traversal_frac of the animation cycle) or 3. While latched, the jump press is kept
/// from TraversalMove so the climb finishes, and on the completion tick the exit velocity is replaced and a jump
/// forced. With mantle_boost_input_setting 0 (the live R5Flowstate value) it never latches and this is the
/// native TraversalMove.
pub(super) fn traversal_move(c: &mut Ctx, just_started: bool) {
    if !c.cv.mantle_boost_enabled {
        c.p.mantle_boost.state = 0;
        c.p.mantle_boost.applied_time = 0.0;
        traversal_move_native(c, just_started);
        return;
    }
    if just_started {
        c.p.mantle_boost.state = 0;
        c.p.mantle_boost.applied_time = 0.0;
        c.p.mantle_boost.sweet_spot_frac = 0.0;
    }
    let p_before = c.p.traversal.p;
    let jt_before = c.p.traversal.jump_time;
    if !just_started && c.p.mantle_boost.state == 0 && c.p.traversal.state == 11 {
        c.p.mantle_boost.state = 1; // a climb that settled into the rest hang can no longer boost
    }
    if !just_started && c.p.mantle_boost.state == 0 {
        let t = &c.p.traversal;
        if boost_activation(c) && !t.drop && t.jump_time == 0.0 {
            c.p.mantle_boost.state = if in_sweet_spot(c) && c.p.traversal.cycle > c.cv.mantle_boost_min_valid_traversal_frac { 4 } else { 3 };
        }
    }
    let mask = c.p.mantle_boost.state >= 3;
    let saved = c.pressed & IN_JUMP;
    if mask {
        c.pressed &= !IN_JUMP;
    }
    traversal_move_native(c, just_started);
    if mask {
        c.pressed |= saved;
    }
    // armed: undo a jump-off stamp made this call
    if c.p.mantle_boost.state >= 3 && jt_before == 0.0 && c.p.traversal.jump_time != 0.0 {
        c.p.traversal.jump_time = 0.0;
    }
    // the sweet spot for a hint UI: progress at which the remaining time equals the window
    let t = &c.p.traversal;
    if t.rate > 0.0 {
        c.p.mantle_boost.sweet_spot_frac = (1.0 - t.rate * MANTLE_BOOST_SWEET_SPOT_TIME).max(c.cv.mantle_boost_min_valid_traversal_frac);
    }
    if c.p.mantle_boost.state >= 3 && p_before < 1.0 && c.p.traversal.p == 1.0 && c.p.move_type == MoveType::Walk {
        apply_boost(c);
    }
}

/// The sweet spot (stand-in for the camera-pitch test): the last MANTLE_BOOST_SWEET_SPOT_TIME of the climb at last
/// tick's rate. The SDK's frac gate (animation cycle above mantle_boost_min_valid_traversal_frac) is applied by
/// the caller.
fn in_sweet_spot(c: &Ctx) -> bool {
    let t = &c.p.traversal;
    if t.rate <= 0.0 {
        return false;
    }
    (1.0 - t.p) / t.rate <= MANTLE_BOOST_SWEET_SPOT_TIME
}

/// MantleBoost_ApplyBoost: exit velocity = input direction mv+0xb4 (traversal forward if none, mirrored if it
/// points back off the ledge) * standing sprint speed (* 1.3 for a boost), then a forced Jump whose vz is
/// recomposed as sqrt(2 g h), h = 90 (boost) or 56 (plain). Crouch activation (setting 2) ducks instead of
/// jumping on a failed (3) climb. A boost stamps the slide time and grants the slide state (long jump allowed).
fn apply_boost(c: &mut Ctx) {
    let state = c.p.mantle_boost.state;
    let f = c.p.traversal.ledge_forward;
    let mut dir = c.wish;
    if dir.x.abs().max(dir.y.abs()).max(dir.z.abs()) <= 0.0099999998 {
        dir = f;
    }
    let d = dir.x * f.x + dir.y * f.y + dir.z * f.z;
    if d < 0.0 {
        dir -= f * (2.0 * d);
    }
    let mut speed = c.p.speed_scale * c.s.stand.sprintspeed; // GetPoseSpeed_Sprint(pl, standing)
    if state == 4 {
        speed *= c.cv.mantle_boost_sprint_mult;
    }
    if dir.x.abs().max(dir.y.abs()).max(dir.z.abs()) > 0.0099999998 {
        c.p.vel = dir * speed;
    }
    if state == 4 {
        c.p.last_slide_time = c.now;
    }
    let crouch_mode = c.cv.mantle_boost_input_setting == 2;
    if state == 4 || !crouch_mode {
        // forced Jump: IN_JUMP set in pressed (and old buttons, which CheckJumpButton does not read), the landing
        // time pushed out of the bunny-hop window, then vz recomposed
        let (sp, sl) = (c.pressed, c.p.last_land_time);
        c.pressed |= IN_JUMP;
        c.p.last_land_time = -100000.0;
        let before = c.p.vel;
        walk::check_jump_button(c);
        let h = if state == 4 { c.cv.mantle_boost_jump_height } else { 56.0 };
        c.p.vel = Vec3::new(before.x, before.y, before.z + (2.0 * c.cv.sv_gravity * h).sqrt());
        c.p.last_land_time = sl;
        c.pressed = sp;
    } else if state == 3 {
        let b = c.buttons;
        c.buttons |= IN_DUCK;
        duck::duck(c);
        c.buttons = b;
    }
    if state == 4 {
        c.p.sliding = true;
        c.p.slide_long_jump = true;
        c.p.mantle_boost.applied_time = c.now;
    }
}

/// TraversalMove 0x140a45380 (ds 0x140b981b0, movemodes.md B3), type 0 (automantle). The origin follows the path
/// (traversal_anim 0 math); the state choice and the cycle pl+0x2234 follow the traversal_anim 1 code.
fn traversal_move_native(c: &mut Ctx, b_start: bool) {
    let cv = c.cv;
    let mh = c.s.mantle_height;
    let now = c.now;
    // step 1: state remap and rest fraction
    let st0 = c.p.traversal.state;
    let st = if (st0 - 7) as u32 <= 3 { st0 - 4 } else { st0 };
    let mut rest = if ((st0 - 3) & !4) == 0 { cv.automantle_rest_frac_below } else { cv.automantle_rest_frac };
    // step 2
    let p0 = c.p.traversal.p;
    c.p.traversal.end_time = 0.0;
    c.p.vel = Vec3::ZERO;
    let cc = c.p.traversal.c;
    // step 3: input flags
    let f = c.p.traversal.ledge_forward;
    let w = c.wish; // mv+0x84
    let fd = w.y * f.y + w.x * f.x + w.z * f.z;
    let jp = c.pressed & IN_JUMP != 0;
    let duck = c.buttons & IN_DUCK != 0;
    let raw_fwd = fd > 0.1;
    let mut fwd = raw_fwd;
    let mut back = -0.3 > fd;
    let mut back_e = back;
    let grace = false; // grapple_autoMantle: no grapple
    if fwd {
        c.p.traversal.fwd_latch = true;
    } else if back {
        c.p.traversal.fwd_latch = false;
    }
    // step 4
    let eye_z = c.p.view_z;
    // step 5: jump request and effective input
    {
        let t = &mut c.p.traversal;
        if jp && !t.drop && (!raw_fwd || p0 >= cc) && t.jump_time == 0.0 {
            t.jump_time = now;
        }
        if t.jump_time != 0.0 {
            fwd = false;
            back = false;
            back_e = false;
        }
        if p0 > rest && !fwd && !back && !jp {
            fwd = if p0 > cc { true } else { t.fwd_latch };
        }
        if t.drop {
            back = true;
            fwd = false;
            back_e = true;
        }
    }
    // step 6: progress rate
    let (x, d) = match st {
        4 => (cv.automantle_height_below - eye_z, c.s.mantle_duration_level),
        5 => (cv.automantle_height_level - eye_z, c.s.mantle_duration_above),
        6 => (cv.automantle_height_above - eye_z, c.s.mantle_duration_high),
        _ => (-mh, c.s.mantle_duration_below),
    };
    let rate = (1.0 - (((x + mh) / mh) * cc)) / d;
    c.p.traversal.rate = rate;
    // step 7: target and progress
    let ft = c.ft;
    let (mut target, mut p);
    {
        let t = &mut c.p.traversal;
        if t.jump_time == 0.0 {
            // (the rest target also needs the ledge frame upright, |M[2][2]| > 0.99: always for the world)
            target = if fwd || (!t.can_rest && grace) {
                t.min_target.max(1.0)
            } else if back || !t.can_rest {
                t.min_target.max(0.0)
            } else {
                t.min_target.max(rest)
            };
            p = if target > p0 {
                target.min((rate * ft) + p0)
            } else if p0 > target {
                target.max(p0 - (rate * ft))
            } else {
                p0
            };
            if p == target && target == rest {
                rest = cv.automantle_rest_frac;
                target = rest;
                p = rest;
            }
        } else {
            target = p0;
            p = p0;
        }
    }
    // step 9 (traversal_anim 1, minus the sequences): pick the climb animation from the eye height over the lip
    let mut s = st;
    if b_start {
        let lp = c.p.traversal.ledge_point;
        let eye = c.p.pos + Vec3::new(0.0, 0.0, eye_z);
        let dz = eye.z - lp.z;
        let hw = c.half_width();
        s = if !(cv.automantle_height_below <= dz) && !(((eye.y - lp.y) * f.y + (eye.x - lp.x) * f.x + dz * f.z) <= -hw - 4.0) {
            3
        } else if cv.automantle_height_level <= dz {
            if cv.automantle_height_above <= dz { 6 } else { 5 }
        } else {
            4
        };
    }
    {
        let t = &c.p.traversal;
        if target == rest && 0.001 > (p - cv.automantle_rest_frac).abs() {
            s = 11;
        } else if t.can_rest && ((!fwd && p > rest) || s == 11) {
            s = 4;
        }
    }
    if c.p.duck_state == 2 && (s - 11) as u32 > 1 {
        s += 4;
    }
    if c.p.traversal.jump_time > now {
        s = 12;
    }
    let anim_set = b_start || s != c.p.traversal.state;
    if anim_set {
        c.p.traversal.state = s;
    }
    // the cycle (pl+0x2234): eases toward the progress mapped past the state's start offset
    {
        let t = &mut c.p.traversal;
        let x = match t.state {
            4 => cv.automantle_height_below - eye_z,
            5 => cv.automantle_height_level - eye_z,
            6 => cv.automantle_height_above - eye_z,
            _ => -mh,
        };
        let off = ((x + mh) / mh) * cc;
        let ct = ((p - off) / (1.0 - off)).max(0.0).min(1.0);
        if anim_set && !b_start {
            t.cycle = ct;
        }
        let step = rate * ft * 1.5 * (1.0 / (1.0 - off));
        let cur = t.cycle;
        let (nc, short) = if ct <= cur { (cur - step, ct < cur - step) } else { (step + cur, step + cur < ct) };
        t.cycle = if !short && nc != ct { ct } else { nc };
        t.cycle = t.cycle.max(0.0).min(1.0);
    }
    // step 10: completion (P exactly 1)
    if p == 1.0 {
        c.p.traversal.p = 1.0;
        finish_traversal(c);
        c.p.pos = c.p.traversal.end;
        if raw_fwd {
            let cap = c.speed_cap(c.p.sprint_latch);
            c.p.vel = c.wish * cap;
        }
        return;
    }
    // step 11: drive the origin along the path at the stored progress (traversal_anim 1 plays the animation's
    // root motion here instead)
    c.p.pos = path_pos(&c.p.traversal, p0);
    // step 12: leaving
    let t = c.p.traversal;
    if t.jump_time != 0.0 && (t.jump_time + cv.automantle_jumpoff_duration <= now || cv.automantle_jumpoff_anim_maxfrac < p) {
        // JUMPOFF
        let f3 = t.ledge_forward;
        end_traversal(c);
        c.p.jumped = true;
        if !(p < cc) {
            // past the corner: input direction (mv+0xb4) at full (sprint) speed, then CheckJumpButton
            let cap = c.speed_cap(c.p.sprint_latch);
            c.p.vel = c.wish * cap;
            walk::check_jump_button(c);
        } else {
            // still on the wall: hop back off it
            let a = f3 * -c.s.wallrun_jump_outward_speed;
            let dd = UP.dot(a);
            let u = c.s.wallrun_jump_up_speed - dd;
            c.p.vel = UP * u + a;
        }
        return;
    }
    if back_e && (cc <= p || cv.automantle_backoff_anim_maxfrac <= p) {
        end_traversal(c);
        return;
    }
    if (p <= t.min_target || (target == 0.0 && t.cycle <= 0.0)) && ((!raw_fwd && !grace) || t.drop) {
        end_traversal(c);
        return;
    }
    if back_e || (t.state == 11 && duck) {
        if st == 11 {
            c.p.traversal.drop = true;
        } else if !t.drop {
            end_traversal(c);
            return;
        }
    }
    // TAIL
    c.p.traversal.p = p;
}
