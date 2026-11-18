//! FullWalkMove and everything it calls on the ground and in the air
//! (fullwalkmove.md, walkmove.md, airmove.md; Friction/Accelerate/AirAccelerate/CheckVelocity from
//! R5Regenis native/r5r_engine/src/movement.cpp, GetAccelerate disassembled for this port).

use super::{duck, ground, step, upright, wall, Ctx, IN_BACK, IN_DUCK, IN_FORWARD, IN_JUMP, IN_MOVELEFT, IN_MOVERIGHT, UP};
use glam::Vec3;

/// The engine's MAX(len, eps) epsilon (runtime value; any tiny positive number behaves the same).
const EPS: f32 = 1e-8;

/// FullWalkMove, S21 0x140a4c810 (ds 0x140b9ead0, fullwalkmove.md section 2). The S21 order is the ds one; the
/// extra S21 steps are all gated on things a stock pilot never has (listed below where they run).
pub(super) fn full_walk_move(c: &mut Ctx) {
    let dt = c.ft; // TimeScale 0x140b0f6d0 = 1 (no air slow-mo)
    // S21 entry: the dodge flags pl+0x36aa / 0x363c expire (dodge only)
    // step 1: start gravity (S21 0x140a36130)
    apply_half_gravity(c, dt);
    // step 2: CheckVelocity 0x140a50da0. Then, before the jump, S21 runs: trigger touches / launch pads
    // 0x140a37390, the pull-to-target 0x140a379c0 (pl+0x2be2), the path mover 0x140a37560 (pl+0x4838), the
    // scripted launch 0x140a3b310 (pl+0x2cb8) and the trigger steer 0x140a4c4e0 (pl+0x30f4): none for a pilot
    check_velocity(c);
    // CheckJumpButton 0x140a3b550
    check_jump_button(c);
    // step 3: WallRunUpdate (when a wall climb ends). Constraints 0x140a3e4f0 / tethers 0x140a3f050 / the
    // gravity-grenade pull fields (the FUN_140aca1a0 loop): not in scope
    wall::wall_run_update(c);
    // step 5
    check_velocity(c);
    if c.gnd() {
        // step 6G: no slip triggers (pl+0x3078 count 0), so the slip direction is zero and minPush is 0; S21
        // computes minPush as the velocity along it (capped by slipSpeed via 0x140a36a90 / 0x140a36c20) and
        // re-applies it after WalkMove 0x140a4a7c0. (On a wall the wall is the ground: WalkMove's wall branch.)
        walk_move(c, dt);
    } else {
        // step 7A
        if !upright(c.p.surface_normal) {
            wall::end_wall_run(c);
        }
        air_move(c, dt);
    }
    // step 8 (ImpactCheck 0x140a4d040: sounds/events only), CategorizePosition 0x140a51bc0, end gravity
    ground::categorize_position(c);
    apply_half_gravity(c, dt);
    // step 9: strip velocity along n (S21 pl+0x2ab0, the surface normal) when grounded
    if c.gnd() {
        let n = c.p.surface_normal;
        let v = c.p.vel;
        let d = ((v.y * n.y) + (v.x * n.x)) + (v.z * n.z);
        c.p.vel = Vec3::new(v.x - (n.x * d), v.y - (n.y * d), v.z - (n.z * d));
    }
    // step 10 (CheckFalling: fall damage, not in scope)
    check_velocity(c);
}

/// ApplyHalfGravity / AddGravity, S21 0x140a36130 (ds 0x140b8c2a0; same math: the double-precision
/// `(gs*sv_gravity)*0.5*dt*W`, gravity dir (0,0,-1) at 0x1427c9988; S21 skips it entirely on a launch-pad latch
/// pl+0x2bba or the launch fall limiter pl+0x4960/0x4964) (fullwalkmove.md 3, slide.md 2E): half of gravity*dt per call,
/// plus the slope-parallel part of gravity while sliding on the ground. On a wall it is scaled by
/// WallSpeedFrac (0 while climbing into the wall).
pub(super) fn apply_half_gravity(c: &mut Ctx, dt: f32) {
    let gs = if c.s.gravity_scale == 0.0 { 1.0 } else { c.s.gravity_scale };
    let mut w = 1.0f32;
    if !upright(c.p.surface_normal) {
        w = wall::wall_speed_frac(c, c.now - c.p.wall.start_time);
    }
    let g = (((((gs * c.cv.sv_gravity) as f64) * 0.5) * dt as f64) * w as f64) as f32;
    let v = &mut c.p.vel;
    v.z -= g;
    if c.p.sliding && c.p.ground {
        let n = c.p.ground_normal;
        v.x += (g * n.x) * n.z;
        v.y += (g * n.y) * n.z;
    }
}

/// CheckVelocity, S21 0x140a50da0 (ds 0x140ba3b60): NaN guard and sv_maxvelocity clamp (scales the whole vector,
/// keeping direction). Skipped while the velocity is within 0.001 (squared, compared as a double) of the last
/// checked one.
pub(super) fn check_velocity(c: &mut Ctx) {
    let v = &mut c.p.vel;
    let d = c.p.cv_cache - *v;
    if !(0.001f64 <= (((d.y * d.y) + (d.x * d.x)) + (d.z * d.z)) as f64) {
        return;
    }
    for i in 0..3 {
        if !v[i].is_finite() {
            v[i] = 0.0;
        }
        if !c.p.pos[i].is_finite() {
            c.p.pos[i] = 0.0;
        }
        c.p.pos[i] = c.p.pos[i].clamp(-65536.0, 65536.0);
        let m = c.cv.sv_maxvelocity;
        let x = v[i];
        let scale = if x > m {
            m / x
        } else if -x > m {
            -(m / x)
        } else {
            continue;
        };
        *v *= scale;
    }
    c.p.cv_cache = *v;
}

