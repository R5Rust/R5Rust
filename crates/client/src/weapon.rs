//! The player's weapon (the R-99 for now, no switching): retail settings (`weapons/<name>/weapon.json` from
//! tools/build_weapon.py: the game's own weapon settings file + its viewkick pattern), fired at 60 ticks/s.
//!
//! - fire: automatic at `fire_rate`, `ammo_clip_size` rounds, reload `reload_time` / `reloadempty_time` (empty)
//! - ADS: `zoom_time_in` / `zoom_time_out`, FOV `zoom_fov` (x player FOV / 70, Apex's "scaled" ADS FOV), move speed x
//!   `ads_move_speed_scale`; sprinting cancels it, ADS and firing cancel sprint
//! - spread: the `spread_*` cone (degrees) for the stance; recoil: the `viewkick_pattern` table, one row per bullet
//!   (yaw, pitch degrees; negative pitch = up), looping from `loop_offset`, reset after a short pause
//! - bullets: projectiles at `projectile_launch_speed` with gravity (sv_gravity x `projectile_gravity_scale`), traced
//!   against the world collision each tick
//! - first-person animation: the game's sequences (viewmodel.rs), base pose + additive idle / fire layers

use crate::viewmodel::{self, Viewmodel};
use glam::{Quat, Vec3};
use shared::collision::Collision;
use std::collections::VecDeque;

pub struct Stats {
    pub name: String,
    /// viewmodel sway / bob (sway.rs)
    pub sway: crate::sway::Settings,
    pub fire_rate: f32,
    pub clip: u32,
    pub damage: f32,
    pub head_scale: f32,
    pub reload: f32,
    pub reload_empty: f32,
    pub zoom_fov: f32,
    pub zoom_in: f32,
    pub zoom_out: f32,
    /// deployfirst_time (the first draw, charging handle) / deploy_time (later draws)
    pub deploy: f32,
    pub deploy_again: f32,
    pub holster_time: f32,
    /// into sprint (lower_time) / back out of it (raise_time), seconds
    pub lower_time: f32,
    pub raise_time: f32,
    /// leaving a sprint (0 = the sequence's own length), the sprint loop's cycle (0 = its own length)
    pub raise_from_sprint_time: f32,
    pub sprintcycle_time: f32,
    /// retail move_speed_modifier (1 when unset: no gun slows you while running in Apex)
    pub move_speed: f32,
    pub ads_move: f32,
    pub speed: f32,
    pub gravity_scale: f32,
    pub drag: f32,
    /// stand / run / sprint / crouch / air (hip), stand / crouch / air (ADS), degrees
    pub spread_hip: [f32; 5],
    pub spread_ads: [f32; 3],
    pub kick: Vec<[f32; 4]>,
    pub kick_loop: usize,
    /// the view punch spring + per-shot kick (viewkick.rs)
    pub viewkick: crate::viewkick::Settings,
}

