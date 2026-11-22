//! The melee slot: empty hands or an heirloom (Wraith's kunai, Octane's balisong, the Grand Slam bat), with the retail
//! weapon's settings (melee_*.txt) and its own first-person sequences (tools/build_melee.py -> `melee/<id>/view.bin`),
//! played the way the game plays them (anim.rs): the state picks an activity, the activity a sequence (modifiers +
//! weights), the node graph adds the transitions (the kunai's grip flip into a sprint), the transitioner fades.
//!
//! Swing flow (S21, sequence_playback.md item 10): ACT_VM_MELEE_ATTACK2, timed so its AE_MELEE_ATTACK event lands at
//! 0.5 s (melee_attack_animtime 0), then ACT_VM_RAISE_FROM_MELEE stretched to melee_raise_recovery_animtime_normal,
//! then idle. Sprinting plays ACT_VM_SPRINT (through idle_to_sprint...) and leaving it ACT_VM_RAISE_FROM_SPRINT.

use crate::anim::{Animator, Params};
use crate::viewmodel::{AnimEvent, Viewmodel};
use glam::{Quat, Vec3};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct MeleeStats {
    pub name: String,
    pub holster_time: f32,
    pub deploy_time: f32,
    pub deployfirst_time: f32,
    pub raise_time: f32,
    pub lower_time: f32,
    pub raise_from_sprint_time: f32,
    pub sprintcycle_time: f32,
    pub damage: f32,
    pub range: f32,
    pub lunge_range: f32,
    pub lunge_time: f32,
    pub move_speed: f32,
    /// melee_attack_animtime (0 = time the swing so its hit lands at 0.5 s)
    pub attack_time: f32,
    pub raise_recovery: f32,
    /// viewmodel sway / bob (the held item's mp_weapon_*_primary settings)
    pub sway: crate::sway::Settings,
}

impl MeleeStats {
    pub fn load(path: &Path) -> Result<MeleeStats, String> {
        let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())?;
        let s = &j["settings"];
        let f = |k: &str, d: f32| s[k].as_str().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(d);
        Ok(MeleeStats {
            name: j["display_name"].as_str().unwrap_or("Melee").to_string(),
            holster_time: f("holster_time", 0.3),
            // missing = 0 = the sequence plays at its natural length (DesiredDuration <= 0 -> rate 1): retail sets no
            // deploy_time / raise_time on heirlooms, whose draws are 1-3 s flourishes
            deploy_time: f("deploy_time", 0.0),
            deployfirst_time: f("deployfirst_time", f("deploy_time", 0.0)),
            raise_time: f("raise_time", 0.0),
            lower_time: f("lower_time", 0.3),
            raise_from_sprint_time: f("raise_from_sprint_time", 0.0),
            sprintcycle_time: f("sprintcycle_time", 0.0),
            damage: f("melee_damage", 30.0),
            range: f("melee_range", 60.0),
            lunge_range: f("melee_lunge_target_range", 60.0),
            lunge_time: f("melee_lunge_time", 0.35),
            move_speed: f("move_speed_modifier", 1.0),
            attack_time: f("melee_attack_animtime", 0.0),
            raise_recovery: f("melee_raise_recovery_animtime_normal", 0.75),
            sway: crate::sway::Settings::from_json(s),
        })
    }
}

/// What the melee item did this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeleeEvent {
    Swing,
    Inspect,
}

#[derive(Default, Clone, Copy)]
pub struct MeleeInput {
    pub attack: bool,
    pub inspect: bool,
    pub sprinting: bool,
    pub crouched: bool,
    pub air: bool,
    pub sliding: bool,
    /// mantling / wall climbing (the game lowers held items)
    pub lowered: bool,
    pub speed: f32,
}

/// What the item is doing (decides the activity).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Draw { first: bool },
    Holster,
    Attack,
    RaiseFromMelee,
    Inspect,
}

pub struct Melee {
    pub id: String,
    pub stats: MeleeStats,
    state: State,
    /// seconds in the current state (holster time, deploy time)
    state_t: f32,
    drawn_once: bool,
    pub anim: Animator,
    sent: String,
    sent_mods: Vec<&'static str>,
    raising: Option<&'static str>,
    inp: MeleeInput,
    crouch_frac: f32,
    /// a swing asked for since the last frame (sent from `pose`, which has the frame's state)
    swing_pending: bool,
    pub events: Vec<MeleeEvent>,
    /// this frame's sequence events (sounds, props, particles), drained by main.rs
    pub anim_events: Vec<AnimEvent>,
}

