//! Apex Legends (Season 21 / R5) player movement, ported from the bit-exact specs of r5apex_ds.exe in
//! `C:\Code\apex\R5Regenis\docs\engine\*.md` (see docs/movement.md for what is ported and what is approximated).
//! Source units (inches), Z up, `pos` = feet (the hull's mins.z is 0).
//!
//! Deterministic: plain f32 maths in a fixed order, no threads, no wall clock. The same code will run on the
//! client (prediction) and the dedicated server (authority) once netcode lands.
//!
//! Layout (one file per spec):
//! - `mod.rs`: PlayerMove 0x140ba8fc0 (the per-tick driver), input/buttons (SetupMove), UpdateSprint 0x140bedf60
//! - `walk.rs`: FullWalkMove 0x140b9ead0, ApplyHalfGravity, CheckJumpButton 0x140b8d490, WalkMove 0x140b9cdb0,
//!   Friction, Accelerate, GetAccelerate, AirMove 0x140b9b740, AirAccelerate, CheckVelocity
//! - `ground.rs`: CategorizePosition 0x140ba4f30, SetGroundEntity 0x140ba46d0, StayOnGround, step smoothing, CheckStuck
//! - `duck.rs`: Duck 0x140ba7500, StartSliding 0x140ba7ed0, StopSliding 0x140ba8250
//! - `step.rs`: StepSlide 0x140b9ad70, TryPlayerMoveStep 0x140b9a970, StepMove 0x140bb0610, TryPlayerMove core
//! - `traversal.rs`: TraversalMove 0x140b981b0 (mantle), CheckAutoMantle, mantle boost
//! - `wall.rs`: wall climb: SurfaceTransition 0x140ba1990, its classifier 0x140ba0a80, WallRunUpdate 0x140b9f6e0,
//!   WallJump 0x140ba2c90, EndWallRun 0x140bf0ba0 and their helpers

mod duck;
mod ground;
mod step;
mod traversal;
mod walk;
mod wall;

use crate::collision::{Collision, Trace};
pub use crate::settings::{Cvars, PlayerSettings, Stance, CVARS, STOCK};
use glam::Vec3;

/// Standing eye height (stance 0 `viewheight`).
pub const EYE_HEIGHT: f32 = 60.0;

// Source / Respawn button bits (in_buttons.h), as the specs use them.
pub const IN_JUMP: u32 = 0x2;
pub const IN_DUCK: u32 = 0x4;
pub const IN_FORWARD: u32 = 0x8;
pub const IN_BACK: u32 = 0x10;
pub const IN_MOVELEFT: u32 = 0x200;
pub const IN_MOVERIGHT: u32 = 0x400;
pub const IN_SPEED: u32 = 0x8000;

/// Wall state (pl+0x6644..0x6694, pl+0x5c4c..0x5c68, pl+0x5da0). Stock pilots only climb (no wallrun / hang).
#[derive(Clone, Copy, Debug, Default)]
pub struct Wall {
    /// pl+0x6650: when the current wall attach started (climb time, gravity ramp, slip)
    pub start_time: f32,
    /// pl+0x6654: when the last wall ended
    pub end_time: f32,
    /// pl+0x6644..0x664c: origin at the attach (WallTop, same-wall height test)
    pub attach_pos: Vec3,
    /// pl+0x6658: walls attached since the last ground jump
    pub count: i32,
    /// pl+0x665c: the up-wall boost / gravity ramp is spent (UpWallBoostScale returns 0)
    pub no_boost: bool,
    /// pl+0x6670: when pulling away from the wall started (0 = not pulling)
    pub push_away_time: f32,
    /// pl+0x667c / 0x6680 / 0x668c: deferred surface transition (retry time, origin, wall normal)
    pub retry_time: f32,
    pub retry_pos: Vec3,
    pub retry_normal: Vec3,
    /// pl+0x5c4c / 0x5c64 / 0x5c68: the last wall left (flat normal), when, and whether the record is set
    pub last_normal: Vec3,
    pub last_time: f32,
    pub last_count: i32,
    /// pl+0x5da0: reference height for verticalGainCutoff (the floor you last stood on, lowered while falling,
    /// and lowered by wallclimb_vertical_gain_reduction after each climb)
    pub ref_height: f32,
}

