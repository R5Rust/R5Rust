//! Mantling: CheckAutoMantle 0x140b95a40 (rebuilt here: the spec has it at decompile-summary level only),
//! StartTraversal 0x140b940a0, TraversalMove 0x140b981b0 (movemodes.md part B, with traversal_anim 0: we have
//! no first-person animations, so the origin follows the two-segment path, which is the engine's own
//! traversal_anim 0 code path), and the mantle boost (R5Regenis engine/s21-r5sdk mantle_boost.cpp).

use super::{ground, upright, walk, wall, Ctx, MoveType, Traversal, IN_DUCK, IN_JUMP, UP};
use glam::Vec3;

/// Remaining mantle time in which a press counts as the mantle-boost sweet spot. The reconstruction measures
/// |traversal camera pitch - eye pitch| < 1.5 deg (bridge_mantle_boost_sweet_spot_angle) on the animation and
/// notes "1.5 opens about the last 0.10-0.13 s of a mantle"; without the animation we use the middle of that.
pub const MANTLE_BOOST_SWEET_SPOT_TIME: f32 = 0.115;

/// TraversalPathPos 0x140bf15d0 (B4): up the wall to the corner, then forward onto the ledge.
fn path_pos(t: &Traversal, p: f32) -> Vec3 {
    let (a, b, f) = if t.c > p { (t.start, t.corner, p / t.c) } else { (t.corner, t.end, (p - t.c) / (1.0 - t.c)) };
    (b - a) * f + a
}

/// CheckAutoMantle 0x140b95a40. The gates follow the spec's summary (enable cvars, cooldown, jump held when on
/// or just off the ground); the ledge search is ours: a wall within automantle_searchdist along the view yaw,
/// a ledge top within mantleHeight above the feet with room for the hull, then StartTraversal with the spec's
/// path (start below the ledge by mantleHeight, corner, end) and C = mH / (h + mH).
pub(super) fn check_auto_mantle(c: &mut Ctx) -> bool {
    if !c.cv.automantle_enable || !c.s.automantle || c.p.move_type != MoveType::Walk {
        return false;
    }
    let jump_held = c.buttons & IN_JUMP != 0;
    // on upright ground it needs jump held; on a wall (climbing) it does not
    if c.gnd() && upright(c.p.surface_normal) && !jump_held {
        return false;
    }
    if 0.25 > c.now - c.p.near_ground_time && !jump_held {
        return false;
    }
    if c.cv.automantle_cooldown > c.now - c.p.traversal.start_time {
        return false;
    }
    // ours: you mantle what you push into
    if c.fwd <= 0.1 {
        return false;
    }
    let o = c.p.pos;
    let yaw = c.flat_f;
    let along = c.p.vel.dot(yaw).max(0.0) * c.ft;
    let dist = (0.5f32).max(c.cv.automantle_searchdist - 0.5).max(along);
    let tr = c.trace(o, o + yaw * dist);
    if !tr.hit() || tr.startsolid || tr.normal.z.abs() >= 0.7 || tr.normal.dot(yaw) > -0.5 {
        return false;
    }
    let into = Vec3::new(-tr.normal.x, -tr.normal.y, 0.0).normalize_or_zero();
    let wall = tr.endpos;
    let mh = c.s.mantle_height;
    let hw = c.half_width();
    // head room straight up from where we touch the wall
    let up = c.trace(wall, wall + UP * (mh + 2.0));
    if up.startsolid {
        return false;
    }
    let top = up.endpos;
    // over the lip onto the ledge: far enough that the whole hull stands on it
    let end_xy = top + into * (hw * 2.0 + 1.0);
    let over = c.trace(top, end_xy);
    if over.hit() || over.startsolid {
        return false;
    }
    let down = c.trace(end_xy, Vec3::new(end_xy.x, end_xy.y, o.z));
    if !down.hit() || down.startsolid || down.normal.z < 0.7 {
        return false;
    }
    let ledge_z = down.endpos.z;
    let rise = ledge_z - o.z;
    if rise <= 1.0 || rise > mh + 1.0 {
        return false;
    }
    // path (the engine's: start = mantleHeight below the ledge, corner = at the wall at ledge height, end)
    let corner = Vec3::new(wall.x, wall.y, ledge_z);
    let end = down.endpos;
    let start = Vec3::new(wall.x, wall.y, ledge_z - mh);
    let h = Vec3::new(end.x - corner.x, end.y - corner.y, 0.0).length();
    let cc = mh / (h + mh);
    let p0 = ((o.z - start.z) / mh).min(1.0).max(0.0) * cc;
    let ledge_point = Vec3::new(wall.x, wall.y, ledge_z) + into * hw;
    start_traversal(c, start, corner, end, cc, p0, into, ledge_point);
    true
}