/// CheckJumpButton, S21 0x140a3b550 (ds 0x140b8d490; fullwalkmove.md 4, movemodes.md C). Ground jump and the
/// jump off a wall (WallJump 0x140a50050): this legend has no double jump, dodge or boost. The S21 one is the ds
/// one for a stock pilot (same constants 0.4 / 0.85 / 30 / -0.999 and cvar defaults 0.2 / 1.0 / 450 / 1.0 / -1 /
/// 100). Its extra refusals (path mover pl+0x4838, pl+0x19b0 == 13) and the slide super jump
/// (`player_slideSuperJumpEnabled` + `slide_super_jump_enabled`, a mod) are not stock paths. Not modelled: the
/// surface jump factor (surface data +0x48 at pl+0x4058, 1 on default surfaces) and LedgeAssist 0x140a58ec0.
pub(super) fn check_jump_button(c: &mut Ctx) -> bool {
    // J2: a jump press this tick (holding never re-jumps: there is no auto-hop)
    if c.pressed & IN_JUMP == 0 {
        return false;
    }
    // J3
    let slide_ok = c.p.sliding || c.s.can_jump_while_crouched;
    // J4: ground, or the 0.2 s coyote grace after leaving it (if we have not jumped since)
    let can_ground = if c.gnd() {
        true
    } else if c.cv.jump_graceperiod > c.now - c.p.last_ground_time && !c.p.jumped {
        // the grace jump is refused when the last ground was a wall
        if !upright(c.p.last_ground_normal) {
            return false;
        }
        true
    } else {
        false
    };
    // J5: toggle-crouch / forced stance: none here. J6: airborne without double jump would be a dodge, and
    // pilots have no dodge, so it is refused.
    if !can_ground {
        return false;
    }
    if c.p.duck_state == 3 && !slide_ok {
        return false;
    }
    // J7 normal
    if !c.s.jump {
        return false;
    }
    c.p.near_ground_time = 0.0;
    // J8: slide-jump speed rules
    if c.p.sliding && c.p.slide_long_jump {
        let v = &mut c.p.vel;
        let h2 = v.x * v.x + v.y * v.y;
        if h2 > 0.0 {
            let s = h2.sqrt();
            let mut sc = s;
            // jumping within 0.4 s of the slide start (still in the crouch transition) takes the boost back
            if c.p.duck_state != 2 && c.p.last_slide_time > c.now - 0.4 {
                let ns = (s - c.p.last_slide_boost).max(0.0);
                *v *= ns / s;
                sc = ns;
            }
            // 85..100 % of slideMaxJumpSpeed snaps up to it
            let b = c.s.slide_max_jump_speed;
            if h2 > ((b * 0.85) * b) * 0.85 && b * b > h2 {
                *v *= b / sc;
            }
            c.p.slide_long_jump = false;
        }
    }
    // J9 wall-jump flag and normal height
    let wall_jump = !upright(c.p.surface_normal);
    let mut h = if c.p.sliding { c.s.slide_jump_height } else { c.s.jump_height };
    let t = c.now - c.p.last_land_time;
    if c.cv.skip_time > t && upright(c.p.surface_normal) {
        // bunny-hop "skip": jumping within 1 s of landing bleeds speed above the cap (sprint speed)
        let v = &mut c.p.vel;
        let h2 = (v.y * v.y + v.x * v.x) + 0.0;
        if h2 > c.cv.skip_jump_height_speed * c.cv.skip_jump_height_speed {
            h *= c.cv.skip_jump_height_fraction;
        }
        let mut cap = c.cv.skip_speed_retain;
        if 0.0 > cap {
            cap = c.p.speed_scale * c.s.stand.sprintspeed;
        }
        if h2 > cap * cap {
            let s = h2.sqrt();
            let inv = 1.0 / s;
            let a = s - c.cv.skip_speed_reduce;
            let ns = if cap > a { cap } else { a };
            v.x = ((ns * v.x) * inv) + (v.x - v.x);
            v.y = ((v.y * ns) * inv) + (v.y - v.y);
        }
    }
    if c.s.anti_multi_jump_time_max > t && c.p.air_from_jump {
        // anti multi-jump: height fades back in after landing from a jump
        let lo = c.s.anti_multi_jump_time_min;
        let k = if lo > t { 1.0 } else { (t - c.s.anti_multi_jump_time_max) / (lo - c.s.anti_multi_jump_time_max) };
        h *= 1.0 - ((1.0 - c.s.anti_multi_jump_height_frac) * k);
    }
    // J10 apply
    ground::set_ground(c, None);
    let js = ((c.cv.sv_gravity + c.cv.sv_gravity) * h).sqrt();
    c.p.jumped = true;
    if wall_jump {
        wall::wall_jump(c);
    } else {
        c.p.wall.count = 0;
        c.p.wall.no_boost = false;
        c.p.vel += UP * js;
        let cp = c.p.contact_plane;
        if 0.0 > cp.dot(c.wish) {
            c.p.vel -= cp * 30.0; // push off what you lean into
        }
        // LedgeAssist 0x140baca70 (jump assist target search): not ported
    }
    // FIN
    duck::stop_sliding(c);
    apply_half_gravity(c, c.ft);
    check_velocity(c);
    c.p.last_jump_time = c.now;
    // S21 jump 0x140a3b550 tail: pl+0x3670 = now, lurch strength 1, tap count 0
    c.p.grace.mult = 1.0;
    c.p.grace.taps = 0;
    c.p.air_from_jump = true;
    true
}