/// One tick of player input.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    /// -1..1, +1 = forward
    pub forward: f32,
    /// -1..1, +1 = right
    pub right: f32,
    /// jump held
    pub jump: bool,
    /// a jump press happened since the last tick (mouse-wheel binds press and release between ticks); counts as
    /// pressed and held for this tick
    pub jump_pressed: bool,
    /// sprint held (Apex: a press requests sprint for 3 s and it latches while you keep moving forward)
    pub sprint: bool,
    /// crouch held (crouch / slide)
    pub crouch: bool,
    /// fly mode: descend
    pub down: bool,
    /// degrees, 0 = +X, 90 = +Y (Source convention)
    pub yaw: f32,
    /// degrees, + = looking down (Source convention)
    pub pitch: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveType {
    Walk,
    /// TRAVERSE (12): mantling
    Traverse,
}

/// TraversalMove's player state (pl+0x66a4..0x6720), in world space (the ledge is always the world here).
#[derive(Clone, Copy, Debug, Default)]
pub struct Traversal {
    pub start: Vec3,
    pub corner: Vec3,
    pub end: Vec3,
    /// pl+0x66d0: progress at which the path turns from "up" to "forward"
    pub c: f32,
    /// pl+0x66ec: stored progress 0..1
    pub p: f32,
    /// pl+0x6720: minimum target progress
    pub min_target: f32,
    /// pl+0x66a8: animation state (3 below, 4 level, 5 above, 6 high, +4 crouched, 11 rest, 12 jump-off)
    pub state: i32,
    /// pl+0x6714: jump-off request time (0 = none)
    pub jump_time: f32,
    pub drop: bool,
    pub fwd_latch: bool,
    pub can_rest: bool,
    /// pl+0x66f0 / pl+0x66f8: traversal start / end time (cooldowns)
    pub start_time: f32,
    pub end_time: f32,
    pub ledge_forward: Vec3,
    pub ledge_point: Vec3,
    /// progress per second used last tick (for the mantle-boost window)
    pub rate: f32,
}

/// Mantle boost FSM (R5Regenis mantle_boost.cpp `MantleBoostSlot_t`): 0 idle, 1 invalid (hang),
/// 3 failed (press outside the sweet spot: plain mantle jump), 4 boost.
#[derive(Clone, Copy, Debug, Default)]
pub struct MantleBoost {
    pub state: u8,
    pub applied_time: f32,
    /// 0..1 mantle progress at which this climb's sweet spot opens (for a hint UI); 0 when unknown
    pub sweet_spot_frac: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Player {
    /// feet position (mv+0x124)
    pub pos: Vec3,
    /// mv+0x130
    pub vel: Vec3,
    /// read-only mirror of the ground entity (pl+0x3c4 valid)
    pub on_ground: bool,
    /// dev noclip-ish fly (still collides)
    pub flying: bool,
    pub fly_speed: f32,
    /// the legend's settings block
    pub settings: PlayerSettings,
    /// movement cvars (per player so tests and the server can flip them)
    pub cvars: Cvars,
    /// pl+0x6134 speed scale (1.0 = weapon out; Apex holstered is ~1.15)
    pub speed_scale: f32,
    pub move_type: MoveType,
    /// pl+0x65f0: 0 standing, 1 ducking, 2 ducked, 3 unducking
    pub duck_state: u8,
    /// FL_DUCKING: the hull is the crouched one
    pub ducked: bool,
    /// pl+0x574: current eye height above the feet (eases between stances)
    pub view_z: f32,
    /// pl+0x6b68: view offset that smooths steps (decays to 0)
    pub step_smooth: Vec3,
    pub sliding: bool,
    /// pl+0x6d24
    pub sprinting: bool,
    pub traversal: Traversal,
    pub mantle_boost: MantleBoost,
    /// pl+0x662c: the surface up-normal. World up, or the wall's normal while on a wall (climbing).
    pub surface_normal: Vec3,
    pub wall: Wall,

    time: f64,
    ground: bool,
    duck_timer_ms: i32,
    duck_in_air: bool,
    slide_long_jump: bool,
    last_slide_time: f32,
    last_slide_boost: f32,
    sprint_latch: bool,
    sprint_req_until: f32,
    /// pl+0x724c
    ground_normal: Vec3,
    /// pl+0x5da4: normal of the ground entity's trace (SetGroundEntity), zero in the air
    ground_trace_normal: Vec3,
    /// pl+0x6ce0 m_surfaceFriction
    surface_friction: f32,
    /// pl+0x6214 / pl+0x6224: last time on (non-player) ground and the normal there
    last_ground_time: f32,
    last_ground_normal: Vec3,
    /// pl+0x6230: jumped since leaving the ground (kills the coyote grace)
    jumped: bool,
    /// pl+0x6231: the current/last air time began with a jump (anti multi-jump)
    air_from_jump: bool,
    /// pl+0x6218 / 0x621c / 0x6220 / 0x6934
    last_jump_time: f32,
    last_land_time: f32,
    land_impact: f32,
    near_ground_time: f32,
    /// pl+0x6938: contact plane from the last blocked move
    contact_plane: Vec3,
    /// pl+0x6bd8 / pl+0x5a88 / stuck retry time
    start_solid: bool,
    stuck_last: i32,
    stuck_check_time: f32,
    /// CheckVelocity's cache (gameMovement+0x46c)
    cv_cache: Vec3,
    old_buttons: u32,
}

impl Player {
    pub fn new(pos: Vec3) -> Self {
        Self::with_settings(pos, STOCK)
    }

