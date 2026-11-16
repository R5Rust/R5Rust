//! Duck 0x140ba7500 (playermove.md 7 / slide.md 2C), CanUnduck, FinishUnDuck, SetDuckViewOffset,
//! StartSliding 0x140ba7ed0, StopSliding 0x140ba8250 (slide.md 2A/2B).

use super::{ground, Ctx, MoveType, IN_DUCK, UP};
use glam::Vec3;

fn clamp01(f: f32) -> f32 {
    if 0.0 > f { 0.0 } else { f.min(1.0) }
}

/// DuckFrac 0x1407b9cb0
fn duck_frac(ms: i32, sliding: bool) -> f32 {
    clamp01(ms as f32 / if sliding { 200.0 } else { 400.0 })
}

/// Duck 0x140ba7500: the 4-state crouch machine. The hull swaps when the 400 ms (200 ms when sliding) timer
/// runs out; only the eye height eases (smoothstep). A crouch press while fast enough starts a slide.
pub(super) fn duck(c: &mut Ctx) {
    let was = c.p.duck_state == 1 || c.p.duck_state == 2;
    // 7.1 toggle-crouch: not bound. 7.3 want
    let mut want = c.buttons & IN_DUCK != 0;
    let v = c.p.vel;
    let msp = c.s.slide_max_stop_speed;
    if c.p.sliding && (v.x * v.x + v.y * v.y) > msp * msp {
        want = true; // can't stand up out of a slide faster than slideMaxStopSpeed
    }
    let d = c.s.crouch_enabled && want;
    let mut fast = c.p.sliding;
    let go_duck = if d {
        true
    } else if can_unduck(c) {
        false
    } else {
        fast = true;
        true
    };
    if go_duck {
        // 7.4
        if !was {
            let gn = c.p.ground_normal;
            start_sliding(c, gn, false);
            if c.p.sliding {
                fast = true;
            }
        }
        if c.p.duck_state == 0 {
            c.p.duck_state = 1;
            c.p.duck_timer_ms = if fast { 200 } else { 400 };
            c.p.duck_in_air = !c.gnd();
        } else if c.p.duck_state == 3 {
            c.p.duck_state = 1;
            let f = clamp01((200 - c.p.duck_timer_ms) as f32 / 200.0);
            c.p.duck_timer_ms = ((if fast { 200.0 } else { 400.0 }) * f) as i32;
        }
        if c.p.duck_state == 1 {
            if c.p.duck_timer_ms <= 0 {
                let old = c.p.ducked;
                c.p.ducked = true;
                c.p.duck_state = 2;
                if !old && c.p.duck_in_air {
                    // ducked in the air: lift the feet so the head stays put
                    let (s, dk) = (&c.s.stand, &c.s.crouch);
                    let lift = ((s.hull_max - s.hull_min) - (dk.hull_max - dk.hull_min)) * 0.5;
                    c.p.pos += lift;
                }
                ground::categorize_position(c);
            }
            let f = clamp01(c.p.duck_timer_ms as f32 / if fast { 200.0 } else { 400.0 });
            set_duck_view_offset(c, 1.0 - ((3.0 - (f + f)) * (f * f)));
        }
    } else {
        // 7.5 unduck
        if c.p.duck_state == 2 {
            c.p.duck_state = 3;
            c.p.duck_timer_ms = 200;
            finish_unduck(c);
        } else if c.p.duck_state == 1 {
            c.p.duck_state = 3;
            c.p.duck_timer_ms = ((1.0 - duck_frac(c.p.duck_timer_ms, c.p.sliding)) * 200.0) as i32;
            if c.p.ducked {
                finish_unduck(c);
            }
        }
        if c.p.duck_state == 3 {
            if c.p.duck_timer_ms <= 0 {
                c.p.duck_state = 0;
            }
            let f = clamp01(c.p.duck_timer_ms as f32 / 200.0);
            set_duck_view_offset(c, (3.0 - (f + f)) * (f * f));
        }
    }
    // 7.6 tail: standing up ends the slide
    if !(c.p.duck_state == 1 || c.p.duck_state == 2) && was {
        stop_sliding(c);
    }
}