impl Stats {
    pub fn load(path: &std::path::Path) -> Result<Stats, String> {
        let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())?;
        let s = &j["settings"];
        let f = |k: &str, d: f32| s[k].as_str().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(d);
        let kick: Vec<[f32; 4]> = j["viewkick"]["bullets"].as_array().map(|a| a.iter().map(|r| std::array::from_fn(|i| r[i].as_f64().unwrap_or(0.0) as f32)).collect()).unwrap_or_default();
        Ok(Stats {
            name: display_name(s["printname"].as_str().unwrap_or("")),
            sway: crate::sway::Settings::from_json(s),
            fire_rate: f("fire_rate", 18.0),
            clip: f("ammo_clip_size", 18.0) as u32,
            damage: f("damage_near_value", 12.0),
            head_scale: f("damage_headshot_scale", 1.25),
            reload: f("reload_time", 1.8),
            reload_empty: f("reloadempty_time", 2.45),
            zoom_fov: f("zoom_fov", 60.0),
            zoom_in: f("zoom_time_in", 0.22),
            zoom_out: f("zoom_time_out", 0.2),
            deploy: f("deployfirst_time", 1.0),
            deploy_again: f("deploy_time", 0.35),
            holster_time: f("holster_time", 0.3),
            lower_time: f("lower_time", 0.25),
            raise_time: f("raise_time", 0.3),
            raise_from_sprint_time: f("raise_from_sprint_time", 0.0),
            sprintcycle_time: f("sprintcycle_time", 0.0),
            move_speed: f("move_speed_modifier", 1.0),
            ads_move: f("ads_move_speed_scale", 0.75),
            speed: f("projectile_launch_speed", 19000.0),
            gravity_scale: f("projectile_gravity_scale", 1.0),
            drag: f("projectile_drag_coefficient", 0.0),
            spread_hip: [f("spread_stand_hip", 2.0), f("spread_stand_hip_run", 3.0), f("spread_stand_hip_sprint", 5.0), f("spread_crouch_hip", 1.6), f("spread_air_hip", 7.0)],
            spread_ads: [f("spread_stand_ads", 0.35), f("spread_crouch_ads", 0.25), f("spread_air_ads", 5.0)],
            kick_loop: j["viewkick"]["loop_offset"].as_u64().unwrap_or(0) as usize,
            kick,
            viewkick: crate::viewkick::Settings::from_json(s, &j["springs"]),
        })
    }
}

/// What the player is doing, for this tick.
#[derive(Default, Clone, Copy)]
pub struct WeaponInput {
    pub fire: bool,
    pub ads: bool,
    pub reload: bool,
    pub inspect: bool,
    pub sprinting: bool,
    pub crouched: bool,
    pub on_ground: bool,
    pub moving: bool,
    pub sliding: bool,
    /// mantling / wall climbing: the game lowers the weapon (WeaponLowerMask, FUN_1409f3880)
    pub lowered: bool,
    /// horizontal speed (u/s), the `velocity` pose parameter
    pub speed: f32,
}

pub struct Projectile {
    pub pos: Vec3,
    pub vel: Vec3,
    pub life: f32,
    /// seconds since fired, and the eye -> on-screen muzzle offset at the time (tracers start at the gun and
    /// converge onto the real path, fx.rs)
    pub age: f32,
    pub muzzle_off: Vec3,
}

/// What the weapon did this tick, for sounds and effects (drained by main.rs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// a round left the barrel; `first` = the first of a spray
    Shot { first: bool },
    /// the trigger let go (or the mag ran dry) after a spray
    SprayEnd,
    DryFire,
    Reload { empty: bool },
    AdsIn,
    AdsOut,
    Inspect,
    Impact { pos: Vec3, normal: Vec3 },
}

#[derive(Clone, Copy)]
pub struct Impact {
    pub pos: Vec3,
    pub normal: Vec3,
    pub time: f32,
    /// surfaceproperties id of the hit triangle (decals.rs picks the decal), and a per-shot seed (variant, spin)
    pub surface: u8,
    pub seed: u32,
}

/// One attachment in a point: the apex-content-kit data.js id (laser-sight, light-mag, hcog-classic, standard-stock...)
/// and its tier (1 white .. 4 gold, 5 = corrupted / care package).
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub tier: u8,
}

/// The weapon's attachment points (the HUD's barrel / mag / optic / stock slots).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attachments {
    pub barrel: Option<Attachment>,
    pub mag: Option<Attachment>,
    pub sight: Option<Attachment>,
    pub grip: Option<Attachment>,
}

impl Attachments {
    pub fn get_mut(&mut self, point: &str) -> Option<&mut Option<Attachment>> {
        match point {
            "barrel" => Some(&mut self.barrel),
            "mag" => Some(&mut self.mag),
            "sight" | "optic" => Some(&mut self.sight),
            "grip" | "stock" => Some(&mut self.grip),
            _ => None,
        }
    }
    /// The HUD request's attachments object (kit gamehud.js toSlot input).
    pub fn json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (k, a) in [("barrel", &self.barrel), ("mag", &self.mag), ("sight", &self.sight), ("grip", &self.grip)] {
            if let Some(a) = a {
                let tier = if a.tier >= 5 { serde_json::json!("c") } else { serde_json::json!(a.tier) };
                m.insert(k.into(), serde_json::json!({ "id": a.id, "tier": tier }));
            }
        }
        serde_json::Value::Object(m)
    }
}