    pub fn with_settings(pos: Vec3, settings: PlayerSettings) -> Self {
        Self {
            pos,
            vel: Vec3::ZERO,
            on_ground: false,
            flying: false,
            fly_speed: 1200.0,
            settings,
            cvars: CVARS,
            speed_scale: 1.0,
            move_type: MoveType::Walk,
            duck_state: 0,
            ducked: false,
            view_z: settings.stand.viewheight,
            step_smooth: Vec3::ZERO,
            sliding: false,
            sprinting: false,
            traversal: Traversal::default(),
            mantle_boost: MantleBoost::default(),
            surface_normal: UP,
            wall: Wall { ref_height: pos.z, ..Wall::default() },
            time: 0.0,
            ground: false,
            duck_timer_ms: 0,
            duck_in_air: false,
            slide_long_jump: false,
            last_slide_time: -100.0,
            last_slide_boost: 0.0,
            sprint_latch: false,
            sprint_req_until: 0.0,
            ground_normal: Vec3::ZERO,
            ground_trace_normal: Vec3::ZERO,
            surface_friction: 1.0,
            last_ground_time: -100.0,
            last_ground_normal: Vec3::Z,
            jumped: false,
            air_from_jump: false,
            last_jump_time: -100.0,
            last_land_time: -100.0,
            land_impact: 0.0,
            near_ground_time: -100.0,
            contact_plane: Vec3::ZERO,
            start_solid: false,
            stuck_last: 0,
            stuck_check_time: -100.0,
            cv_cache: Vec3::ZERO,
            old_buttons: 0,
        }
    }

    /// Camera position: feet + the stance's (eased) view height + the step-smoothing offset.
    pub fn eye(&self) -> Vec3 {
        self.pos + Vec3::new(0.0, 0.0, self.view_z) + self.step_smooth
    }

    pub fn hull(&self) -> (Vec3, Vec3) {
        let st = if self.ducked { &self.settings.crouch } else { &self.settings.stand };
        (st.hull_min, st.hull_max)
    }

    pub fn mantling(&self) -> bool {
        self.move_type == MoveType::Traverse
    }

    /// On a wall (wall climb): the surface normal is not upright.
    pub fn on_wall(&self) -> bool {
        !upright(self.surface_normal)
    }

    /// Simulation clock (seconds since spawn).
    pub fn time(&self) -> f64 {
        self.time
    }