/// Stance block GetAccelerate / Friction read (S21 ctx+0x20 / ctx+0x28): the one latched at PlayerMove entry,
/// before Duck. On the tick the stance changes it is the old one, while GetMaxSpeed already uses the new one
/// (real: a slide that ends at 167 u/s reaches 299 in one tick on the crouch stance's acceleration 2500; ducking at
/// walk speed decelerates at the standing 1250, not the crouch 0.6 x 2500).
fn move_stance(c: &Ctx) -> crate::settings::Stance {
    if c.move_ducked { c.s.crouch } else { c.s.stand }
}

/// GetAccelerate, S21 0x140a36450 (ds 0x140b8c5d0; slide branch from slide.md 2F): slideAccel while sliding,
/// sprintAcceleration above walk speed - 1, lowAcceleration below lowSpeed, else acceleration; all x pl+0x1e00 (1)
/// x speed scale. Stance numbers from `move_stance`.
pub(super) fn get_accelerate(c: &Ctx) -> f32 {
    let sc = c.p.speed_scale;
    if c.p.sliding {
        return c.s.slide_accel * sc;
    }
    let st = move_stance(c);
    let v = c.p.vel;
    let h2 = v.x * v.x + v.y * v.y;
    if st.sprint_acceleration >= 0.0 {
        let cap = sc * st.speed - 1.0;
        if h2 > cap * cap {
            return st.sprint_acceleration * sc;
        }
    }
    let low = sc * st.low_speed;
    if low * low > h2 && st.low_acceleration >= 0.0 {
        return st.low_acceleration * sc;
    }
    st.acceleration * sc
}

/// Friction, S21 0x140a4a280 with its deceleration getter 0x140a36580 (ds 0x140b9c7c0). Velocity along the wish direction (up to the wish speed) is not slowed at all; the
/// rest decelerates linearly at the stance's deceleration (sprintDeceleration above walk speed, the slide
/// decels while sliding, which also decay by slideVelocityDecay^dt).
fn friction(c: &mut Ctx, wish: Vec3, redirect: f32) {
    let v = c.p.vel;
    let n = UP;
    // 1. the part along n is kept
    let d = n.dot(v);
    let mut g = n * d;
    let mut r = v - g;
    // 2. wish redirect
    let dw = r.dot(wish);
    if dw > 0.0 {
        let t = ((dw * redirect) / r.length_squared()).min(1.0);
        let pl = c.p.contact_plane;
        let side = pl.dot(wish);
        let a = if side >= 0.0 {
            wish * (t * dw)
        } else {
            let a = pl * (0.0078125 - side) + wish;
            let a = a / a.length().max(EPS);
            let e = a.dot(r).max(0.0) * t;
            a * e
        };
        g += a;
        r -= a;
    }
    // 3. which deceleration (0x140a36580; stance numbers from `move_stance`)
    let speed = r.length();
    let st = move_stance(c);
    let mut fric = -1.0f32;
    let cap = c.p.speed_scale * st.speed;
    if v.length_squared() > cap * cap {
        if st.sprint_deceleration >= 0.0 {
            fric = st.sprint_deceleration;
        } else if st.sprint_acceleration >= 0.0 {
            fric = st.sprint_acceleration;
        }
    }
    if c.p.sliding {
        fric = if c.buttons & IN_DUCK != 0 {
            let dd = v.dot(c.wish);
            if 0.0 > dd { c.s.slide_want_to_stop_decel } else { c.s.slide_decel }
        } else {
            c.s.slide_want_to_stop_decel
        };
    }
    if 0.0 > fric {
        fric = st.deceleration;
        if !(fric >= 0.0) {
            fric = st.acceleration * 0.6;
        }
    }
    // 4. time ramp (pl+0x6258 unused here: 1) 5. drop
    let drop = (fric * c.p.surface_friction) * c.ft;
    let mut next = (speed - drop).max(0.0);
    if c.p.sliding {
        next *= c.s.slide_velocity_decay.powf(c.ft);
    }
    // 6.
    if !(next >= speed) {
        let k = next / speed;
        r *= k;
        c.p.vel = g + r;
    }
}

/// Accelerate, S21 0x140a4a0d0 (ds 0x140b9c610): linear (u/s^2) acceleration toward the wish speed, never past max(|v|, wishspeed).
fn accelerate(c: &mut Ctx, dir: Vec3, ws: f32, accel: f32) {
    let v = c.p.vel;
    let max2 = v.length_squared().max(ws * ws);
    let add = ws - dir.dot(v);
    if 0.0 >= add {
        return;
    }
    let a = add.min((accel * c.ft) * c.p.surface_friction);
    let mut nv = dir * a + v;
    let n2 = nv.length_squared();
    if n2 > max2 {
        nv *= (max2 / n2).sqrt();
    }
    c.p.vel = nv;
}

