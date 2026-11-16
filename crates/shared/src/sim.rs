//! Deterministic player movement (Source units = inches, Z up). Same code will run on the client
//! (prediction) and the dedicated server (authority) once netcode lands in v2.

use crate::collision::{Collision, AXIS_LEN, RADIUS};
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
}

impl Player {
    pub fn new(pos: Vec3) -> Self {
        Self { pos, vel: Vec3::ZERO, on_ground: false, flying: false, fly_speed: 1200.0, jump_held: false }
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
    let jump_pressed = input.jump && !p.jump_held;
    p.jump_held = input.jump;

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
            p.vel.z = JUMP_SPEED;
            p.on_ground = false;
        }
    } else {
        // Source-style air control: small wish speed cap, so strafing steers without adding speed
        accelerate(&mut p.vel, wish_dir, wish_speed.min(60.0), AIR_ACCEL * 10.0, dt);
        p.vel.z -= GRAVITY * dt;
    }

    let was_ground = p.on_ground;
    let delta = p.vel * dt;
    let c = col.move_capsule(&mut p.pos, delta, &mut p.vel);
    p.on_ground = c.ground && p.vel.z <= 1.0;

    // stick to the floor when walking down slopes / off small steps
    if !p.on_ground && was_ground && p.vel.z <= 0.0 {
        let save = p.pos;
        let mut v = p.vel;
        let c2 = col.move_capsule(&mut p.pos, Vec3::new(0.0, 0.0, -STEP_DOWN), &mut v);
        if c2.ground {
            p.on_ground = true;
            p.vel.z = 0.0;
        } else {
            p.pos = save;
        }
    }
    if p.on_ground {
        p.vel.z = 0.0;
    }
}
