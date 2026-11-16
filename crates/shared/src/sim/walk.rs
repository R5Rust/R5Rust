//! FullWalkMove and everything it calls on the ground and in the air
//! (fullwalkmove.md, walkmove.md, airmove.md; Friction/Accelerate/AirAccelerate/CheckVelocity from
//! R5Regenis native/r5r_engine/src/movement.cpp, GetAccelerate disassembled for this port).

use super::{duck, ground, step, Ctx, IN_BACK, IN_DUCK, IN_FORWARD, IN_JUMP, IN_MOVELEFT, IN_MOVERIGHT, UP};
use glam::Vec3;

/// The engine's MAX(len, eps) epsilon (runtime value; any tiny positive number behaves the same).
const EPS: f32 = 1e-8;

/// FullWalkMove 0x140b9ead0 (fullwalkmove.md section 2).
pub(super) fn full_walk_move(c: &mut Ctx) {
    let dt = c.ft; // TimeScale = 1 (no air slow-mo)
    // step 1: start gravity
    apply_half_gravity(c, dt);
    // step 2
    check_velocity(c);
    check_jump_button(c);
    // steps 3-4: wallrun / constraints / tethers / gravity-grenade fields: not in scope
    // step 5
    check_velocity(c);
    if c.gnd() {
        // step 6G: no slip triggers, minPush 0
        walk_move(c, dt);
    } else {
        // step 7A (EndWallRun not needed: never on a wall)
        air_move(c, dt);
    }
    // step 8 (ImpactCheck: sounds/events only)
    ground::categorize_position(c);
    apply_half_gravity(c, dt);
    // step 9: strip velocity along n when grounded
    if c.gnd() {
        let d = c.p.vel.dot(UP);
        c.p.vel -= UP * d;
    }
    // step 10 (CheckFalling: fall damage, not in scope)
    check_velocity(c);
}

/// ApplyHalfGravity / AddGravity 0x140b8c2a0 (fullwalkmove.md 3, slide.md 2E): half of gravity*dt per call,
/// plus the slope-parallel part of gravity while sliding on the ground.
pub(super) fn apply_half_gravity(c: &mut Ctx, dt: f32) {
    let gs = if c.s.gravity_scale == 0.0 { 1.0 } else { c.s.gravity_scale };
    let g = ((((gs * c.cv.sv_gravity) as f64) * 0.5) * dt as f64) as f32;
    let v = &mut c.p.vel;
    v.z -= g;
    if c.p.sliding && c.p.ground {
        let n = c.p.ground_normal;
        v.x += (g * n.x) * n.z;
        v.y += (g * n.y) * n.z;
    }
}

/// CheckVelocity 0x140ba3b60: NaN guard and sv_maxvelocity clamp (scales the whole vector, keeping direction).
pub(super) fn check_velocity(c: &mut Ctx) {
    let v = &mut c.p.vel;
    if (c.p.cv_cache - *v).length_squared() < 0.001 {
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

/// CheckJumpButton 0x140b8d490 (fullwalkmove.md 4, movemodes.md C). Ground jump only: this legend has no
/// double jump, dodge, boost or wallrun.
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
            if h2 > (b * 0.85) * (b * 0.85) && b * b > h2 {
                *v *= b / sc;
            }
            c.p.slide_long_jump = false;
        }
    }
    // J9 normal height
    let mut h = if c.p.sliding { c.s.slide_jump_height } else { c.s.jump_height };
    let t = c.now - c.p.last_land_time;
    if c.cv.skip_time > t {
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
    c.p.vel += UP * js;
    let cp = c.p.contact_plane;
    if 0.0 > cp.dot(c.wish) {
        c.p.vel -= cp * 30.0; // push off what you lean into
    }
    // LedgeAssist 0x140baca70 (jump assist target search): not ported
    // FIN
    duck::stop_sliding(c);
    apply_half_gravity(c, c.ft);
    check_velocity(c);
    c.p.last_jump_time = c.now;
    c.p.air_from_jump = true;
    true
}