/// StartTraversal 0x140b940a0 (type 0, automantle)
fn start_traversal(c: &mut Ctx, start: Vec3, corner: Vec3, end: Vec3, cc: f32, p0: f32, fwd: Vec3, ledge_point: Vec3) {
    let now = c.now;
    c.p.traversal = Traversal {
        start,
        corner,
        end,
        c: cc,
        p: p0,
        min_target: 0.0,
        state: 0,
        jump_time: 0.0,
        drop: false,
        fwd_latch: false,
        can_rest: false,
        start_time: now,
        end_time: c.p.traversal.end_time,
        ledge_forward: fwd,
        ledge_point,
        rate: 0.0,
    };
    c.p.sliding = false;
    c.p.air_from_jump = false;
    c.p.ground = true; // the ledge becomes the ground entity
    c.p.vel = Vec3::ZERO;
    c.p.move_type = MoveType::Traverse;
    // StartTraversal ends a wall climb (EndWallRun call at 0x140b94cb0)
    wall::end_wall_run(c);
}

/// FinishTraversal 0x140b99b60: origin at the stored progress, back to WALK, CategorizePosition.
fn finish_traversal(c: &mut Ctx) {
    c.p.pos = path_pos(&c.p.traversal, c.p.traversal.p);
    c.p.move_type = MoveType::Walk;
    ground::categorize_position(c);
}

/// EndTraversal 0x140b99c10 = FinishTraversal + the traversal_cooldown stamp
fn end_traversal(c: &mut Ctx) {
    finish_traversal(c);
    c.p.traversal.end_time = c.now;
}

