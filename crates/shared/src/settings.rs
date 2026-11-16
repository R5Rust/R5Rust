//! Apex player settings (`S[name]` in the R5Regenis specs) and movement cvar defaults (`cv(name)`).
//!
//! Player settings: copied from `settings/player/mp/pilot_survival_firesupport.json` (Bangalore; a standard
//! 72-tall legend: 20 of the 28 pilot_survival legends share every movement value below). `movecheck` re-reads
//! the file (when the assets are present) and asserts these consts still match it.
//! Cvar defaults: read from r5apex_ds.exe's static ConVar objects (default string at ConVar+0x50) for every cvar
//! whose ConVar pointer the specs give (every value below was read that way).

use glam::Vec3;

/// One `poseSettings[i]` block. Index 0 = standing, 1 = crouched (hull 47, view 32), 2 = dead, 3 = unused.
#[derive(Clone, Copy, Debug)]
pub struct Stance {
    pub hull_min: Vec3,
    pub hull_max: Vec3,
    pub viewheight: f32,
    pub low_speed: f32,
    pub speed: f32,
    pub sprintspeed: f32,
    pub acceleration: f32,
    pub deceleration: f32,
    pub low_acceleration: f32,
    pub sprint_acceleration: f32,
    pub sprint_deceleration: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct PlayerSettings {
    pub stand: Stance,
    pub crouch: Stance,
    pub step_height: f32,
    pub speed_scale_side: f32,
    pub speed_scale_back: f32,
    pub air_speed: f32,
    pub air_acceleration: f32,
    pub air_friction: f32,
    pub air_drag: f32,
    pub air_strafe_enabled: bool,
    pub slip_air_restrict_duration: f32,
    pub gravity_scale: f32,
    pub jump: bool,
    pub jump_height: f32,
    pub can_jump_while_crouched: bool,
    pub anti_multi_jump_height_frac: f32,
    pub anti_multi_jump_time_min: f32,
    pub anti_multi_jump_time_max: f32,
    pub land_slowdown_frac: f32,
    pub land_slowdown_duration: f32,
    pub land_slowdown_time_power: f32,
    pub land_slowdown_height_min: f32,
    pub land_slowdown_height_max: f32,
    pub land_slowdown_no_sprint_frac: f32,
    pub crouch_enabled: bool,
    pub sprint: bool,
    pub slide: bool,
    pub slide_boost_enabled: bool,
    pub slide_boost_cooldown: f32,
    pub slide_decel: f32,
    pub slide_want_to_stop_decel: f32,
    pub slide_velocity_decay: f32,
    pub slide_accel: f32,
    pub slide_required_start_speed: f32,
    pub slide_required_start_speed_air: f32,
    pub slide_stop_speed: f32,
    pub slide_max_stop_speed: f32,
    pub slide_max_jump_speed: f32,
    pub slide_speed_boost: f32,
    pub slide_speed_boost_cap: f32,
    pub slide_jump_height: f32,
    pub automantle: bool,
    pub mantle_height: f32,
    pub mantle_duration_below: f32,
    pub mantle_duration_level: f32,
    pub mantle_duration_above: f32,
    pub mantle_duration_high: f32,
    pub wallrun_jump_up_speed: f32,
    pub wallrun_jump_outward_speed: f32,
}

const fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// pilot_survival_firesupport.json
pub const STOCK: PlayerSettings = PlayerSettings {
    stand: Stance {
        hull_min: v(-16.0, -16.0, 0.0),
        hull_max: v(16.0, 16.0, 72.0),
        viewheight: 60.0,
        low_speed: 120.0,
        speed: 173.5,
        sprintspeed: 260.0,
        acceleration: 450.0,
        deceleration: 1250.0,
        low_acceleration: 2500.0,
        sprint_acceleration: 100.0,
        sprint_deceleration: 1250.0,
    },
    crouch: Stance {
        hull_min: v(-16.0, -16.0, 0.0),
        hull_max: v(16.0, 16.0, 47.0),
        viewheight: 32.0,
        low_speed: -1.0,
        speed: 80.0,
        sprintspeed: 0.0,
        acceleration: 2500.0,
        deceleration: -1.0,
        low_acceleration: -1.0,
        sprint_acceleration: -1.0,
        sprint_deceleration: -1.0,
    },
    step_height: 22.0,
    speed_scale_side: 1.0,
    speed_scale_back: 1.0,
    air_speed: 60.0,
    air_acceleration: 500.0,
    air_friction: 0.0,
    air_drag: 0.0,
    air_strafe_enabled: false,
    slip_air_restrict_duration: 1.0,
    gravity_scale: 1.0,
    jump: true,
    jump_height: 56.0,
    can_jump_while_crouched: false,
    anti_multi_jump_height_frac: 0.3,
    anti_multi_jump_time_min: 0.15,
    anti_multi_jump_time_max: 0.75,
    land_slowdown_frac: 0.0,
    land_slowdown_duration: 1.0,
    land_slowdown_time_power: 2.0,
    land_slowdown_height_min: 300.0,
    land_slowdown_height_max: 800.0,
    land_slowdown_no_sprint_frac: 0.7,
    crouch_enabled: true,
    sprint: true,
    slide: true,
    slide_boost_enabled: true,
    slide_boost_cooldown: 2.0,
    slide_decel: 100.0,
    slide_want_to_stop_decel: 400.0,
    slide_velocity_decay: 0.7,
    slide_accel: 50.0,
    slide_required_start_speed: 200.0,
    slide_required_start_speed_air: 90.0,
    slide_stop_speed: 125.0,
    slide_max_stop_speed: 350.0,
    slide_max_jump_speed: 350.0,
    slide_speed_boost: 150.0,
    slide_speed_boost_cap: 400.0,
    slide_jump_height: 50.0,
    automantle: true,
    mantle_height: 80.0,
    mantle_duration_below: 0.7,
    mantle_duration_level: 0.6,
    mantle_duration_above: 0.5,
    mantle_duration_high: 0.35,
    wallrun_jump_up_speed: 240.0,
    wallrun_jump_outward_speed: 188.0,
};

/// Movement cvars. `exe` = default string read from r5apex_ds.exe (ConVar+0x50); `spec` = value the spec states.
#[derive(Clone, Copy, Debug)]
pub struct Cvars {
    pub sv_gravity: f32,                     // exe "750"
    pub sv_maxvelocity: f32,                 // exe "34000" (ConVar the rebuilt CheckVelocity reads)
    pub air_control: f32,                    // exe "2.0" (AirAccelerate's second-branch ConVar 0x1423705d8; name not resolved)
    pub jump_graceperiod: f32,               // exe "0.2"
    pub skip_time: f32,                      // exe "1.0"
    pub skip_jump_height_speed: f32,         // exe "450"
    pub skip_jump_height_fraction: f32,      // exe "1.0"
    pub skip_speed_retain: f32,              // exe "-1"
    pub skip_speed_reduce: f32,              // exe "100"
    pub jump_keyboardgraceperiodmax: f32,    // exe "0.5"
    pub jump_keyboardgraceperiodmin: f32,    // exe "0.2"
    pub jump_keyboardgrace_strength: f32,    // exe "0.7"
    pub jump_keyboardgrace_max: f32,         // exe "0.7"
    pub slide_max_angle_dot: f32,            // exe "0.6"
    pub slide_while_in_air: bool,            // exe "0"
    pub slide_auto_stand: bool,              // exe "0"
    pub slide_step_velocity_reduction: f32,  // exe "10"
    pub sv_optimizedmovement: bool,          // exe "1"
    pub traversal_enable: bool,              // exe "1"
    pub traversal_cooldown: f32,             // exe "0.5"
    pub automantle_enable: bool,             // exe "1"
    pub automantle_cooldown: f32,            // exe ".25"
    pub automantle_searchdist: f32,          // exe "5.f"
    pub automantle_mindist: f32,             // exe "18.f"
    pub automantle_height_level: f32,        // exe "10"
    pub automantle_height_above: f32,        // exe "30"
    pub automantle_height_below: f32,        // exe "-10"
    pub automantle_jumpoff_duration: f32,    // exe "0.1"
    pub automantle_jumpoff_anim_maxfrac: f32,// exe "0.5"
    pub automantle_rest_frac: f32,           // exe "0.4"
    pub automantle_rest_frac_below: f32,     // exe "0.3"
    pub automantle_backoff_anim_maxfrac: f32,// exe "0.7"
    // Mantle boost and tap-strafe gates: R5Regenis engine/s21-r5sdk mantle_boost.cpp / tapstrafe.cpp defaults
    // (that bridge rebuilds the S21 client feature on the dedicated server; not in r5apex_ds.exe itself).
    pub mantle_boost_enabled: bool,              // "1" (playlist var mantle_boost_enabled, default true)
    pub mantle_boost_disables_tap_strafes: bool, // "1"
    pub mantle_boost_min_valid_traversal_frac: f32, // "0.5"
    pub mantle_boost_sprint_mult: f32,           // "1.3" (bridge_mantle_boost_sprint_mult)
    pub mantle_boost_jump_height: f32,           // "90" (bridge_mantle_boost_jump_height)
    pub lurch_cutoff_speed: f32,                 // "1100" (bridge_lurch_cutoff_speed = client jump_grace_cutoff_speed)
}

pub const CVARS: Cvars = Cvars {
    sv_gravity: 750.0,
    sv_maxvelocity: 34000.0,
    air_control: 2.0,
    jump_graceperiod: 0.2,
    skip_time: 1.0,
    skip_jump_height_speed: 450.0,
    skip_jump_height_fraction: 1.0,
    skip_speed_retain: -1.0,
    skip_speed_reduce: 100.0,
    jump_keyboardgraceperiodmax: 0.5,
    jump_keyboardgraceperiodmin: 0.2,
    jump_keyboardgrace_strength: 0.7,
    jump_keyboardgrace_max: 0.7,
    slide_max_angle_dot: 0.6,
    slide_while_in_air: false,
    slide_auto_stand: false,
    slide_step_velocity_reduction: 10.0,
    sv_optimizedmovement: true,
    traversal_enable: true,
    traversal_cooldown: 0.5,
    automantle_enable: true,
    automantle_cooldown: 0.25,
    automantle_searchdist: 5.0,
    automantle_mindist: 18.0,
    automantle_height_level: 10.0,
    automantle_height_above: 30.0,
    automantle_height_below: -10.0,
    automantle_jumpoff_duration: 0.1,
    automantle_jumpoff_anim_maxfrac: 0.5,
    automantle_rest_frac: 0.4,
    automantle_rest_frac_below: 0.3,
    automantle_backoff_anim_maxfrac: 0.7,
    mantle_boost_enabled: true,
    mantle_boost_disables_tap_strafes: true,
    mantle_boost_min_valid_traversal_frac: 0.5,
    mantle_boost_sprint_mult: 1.3,
    mantle_boost_jump_height: 90.0,
    lurch_cutoff_speed: 1100.0,
};