/// WalkMove, S21 0x140a4a7c0 (ds 0x140b9cdb0, walkmove.md): the on-surface move. S21's tail is the ds one for a stock
/// pilot (base velocity / ride-along pushes, forced-slide pl+0x1dfc and moving ground are not ported). Upright: the ground walk (Friction + Accelerate).
/// On a wall: the wallrun/climb branch (7W), then the shared tail (trace, StepSlide, SurfaceTransition).
fn walk_move(c: &mut Ctx, dt: f32) {
    if upright(c.p.surface_normal) {
        ground_walk(c);
    } else {
        wall_walk(c);
    }

    // step 8 TAIL (no base velocity / pushers)
    let n = c.p.surface_normal;
    let spd = c.p.vel.length();
    if c.p.sliding && c.s.slide_stop_speed > spd {
        duck::stop_sliding(c);
    }
    if spd < 1.0 {
        c.p.vel = Vec3::ZERO;
        return;
    }
    let o = c.p.pos;
    let mut m = c.p.vel;
    let q = c.p.ground_normal;
    let has_q = q.length_squared() != 0.0;
    if has_q && upright(n) {
        // redirect the (horizontal) velocity along the ground plane, keeping its length
        let qm = q.dot(m);
        let qn = q.dot(n);
        let cc = m * qn - n * qm;
        let rr = m.length_squared() / cc.length_squared();
        let s = rr.min(f32::MAX).sqrt();
        m = cc * s;
    }
    let mut e = m * dt + o;
    if has_q {
        let off = (e - o).dot(q);
        if 0.0078125 > off {
            e += q * (0.0078125 - off);
        }
    }
    c.p.contact_plane = Vec3::ZERO;
    let mut tr = c.trace(o, e);
    if tr.startsolid {
        c.p.start_solid = true;
    }
    if tr.fraction == 1.0 {
        c.p.pos = tr.endpos;
    } else {
        let ov = c.p.vel;
        // S21 WalkMove 0x140a4a7c0 tail (ds walkmove.md 8): a blocking plane steeper than 60 deg (0 < n.z < 0.5) hit while on
        // upright ground is replaced by PlaneFromNormal (S21 0x140a4a720, ds 0x140818280): a vertical wall, so
        // StepSlide does not ride up it
        if 0.5 > tr.normal.z && tr.normal.z > 0.0 && has_q && upright(n) {
            tr.normal = plane_from_normal(tr.normal, q);
        }
        if !step::step_slide(c, &mut tr, true, Some(e), false) {
            let o = c.p.pos;
            if (o - tr.endpos).dot(tr.normal) > -0.03125 {
                c.p.contact_plane = tr.normal;
            }
            wall::surface_transition(c, tr.endpos, tr.normal, Some(&tr), ov);
        }
    }
    ground::stay_on_ground(c);
}

/// PlaneFromNormal, S21 0x140a4a720: the horizontal unit normal (-(a x b).y, (a x b).x, 0) of hit plane `a` against
/// ground plane `b` (MAX(len, eps) as the binary; z is 0 * (1/len)).
fn plane_from_normal(a: Vec3, b: Vec3) -> Vec3 {
    let y = b.z * a.y - a.z * b.y;
    let x = -(a.z * b.x - b.z * a.x);
    let l = (x * x + y * y + 0.0).sqrt().max(EPS);
    let k = 1.0 / l;
    Vec3::new(x * k, y * k, k * 0.0)
}

/// WalkMove steps 1-7G, upright: ground input (sprint x1.25, soft clamp), Friction, Accelerate.
fn ground_walk(c: &mut Ctx) {
    let n = UP;
    // step 1: strip velocity along n
    let d = n.dot(c.p.vel);
    c.p.vel -= n * d;
    // step 2: surface basis (upright: the flattened view basis)
    let (f, r) = (c.flat_f, c.flat_r);
    // step 4: ground input, sprint x1.25, soft clamp, side/back scales
    let sc = if c.p.sprinting { 1.25 } else { 1.0 };
    let mut side = c.side * sc;
    let mut fwd = c.fwd * sc;
    let m2 = side * side + fwd * fwd;
    if m2 > 1.0 {
        let k = 1.0 / m2.sqrt();
        side *= k;
        fwd *= k;
    } else if m2 > 0.0 {
        let q = m2.sqrt();
        let k = ((2.0 - q) * q) / q;
        side *= k;
        fwd *= k;
    }
    side *= c.s.speed_scale_side;
    if 0.0 > fwd {
        fwd *= c.s.speed_scale_back;
    }
    let w = f * fwd + r * side;
    // step 6: onto the surface
    let dw = n.dot(w);
    let mut ww = w - n * dw;
    // 7G: while sliding the wish loses its component along the velocity (you steer, you don't push)
    // (S21 0x140a4a7c0 operand order: the residual is float noise whose sign decides whether Accelerate pulls
    // the slide back by up to slideAccel * ft, so it has to round like the binary)
    if c.p.sliding {
        let v = c.p.vel;
        let dv = ((ww.x * v.x) + (ww.y * v.y)) + (ww.z * v.z);
        if dv > 0.0 {
            let i = 1.0 / (((v.x * v.x) + (v.y * v.y)) + (v.z * v.z));
            ww = Vec3::new(ww.x - ((v.x * dv) * i), ww.y - ((v.y * dv) * i), ww.z - ((v.z * dv) * i));
        }
    }
    let l = (((ww.x * ww.x) + (ww.y * ww.y)) + (ww.z * ww.z)).sqrt();
    let ws = l.min(1.0) * c.max_speed;
    let inv = 1.0 / l.max(EPS);
    let dir = Vec3::new(ww.x * inv, ww.y * inv, ww.z * inv);
    let acc = get_accelerate(c);
    friction(c, dir, ws);
    accelerate(c, dir, ws, acc);
}

/// MIN(1, x) in the asm operand order (MINSS 1, x)
fn min1(x: f32) -> f32 {
    if 1.0 < x { 1.0 } else { x }
}

/// copysign(1, x)
fn sgn(x: f32) -> f32 {
    1.0f32.copysign(x)
}

