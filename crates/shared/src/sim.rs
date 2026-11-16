//! Deterministic player movement (Source units = inches, Z up). Same code will run on the client
//! (prediction) and the dedicated server (authority) once netcode lands in v2.

use crate::collision::{Collision, AXIS_LEN, RADIUS, WALKABLE_Z};
use glam::Vec3;

pub const WALK_SPEED: f32 = 190.0;
pub const SPRINT_SPEED: f32 = 300.0;
pub const GRAVITY: f32 = 750.0;
pub const JUMP_SPEED: f32 = 290.0;
pub const GROUND_ACCEL: f32 = 10.0;
pub const AIR_ACCEL: f32 = 1.5;
pub const FRICTION: f32 = 6.0;
pub const STOP_SPEED: f32 = 100.0;
/// Max height snapped down onto when walking off a step / down a slope.
pub const STEP_DOWN: f32 = 20.0;
/// Max ledge height walked up without jumping (Source/Apex: 18).
pub const STEP_UP: f32 = 18.0;
pub const EYE_HEIGHT: f32 = 60.0;
pub const PLAYER_HEIGHT: f32 = RADIUS * 2.0 + AXIS_LEN;

#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    /// -1..1, +1 = forward
    pub forward: f32,
    /// -1..1, +1 = right
    pub right: f32,
    pub jump: bool,
    pub sprint: bool,
    /// fly mode: descend
    pub down: bool,
    /// degrees, 0 = +X, 90 = +Y (Source convention)
    pub yaw: f32,
    /// degrees, + = looking down (Source convention)
    pub pitch: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Player {
    /// feet position
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub flying: bool,
    pub fly_speed: f32,
    jump_held: bool,
    /// seconds left in which a jump pressed just before landing still fires
    jump_buffer: f32,
}

impl Player {
    pub fn new(pos: Vec3) -> Self {
        Self { pos, vel: Vec3::ZERO, on_ground: false, flying: false, fly_speed: 1200.0, jump_held: false, jump_buffer: 0.0 }
    }
    pub fn eye(&self) -> Vec3 {
        self.pos + Vec3::new(0.0, 0.0, EYE_HEIGHT)
    }
}

pub fn forward_vec(yaw: f32, pitch: f32) -> Vec3 {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    Vec3::new(y.cos() * p.cos(), y.sin() * p.cos(), -p.sin())
}

fn accelerate(vel: &mut Vec3, wish_dir: Vec3, wish_speed: f32, accel: f32, dt: f32) {
    let cur = vel.dot(wish_dir);
    let add = wish_speed - cur;
    if add <= 0.0 {
        return;
    }
    let a = (accel * wish_speed * dt).min(add);
    *vel += wish_dir * a;
}

/// Finds a free spot at or above `pos` (spawn points can sit a hair inside the floor).
pub fn find_free(col: &Collision, pos: Vec3) -> Vec3 {
    for i in 0..64 {
        let p = pos + Vec3::new(0.0, 0.0, i as f32 * 4.0);
        if col.is_free(p) {
            return p;
        }
    }
    pos
}