/// R-99 mag sizes by extended-mag tier: none / white / blue / purple+ (retail's ammo_clip_size mods 21/24/27; the kit's
/// data.js MAGS table, Season 31).
const R99_MAGS: [u32; 4] = [18, 21, 24, 27];

pub struct Weapon {
    pub stats: Stats,
    pub attachments: Attachments,
    pub ammo: u32,
    pub time: f32,
    next_shot: f32,
    /// (elapsed, empty reload)
    pub reload: Option<(f32, bool)>,
    /// rounds a reload can take (the inventory's ammo of this type, set by the game every tick); None = endless
    /// (the Firing Range's infinite-ammo toggle, or no inventory). A finished reload takes what it loaded from here.
    pub reserve: Option<u32>,
    /// weapon inspect: (elapsed, empty-mag variant). The game HUD hides while it plays (Apex)
    pub inspect: Option<(f32, bool)>,
    /// deploy animation time, None once it's in the hands
    pub draw: Option<f32>,
    /// the first draw (drawfirst + deployfirst_time) vs later ones (draw + deploy_time)
    draw_first: bool,
    /// lowering for a switch (holster sequence, holster_time)
    pub holster: Option<f32>,
    /// this frame's sequence events (sounds, bodygroups), drained by main.rs
    pub anim_events: Vec<crate::viewmodel::AnimEvent>,
    /// 0 = hip, 1 = fully aimed
    pub ads: f32,
    ads_rising: bool,
    /// the zoom fraction advanced every rendered frame (the sim's `ads` steps at 60 Hz, which made the viewmodel and
    /// the zoom FOV move in visible steps); kept within one tick of `ads`
    pub ads_view: f32,
    /// the jump / land bounce (ACT_VM_JUMP when leaving the ground, ACT_VM_LAND on touching down), until it ends
    air_anim: Option<&'static str>,
    was_on_ground: bool,
    /// recoil: the view punch spring (viewkick.rs); the camera, gun and bullets read it, the aim never moves
    pub punch: crate::viewkick::Punch,
    /// bullets in the current spray (recoil pattern row)
    shots: usize,
    last_shot: f32,
    /// stance spread (degrees) used for the last shot, for the crosshair
    pub spread: f32,
    pub projectiles: Vec<Projectile>,
    pub impacts: VecDeque<Impact>,
    // animation: the engine's playback (anim.rs) driven by the activity this weapon state asks for
    pub anim: crate::anim::Animator,
    /// the activity last sent and the modifiers it was sent with
    sent: &'static str,
    sent_mods: Vec<&'static str>,
    /// shots fired since the last frame (each restarts ACT_VM_PRIMARYATTACK)
    shots_to_animate: u32,
    /// coming back up after a sprint / a lowering (RAISE_FROM_SPRINT / RAISE until the sequence ends)
    raising: Option<&'static str>,
    /// what the last tick saw
    last_inp: WeaponInput,
    /// seconds of raise left: the weapon can't fire until it's up (next-attack time = raise end)
    raise_left: f32,
    crouch_frac: f32,
    pub shots_total: u64,
    /// this tick's events (sounds, effects)
    pub events: Vec<Event>,
    /// eye -> the muzzle as seen on screen, in world units (main.rs, from the viewmodel pose)
    pub muzzle_off: Vec3,
    spraying: bool,
}

const MAX_IMPACTS: usize = crate::gfx::MAX_IMPACTS;
/// impact marks fade out over this long (seconds)
pub const IMPACT_LIFE: f32 = 30.0;

