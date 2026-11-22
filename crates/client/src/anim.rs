//! The viewmodel's animation playback, as the S21 client does it (reversed in Ghidra + the R5Flowstate SDK; the notes
//! with addresses are in D:\R5Rust\_work_retail\anim_re\sequence_playback.md):
//!
//! - SendWeaponAnim (FUN_1406eb570): an activity picks a sequence (modifier scoring, weighted random). Re-sending the
//!   idle that resolves to the playing sequence doesn't restart it. If the playing sequence's exit node isn't the goal's
//!   entry node, the node graph's transition sequence plays first at rate 1 (the kunai's grip flip), then the goal.
//!   Draw activities never go through a transition. Rate = sequence length / the setting's duration (capped at 12).
//! - The sequence transitioner (FUN_140aba4d0..140abb2b0): on a sequence change the previous one keeps PLAYING as a
//!   fading layer over min(prev.fadeout, new.fadein) (default 0.2 s), weight = smoothstep, multiplied down the stack
//!   (up to 6 old layers), blended over the current one newest to oldest.
//! - StandardBlendingRules (FUN_1407620e0): reference pose -> current sequence at weight 1 -> old layers -> autoplay
//!   sequences. A sequence's `addlayer`s play with its cycle and weight; `delta` sequences add onto the pose.

use crate::viewmodel::{self, AnimEvent, Viewmodel};
use glam::{Quat, Vec3};

/// The pose parameters the viewmodel sequences blend on.
#[derive(Clone, Copy, Default, Debug)]
pub struct Params {
    /// ads_blend: the zoom fraction (linear over zoom_time_in / out)
    pub ads: f32,
    /// crouchFraction
    pub crouch: f32,
    /// velocity: the player's speed (u/s)
    pub velocity: f32,
    /// sprintfrac
    pub sprint: f32,
}

impl Params {
    fn get(&self, name: &str) -> f32 {
        match name {
            "ads_blend" => self.ads,
            "crouchFraction" => self.crouch,
            "velocity" => self.velocity,
            "sprintfrac" => self.sprint,
            _ => 0.0,
        }
    }
}

struct Old {
    seq: usize,
    start_cycle: f32,
    start: f32,
    rate: f32,
    fade: f32,
}

#[derive(Clone, Copy)]
struct Cur {
    seq: usize,
    start: f32,
    rate: f32,
}

pub struct Animator {
    now: f32,
    cur: Option<Cur>,
    /// fading previous sequences, oldest first
    old: Vec<Old>,
    /// the activity asked for and the sequence it resolved to (a transition may be playing on the way)
    pub ideal_act: String,
    ideal_seq: Option<usize>,
    ideal_rate: f32,
    in_transition: bool,
    clock: viewmodel::EventClock,
    seed: u32,
    /// sequence events crossed this frame (sounds, bodygroups, particles), drained by the owner
    pub events: Vec<AnimEvent>,
}

impl Animator {
    /// Debug: the current sequence (cycle) and the fading ones (weight), for anim_probe.
    pub fn describe(&self, vm: &Viewmodel) -> String {
        let name = |s: usize| vm.seqs.get(s).map_or("?", |q| q.name.as_str()).to_string();
        let mut out = match self.cur {
            Some(c) => format!("{} @{:.2}", name(c.seq), (self.now - c.start) * c.rate / vm.seqs[c.seq].duration.max(1e-4)),
            None => "-".into(),
        };
        for o in &self.old {
            out.push_str(&format!(" | old {} fade {:.2}", name(o.seq), o.fade));
        }
        out
    }
}

impl Default for Animator {
    fn default() -> Self {
        Animator { now: 0.0, cur: None, old: Vec::new(), ideal_act: String::new(), ideal_seq: None, ideal_rate: 1.0, in_transition: false, clock: Default::default(), seed: 0x6C07_8965, events: Vec::new() }
    }
}