    /// Short state name for HUDs / logs.
    pub fn state_name(&self) -> &'static str {
        if self.flying {
            "FLY"
        } else if self.mantling() {
            "mantle"
        } else if self.on_wall() {
            "climb"
        } else if self.sliding {
            "slide"
        } else if !self.on_ground {
            "air"
        } else if self.ducked {
            "crouch"
        } else if self.sprinting {
            "sprint"
        } else {
            "ground"
        }
    }
}

pub fn forward_vec(yaw: f32, pitch: f32) -> Vec3 {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    Vec3::new(y.cos() * p.cos(), y.sin() * p.cos(), -p.sin())
}

/// Finds a free spot at or above `pos` (spawn points can sit a hair inside the floor).
pub fn find_free(col: &Collision, pos: Vec3) -> Vec3 {
    let (mn, mx) = (STOCK.stand.hull_min, STOCK.stand.hull_max);
    for i in 0..64 {
        let p = pos + Vec3::new(0.0, 0.0, i as f32 * 4.0);
        if !col.trace_hull(p, p, mn, mx).startsolid {
            return p;
        }
    }
    pos
}

/// World up (and the surface normal pl+0x662c while not on a wall).
pub(crate) const UP: Vec3 = Vec3::Z;

/// UPRIGHT: `(double)(((n.x*0) + (n.y*0)) - (n.z*1)) <= -0.999`. EndWallRun writes (-0, -0, 1), which passes.
pub(crate) fn upright(n: Vec3) -> bool {
    ((((n.x * 0.0) + (n.y * 0.0)) - (n.z * 1.0)) as f64) <= -0.999
}

/// Per-tick movement context: CGameMovement + CMoveData of the specs.
pub(crate) struct Ctx<'a> {
    pub p: &'a mut Player,
    pub col: &'a Collision,
    pub s: PlayerSettings,
    pub cv: Cvars,
    /// gpGlobals frametime / curtime
    pub ft: f32,
    pub now: f32,
    /// mv+0x24 held, mv+0x2c pressed this tick
    pub buttons: u32,
    pub pressed: u32,
    /// mv+0x30 / mv+0x34 forward / side move (-1..1)
    pub fwd: f32,
    pub side: f32,
    /// mv+0x60 / mv+0x6c: flat view forward / right
    pub flat_f: Vec3,
    pub flat_r: Vec3,
    /// AngleVectors(view angles): forward / right (mv+0xc), used for the wall basis
    pub view_f: Vec3,
    pub view_r: Vec3,
    /// mv+0x10 view yaw, degrees
    pub yaw: f32,
    /// mv+0x78 (= mv+0x84 = mv+0xb4 while upright): input direction, length <= 1
    pub wish: Vec3,
    /// mv+0xd4 GetMaxSpeed
    pub max_speed: f32,
}

impl Ctx<'_> {
    pub fn gnd(&self) -> bool {
        self.p.ground
    }
    pub fn stance(&self) -> &Stance {
        if self.p.ducked { &self.s.crouch } else { &self.s.stand }
    }
    /// TraceHull with the player's current hull (GetPlayerHull).
    pub fn trace(&self, a: Vec3, b: Vec3) -> Trace {
        let (mn, mx) = self.p.hull();
        self.col.trace_hull(a, b, mn, mx)
    }
    /// SpeedCap 0x140bf5350 / SprintSpeedCap 0x140bf53c0 for the current stance
    pub fn speed_cap(&self, sprint: bool) -> f32 {
        let st = self.stance();
        self.p.speed_scale * if sprint { st.sprintspeed } else { st.speed }
    }
    /// mv+0xd0: hull half width
    pub fn half_width(&self) -> f32 {
        let (mn, mx) = self.p.hull();
        (mx.x - mn.x) * 0.5
    }
}