impl Weapon {
    pub fn new(stats: Stats) -> Weapon {
        Weapon {
            ammo: stats.clip,
            stats,
            attachments: Attachments::default(),
            time: 0.0,
            next_shot: 0.0,
            reload: None,
            reserve: None,
            inspect: None,
            draw: Some(0.0),
            draw_first: true,
            holster: None,
            anim_events: Vec::new(),
            ads: 0.0,
            ads_rising: false,
            ads_view: 0.0,
            air_anim: None,
            was_on_ground: true,
            punch: Default::default(),
            shots: 0,
            last_shot: -10.0,
            spread: 0.0,
            projectiles: Vec::new(),
            impacts: VecDeque::new(),
            anim: Default::default(),
            sent: "",
            sent_mods: Vec::new(),
            shots_to_animate: 0,
            raising: None,
            last_inp: WeaponInput::default(),
            raise_left: 0.0,
            crouch_frac: 0.0,
            shots_total: 0,
            events: Vec::new(),
            muzzle_off: Vec3::ZERO,
            spraying: false,
        }
    }

    /// Console `weapon_pose`: jump to a pose (screenshots): hip, ads, draw, drawfirst, holster, reload, reload_empty,
    /// inspect, inspect_empty, fire, sprint, sprintraise, lower, raise, or any sequence name; `t` seconds in.
    pub fn debug_pose(&mut self, vm: &Viewmodel, name: &str, t: f32) {
        self.draw = None;
        self.holster = None;
        self.reload = None;
        self.inspect = None;
        self.raising = None;
        self.ads = if name == "ads" { 1.0 } else { 0.0 };
        self.ads_view = self.ads;
        self.ads_rising = name == "ads";
        let act = match name {
            "hip" | "ads" | "idle" => "ACT_VM_IDLE",
            "draw" => "ACT_VM_DRAW",
            "drawfirst" => "ACT_VM_DRAWFIRST",
            "holster" => "ACT_VM_HOLSTER",
            "reload" => "ACT_VM_RELOAD",
            "reload_empty" => "ACT_VM_RELOADEMPTY",
            "inspect" | "inspect_empty" => "ACT_VM_WEAPON_INSPECT",
            "fire" => "ACT_VM_PRIMARYATTACK",
            "sprint" => "ACT_VM_SPRINT",
            "sprintraise" => "ACT_VM_RAISE_FROM_SPRINT",
            "lower" => "ACT_VM_LOWER",
            "raise" => "ACT_VM_RAISE",
            _ => "",
        };
        let seq = if act.is_empty() { vm.seq_by_name.get(name).copied() } else { vm.pick_seq(act, &[], 0.0) };
        if let Some(seq) = seq {
            self.anim.force(vm, seq, t);
            self.sent = "debug";
        }
    }

    /// Rounds a full mag holds with the current extended mag.
    pub fn clip(&self) -> u32 {
        match &self.attachments.mag {
            None => self.stats.clip,
            Some(a) => R99_MAGS[(a.tier as usize).clamp(1, 3)],
        }
    }

    /// Changes an attachment; the mag size follows (ammo is kept, capped at the new size).
    pub fn attach(&mut self, point: &str, a: Option<Attachment>) -> Result<(), String> {
        *self.attachments.get_mut(point).ok_or_else(|| format!("no attachment point {point} (barrel, mag, optic, stock)"))? = a;
        self.ammo = self.ammo.min(self.clip());
        Ok(())
    }

    /// Impact marks for the renderer: (pos, size, normal, alpha, surface, seed, 0, 0); decals.rs resolves the decal.
    pub fn impact_instances(&self) -> Vec<[f32; 12]> {
        self.impacts
            .iter()
            .filter_map(|i| {
                let age = self.time - i.time;
                let a = (1.0 - (age - IMPACT_LIFE * 0.7).max(0.0) / (IMPACT_LIFE * 0.3)).clamp(0.0, 1.0);
                (a > 0.0).then(|| [i.pos.x, i.pos.y, i.pos.z, 1.6, i.normal.x, i.normal.y, i.normal.z, a, i.surface as f32, (i.seed & 0xFF_FFFF) as f32, 0.0, 0.0])
            })
            .collect()
    }