/// WalkMove steps 1-7W, on a wall (walkmove.md 7W): the surface basis from the view, raw input with the
/// upward auto-push, then separate vertical and horizontal accelerate/decelerate. Climbing (CanWallClimb and
/// pushing up the wall) uses the climb table: up-speed climbSpeedStart -> climbSpeedEnd over climbHeight above
/// WallTop, climbAccel / climbDecel, climbHorz*. Otherwise the wallrun table with the slip ramp.
fn wall_walk(c: &mut Ctx) {
    let n = c.p.surface_normal;
    let s = c.s;
    // step 1: strip velocity along n
    let v = c.p.vel;
    let d = ((n.y * v.y) + (v.x * n.x)) + (n.z * v.z);
    c.p.vel = Vec3::new(v.x - n.x * d, v.y - n.y * d, v.z - n.z * d);
    // step 2: surface basis: F up the wall, R along it (P2 when the view's right is along the wall, else P1)
    let (f0, r0) = (c.view_f, c.view_r);
    let b = ((n.x * r0.x) + (n.y * r0.y)) + (n.z * r0.z);
    let (f, r) = if !(0.7 > b.abs()) {
        let cc = Vec3::new(n.z * f0.y - n.y * f0.z, n.x * f0.z - n.z * f0.x, n.y * f0.x - n.x * f0.y);
        let l = (((cc.y * cc.y) + (cc.x * cc.x)) + (cc.z * cc.z)).sqrt();
        let k = 1.0 / l.max(EPS);
        let r = Vec3::new(k * cc.x, k * cc.y, k * cc.z);
        let f = Vec3::new(n.y * r.z - n.z * r.y, n.z * r.x - n.x * r.z, n.x * r.y - n.y * r.x);
        (f, r)
    } else {
        let cc = Vec3::new(n.y * r0.z - n.z * r0.y, n.z * r0.x - n.x * r0.z, n.x * r0.y - n.y * r0.x);
        let l = (((cc.y * cc.y) + (cc.x * cc.x)) + (cc.z * cc.z)).sqrt();
        let k = 1.0 / l.max(EPS);
        let f = Vec3::new(k * cc.x, k * cc.y, k * cc.z);
        let r = Vec3::new(f.y * n.z - f.z * n.y, f.z * n.x - f.x * n.z, f.x * n.y - f.y * n.x);
        (f, r)
    };
    // step 4: wall input: raw, plus wallrun_upwardAutoPush of forward along the wall's horizontal tangent
    let mut fwd = c.fwd;
    let mut side = c.side;
    let a = -n.x;
    let l = (((n.y * n.y) + (a * a)) + 0.0).sqrt();
    let k = 1.0 / l.max(EPS);
    let tf = (((k * a) * f.y) + ((n.y * k) * f.x)) + ((k * 0.0) * f.z);
    let up = ((if fwd > 0.0 { fwd } else { 0.0 }) * tf) * c.cv.wallrun_upward_auto_push;
    let pr = (if tf * side > 0.0 { tf * side } else { 0.0 }) * fwd;
    side += up;
    fwd += pr;
    if (r.z < -0.01 && side > 0.0) || (r.z > 0.01 && side < 0.0) {
        side = 0.0;
    }
    let w = Vec3::new(f.x * fwd + r.x * side, f.y * fwd + r.y * side, f.z * fwd + r.z * side);
    // step 6: onto the surface
    let dd = ((n.x * w.x) + (n.y * w.y)) + (n.z * w.z);
    let ww = Vec3::new(w.x - n.x * dd, w.y - n.y * dd, w.z - n.z * dd);

    // step 7W
    let hang_k = 1.0; // WallHangSlipScale: never hanging
    let climb = wall::can_wall_climb(c) && ww.z > 0.0;
    let el = c.now - c.p.wall.start_time;
    let mut r_slip = (el - c.cv.wallrun_slipstarttime) / c.cv.wallrun_slipduration;
    r_slip = r_slip.max(0.0).min(1.0);
    if fwd.abs() < 0.1 && side.abs() < 0.1 {
        let x = c.cv.wallrun_no_input_slip_frac;
        r_slip = if r_slip > x { r_slip } else { x };
    }
    let m = UP; // mv+0xa8 up axis
    let ext = 1.0; // pl+0x6674 (1.0 from the constructor)
    let (q, aa, dacc, bb, cch, e, fs, gs) = if climb {
        let ba8 = s.climb_height;
        let x = ((wall::wall_top(c) + ba8) - c.p.pos.z) / ba8;
        let x = x.max(0.0).min(1.0);
        let aa = ((1.0 - x) * s.climb_speed_end) + (x * s.climb_speed_start);
        // WallClimbUpdate 0x140bf0e90 runs here: animation only
        (0.0, aa, s.climb_accel, s.climb_decel, s.climb_horz_decel, s.climb_horz_speed, s.climb_horz_speed, s.climb_horz_accel)
    } else {
        let q = wall::wall_speed_frac(c, el);
        (
            q,
            (1.0 - q) * s.wallrun_max_speed_vertical,
            s.wallrun_accelerate_vertical,
            ext * s.wallrun_decelerate_vertical,
            ext * s.wallrun_decelerate_horizontal,
            s.wallrun_max_speed_horizontal,
            s.wallrun_max_speed_horizontal_backward,
            s.wallrun_accelerate_horizontal,
        )
    };
    // prelude: pl+0x6258 time ramp is 0 (ramp 1)
    let rr = 1.0 * c.p.surface_friction;
    let mut dv_dec = (rr * bb) * c.ft;
    let ch = cch * rr;
    let ads = 1.0; // ADS move-speed scale: no weapons
    let u = ads * c.p.speed_scale;
    // vertical
    if ww.z.abs() > 1e-4 {
        let l = ((ww.z * ww.z) + 0.0).sqrt();
        let mn = min1(l);
        let k = 1.0 / l.max(EPS);
        let vd = Vec3::new(k * 0.0, k * 0.0, k * ww.z);
        let vws = if s.wallstick_enabled { 0.0 } else { mn * ((u * aa) * hang_k) };
        let v = c.p.vel;
        let dv = ((vd.x * v.y) + (vd.x * v.x)) + (v.z * vd.z);
        let apply = if dv < 0.0 {
            dv_dec *= (1.0 - q) * (1.0 - r_slip);
            true
        } else {
            dv > vws
        };
        if apply {
            let g = sgn(dv) * dv_dec;
            c.p.vel = Vec3::new(v.x - (g * m.x), v.y - (g * m.y), v.z - (g * m.z));
        }
        accelerate(c, vd, vws, (1.0 - r_slip) * dacc);
    } else {
        let v = c.p.vel;
        let dm = ((v.x * m.x) + (v.y * m.y)) + (v.z * m.z);
        if dm < 0.0 {
            dv_dec *= (1.0 - q) * (1.0 - r_slip);
        }
        c.p.vel = if dv_dec > dm.abs() {
            Vec3::new(v.x - (m.x * dm), v.y - (m.y * dm), v.z - (m.z * dm))
        } else {
            let g = sgn(dm) * dv_dec;
            Vec3::new(v.x - (m.x * g), v.y - (m.y * g), v.z - (m.z * g))
        };
    }
    let dh = if s.wallstick_enabled { 100.0 } else { ch * c.ft };
    // horizontal
    let h2 = ((ww.x * ww.x) + (ww.y * ww.y)) + 0.0;
    if h2 > 1e-4 {
        let hl = h2.sqrt();
        let mn = min1(hl);
        let k = 1.0 / hl.max(EPS);
        let hd = Vec3::new(k * ww.x, k * ww.y, k * 0.0);
        let mut hws = if s.wallstick_enabled {
            0.0
        } else {
            // mv+0x3c..0x40: the view forward (backward along the wall caps at the backward speed)
            let dd = ((c.view_f.y * hd.y) + (c.view_f.x * hd.x)) + (hd.z * 0.0);
            if dd > -0.6 { mn * e } else { mn * fs }
        };
        hws *= u * hang_k;
        let k2 = 1.0 / (((hd.y * hd.y) + (hd.x * hd.x)) + 0.0).sqrt().max(EPS);
        let v = c.p.vel;
        let dhv = ((v.y * (k2 * hd.y)) + (v.x * (k2 * hd.x))) + (v.z * (k2 * 0.0));
        if dhv < 0.0 || dhv > hws {
            let g = sgn(dhv) * dh;
            c.p.vel = Vec3::new(v.x - (k2 * hd.x) * g, v.y - (k2 * hd.y) * g, v.z - g * (k2 * 0.0));
        }
        accelerate(c, hd, hws, (gs * 1.0 /* pl+0x5af0 */) * u);
    } else {
        let v = c.p.vel;
        let dm = ((v.x * m.x) + (m.y * v.y)) + (m.z * v.z);
        let p = Vec3::new(v.x - m.x * dm, v.y - m.y * dm, v.z - m.z * dm);
        let pl = (((p.y * p.y) + (p.x * p.x)) + (p.z * p.z)).sqrt();
        c.p.vel = if dh < pl {
            let f = dh / pl;
            Vec3::new(v.x - (f * p.x), v.y - (f * p.y), v.z - (f * p.z))
        } else {
            v - p
        };
    }
}