/// Advances the player by `dt` seconds (one usercmd). The client calls this at its fixed tick.
pub fn simulate(p: &mut Player, input: &Input, dt: f32, col: &Collision) {
    p.time += dt as f64;
    let now = p.time as f32;

    // SetupMove: buttons, pressed edges, flat basis, the input vector mv+0x78
    let mut b = 0u32;
    if input.forward > 0.0 {
        b |= IN_FORWARD;
    }
    if input.forward < 0.0 {
        b |= IN_BACK;
    }
    if input.right > 0.0 {
        b |= IN_MOVERIGHT;
    }
    if input.right < 0.0 {
        b |= IN_MOVELEFT;
    }
    if input.jump || input.jump_pressed {
        b |= IN_JUMP;
    }
    if input.crouch {
        b |= IN_DUCK;
    }
    if input.sprint {
        b |= IN_SPEED;
    }
    let mut pressed = b & !p.old_buttons;
    if input.jump_pressed {
        pressed |= IN_JUMP;
    }
    let fwd = input.forward.clamp(-1.0, 1.0);
    let side = input.right.clamp(-1.0, 1.0);
    let yr = input.yaw.to_radians();
    let flat_f = Vec3::new(yr.cos(), yr.sin(), 0.0);
    let flat_r = Vec3::new(yr.sin(), -yr.cos(), 0.0);
    let view_f = forward_vec(input.yaw, input.pitch);
    let mut wish = flat_f * fwd + flat_r * side;
    if wish.length_squared() > 1.0 {
        wish = wish.normalize();
    }

    if p.flying {
        fly_move(p, input, dt, col);
        p.old_buttons = b;
        return;
    }

    let mut c = Ctx {
        s: p.settings,
        cv: p.cvars,
        p,
        col,
        ft: dt,
        now,
        buttons: b,
        pressed,
        fwd,
        side,
        flat_f,
        flat_r,
        view_f,
        view_r: flat_r,
        yaw: input.yaw,
        wish,
        max_speed: 0.0,
    };
    player_move(&mut c);
    c.p.on_ground = c.p.ground;
    c.p.old_buttons = b;
}

/// PlayerMove 0x140ba8fc0 (playermove.md section 3), the steps that matter without weapons, ziplines,
/// grapple, doors, water or networking.
fn player_move(c: &mut Ctx) {
    let mt0 = c.p.move_type;

    // step 6: integer-millisecond duck timer
    if c.p.duck_timer_ms > 0 {
        c.p.duck_timer_ms = c.p.duck_timer_ms.wrapping_sub((c.ft * 1000.0) as i32);
        if c.p.duck_timer_ms < 0 {
            c.p.duck_timer_ms = 0;
        }
    }

    // step 13: CheckStuck, only after a move that started in solid. EA skips the rest of the tick while a nudge
    // keeps failing; here the tick still runs (our trace lets an embedded hull walk out), see docs/movement.md.
    if c.p.start_solid && mt0 == MoveType::Walk && ground::check_stuck(c) != 0 {
        c.p.vel = Vec3::ZERO;
    }
    c.p.start_solid = false;

    // step 16: ground maintenance (skipped while traversing)
    if mt0 == MoveType::Walk {
        if c.cv.sv_optimizedmovement {
            let dv = c.p.surface_normal.dot(c.p.vel);
            if dv > 200.0 {
                ground::set_ground(c, None);
            }
        } else if !(c.gnd() && ground::stay_on_ground(c)) {
            ground::categorize_position(c);
        }
    }

    // step 18: Duck
    duck::duck(c);

    // step 20: sprint and max speed
    update_sprint(c);
    c.max_speed = c.speed_cap(c.p.sprinting);

    // step 21: move-type dispatch
    match c.p.move_type {
        MoveType::Traverse => traversal::traversal_move(c, false),
        MoveType::Walk => {
            if c.cv.traversal_enable && !(c.cv.traversal_cooldown + c.p.traversal.end_time > c.now) && traversal::check_auto_mantle(c) {
                traversal::traversal_move(c, true);
            } else {
                walk::full_walk_move(c);
            }
        }
    }

    // step 24: DecayStepSmooth
    ground::decay_step_smooth(c);

    // post-think 0x140bfc660: last time on ground + the normal there (coyote grace; a wall counts, and its
    // normal makes the grace jump refuse)
    if c.gnd() {
        c.p.last_ground_time = c.now;
        c.p.last_ground_normal = c.p.surface_normal;
    }
    // post-think 0x140bfca13: the verticalGainCutoff reference height follows the feet down, and snaps to them
    // on walkable, upright ground (pl+0x67c0 launch-pad latch: no launch pads here)
    let z = c.p.pos.z;
    let w = &mut c.p.wall;
    if w.ref_height > z {
        w.ref_height = z;
    } else if w.ref_height != z && c.p.ground && upright(c.p.surface_normal) && !(0.7 > c.p.ground_trace_normal.z) {
        w.ref_height = z;
    }
}