    /// Leave a console `weapon_pose` freeze: the activity logic runs again (weapon_probe).
    pub fn end_debug(&mut self) {
        if self.sent == "debug" {
            self.sent = "";
        }
    }

    pub fn busy(&self) -> bool {
        self.draw.is_some() || self.reload.is_some() || self.holster.is_some() || self.last_inp.lowered || self.raise_left > 0.0
    }

    /// Start lowering it (switching to the melee slot); `holstered()` once the holster time is up.
    pub fn start_holster(&mut self) {
        self.holster = Some(0.0);
        self.inspect = None;
        self.reload = None;
        self.ads_rising = false;
    }

    pub fn holstered(&self) -> bool {
        self.holster.is_some_and(|t| t >= self.stats.holster_time)
    }

    /// Bring it back up (after the melee slot lowered): the plain draw sequence, deploy_time.
    pub fn deploy(&mut self) {
        self.holster = None;
        self.draw = Some(0.0);
        self.draw_first = false;
        self.ads = 0.0;
        self.spraying = false;
    }

    /// One 60 Hz tick. `eye`/`fwd` = where bullets come from. Returns the permanent view change (pitch, yaw degrees):
    /// retail's viewkick_perm_*, 0 on the R-99; the recoil itself is the punch spring (`punch`), which returns.
    pub fn tick(&mut self, dt: f32, inp: WeaponInput, eye: Vec3, yaw: f32, pitch: f32, col: &Collision) -> (f32, f32) {
        self.time += dt;
        self.last_inp = inp;
        let st = &self.stats;
        if let Some(t) = &mut self.draw {
            *t += dt;
            if *t >= if self.draw_first { st.deploy } else { st.deploy_again } {
                self.draw = None;
            }
        }
        if let Some(t) = &mut self.holster {
            *t += dt;
        }
        // reload
        if let Some((t, empty)) = &mut self.reload {
            *t += dt;
            let dur = if *empty { st.reload_empty } else { st.reload };
            if *t >= dur {
                self.reload = None;
                let need = self.clip().saturating_sub(self.ammo);
                let got = self.reserve.map_or(need, |r| need.min(r));
                if let Some(r) = &mut self.reserve {
                    *r -= got;
                }
                self.ammo += got;
            }
        } else if self.draw.is_none() && self.reserve != Some(0) && ((inp.reload && self.ammo < self.clip()) || (inp.fire && self.ammo == 0)) {
            if inp.fire && self.ammo == 0 && !inp.reload {
                self.events.push(Event::DryFire);
            }
            self.reload = Some((0.0, self.ammo == 0));
            self.events.push(Event::Reload { empty: self.ammo == 0 });
            self.shots = 0;
        }
        // inspect: N when the gun is idle in the hands; firing, aiming, reloading or sprinting cuts it (the end is
        // found in pose(), which knows the sequence length)
        if let Some((t, _)) = &mut self.inspect {
            *t += dt;
            if inp.fire || inp.ads || inp.sprinting || self.busy() {
                self.inspect = None;
            }
        } else if inp.inspect && !inp.sprinting && !self.busy() && self.ads <= 0.0 {
            self.inspect = Some((0.0, self.ammo == 0));
            self.events.push(Event::Inspect);
        }
        // ADS (not while sprinting, reloading or drawing)
        let want_ads = inp.ads && !inp.sprinting && !self.busy();
        let rate = if want_ads { 1.0 / st.zoom_in.max(0.01) } else { -1.0 / st.zoom_out.max(0.01) };
        if want_ads != self.ads_rising {
            self.events.push(if want_ads { Event::AdsIn } else { Event::AdsOut });
        }
        self.ads_rising = want_ads;
        self.ads = (self.ads + rate * dt).clamp(0.0, 1.0);
        // spread for this stance
        self.spread = if self.ads > 0.5 {
            if !inp.on_ground { st.spread_ads[2] } else if inp.crouched { st.spread_ads[1] } else { st.spread_ads[0] }
        } else if !inp.on_ground {
            st.spread_hip[4]
        } else if inp.sprinting {
            st.spread_hip[2]
        } else if inp.crouched {
            st.spread_hip[3]
        } else if inp.moving {
            st.spread_hip[1]
        } else {
            st.spread_hip[0]
        };
        // fire
        let kick = (0.0, 0.0);
        let period = 1.0 / st.fire_rate.max(0.1);
        if self.time - self.last_shot > period * 2.0 + 0.1 {
            self.shots = 0;
        }
        let can_fire = inp.fire && !inp.sprinting && !self.busy() && self.ammo > 0;
        if self.spraying && !can_fire {
            self.spraying = false;
            self.events.push(Event::SprayEnd);
        }
        if can_fire && self.time >= self.next_shot {
            if self.next_shot < self.time - period {
                self.next_shot = self.time;
            }
            self.events.push(Event::Shot { first: !self.spraying });
            self.spraying = true;
            self.next_shot += period;
            self.ammo -= 1;
            self.shots_total += 1;
            self.last_shot = self.time;
            self.shots_to_animate += 1;
            // spread: a uniform point in the cone
            let h = (self.shots_total as u32).wrapping_mul(2654435761) ^ (self.time.to_bits());
            let r1 = ((h >> 8) & 0xFFFF) as f32 / 65535.0;
            let r2 = ((h.wrapping_mul(1597334677) >> 8) & 0xFFFF) as f32 / 65535.0;
            let ang = self.spread.to_radians() * 0.5 * r1.sqrt();
            // bullets fly along the eye angles + the WHOLE punch (FUN_140b09870)
            let (yaw, pitch) = (yaw + self.punch.ang.y, (pitch + self.punch.ang.x).clamp(-89.0, 89.0));
            let fwd = shared::sim::forward_vec(yaw, pitch);
            let right = shared::sim::forward_vec(yaw - 90.0, 0.0);
            let up = right.cross(fwd).normalize();
            let phi = r2 * std::f32::consts::TAU;
            let dir = (fwd * ang.cos() + (right * phi.cos() + up * phi.sin()) * ang.sin()).normalize();
            self.projectiles.push(Projectile { pos: eye, vel: dir * st.speed, life: 3.0, age: 0.0, muzzle_off: self.muzzle_off });
            // recoil: this bullet's pattern row kicks the punch spring
            self.punch.kick(&st.viewkick, &st.kick, st.kick_loop, self.ads, !inp.on_ground, self.time);
            self.shots += 1;
        }
        // bullets
        let g = 750.0 * st.gravity_scale;
        for p in &mut self.projectiles {
            let start = p.pos;
            p.vel.z -= g * dt;
            p.vel *= 1.0 - (st.drag * dt).min(0.5);
            let end = start + p.vel * dt;
            let tr = col.trace_hull(start, end, Vec3::ZERO, Vec3::ZERO);
            if tr.hit() && !tr.startsolid {
                if self.impacts.len() >= MAX_IMPACTS {
                    self.impacts.pop_front();
                }
                self.impacts.push_back(Impact { pos: tr.endpos, normal: tr.normal, time: self.time, surface: tr.surface, seed: self.shots_total as u32 });
                self.events.push(Event::Impact { pos: tr.endpos, normal: tr.normal });
                p.life = 0.0;
            } else {
                p.pos = end;
                p.life -= dt;
                p.age += dt;
            }
        }
        self.projectiles.retain(|p| p.life > 0.0);
        self.raise_left = (self.raise_left - dt).max(0.0);
        self.last_inp = inp;
        kick
    }