/// AirAccelerate, S21 0x140a485e0 (ds 0x140b9b510). Below the wish speed along the wish direction: plain accel*dt. Otherwise the
/// air-control step that turns without gaining speed.
fn air_accelerate(c: &mut Ctx, dir: Vec3, ws: f32, accel: f32, dt: f32) {
    let v = c.p.vel;
    let add = ws - (dir.y * v.y + dir.x * v.x + dir.z * v.z);
    if !(0.0 >= add) {
        let a = add.min(accel * dt);
        c.p.vel = v + dir * a;
        return;
    }
    let max2 = v.length_squared().max(ws * ws);
    // (dir * ft) * player_extraairaccelleration, gpGlobals frametime (not the dt argument)
    let mut nv = (dir * c.ft) * c.cv.air_control + v;
    let n2 = nv.length_squared();
    if n2 > max2 {
        nv *= (max2 / n2).sqrt();
    }
    c.p.vel = nv;
}

/// Velocity-based multi-tap decay, S21 client 0x140a48810: how much of the lurch strength multiplier one lurch
/// at horizontal speed `vh` eats. t = fminf(vh, max_speed) / max_speed, eased, times max_decay (0 = off, default).
fn grace_velocity_decay(cv: &crate::settings::Cvars, vh: f32) -> f32 {
    let m = cv.jump_grace_multi_tap_decay_velocity_based_max_decay;
    if m <= 0.0 {
        return 0.0;
    }
    let ms = cv.jump_grace_multi_tap_decay_velocity_based_max_speed;
    let t = vh.min(ms) / ms;
    match cv.jump_grace_multi_tap_decay_velocity_based_easing_type {
        // easeInSine: the binary evaluates sin(t*pi/2 + pi/2) with its own polynomial; = cos(t*pi/2)
        1 => (m - (t * std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_2).sin() * m) + 0.0,
        // easeInCirc
        2 => 0.0 - ((1.0 - t * t).sqrt() - 1.0) * m,
        3 => t * m + 0.0,
        // step: returns 1.0 (not max_decay) above max_speed, as the binary does
        4 => {
            if ms < vh {
                1.0
            } else {
                0.0
            }
        }
        // 0 and anything else: easeInCubic
        _ => t * t * t * m + 0.0,
    }
}