/// CanSprint 0x140bed930: keeps the sprint latch alive.
fn can_sprint(c: &Ctx) -> bool {
    let m2 = c.fwd * c.fwd + c.side * c.side;
    if 0.64 > m2 {
        return false;
    }
    if m2 != 0.0 && -0.707 > c.fwd / m2.sqrt() {
        return false;
    }
    if c.gnd() && !c.p.sliding && (c.p.duck_state == 1 || c.p.duck_state == 2) {
        return false;
    }
    true
}

/// AllowSprint 0x140bed5a0: whether the player actually sprints this tick.
fn allow_sprint(c: &Ctx) -> bool {
    if !c.s.sprint {
        return false;
    }
    if !c.gnd() && (c.now - c.p.near_ground_time) > 0.25 {
        return false;
    }
    if c.p.move_type != MoveType::Walk {
        return false;
    }
    let m2 = c.fwd * c.fwd + c.side * c.side;
    if 0.64 > m2 || m2 == 0.0 || 0.707 > c.fwd / m2.sqrt() {
        return false;
    }
    if c.p.duck_state == 1 || c.p.duck_state == 2 {
        return false;
    }
    if c.s.land_slowdown_no_sprint_frac > ground::landing_speed_scale(c) {
        return false;
    }
    true
}

/// UpdateSprint 0x140bedf60. A sprint press requests sprint for 3 s (holding keeps extending it by 0.1 s);
/// the request latches while CanSprint holds (moving forward-ish, not crouched), so one tap of Shift keeps
/// you sprinting until you stop or crouch. StartSprint/StopSprint only drive view/animation timers
/// (sprintStartDelay etc.), so they are not ported.
fn update_sprint(c: &mut Ctx) {
    let pressed = c.pressed & IN_SPEED != 0;
    let held = c.buttons & IN_SPEED != 0;
    let req = if pressed {
        Some(c.now + 3.0)
    } else if held {
        Some(c.now + 0.1)
    } else {
        None
    };
    if let Some(t) = req {
        let old = c.p.sprint_req_until;
        c.p.sprint_req_until = if old > t { old } else { t };
    }
    if !c.p.sprint_latch {
        if c.p.sprint_req_until >= c.now && can_sprint(c) {
            c.p.sprint_latch = true;
        }
    } else if !can_sprint(c) {
        c.p.sprint_latch = false;
        c.p.sprint_req_until = 0.0;
    }
    let want = held || c.p.sprint_latch;
    c.p.sprinting = want && allow_sprint(c);
}

/// Dev fly mode: view-relative, snappy, still collides (TryPlayerMove with the standing hull).
fn fly_move(p: &mut Player, input: &Input, dt: f32, col: &Collision) {
    let fwd = forward_vec(input.yaw, input.pitch);
    let right = forward_vec(input.yaw - 90.0, 0.0);
    let mut wish = fwd * input.forward + right * input.right;
    if input.jump {
        wish.z += 1.0;
    }
    if input.down || input.crouch {
        wish.z -= 1.0;
    }
    let speed = p.fly_speed * if input.sprint { 3.0 } else { 1.0 };
    let target = wish.normalize_or_zero() * speed;
    p.vel += (target - p.vel) * (1.0 - (-12.0 * dt).exp());
    let (mn, mx) = p.hull();
    let mut pos = p.pos;
    let mut vel = p.vel;
    step::try_player_move_core(col, mn, mx, &mut pos, &mut vel, dt, None);
    p.pos = pos;
    p.vel = vel;
    p.on_ground = false;
    p.ground = false;
    p.move_type = MoveType::Walk;
    p.sliding = false;
}