    /// The activity this weapon's state asks the viewmodel for (SendWeaponAnim's argument), by priority.
    fn ideal_activity(&self) -> &'static str {
        let sprinting = self.last_inp.sprinting;
        if self.holster.is_some() {
            if sprinting { "ACT_VM_HOLSTER_FROM_SPRINT" } else { "ACT_VM_HOLSTER" }
        } else if self.draw.is_some() {
            if self.draw_first {
                "ACT_VM_DRAWFIRST"
            } else if sprinting {
                "ACT_VM_DRAW_TO_SPRINT"
            } else {
                "ACT_VM_DRAW"
            }
        } else if self.last_inp.lowered {
            "ACT_VM_LOWER"
        } else if let Some((_, empty)) = self.reload {
            if empty { "ACT_VM_RELOADEMPTY" } else { "ACT_VM_RELOAD" }
        } else if self.inspect.is_some() {
            "ACT_VM_WEAPON_INSPECT"
        } else if sprinting {
            "ACT_VM_SPRINT"
        } else if let Some(r) = self.raising {
            r
        } else if self.sent == "ACT_VM_PRIMARYATTACK" {
            "ACT_VM_PRIMARYATTACK"
        } else if let Some(a) = self.air_anim {
            a
        } else {
            "ACT_VM_IDLE"
        }
    }

    /// The setting each activity's sequence is stretched to (DesiredDuration, FUN_140707230); 0 = its own length.
    fn desired_duration(st: &Stats, act: &str) -> f32 {
        match act {
            "ACT_VM_DRAW" | "ACT_VM_DRAW_TO_SPRINT" => st.deploy_again,
            "ACT_VM_DRAWFIRST" => st.deploy,
            "ACT_VM_HOLSTER" | "ACT_VM_HOLSTER_FROM_SPRINT" => st.holster_time,
            "ACT_VM_RAISE" => st.raise_time,
            "ACT_VM_RAISE_FROM_SPRINT" => st.raise_from_sprint_time,
            "ACT_VM_LOWER" => st.lower_time,
            "ACT_VM_SPRINT" => st.sprintcycle_time,
            "ACT_VM_RELOAD" => st.reload,
            "ACT_VM_RELOADEMPTY" => st.reload_empty,
            _ => 0.0,
        }
    }

    /// Advances the animation by the frame time and returns this frame's pose (local, rig space).
    pub fn pose(&mut self, vm: &Viewmodel, dt: f32, out: &mut Vec<(Vec3, Quat)>) {
        {
            let st = &self.stats;
            let rate = if self.ads_rising { 1.0 / st.zoom_in.max(0.01) } else { -1.0 / st.zoom_out.max(0.01) };
            let step = rate.abs() * crate::TICK;
            self.ads_view = (self.ads_view + rate * dt).clamp(0.0, 1.0).clamp(self.ads - step, self.ads + step).clamp(0.0, 1.0);
            self.punch.update(&st.viewkick, self.ads_view, self.time, dt);
        }
        let inp = self.last_inp;
        // activity modifiers (the player's state)
        let mut mods: Vec<&'static str> = Vec::new();
        if inp.crouched {
            mods.push("crouch");
        }
        if !inp.on_ground {
            mods.push("air");
        }
        if inp.sprinting {
            mods.push("sprinting");
        }
        if inp.sliding {
            mods.push("sliding");
        }
        if !inp.on_ground && inp.sprinting {
            mods.push("airsprint");
        }
        // jump / land bounce: the rig's ACT_VM_JUMP / ACT_VM_LAND, played once on the ground change
        if inp.on_ground != self.was_on_ground {
            self.air_anim = Some(if inp.on_ground { "ACT_VM_LAND" } else { "ACT_VM_JUMP" });
            log::debug!("weapon: ground change -> {:?} (sent {})", self.air_anim, self.sent);
            self.was_on_ground = inp.on_ground;
        }
        // leaving a sprint / a lowering raises the weapon (StopSprint -> Raise(fromSprint), UpdateWeaponsLowered)
        if self.sent == "ACT_VM_SPRINT" && !inp.sprinting {
            self.raising = Some("ACT_VM_RAISE_FROM_SPRINT");
        }
        if self.sent == "ACT_VM_LOWER" && !inp.lowered {
            self.raising = Some("ACT_VM_RAISE");
        }
        let st = &self.stats;
        let dur = |act: &str, _seq: usize| Self::desired_duration(st, act);
        if self.sent != "debug" {
            // every shot restarts the fire sequence (FUN_1406f73a0)
            if self.shots_to_animate > 0 && !inp.sprinting && self.holster.is_none() {
                self.anim.send(vm, "ACT_VM_PRIMARYATTACK", &mods, &dur);
                self.sent = "ACT_VM_PRIMARYATTACK";
                self.raising = None;
            }
            // one-shots that ran out fall back to idle
            if self.air_anim.is_some_and(|a| a == self.sent) && self.anim.done(vm) {
                self.air_anim = None;
            }
            if (self.sent == "ACT_VM_PRIMARYATTACK" || self.raising.is_some_and(|r| r == self.sent)) && self.anim.done(vm) {
                self.raising = None;
                if self.sent == "ACT_VM_PRIMARYATTACK" {
                    self.sent = "";
                }
            }
            let want = self.ideal_activity();
            if want != self.sent || (want == "ACT_VM_IDLE" && mods != self.sent_mods) || (self.raising == Some(want) && self.anim.ideal_act != want) {
                let ok = self.anim.send(vm, want, &mods, &dur);
                if want == "ACT_VM_JUMP" || want == "ACT_VM_LAND" {
                    log::debug!("weapon: send {want} {mods:?} -> {ok}");
                }
                if ok {
                    self.sent = want;
                    self.sent_mods = mods.clone();
                    if self.raising == Some(want) {
                        // the next attack waits for the raise (next-attack time = its end)
                        self.raise_left = self.anim.remaining(vm);
                    }
                } else if want != "ACT_VM_IDLE" {
                    // no sequence for it in this rig: fall back to idle (as the game does for LOWER / RAISE)
                    self.anim.send(vm, "ACT_VM_IDLE", &mods, &dur);
                    self.sent = want;
                }
            }
            // an inspect ends with its sequence
            if self.sent == "ACT_VM_WEAPON_INSPECT" && self.anim.done(vm) {
                self.inspect = None;
            }
        }
        self.shots_to_animate = 0;
        self.anim.update(vm, dt);
        self.anim_events.extend(self.anim.events.drain(..));
        // pose parameters
        let k = (dt / 0.2).min(1.0);
        self.crouch_frac += ((inp.crouched as u32 as f32) - self.crouch_frac) * k;
        let params = crate::anim::Params { ads: self.ads_view, crouch: self.crouch_frac, velocity: inp.speed, sprint: inp.sprinting as u32 as f32 };
        // The gun's idle / fire sequences are deltas authored against the iron ADS-in pose (hip) and ADS-out pose
        // (aimed) ("idle ... sub ptpov_r97_iron_ads_in"): they add onto the hip <-> ADS pose, sampled by the zoom
        // fraction through ads_in / ads_out. Absolute sequences (draw, reload, sprint...) replace it.
        let mut base = vm.bind_pose();
        let (seq, cycle) = if self.ads_rising { (vm.seq_by_name.get("ads_in"), self.ads_view) } else { (vm.seq_by_name.get("ads_out"), 1.0 - self.ads_view) };
        if let Some(&s) = seq {
            vm.sample_seq(s, cycle.clamp(0.0, 0.999), &|n| match n {
                "crouchFraction" => self.crouch_frac,
                _ => 0.0,
            }, &mut base);
        }
        self.anim.pose(vm, &params, Some(&base), out);
    }
}

/// The HUD name for a weapon's printname localization token (no localization files read yet).
fn display_name(token: &str) -> String {
    match token.trim_start_matches('#') {
        "WPN_R97" | "" => "R-99".into(),
        t => t.trim_start_matches("WPN_").replace('_', " "),
    }
}