/// The mantle-boost bridge around TraversalMove (mantle_boost.cpp Hook_CGameMovement_TraversalMove): the first
/// activation press (input setting 1 = jump) during a mantle latches state 4 (inside the sweet spot, past
/// mantle_boost_min_valid_traversal_frac) or 3 (outside), the jump is then kept away from TraversalMove so the
/// climb finishes, and on the completion tick the exit velocity is overwritten and a jump forced.
pub(super) fn traversal_move(c: &mut Ctx, just_started: bool) {
    if !c.cv.mantle_boost_enabled {
        traversal_move_native(c, just_started);
        return;
    }
    if just_started {
        c.p.mantle_boost.state = 0;
        c.p.mantle_boost.sweet_spot_frac = 0.0;
    }
    let p_before = c.p.traversal.p;
    if !just_started && c.p.mantle_boost.state == 0 && c.p.traversal.state == 11 {
        c.p.mantle_boost.state = 1; // a climb that settled into the rest hang can no longer boost
    }
    if !just_started && c.p.mantle_boost.state == 0 && c.pressed & IN_JUMP != 0 && c.p.traversal.jump_time == 0.0 && !c.p.traversal.drop {
        c.p.mantle_boost.state = if in_sweet_spot(c) { 4 } else { 3 };
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
    // the sweet spot for a hint UI: progress at which the remaining time equals the window
    let t = &c.p.traversal;
    if t.rate > 0.0 {
        c.p.mantle_boost.sweet_spot_frac = (1.0 - t.rate * MANTLE_BOOST_SWEET_SPOT_TIME).max(c.cv.mantle_boost_min_valid_traversal_frac);
    }
    if c.p.mantle_boost.state >= 3 && p_before < 1.0 && c.p.traversal.p == 1.0 && c.p.move_type == MoveType::Walk {
        apply_boost(c);
    }
}

/// The sweet spot: the last MANTLE_BOOST_SWEET_SPOT_TIME of the climb (at last tick's rate), and past
/// mantle_boost_min_valid_traversal_frac.
fn in_sweet_spot(c: &Ctx) -> bool {
    let t = &c.p.traversal;
    if t.rate <= 0.0 {
        return false;
    }
    let remaining = (1.0 - t.p) / t.rate;
    remaining <= MANTLE_BOOST_SWEET_SPOT_TIME && t.p > c.cv.mantle_boost_min_valid_traversal_frac
}

/// MantleBoost_ApplyBoost: exit velocity = input direction (traversal forward if none, mirrored if it points
/// back off the ledge) * standing sprint speed (* 1.3 for a boost), then a forced jump whose height is 90
/// (boost) or 56 (plain), and a boost grants the slide state (long-jump allowed) and stamps the slide time.
fn apply_boost(c: &mut Ctx) {
    let state = c.p.mantle_boost.state;
    let f = c.p.traversal.ledge_forward;
    let mut dir = c.wish;
    if dir.x.abs().max(dir.y.abs()).max(dir.z.abs()) <= 0.0099999998 {
        dir = f;
    }
    let d = dir.dot(f);
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
        c.p.mantle_boost.applied_time = c.now;
    }
    // forced Jump: jump pressed + held, the bunny-hop window pushed out of the way, then vz recomposed
    let (sb, sp, sl) = (c.buttons, c.pressed, c.p.last_land_time);
    c.buttons |= IN_JUMP;
    c.pressed |= IN_JUMP;
    c.p.last_land_time = -100000.0;
    let before = c.p.vel;
    walk::check_jump_button(c);
    let h = if state == 4 { c.cv.mantle_boost_jump_height } else { c.s.jump_height };
    c.p.vel = before + UP * (2.0 * c.cv.sv_gravity * h).sqrt();
    c.p.last_land_time = sl;
    c.pressed = sp;
    c.buttons = sb;
    if state == 4 {
        c.p.sliding = true;
        c.p.slide_long_jump = true;
    }
}

/// TraversalMove 0x140b981b0 (movemodes.md B3), type 0 (automantle) with traversal_anim 0.
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
    let fd = c.wish.dot(f);
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
        if jp && !t.drop && (!raw_fwd || p0 >= cc) && t.jump_time == 0.0 && t.jump_time != now {
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
    // step 9 (state bookkeeping only; animation not driven): pick the climb animation from the eye height
    let mut s = st;
    if b_start {
        let lp = c.p.traversal.ledge_point;
        let eye = c.p.pos + Vec3::new(0.0, 0.0, eye_z);
        let dlt = eye - lp;
        let hw = c.half_width();
        s = if cv.automantle_height_below > dlt.z && dlt.dot(f) > (-hw - 4.0) {
            3
        } else if cv.automantle_height_level > dlt.z {
            4
        } else if cv.automantle_height_above <= dlt.z {
            6
        } else {
            5
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
    if b_start || s != c.p.traversal.state {
        c.p.traversal.state = s;
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
    // step 11: drive the origin along the path (traversal_anim 0: the stored progress)
    c.p.pos = path_pos(&c.p.traversal, p0);
    // step 12: leaving
    let t = c.p.traversal;
    if t.jump_time != 0.0 && (now >= t.jump_time + cv.automantle_jumpoff_duration || p > cv.automantle_jumpoff_anim_maxfrac) {
        // JUMPOFF
        let f3 = t.ledge_forward;
        end_traversal(c);
        c.p.jumped = true;
        if !(p < cc) {
            // past the corner: input direction at full (sprint) speed, then the jump itself
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
    if back_e && (p >= cc || p >= cv.automantle_backoff_anim_maxfrac) {
        end_traversal(c);
        return;
    }
    // no animation cycle: pl+0x6b14 stays 0
    if (t.min_target >= p || target == 0.0) && ((!raw_fwd && !grace) || t.drop) {
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