impl Melee {
    pub fn new(id: &str, stats: MeleeStats, _vm: &Viewmodel) -> Melee {
        Melee {
            id: id.to_string(),
            stats,
            state: State::Idle,
            state_t: 0.0,
            drawn_once: false,
            anim: Animator::default(),
            sent: String::new(),
            sent_mods: Vec::new(),
            raising: None,
            inp: MeleeInput::default(),
            crouch_frac: 0.0,
            swing_pending: false,
            events: Vec::new(),
            anim_events: Vec::new(),
        }
    }

    fn set(&mut self, s: State) {
        self.state = s;
        self.state_t = 0.0;
    }

    /// Bring it out (after the other slot lowered): drawfirst the first time.
    pub fn deploy(&mut self, _vm: &Viewmodel) {
        let first = !self.drawn_once;
        self.drawn_once = true;
        self.set(State::Draw { first });
    }

    pub fn start_holster(&mut self, _vm: &Viewmodel) {
        self.set(State::Holster);
    }

    pub fn holstered(&self) -> bool {
        self.state == State::Holster && self.state_t >= self.stats.holster_time
    }

    pub fn attacking(&self) -> bool {
        matches!(self.state, State::Attack) || self.swing_pending
    }

    pub fn inspecting(&self) -> bool {
        self.state == State::Inspect
    }

    fn drawing(&self) -> bool {
        matches!(self.state, State::Draw { first } if self.state_t < if first { self.stats.deployfirst_time } else { self.stats.deploy_time })
    }

    /// Swing now (quick melee from the gun, or the attack button with it out).
    pub fn swing(&mut self, _vm: &Viewmodel) {
        self.set(State::Attack);
        self.swing_pending = true;
        self.events.push(MeleeEvent::Swing);
    }

    pub fn tick(&mut self, dt: f32, inp: MeleeInput, vm: &Viewmodel) {
        self.inp = inp;
        self.state_t += dt;
        // the next swing waits for the raise after the last one (next-attack time = its end)
        let busy = matches!(self.state, State::Holster | State::Attack | State::RaiseFromMelee) || self.drawing() || inp.lowered;
        if inp.attack && !busy {
            self.swing(vm);
        }
        if self.state == State::Inspect && (inp.attack || inp.sprinting) {
            self.set(State::Idle);
        }
        if inp.inspect && !inp.sprinting && self.state == State::Idle && self.raising.is_none() {
            self.set(State::Inspect);
            self.events.push(MeleeEvent::Inspect);
        }
    }