/// Advances the player by `dt` seconds.
pub fn simulate(p: &mut Player, input: &Input, dt: f32, col: &Collision) {
    if input.jump && !p.jump_held {
        p.jump_buffer = 0.12;
    }
    p.jump_held = input.jump;
    let jump_pressed = p.jump_buffer > 0.0;
    p.jump_buffer = (p.jump_buffer - dt).max(0.0);

    if p.flying {
        let fwd = forward_vec(input.yaw, input.pitch);
        let right = forward_vec(input.yaw - 90.0, 0.0);
        let mut wish = fwd * input.forward + right * input.right;
        if input.jump {
            wish.z += 1.0;
        }
        if input.down {
            wish.z -= 1.0;
        }
        let speed = p.fly_speed * if input.sprint { 3.0 } else { 1.0 };
        let target = wish.normalize_or_zero() * speed;
        // snappy but not instant
        p.vel += (target - p.vel) * (1.0 - (-12.0 * dt).exp());
        let delta = p.vel * dt;
        col.move_capsule(&mut p.pos, delta, &mut p.vel);
        p.on_ground = false;
        return;
    }

    let fwd = forward_vec(input.yaw, 0.0);
    let right = forward_vec(input.yaw - 90.0, 0.0);
    let wish = fwd * input.forward + right * input.right;
    let wish_dir = wish.normalize_or_zero();
    let wish_speed = if input.sprint && input.forward > 0.0 { SPRINT_SPEED } else { WALK_SPEED } * wish.length().min(1.0);

    if p.on_ground {
        // friction
        let mut h = Vec3::new(p.vel.x, p.vel.y, 0.0);
        let speed = h.length();
        if speed > 0.01 {
            let drop = speed.max(STOP_SPEED) * FRICTION * dt;
            h *= ((speed - drop) / speed).max(0.0);
            p.vel.x = h.x;
            p.vel.y = h.y;
        }
        accelerate(&mut p.vel, wish_dir, wish_speed, GROUND_ACCEL, dt);
        p.vel.z = 0.0;
        if jump_pressed {
            p.jump_buffer = 0.0;
            p.vel.z = JUMP_SPEED;
            p.on_ground = false;
        }
    } else {
        // Source-style air control: small wish speed cap, so strafing steers without adding speed
        accelerate(&mut p.vel, wish_dir, wish_speed.min(60.0), AIR_ACCEL * 10.0, dt);
        p.vel.z -= GRAVITY * dt;
    }

    if p.on_ground {
        ground_move(p, dt, col);
    } else {
        col.move_capsule(&mut p.pos, p.vel * dt, &mut p.vel);
        // landed? (only when not moving up)
        if p.vel.z <= 0.0 {
            if let Some((t, n)) = col.sweep_down(p.pos + Vec3::new(0.0, 0.0, 2.0), 3.0) {
                if n.z > WALKABLE_Z {
                    p.pos.z += 2.0 - t;
                    p.vel.z = 0.0;
                    p.on_ground = true;
                }
            }
        }
    }
}

fn horiz(a: Vec3, b: Vec3) -> f32 {
    Vec3::new(b.x - a.x, b.y - a.y, 0.0).length()
}

/// Walking: plain slide vs. step up STEP_UP / slide / step down, whichever gets further
/// (Quake/Source StepMove), then stick to the floor within STEP_DOWN or start falling.
fn ground_move(p: &mut Player, dt: f32, col: &Collision) {
    let start = p.pos;
    let delta = Vec3::new(p.vel.x, p.vel.y, 0.0) * dt;

    let mut pa = start;
    let mut va = p.vel;
    col.move_capsule(&mut pa, delta, &mut va);

    let mut pb = start;
    let mut vb = p.vel;
    let mut tmp = Vec3::ZERO;
    col.move_capsule(&mut pb, Vec3::new(0.0, 0.0, STEP_UP), &mut tmp);
    let raised = pb.z - start.z;
    col.move_capsule(&mut pb, delta, &mut vb);
    let landed = col.sweep_down(pb, raised + STEP_DOWN).filter(|(_, n)| n.z > WALKABLE_Z);
    let use_b = match landed {
        Some((t, _)) => {
            pb.z -= t;
            pb.z > start.z + 0.5 && horiz(start, pb) > horiz(start, pa) + 0.05
        }
        None => false,
    };
    if use_b {
        p.pos = pb;
        p.vel.x = vb.x;
        p.vel.y = vb.y;
    } else {
        p.pos = pa;
        p.vel.x = va.x;
        p.vel.y = va.y;
    }
    p.vel.z = 0.0;

    // stay glued to the floor (ramps, small drops); otherwise we walked off a ledge
    // probe starts a few units up: wall push-outs can leave the feet a hair inside the floor
    const LIFT: f32 = 4.0;
    match col.sweep_down(p.pos + Vec3::new(0.0, 0.0, LIFT), STEP_DOWN + LIFT) {
        Some((t, n)) if n.z > WALKABLE_Z => {
            p.pos.z += LIFT - t;
            p.on_ground = true;
        }
        _ => p.on_ground = false,
    }
}