/// GetAccelerate 0x140b8c5d0 (slide branch from slide.md 2F; the rest disassembled for this port):
/// sprintAcceleration above walk speed, lowAcceleration below lowSpeed, else acceleration.
pub(super) fn get_accelerate(c: &Ctx) -> f32 {
    let sc = c.p.speed_scale;
    if c.p.sliding {
        return c.s.slide_accel * sc;
    }
    let st = c.stance();
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

/// Friction 0x140b9c7c0. Velocity along the wish direction (up to the wish speed) is not slowed at all; the
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
    // 3. which deceleration
    let speed = r.length();
    let st = *c.stance();
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

/// Accelerate 0x140b9c610: linear (u/s^2) acceleration toward the wish speed, never past max(|v|, wishspeed).
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

/// WalkMove 0x140b9cdb0, ground path (walkmove.md; the wallrun path needs a wall normal and is out of scope).
fn walk_move(c: &mut Ctx, dt: f32) {
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
    if c.p.sliding {
        let v = c.p.vel;
        let dv = v.dot(ww);
        if dv > 0.0 {
            let i = 1.0 / v.length_squared();
            ww -= v * (dv * i);
        }
    }
    let l = ww.length();
    let dir = ww * (1.0 / l.max(EPS));
    let ws = l.min(1.0) * c.max_speed;
    let acc = get_accelerate(c);
    friction(c, dir, ws);
    accelerate(c, dir, ws, acc);

    // step 8 TAIL (no base velocity / pushers)
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
    if has_q {
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
    } else if !step::step_slide(c, &mut tr, true, Some(e), false) {
        let o = c.p.pos;
        if (o - tr.endpos).dot(tr.normal) > -0.03125 {
            c.p.contact_plane = tr.normal;
        }
        // SurfaceTransition (wallrun start): not in scope
    }
    ground::stay_on_ground(c);
}

/// AirAccelerate 0x140b9b510. Below the wish speed along the wish direction: plain accel*dt. Otherwise the
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
    let mut nv = dir * (c.ft * c.cv.air_control) + v;
    let n2 = nv.length_squared();
    if n2 > max2 {
        nv *= (max2 / n2).sqrt();
    }
    c.p.vel = nv;
}

/// AirMove 0x140b9b740 (airmove.md).
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
        let dd = ((h * c.s.air_drag) + c.s.air_friction) * (h * dt);
        let f = (h - dd).max(0.0) / h;
        c.p.vel = Vec3::new((v.x - v.x) + f * v.x, (v.y - v.y) + f * v.y, v.z);
    }
    // step 6
    let ws = (1.0 * c.p.speed_scale) * mn;
    air_accelerate(c, dir, ws, c.s.air_acceleration, dt);

    // step 7: jump keyboard grace (tap-strafe). R5Regenis tapstrafe.cpp masks the move-key presses here while a
    // full mantle boost is in the air (until landing) and above 1100 u/s horizontal (the client's
    // jump_grace_cutoff_speed).
    let mut pressed = c.pressed;
    {
        let mb = &mut c.p.mantle_boost;
        if mb.state == 4 && c.p.last_land_time > mb.applied_time {
            mb.state = 0;
        }
        let vh2 = c.p.vel.x * c.p.vel.x + c.p.vel.y * c.p.vel.y;
        if (c.cv.mantle_boost_enabled && c.cv.mantle_boost_disables_tap_strafes && mb.state == 4) || vh2 > c.cv.lurch_cutoff_speed * c.cv.lurch_cutoff_speed {
            pressed &= !(IN_FORWARD | IN_BACK | IN_MOVELEFT | IN_MOVERIGHT);
        }
    }
    if pressed & (IN_FORWARD | IN_BACK | IN_MOVELEFT | IN_MOVERIGHT) != 0 {
        let el = now - c.p.last_jump_time;
        let gmax = c.cv.jump_keyboardgraceperiodmax;
        if gmax > el {
            let b = c.buttons;
            let kf = (if b & IN_FORWARD != 0 { 1.0 } else { 0.0 }) - (if b & IN_BACK != 0 { 1.0 } else { 0.0 });
            let ks = (if b & IN_MOVERIGHT != 0 { 1.0 } else { 0.0 }) - (if b & IN_MOVELEFT != 0 { 1.0 } else { 0.0 });
            let v = c.p.vel;
            let k = c.flat_f * kf + c.flat_r * ks;
            let kk = 1.0 / k.length().max(EPS);
            let vh = (v.x * v.x + v.y * v.y).sqrt();
            let vk = 1.0 / vh.max(EPS);
            let (ux, uy) = (v.x * vk, v.y * vk);
            let gmin = c.cv.jump_keyboardgraceperiodmin;
            let r = ((el - gmax) / (gmin - gmax)).max(0.0).min(1.0) * c.cv.jump_keyboardgrace_strength;
            let mut nx = ((kk * k.x - ux) * r) + ux;
            let mut ny = ((kk * k.y - uy) * r) + uy;
            let nl = (ny * ny + nx * nx).sqrt();
            if nl > 0.01 {
                let cc = vh / nl;
                nx *= cc;
                ny *= cc;
            }
            let cap = (c.p.speed_scale * c.s.stand.sprintspeed) * c.cv.jump_keyboardgrace_max;
            let mut dx = nx - v.x;
            let mut dy = ny - v.y;
            let d2 = dy * dy + dx * dx;
            if d2 > cap * cap {
                let q = cap / d2.sqrt();
                dx *= q;
                dy *= q;
            }
            c.p.vel = Vec3::new(v.x + dx, v.y + dy, v.z);
            // JumpAssist 0x140baca70: not ported
        }
    }

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
    // step 11: unobstructed
    if tr.fraction == 1.0 {
        c.p.pos = tr.endpos.clamp(Vec3::splat(-65535.0), Vec3::splat(65535.0));
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
        let e2 = tr.endpos - UP * c.s.step_height;
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
        tr.normal = UP;
    }
    c.p.contact_plane = tr.normal;
}