/// Jump grace: the lurch / tap-strafe. S21 client C_GameMovement::AirMove_TapStrafe 0x140a48a30 (r5apex_dx12.exe),
/// called by AirMove 0x140a48f90 right after AirAccelerate. A move key pressed within jump_graceperiodmax of the
/// jump bends the horizontal velocity toward the held keys (camera-relative, opposites cancel), keeping the speed
/// unless the change exceeds jump_grace_max x sprint speed, which caps it (that cap is what costs speed on wide
/// lurches). Differences from the 2019 server's keyboard grace (jump_keyboardgrace*, the previous port):
/// cutoff speed, tap count / min period / multi-tap and velocity decay, the optional limit-to-90 back rule,
/// analog input when require_tap is off, and no velocity change when the blended direction is shorter than 0.01.
fn jump_grace(c: &mut Ctx) {
    let cv = c.cv;
    // R5Flowstate SDK client hook Hook_AirMove_TapStrafe (mantle_boost.cpp): a full mantle boost (state 4) skips
    // this whole function until landing. The S21 binary has no such gate; R5Flowstate (the reference) does.
    {
        let mb = &mut c.p.mantle_boost;
        if mb.state == 4 && c.p.last_land_time > mb.applied_time {
            mb.state = 0;
        }
        if cv.mantle_boost_enabled && cv.mantle_boost_disables_tap_strafes && mb.state == 4 {
            return;
        }
    }
    const MOVE_KEYS: u32 = IN_FORWARD | IN_BACK | IN_MOVELEFT | IN_MOVERIGHT; // 0x618
    // mv+0x2c: keys newly pressed this tick
    if c.pressed & MOVE_KEYS == 0 && cv.jump_grace_require_tap {
        return;
    }
    let now = c.now;
    // (now - pl+0x46e0 last controller input >= jump_grace_controller_disable_duration: keyboard only here, and the
    // default -1 always passes, so the gate is left out)
    let t0 = if cv.jump_grace_jump { c.p.last_jump_time } else { c.p.last_ground_time }; // pl+0x3670 / 0x366c
    let el = now - t0;
    let pmax = cv.jump_graceperiodmax;
    if !(el < pmax && cv.jump_grace_min_tap_period < now - c.p.grace.last_tap && c.p.grace.taps < cv.jump_grace_max_tap_count) {
        return;
    }
    let v = c.p.vel;
    let vh = (v.x * v.x + v.y * v.y + 0.0).sqrt();
    let vk = 1.0 / vh.max(EPS);
    let (ux, uy) = (v.x * vk, v.y * vk);
    if !(vh <= cv.jump_grace_cutoff_speed) {
        return;
    }
    c.p.grace.last_tap = now;
    // the push: held keys (mv+0x24) with require_tap, else the analog move (mv+0x30 / 0x34)
    let (kf, ks) = if cv.jump_grace_require_tap {
        let b = c.buttons;
        let f = (if b & IN_FORWARD != 0 { 1.0 } else { 0.0 }) - (if b & IN_BACK != 0 { 1.0 } else { 0.0 });
        let s = (if b & IN_MOVERIGHT != 0 { 1.0 } else { 0.0 }) - (if b & IN_MOVELEFT != 0 { 1.0 } else { 0.0 });
        (f, s)
    } else {
        (c.fwd, c.side)
    };
    let (f, r) = (c.flat_f, c.flat_r); // mv+0x60 / 0x6c
    let kx = kf * f.x + ks * r.x;
    let kz = kf * f.z + ks * r.z;
    let ky = kf * f.y + ks * r.y;
    let kl = 1.0 / (ky * ky + kx * kx + kz * kz).sqrt().max(EPS); // a null input (A+D) stays 0
    let mut wx = kl * kx;
    let mut wy = kl * ky;
    if cv.jump_grace_limit_to_90 {
        let d = wy * uy + wx * ux + kl * kz * vk * 0.0;
        if d < 0.0 {
            // drop the part of the push that points against the velocity, then renormalise (rsqrtss + one
            // Newton step in the binary; 1/sqrt here)
            let nd = -d;
            wx += ux * nd;
            wy += uy * nd;
            let wz = nd * vk * 0.0 + kl * kz;
            let l2 = wx * wx + 1.1920929e-7 + wy * wy + wz * wz;
            let k = 1.0 / l2.sqrt();
            wy *= k;
            wx *= k;
        }
    }
    c.p.grace.mult *= 1.0 - grace_velocity_decay(&cv, vh);
    let mut ramp = ((el - pmax) / (cv.jump_graceperiodmin - pmax)).max(0.0);
    if 1.0 <= ramp {
        ramp = 1.0;
    }
    let s = ramp * c.p.grace.mult * cv.jump_grace_strength;
    let nx = (wx - ux) * s + ux;
    let ny = (wy - uy) * s + uy;
    let nl = (ny * ny + nx * nx).sqrt();
    if !(0.01 < nl) {
        return;
    }
    c.p.grace.taps += 1;
    if cv.jump_grace_decay_start_tap_count <= c.p.grace.taps {
        c.p.grace.mult *= cv.jump_grace_multi_tap_decay;
    }
    let inv = 1.0 / nl;
    // 0x140b09c00(pl, 0) = pl+0x3278 speed scale x sprintspeed of stance 0
    let cap = (c.p.speed_scale * c.s.stand.sprintspeed) * cv.jump_grace_max;
    let mut dx = vh * nx * inv - v.x;
    let mut dy = vh * ny * inv - v.y;
    let d2 = dy * dy + dx * dx;
    if cap * cap < d2 {
        let q = cap / d2.sqrt();
        dx *= q;
        dy *= q;
    }
    c.p.vel = Vec3::new(v.x + dx, v.y + dy, v.z);
    // 0x140a58ec0 -> 0x140a58560 (jump assist toward a ledge target): not ported, as in the S3 port
}