/// Crouched height minus standing height (negative), halved when the contact plane is exactly flat.
fn air_hull_delta(c: &Ctx) -> Option<Vec3> {
    let x = c.p.contact_plane.z;
    if 0.0 < x {
        return None;
    }
    let (s, dk) = (&c.s.stand, &c.s.crouch);
    let mut dlt = (dk.hull_max - dk.hull_min) - (s.hull_max - s.hull_min);
    if x == 0.0 {
        dlt *= 0.5;
    }
    Some(dlt)
}

/// CanUnduck 0x140bfd500: the standing hull must fit (in the air: lowered so the head stays, and not within
/// 2 units of the ground).
fn can_unduck(c: &Ctx) -> bool {
    if !c.p.ducked {
        return true;
    }
    if c.p.move_type == MoveType::Traverse {
        return false;
    }
    let mut s = c.p.pos;
    let mut e = s;
    if !c.gnd() {
        if let Some(d) = air_hull_delta(c) {
            s += d;
        }
        e = s - UP * 2.0;
    }
    let t = c.col.trace_hull(s, e, c.s.stand.hull_min, c.s.stand.hull_max);
    !t.startsolid && t.fraction == 1.0
}

/// FinishUnDuck 0x140ba6fa0
fn finish_unduck(c: &mut Ctx) {
    c.p.ducked = false;
    c.p.duck_state = 3;
    c.p.duck_in_air = false;
    if !c.gnd() {
        if let Some(d) = air_hull_delta(c) {
            c.p.pos += d;
            c.p.duck_in_air = true;
            ground::categorize_position(c);
        }
    }
}

/// SetDuckViewOffset 0x140ba7330: eye height between standing and crouched, `frac` 1 = crouched.
fn set_duck_view_offset(c: &mut Ctx, frac: f32) {
    let (s, dk) = (&c.s.stand, &c.s.crouch);
    let mut dz = dk.viewheight;
    let mut uz = s.viewheight;
    let sh = s.hull_max.z - s.hull_min.z;
    let dh = dk.hull_max.z - dk.hull_min.z;
    if c.p.duck_state != 2 && c.p.duck_in_air {
        if !c.p.ducked {
            dz += (sh - dh) * 0.5;
        } else if c.p.duck_state == 3 {
            uz += (dh - sh) * 0.5;
        }
    }
    c.p.view_z = ((1.0 - frac) * uz) + (dz * frac);
}

/// StartSliding 0x140ba7ed0 (slide.md 2A). Needs a walkable ground normal, slideRequiredStartSpeed (Air: 90
/// when landing while falling faster than 200), and on the ground moving within ~53 deg of the view. The
/// one-shot boost (+150, capped at 400) has a 2 s cooldown from the last slide start.
pub(super) fn start_sliding(c: &mut Ctx, ground_normal: Vec3, from_air: bool) {
    if 0.7 > ground_normal.z && !c.cv.slide_while_in_air {
        return;
    }
    if !c.s.slide {
        return;
    }
    let v = c.p.vel;
    let m2 = (v.y * v.y + v.x * v.x) + 0.0;
    let mut req = c.s.slide_required_start_speed;
    if from_air {
        let up = v.dot(UP);
        if !(up > -200.0) {
            req = c.s.slide_required_start_speed_air;
        }
    }
    if req * req > m2 {
        return;
    }
    if c.gnd() {
        let a = c.wish.dot(c.flat_f);
        if c.cv.slide_max_angle_dot > a {
            return;
        }
    }
    c.p.sliding = true;
    c.p.ducked = true;
    if c.s.slide_boost_enabled && (c.now - c.p.last_slide_time) > c.s.slide_boost_cooldown {
        let s = m2.sqrt();
        let sc = c.p.speed_scale;
        let a = (sc * c.s.slide_speed_boost) + s;
        let b = sc * c.s.slide_speed_boost_cap;
        let t = if a < b { a } else { b };
        if t > s {
            c.p.last_slide_boost = t - s;
            let k = t / s;
            c.p.vel.y *= k;
            c.p.vel.x *= k;
        } else {
            c.p.last_slide_boost = 0.0;
        }
        c.p.slide_long_jump = true;
    }
    c.p.last_slide_time = c.now;
}

/// StopSliding 0x140ba8250: only clears the flag (standing up is Duck's job).
pub(super) fn stop_sliding(c: &mut Ctx) {
    if !c.p.sliding {
        return;
    }
    c.p.sliding = false;
}
