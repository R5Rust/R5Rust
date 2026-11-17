//! Apex player settings (`S[name]` in the R5Regenis specs) and movement cvar defaults (`cv(name)`).
//!
//! Player settings: copied from `settings/player/mp/pilot_survival_firesupport.json` (Bangalore; a standard
//! 72-tall legend: 20 of the 28 pilot_survival legends share every movement value below). `movecheck` re-reads
//! the file (when the assets are present) and asserts these consts still match it.
//! Cvar defaults: read from r5apex_ds.exe's static ConVar objects (default string at ConVar+0x50) for every cvar
//! whose ConVar pointer the specs give (every value below was read that way), except the jump grace (lurch) block,
//! which comes from the S21 client r5apex_dx12.exe (see docs/movement.md "Lurch").

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
    pub wallrun_jump_input_dir_speed: f32,
    // Wall climb (sim/wall.rs, walkmove.md 7W). Stock pilots climb (climbEnabled) and do not wallrun.
    pub climb_enabled: bool,
    pub wallrun: bool,
    pub wallstick_enabled: bool,
    pub wallrun_duck_causes_fall_off: bool,
    pub wallrun_same_wall_allowed: bool,
    pub wallrun_ads_type: i32,
    pub climb_height: f32,
    pub climb_final_jump_up_height: f32,
    pub climb_speed_start: f32,
    pub climb_speed_end: f32,
    pub climb_accel: f32,
    pub climb_decel: f32,
    pub climb_horz_speed: f32,
    pub climb_horz_accel: f32,
    pub climb_horz_decel: f32,
    pub vertical_gain_cutoff_wallrun: f32,
    pub wallrun_time_limit: f32,
    pub wallrun_ceiling_limit: f32,
    pub wallrun_allowed_wall_distance: f32,
    pub wallrun_up_wall_boost: f32,
    pub wallrun_max_speed_vertical: f32,
    pub wallrun_max_speed_horizontal: f32,
    pub wallrun_max_speed_horizontal_backward: f32,
    pub wallrun_accelerate_vertical: f32,
    pub wallrun_decelerate_vertical: f32,
    pub wallrun_accelerate_horizontal: f32,
    pub wallrun_decelerate_horizontal: f32,
    pub wallrun_gravity_ramp_up_time: f32,
    pub wallrun_same_wall_height: f32,
    pub mantle_angle_scale: f32,
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
    wallrun_jump_input_dir_speed: 75.0,
    climb_enabled: true,
    wallrun: false,
    wallstick_enabled: false,
    wallrun_duck_causes_fall_off: true,
    wallrun_same_wall_allowed: true,
    wallrun_ads_type: 2,
    climb_height: 100.0,
    climb_final_jump_up_height: 28.0,
    climb_speed_start: 200.0,
    climb_speed_end: 50.0,
    climb_accel: 500.0,
    climb_decel: 300.0,
    climb_horz_speed: 100.0,
    climb_horz_accel: 100.0,
    climb_horz_decel: 500.0,
    vertical_gain_cutoff_wallrun: 175.0,
    wallrun_time_limit: 2.0,
    wallrun_ceiling_limit: 0.5,
    wallrun_allowed_wall_distance: 17.0,
    wallrun_up_wall_boost: 250.0,
    wallrun_max_speed_vertical: 112.0,
    wallrun_max_speed_horizontal: 225.0,
    wallrun_max_speed_horizontal_backward: 50.0,
    wallrun_accelerate_vertical: 450.0,
    wallrun_decelerate_vertical: 300.0,
    wallrun_accelerate_horizontal: 200.0,
    wallrun_decelerate_horizontal: 500.0,
    wallrun_gravity_ramp_up_time: 1.0,
    wallrun_same_wall_height: 0.0,
    mantle_angle_scale: 1.0,
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
    // Jump grace (lurch / tap-strafe): the S21 client's cvars (r5apex_dx12.exe, default string read from each
    // ConVar's registration function; read by AirMove_TapStrafe 0x140a48a30 through ConVar+0x40 -> +0x60/+0x64).
    // They replace the 2019 server's jump_keyboardgrace* (periodmax 0.5, periodmin 0.2, strength 0.7, max 0.7).
    pub jump_graceperiodmax: f32,            // dx12 "0.5"  (reg 0x14019fd60) window after the jump
    pub jump_graceperiodmin: f32,            // dx12 "0.2"  (reg 0x14019fb90) full strength until here
    pub jump_grace_strength: f32,            // dx12 "0.7"  (reg 0x14019ff30) blend toward the keys
    pub jump_grace_max: f32,                 // dx12 "0.7"  (reg 0x1401a0100) x sprint speed: max change per lurch
    pub jump_grace_cutoff_speed: f32,        // dx12 "1100" (reg 0x1401a0f80) no lurch above this h-speed
    pub jump_grace_require_tap: bool,        // dx12 "1"    (reg 0x1401a02d0) only a newly pressed move key fires
    pub jump_grace_jump: bool,               // dx12 "1"    (reg 0x1401a0db0) window from the jump (0: from last ground)
    pub jump_grace_min_tap_period: f32,      // dx12 "0.0"  (reg 0x1401a04a0) min time between lurches
    pub jump_grace_max_tap_count: i32,       // dx12 "1000" (reg 0x1401a0a10) lurches per jump
    pub jump_grace_decay_start_tap_count: i32, // dx12 "1000" (reg 0x1401a0670) from this lurch on, strength x decay
    pub jump_grace_multi_tap_decay: f32,     // dx12 "1.0"  (reg 0x1401a0840)
    pub jump_grace_multi_tap_decay_velocity_based_max_decay: f32, // dx12 "0.0" (reg 0x1401a1150)
    pub jump_grace_multi_tap_decay_velocity_based_max_speed: f32, // dx12 "900" (reg 0x1401a1320)
    pub jump_grace_multi_tap_decay_velocity_based_easing_type: i32, // dx12 "1" (reg 0x1401a14f0) 0 cubic 1 sine 2 circ 3 linear 4 step
    pub jump_grace_limit_to_90: bool,        // dx12 "0"    (reg 0x1401a0be0) drop the key push's backward part
    pub jump_grace_controller_disable_duration: f32, // dx12 "-1.0" (reg 0x1401a16c0) no controller tracking here
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
    // Mantle boost and its tap-strafe gate: R5Flowstate SDK (s21-r5sdk) mantle_boost.cpp defaults. The client SDK
    // hooks AirMove_TapStrafe and skips it while a full boost is in the air (not in r5apex_dx12.exe itself).
    pub mantle_boost_enabled: bool,              // "1" (playlist var mantle_boost_enabled, default true)
    pub mantle_boost_disables_tap_strafes: bool, // "1"
    pub mantle_boost_min_valid_traversal_frac: f32, // "0.5"
    pub mantle_boost_sprint_mult: f32,           // "1.3" (bridge_mantle_boost_sprint_mult)
    pub mantle_boost_jump_height: f32,           // "90" (bridge_mantle_boost_jump_height)
    // Wall climb cvars (ConVar pointer -> name from the registration code, default from ConVar+0x50)
    pub wallrun_enable: bool,                    // 0x1422e9e28 exe "1"
    pub wallrun_retry_interval: f32,             // 0x14236f348 exe "0.07"
    pub wallrun_same_wall_dot: f32,              // 0x142315528 exe "0.9"
    pub wallrun_same_wall_dist: f32,             // 0x142314cd8 exe "100"
    pub wallrun_same_wall_slope: f32,            // 0x1422e4eb8 exe "0"
    pub wallrun_strength_loss_start: f32,        // 0x14234b888 exe "1000"
    pub wallrun_strength_loss_end: f32,          // 0x14234bb88 exe "1000"
    pub wallrun_min_angle_air: f32,              // 0x142364e28 exe "180.0"
    pub wallrun_upward_auto_push: f32,           // 0x142371958 exe "0.65"
    pub wallrun_angle_change_min_cos: f32,       // 0x142385238 exe "0.8"
    pub wallrun_push_away_fall_off_time: f32,    // 0x1422e8648 exe "0.05"
    pub wallrun_fall_away_speed: f32,            // 0x142372ef8 exe "70.0"
    pub wallclimb_vertical_gain_reduction: f32,  // 0x142373ad8 exe "128"
    pub wallrun_rotate_max_rate: f32,            // 0x142315b18 exe "3"
    pub wallrun_slipstarttime: f32,              // 0x142372af8 exe "1.5"
    pub wallrun_slipduration: f32,               // 0x142384b48 exe "1.0"
    pub wallrun_no_input_slip_frac: f32,         // 0x142372e58 exe "0.7"
    pub wallrun_repel_enable: bool,              // 0x14236d758 exe "1" (gates the last-wall record)
    pub automantle_maxangle_push: f32,           // 0x14236fb18 exe "50"
    pub automantle_maxangle_view: f32,           // 0x142371d38 exe "50"
    pub automantle_wallrun_maxangle_view: f32,   // 0x1423719f8 exe "45"
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
    jump_graceperiodmax: 0.5,
    jump_graceperiodmin: 0.2,
    jump_grace_strength: 0.7,
    jump_grace_max: 0.7,
    jump_grace_cutoff_speed: 1100.0,
    jump_grace_require_tap: true,
    jump_grace_jump: true,
    jump_grace_min_tap_period: 0.0,
    jump_grace_max_tap_count: 1000,
    jump_grace_decay_start_tap_count: 1000,
    jump_grace_multi_tap_decay: 1.0,
    jump_grace_multi_tap_decay_velocity_based_max_decay: 0.0,
    jump_grace_multi_tap_decay_velocity_based_max_speed: 900.0,
    jump_grace_multi_tap_decay_velocity_based_easing_type: 1,
    jump_grace_limit_to_90: false,
    jump_grace_controller_disable_duration: -1.0,
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
    wallrun_enable: true,
    wallrun_retry_interval: 0.07,
    wallrun_same_wall_dot: 0.9,
    wallrun_same_wall_dist: 100.0,
    wallrun_same_wall_slope: 0.0,
    wallrun_strength_loss_start: 1000.0,
    wallrun_strength_loss_end: 1000.0,
    wallrun_min_angle_air: 180.0,
    wallrun_upward_auto_push: 0.65,
    wallrun_angle_change_min_cos: 0.8,
    wallrun_push_away_fall_off_time: 0.05,
    wallrun_fall_away_speed: 70.0,
    wallclimb_vertical_gain_reduction: 128.0,
    wallrun_rotate_max_rate: 3.0,
    wallrun_slipstarttime: 1.5,
    wallrun_slipduration: 1.0,
    wallrun_no_input_slip_frac: 0.7,
    wallrun_repel_enable: true,
    automantle_maxangle_push: 50.0,
    automantle_maxangle_view: 50.0,
    automantle_wallrun_maxangle_view: 45.0,
};