/// AirMove, S21 0x140a48f90 (ds 0x140b9b740, airmove.md). Same numbers for a stock pilot: wish from mv+0x78,
/// airSpeed x min(|wish|, 1) (0 with airStrafeEnabled), airDrag/airFriction, AirAccelerate with airAcceleration,
/// then the lurch. Not ported (not stock): slipAirRestrictDuration, jetpack pitch movement and post-effect drag,
/// grapple swing, the pl+0x4960 launch fall limiter, base-velocity push.
fn air_move(c: &mut Ctx, dt: f32) {
    let now = c.now;
    // steps 1-4 (no slip / jetpack / grapple): wish direction from mv+0x78, keeping its analog length
    let w = c.wish;
    let l = w.length();
    let mut mn = l.min(1.0);
    let dir = w * (1.0 / l.max(EPS));
    // step 5: air speed / accel, air drag (airDrag/airFriction are 0 for pilots)
    let sp = if c.s.air_strafe_enabled { 0.0 } else { c.s.air_speed };
    mn *= sp;
    let v = c.p.vel;
    let h = (v.y * v.y + v.x * v.x).sqrt();
    if h > 0.0 {
        // S21: (((drag*h + friction) * ramp) * h) * dt, ramp (pl+0x2a50) = 1; S21 picks jetpackPostEffectDrag
        // instead of airDrag right after a jetpack (0x140b00f50), not a stock-pilot path
        let dd = (((h * c.s.air_drag) + c.s.air_friction) * h) * dt;
        let f = (h - dd).max(0.0) / h;
        c.p.vel = Vec3::new((v.x - v.x) + f * v.x, (v.y - v.y) + f * v.y, v.z);
    }
    // step 6
    let ws = (1.0 * c.p.speed_scale) * mn;
    air_accelerate(c, dir, ws, c.s.air_acceleration, dt);

    // step 7: jump grace (lurch / tap-strafe), S21 client
    jump_grace(c);

    // step 9: trace end, contact plane
    let o = c.p.pos;
    let v = c.p.vel;
    let e = o + v * dt;
    c.p.contact_plane = Vec3::ZERO;
    // step 10
    let mut tr = c.trace(o, e);
    if tr.startsolid {
        c.p.start_solid = true;
    }
    // step 11: unobstructed; a deferred wall attach (the classifier asked to retry) fires here
    if tr.fraction == 1.0 {
        c.p.pos = tr.endpos.clamp(Vec3::splat(-65535.0), Vec3::splat(65535.0));
        let t = c.p.wall.retry_time;
        if t != 0.0 && now >= t {
            c.p.wall.retry_time = 0.0;
            let nn = c.p.wall.retry_normal;
            let pp = c.p.wall.retry_pos;
            let o = c.p.pos;
            let d = (((o.y - pp.y) * nn.y) + ((o.x - pp.x) * nn.x)) + ((o.z - pp.z) * nn.z);
            if 1.0 > d.abs() {
                wall::surface_transition(c, o, nn, None, -nn);
            }
        }
        return;
    }
    // step 12: blocked: TryPlayerMove for steep falls into overhangs, else StepSlide
    let ov = c.p.vel;
    let nz = tr.normal.z;
    let a = (now - c.p.near_ground_time) > 0.1 && 0.0 > ov.z && 90000.0 > (ov.y * ov.y + ov.x * ov.x) && 0.7 > nz;
    let tpm = if !a {
        false
    } else if 0.0 < nz {
        let vz2 = ((0.0078125 - tr.normal.dot(ov)) * nz) + ov.z;
        0.0 > vz2
    } else {
        let e2 = tr.endpos - c.p.surface_normal * c.s.step_height;
        let tr2 = c.trace(tr.endpos, e2);
        if tr2.fraction >= 1.0 { true } else { 0.7 > tr2.normal.z }
    };
    if tpm {
        step::try_player_move(c, &mut tr);
    } else if step::step_slide(c, &mut tr, false, None, false) {
        return;
    }
    // step 13: stuck on a steep slope with no vertical progress: fake a ground contact
    let nz = tr.normal.z;
    let vz = c.p.vel.z;
    if nz > 0.0 && 0.7 > nz && c.p.pos.z == o.z && vz >= 0.0 && 0.013531647 >= vz {
        tr.normal = c.p.surface_normal;
        // S21 also caches this faked trace for CategorizePosition (ctx+0x4a0 / 0x620, end origin - 2n) and
        // remembers the origin (pl+0x369c)
        ground::remember_stall(c, &tr);
    }
    c.p.contact_plane = tr.normal;
    // SurfaceTransition: attach to the wall the move was blocked by
    wall::surface_transition(c, tr.endpos, tr.normal, Some(&tr), ov);
}