fn smooth(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// The draw family never plays a transition first (they start from the holstered pose).
fn is_draw(act: &str) -> bool {
    matches!(act, "ACT_VM_DRAW" | "ACT_VM_DRAW_TO_SPRINT" | "ACT_VM_DRAWFIRST" | "ACT_VM_DRAWCATCH" | "ACT_VM_ONEHANDED_DRAW" | "ACT_VM_ONEHANDED_DRAWFIRST")
}

impl Animator {
    fn rnd(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / 16_777_216.0
    }

    fn cycle_of(vm: &Viewmodel, seq: usize, start_cycle: f32, start: f32, rate: f32, now: f32) -> f32 {
        let s = &vm.seqs[seq];
        let c = start_cycle + (now - start).max(0.0) * rate / s.duration.max(1e-3);
        if s.looping() { c.fract() } else { c.clamp(0.0, 0.999) }
    }

    /// The playing sequence and its cycle.
    pub fn current(&self, vm: &Viewmodel) -> Option<(usize, f32)> {
        self.cur.map(|c| (c.seq, Self::cycle_of(vm, c.seq, 0.0, c.start, c.rate, self.now)))
    }

    /// The playing (non-looping) sequence reached its end.
    pub fn finished(&self, vm: &Viewmodel) -> bool {
        match self.cur {
            Some(c) => !vm.seqs[c.seq].looping() && (self.now - c.start) * c.rate >= vm.seqs[c.seq].duration * 0.999,
            None => true,
        }
    }

    /// The goal of the last activity sent (the sequence it resolved to) has finished playing.
    pub fn done(&self, vm: &Viewmodel) -> bool {
        !self.in_transition && self.finished(vm)
    }

    /// Seconds left on the playing sequence at its rate.
    pub fn remaining(&self, vm: &Viewmodel) -> f32 {
        match self.cur {
            Some(c) => (vm.seqs[c.seq].duration / c.rate.max(1e-3) - (self.now - c.start)).max(0.0),
            None => 0.0,
        }
    }

    fn set_sequence(&mut self, vm: &Viewmodel, seq: usize, rate: f32) {
        if let Some(c) = self.cur {
            let new = &vm.seqs[seq];
            if new.snap() {
                self.old.clear();
            } else {
                let fade = vm.seqs[c.seq].fadeout.min(new.fadein);
                if fade > 0.0 {
                    if self.old.len() >= 6 {
                        self.old.remove(0);
                    }
                    let start_cycle = Self::cycle_of(vm, c.seq, 0.0, c.start, c.rate, self.now);
                    self.old.push(Old { seq: c.seq, start_cycle, start: self.now, rate: c.rate, fade });
                }
            }
        }
        self.cur = Some(Cur { seq, start: self.now, rate });
        self.clock.reset();
    }

    /// SendWeaponAnim: play `act` (picked by `mods`); `duration(act, seq)` = the setting's time for it (<= 0: the
    /// sequence's own length). Returns false if the rig has no sequence for the activity.
    pub fn send(&mut self, vm: &Viewmodel, act: &str, mods: &[&str], duration: &dyn Fn(&str, usize) -> f32) -> bool {
        let r = self.rnd();
        let Some(goal) = vm.pick_seq(act, mods, r) else { return false };
        let idle = act.starts_with("ACT_VM_IDLE") || act == "ACT_VM_ONEHANDED_IDLE";
        if Some(goal) == self.ideal_seq && idle && !self.in_transition && self.cur.is_some() {
            self.ideal_act = act.to_string();
            return true;
        }
        if self.in_transition && !self.finished(vm) {
            if Some(goal) == self.ideal_seq {
                return true;
            }
            if let Some(c) = self.cur {
                if vm.seqs[c.seq].exit == vm.seqs[goal].entry {
                    // retarget: the playing transition already leads into the new goal
                    self.ideal_act = act.to_string();
                    self.ideal_seq = Some(goal);
                    self.ideal_rate = Self::rate_for(vm, goal, duration(act, goal));
                    return true;
                }
            }
        }
        self.ideal_act = act.to_string();
        self.ideal_seq = Some(goal);
        self.ideal_rate = Self::rate_for(vm, goal, duration(act, goal));
        let mut seq = goal;
        if let Some(c) = self.cur {
            if c.seq != goal {
                seq = vm.find_transition(c.seq, goal);
            }
        }
        if is_draw(act) {
            seq = goal;
        }
        self.in_transition = seq != goal;
        let rate = if self.in_transition { 1.0 } else { self.ideal_rate };
        self.set_sequence(vm, seq, rate);
        true
    }

    fn rate_for(vm: &Viewmodel, seq: usize, desired: f32) -> f32 {
        if desired > 0.0 { (vm.seqs[seq].duration / desired).min(12.0).max(1e-3) } else { 1.0 }
    }

    /// Advance by `dt`: retire finished fades, fire the sequence events crossed, start the goal when a transition ends.
    pub fn update(&mut self, vm: &Viewmodel, dt: f32) {
        self.now += dt.max(0.0);
        // a layer is gone once its fade is over, and every older one with it
        if let Some(i) = self.old.iter().rposition(|l| l.start + l.fade <= self.now) {
            self.old.drain(..=i);
        }
        if let Some(c) = self.cur {
            let s = &vm.seqs[c.seq];
            let t = Self::cycle_of(vm, c.seq, 0.0, c.start, c.rate, self.now) * s.duration;
            self.clock.advance(vm, &s.name, t, &mut self.events);
        }
        if self.in_transition && self.finished(vm) {
            if let Some(goal) = self.ideal_seq {
                self.in_transition = false;
                let rate = self.ideal_rate;
                self.set_sequence(vm, goal, rate);
            }
        }
    }

    /// Accumulates one sequence (and its addlayers) into `pose` with weight `w` (AccumulatePose).
    fn accumulate(vm: &Viewmodel, seq: usize, cycle: f32, w: f32, p: &Params, pose: &mut [(Vec3, Quat)], tmp: &mut Vec<(Vec3, Quat)>, depth: u32) {
        if w <= 0.0 {
            return;
        }
        let s = &vm.seqs[seq];
        tmp.clear();
        if s.delta() {
            tmp.resize(pose.len(), (Vec3::ZERO, Quat::IDENTITY));
        } else {
            tmp.extend_from_slice(pose);
        }
        vm.sample_seq(seq, cycle, &|n| p.get(n), tmp);
        for (b, (dst, src)) in pose.iter_mut().zip(tmp.iter()).enumerate() {
            let k = w * s.weights.as_ref().map_or(1.0, |ws| ws.get(b).copied().unwrap_or(0.0));
            if k <= 0.0 {
                continue;
            }
            if s.delta() {
                // the sequences' additive root curve (jx_c_delta) carries the anim frame's constant turn, not a delta
                if vm.parent(b) < 0 {
                    continue;
                }
                dst.0 += src.0 * k;
                dst.1 = (dst.1 * Quat::IDENTITY.slerp(src.1, k)).normalize();
            } else {
                dst.0 = dst.0.lerp(src.0, k);
                dst.1 = dst.1.slerp(src.1, k).normalize();
            }
        }
        if depth < 2 {
            for &l in &s.layers {
                let mut t2 = Vec::new();
                Self::accumulate(vm, l, cycle, w, p, pose, &mut t2, depth + 1);
            }
        }
    }

    /// This frame's local pose (rig space): base pose -> current sequence -> fading layers -> autoplay layers.
    /// `base` = what the delta sequences add onto (None = the reference pose).
    pub fn pose(&self, vm: &Viewmodel, p: &Params, base: Option<&[(Vec3, Quat)]>, out: &mut Vec<(Vec3, Quat)>) {
        out.clear();
        match base {
            Some(b) => out.extend_from_slice(b),
            None => out.extend(vm.bind_pose()),
        }
        let mut tmp = Vec::with_capacity(out.len());
        // the fading layers' weights: a running product from the newest down
        let mut ws = vec![0.0f32; self.old.len()];
        let mut acc = 1.0;
        for (i, l) in self.old.iter().enumerate().rev() {
            let s = (1.0 - (self.now - l.start) / l.fade.max(1e-4)).clamp(0.0, 1.0);
            acc *= smooth(s);
            ws[i] = acc;
        }
        let covered = ws.last().is_some_and(|w| *w >= 1.0);
        if let (Some(c), false) = (self.cur, covered) {
            let cyc = Self::cycle_of(vm, c.seq, 0.0, c.start, c.rate, self.now);
            Self::accumulate(vm, c.seq, cyc, 1.0, p, out, &mut tmp, 0);
        }
        for i in (0..self.old.len()).rev() {
            if i + 1 < ws.len() && ws[i + 1] >= 1.0 {
                continue;
            }
            let l = &self.old[i];
            let cyc = Self::cycle_of(vm, l.seq, l.start_cycle, l.start, l.rate, self.now);
            Self::accumulate(vm, l.seq, cyc, ws[i], p, out, &mut tmp, 0);
        }
        for (i, s) in vm.seqs.iter().enumerate() {
            if s.autoplay() {
                let cyc = (self.now / s.duration.max(1e-3)).fract();
                Self::accumulate(vm, i, cyc, 1.0, p, out, &mut tmp, 0);
            }
        }
    }

    /// Jump straight to a sequence at a time (console poses / screenshots): no fade, no transition.
    pub fn force(&mut self, vm: &Viewmodel, seq: usize, t: f32) {
        self.old.clear();
        self.in_transition = false;
        self.ideal_seq = Some(seq);
        self.ideal_act = vm.seqs[seq].activity.clone();
        self.ideal_rate = 1.0;
        self.cur = Some(Cur { seq, start: self.now - t, rate: 1.0 });
        self.clock.reset();
    }
}