    fn ideal_activity(&self) -> &'static str {
        let sprinting = self.inp.sprinting;
        match self.state {
            State::Holster => if sprinting { "ACT_VM_HOLSTER_FROM_SPRINT" } else { "ACT_VM_HOLSTER" },
            State::Draw { first: true } => "ACT_VM_DRAWFIRST",
            State::Draw { first: false } => if sprinting { "ACT_VM_DRAW_TO_SPRINT" } else { "ACT_VM_DRAW" },
            State::Attack => "ACT_VM_MELEE_ATTACK2",
            State::RaiseFromMelee => "ACT_VM_RAISE_FROM_MELEE",
            State::Inspect => "ACT_VM_WEAPON_INSPECT",
            State::Idle => {
                if self.inp.lowered {
                    "ACT_VM_LOWER"
                } else if sprinting {
                    "ACT_VM_SPRINT"
                } else if let Some(r) = self.raising {
                    r
                } else {
                    "ACT_VM_IDLE"
                }
            }
        }
    }

    /// DesiredDuration (FUN_140707230) for this item's activities; 0 = the sequence's own length.
    fn desired(st: &MeleeStats, vm: &Viewmodel, act: &str, seq: usize) -> f32 {
        match act {
            "ACT_VM_DRAW" | "ACT_VM_DRAW_TO_SPRINT" => st.deploy_time,
            "ACT_VM_DRAWFIRST" => st.deployfirst_time,
            "ACT_VM_HOLSTER" | "ACT_VM_HOLSTER_FROM_SPRINT" => st.holster_time,
            "ACT_VM_RAISE" => st.raise_time,
            "ACT_VM_RAISE_FROM_SPRINT" => st.raise_from_sprint_time,
            "ACT_VM_LOWER" => st.lower_time,
            "ACT_VM_SPRINT" => st.sprintcycle_time,
            "ACT_VM_RAISE_FROM_MELEE" => st.raise_recovery,
            "ACT_VM_MELEE_ATTACK1" | "ACT_VM_MELEE_ATTACK2" | "ACT_VM_MELEE_ATTACK3" => {
                if st.attack_time > 0.0 {
                    st.attack_time
                } else {
                    // stretched so the first AE_MELEE_ATTACK lands at the lunge time (0.5 s)
                    let s = &vm.seqs[seq];
                    let hit = vm.events.get(&s.name).and_then(|v| v.iter().find(|e| e.kind == "AE_MELEE_ATTACK")).map(|e| e.t);
                    match hit {
                        Some(h) if h > 0.0 => s.duration * 0.5 / h,
                        _ => 0.0,
                    }
                }
            }
            _ => 0.0,
        }
    }

    /// Advances the animation by the frame time; this frame's pose (local, rig space).
    pub fn pose(&mut self, vm: &Viewmodel, dt: f32, out: &mut Vec<(Vec3, Quat)>) {
        let inp = self.inp;
        let mut mods: Vec<&'static str> = Vec::new();
        if inp.crouched {
            mods.push("crouch");
        }
        if inp.air {
            mods.push("air");
        }
        if inp.sprinting {
            mods.push("sprinting");
        }
        if inp.sliding {
            mods.push("sliding");
        }
        if inp.air && inp.sprinting {
            mods.push("airsprint");
        }
        if self.sent == "ACT_VM_SPRINT" && !inp.sprinting && self.state == State::Idle {
            self.raising = Some("ACT_VM_RAISE_FROM_SPRINT");
        }
        if self.sent == "ACT_VM_LOWER" && !inp.lowered && self.state == State::Idle {
            self.raising = Some("ACT_VM_RAISE");
        }
        if self.sent != "debug" {
            // one-shot states end with their sequence
            let done = self.anim.done(vm);
            match self.state {
                State::Attack if !self.swing_pending && done && self.sent == "ACT_VM_MELEE_ATTACK2" => self.set(State::RaiseFromMelee),
                State::RaiseFromMelee if done && self.sent == "ACT_VM_RAISE_FROM_MELEE" => self.set(State::Idle),
                State::Inspect if done && self.sent == "ACT_VM_WEAPON_INSPECT" => self.set(State::Idle),
                State::Draw { .. } if done && self.sent.starts_with("ACT_VM_DRAW") && !self.drawing() => self.set(State::Idle),
                _ => {}
            }
            if let Some(r) = self.raising {
                if self.sent == r && done {
                    self.raising = None;
                }
            }
            let st = self.stats.clone();
            let dur = |act: &str, seq: usize| Self::desired(&st, vm, act, seq);
            let want = self.ideal_activity();
            let restart = self.swing_pending;
            self.swing_pending = false;
            if want != self.sent || restart || (want == "ACT_VM_IDLE" && mods != self.sent_mods) || (self.raising == Some(want) && self.anim.ideal_act != want) {
                let ok = self.anim.send(vm, want, &mods, &dur) || (want == "ACT_VM_MELEE_ATTACK2" && self.anim.send(vm, "ACT_VM_MELEE_ATTACK1", &mods, &dur));
                if !ok && want != "ACT_VM_IDLE" {
                    self.anim.send(vm, "ACT_VM_IDLE", &mods, &dur);
                }
                self.sent = want.to_string();
                self.sent_mods = mods.clone();
            }
        }
        self.anim.update(vm, dt);
        self.anim_events.extend(self.anim.events.drain(..));
        let k = (dt / 0.2).min(1.0);
        self.crouch_frac += ((inp.crouched as u32 as f32) - self.crouch_frac) * k;
        let params = Params { ads: 0.0, crouch: self.crouch_frac, velocity: inp.speed, sprint: inp.sprinting as u32 as f32 };
        self.anim.pose(vm, &params, None, out);
    }

    /// Console `melee_pose`: an activity (draw, drawfirst, holster, inspect, attack, raise, sprint, idle) or a sequence.
    pub fn debug_pose(&mut self, vm: &Viewmodel, name: &str, t: f32) {
        self.set(State::Idle);
        self.raising = None;
        let act = match name {
            "draw" => "ACT_VM_DRAW",
            "drawfirst" => "ACT_VM_DRAWFIRST",
            "holster" => "ACT_VM_HOLSTER",
            "inspect" => "ACT_VM_WEAPON_INSPECT",
            "attack" => "ACT_VM_MELEE_ATTACK2",
            "raise" => "ACT_VM_RAISE_FROM_MELEE",
            "sprint" => "ACT_VM_SPRINT",
            "idle" => "ACT_VM_IDLE",
            _ => "",
        };
        let seq = if act.is_empty() { vm.seq_by_name.get(name).copied() } else { vm.pick_seq(act, &[], 0.0) };
        if let Some(seq) = seq {
            self.anim.force(vm, seq, t);
            self.sent = "debug".into();
        }
        self.drawn_once = true;
    }
}
