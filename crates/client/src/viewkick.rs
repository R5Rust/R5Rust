//! Retail Apex recoil: the view punch spring (S21 r5apex_dx12, reversed 2026-11-18; notes in the session's
//! viewkick_re.md, summarised in HANDOFF "Recoil").
//!
//! - Per shot (FUN_140711ce0): the pattern row (c0 yaw, c1 pitch, c2 / c3 random scales) x the weapon's base / random
//!   values gives a kick; `hardScale` x kick jumps the punch angle, `softScale` x kick x 20 is added to its velocity.
//! - Per frame (FUN_140a34170 -> exact spring FUN_140521af0): x'' = -k x - c x' per axis (pitch, yaw, roll), the
//!   constants lerped normal -> hot by the heat (1 on every shot, held `cooldown_holdtime`, faded over
//!   `cooldown_fadetime`), then hip -> ADS by the zoom fraction. The punch springs back to 0: the aim returns.
//! - Display: the camera shows (1 - w) of the punch, the gun w x vmScale more, the bullets the whole punch
//!   (w = lerp(hipfire_weaponFraction, ads_weaponFraction, ads)). Eye angles only change by viewkick_perm_* (0 here).
//! - Mouse against the punch eats the punch first (FUN_140a33ff0), so pulling down during a spray doesn't leave the
//!   aim low once it springs back.

use glam::Vec3;

/// One spring set: (k, c) per axis (pitch, yaw, roll), hipfire and ADS.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpringSet {
    pub k_hip: [f32; 3],
    pub c_hip: [f32; 3],
    pub k_ads: [f32; 3],
    pub c_ads: [f32; 3],
}

impl SpringSet {
    pub fn from_json(j: &serde_json::Value) -> Option<SpringSet> {
        let f = |k: &str| j[k].as_str().and_then(|v| v.trim().parse::<f32>().ok()).or_else(|| j[k].as_f64().map(|v| v as f32)).unwrap_or(0.0);
        if !j.is_object() {
            return None;
        }
        let three = |pre: &str, what: &str| [f(&format!("{pre}_pitch_{what}")), f(&format!("{pre}_yaw_{what}")), f(&format!("{pre}_roll_{what}"))];
        Some(SpringSet { k_hip: three("hipfire", "springConstant"), c_hip: three("hipfire", "damping"), k_ads: three("ADS", "springConstant"), c_ads: three("ADS", "damping") })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub pitch: [f32; 5], // base, random, innerexclude, soft, hard
    pub yaw: [f32; 5],
    pub roll: [f32; 5], // base, randomMin, randomMax, soft, hard
    pub decay_delay: f32,
    pub decay_rate: f32,
    pub heat_per_shot: f32,
    pub hold: f32,
    pub fade: f32,
    pub wf_hip: f32,
    pub wf_ads: f32,
    pub vm_hip: f32,
    pub vm_ads: f32,
    pub air_scale_ads: f32,
    pub normal: SpringSet,
    pub hot: Option<SpringSet>,
}

impl Settings {
    /// weapon.json "settings" + "springs" {normal, hot}.
    pub fn from_json(s: &serde_json::Value, springs: &serde_json::Value) -> Settings {
        let f = |k: &str, d: f32| s[k].as_str().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(d);
        Settings {
            pitch: [f("viewkick_pitch_base", 0.0), f("viewkick_pitch_random", 0.0), f("viewkick_pitch_random_innerexclude", 0.0), f("viewkick_pitch_softScale", 0.0), f("viewkick_pitch_hardScale", 0.0)],
            yaw: [f("viewkick_yaw_base", 0.0), f("viewkick_yaw_random", 0.0), f("viewkick_yaw_random_innerexclude", 0.0), f("viewkick_yaw_softScale", 0.0), f("viewkick_yaw_hardScale", 0.0)],
            roll: [f("viewkick_roll_base", 0.0), f("viewkick_roll_randomMin", 0.0), f("viewkick_roll_randomMax", 0.0), f("viewkick_roll_softScale", 0.0), f("viewkick_roll_hardScale", 0.0)],
            decay_delay: f("viewkick_scale_valueDecayDelay", 0.0),
            decay_rate: f("viewkick_scale_valueDecayRate", 0.0),
            heat_per_shot: f("viewkick_spring_heatpershot", 0.0),
            hold: f("viewkick_spring_cooldown_holdtime", 0.0),
            fade: f("viewkick_spring_cooldown_fadetime", 0.0),
            wf_hip: f("viewkick_hipfire_weaponFraction", 0.0),
            wf_ads: f("viewkick_ads_weaponFraction", 0.0),
            vm_hip: f("viewkick_hipfire_weaponFraction_vmScale", 1.0),
            vm_ads: f("viewkick_ads_weaponFraction_vmScale", 1.0),
            air_scale_ads: f("viewkick_air_scale_ads", 1.0),
            normal: SpringSet::from_json(&springs["normal"]).unwrap_or_default(),
            hot: SpringSet::from_json(&springs["hot"]),
        }
    }
}

/// The punch state: angles (pitch, yaw, roll degrees; pitch < 0 = up, yaw > 0 = left) and their velocity.
#[derive(Clone, Debug, Default)]
pub struct Punch {
    pub ang: Vec3,
    pub vel: Vec3,
    heat_at_shot: f32,
    heat_hold_until: f32,
    /// the pattern counter: +1 per shot, decays after decay_delay at decay_rate / s (the bullet row index)
    pattern_value: f32,
    last_fire: f32,
    rng: u32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Exact solution of x'' = -k x - c x' over dt (FUN_140521af0).
fn spring(x: f32, v: f32, k: f32, c: f32, dt: f32) -> (f32, f32) {
    let disc = c * c - 4.0 * k;
    if disc.abs() <= 0.001 {
        let h = c * 0.5;
        let b = v + h * x;
        let xx = x + b * dt;
        let e = (-h * dt).exp();
        (xx * e, (b - xx * h) * e)
    } else if disc > 0.0 {
        let s = disc.sqrt();
        let (r1, r2) = ((-c + s) * 0.5, (-c - s) * 0.5);
        // x = A e^{r1 t} + B e^{r2 t}
        let b = (v - r1 * x) / (r2 - r1);
        let a = x - b;
        let (e1, e2) = ((r1 * dt).exp(), (r2 * dt).exp());
        (a * e1 + b * e2, a * r1 * e1 + b * r2 * e2)
    } else {
        let w = (4.0 * k - c * c).sqrt() * 0.5;
        let h = c * 0.5;
        let e = (-h * dt).exp();
        let (sn, cs) = (w * dt).sin_cos();
        let b = (v + x * h) / w;
        let xx = e * (x * cs + b * sn);
        let vv = e * (-h * (x * cs + b * sn) + (-x * w * sn + b * w * cs));
        (xx, vv)
    }
}

impl Punch {
    fn rand(&mut self) -> f32 {
        if self.rng == 0 {
            self.rng = 0x2545_F491;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / 16_777_216.0
    }

    fn decayed(&self, v: f32, st: &Settings, now: f32) -> f32 {
        if v < 1.19e-7 {
            return 0.0;
        }
        let idle = now - self.last_fire - st.decay_delay;
        if idle <= 0.0 {
            v
        } else {
            (v - idle * st.decay_rate).max(0.0)
        }
    }

    pub fn heat(&self, st: &Settings, now: f32) -> f32 {
        if st.fade <= 0.0 || now < self.heat_hold_until {
            self.heat_at_shot
        } else {
            (self.heat_at_shot - (now - self.heat_hold_until) / st.fade).clamp(0.0, 1.0)
        }
    }

    /// One shot (`rows` = the viewkick pattern: c0 yaw, c1 pitch, c2 yaw random, c3 pitch random; `loop_offset`).
    pub fn kick(&mut self, st: &Settings, rows: &[[f32; 4]], loop_offset: usize, ads: f32, in_air: bool, now: f32) {
        let air = if in_air { lerp(1.0, st.air_scale_ads, ads) } else { 1.0 };
        let (p_scale, y_scale) = (air, air);
        let (mut pb, mut yb, mut c2, mut c3) = (st.pitch[0], st.yaw[0], 1.0, 1.0);
        if !rows.is_empty() {
            let i = self.decayed(self.pattern_value, st, now) as usize;
            let row = rows[i.min(rows.len() - 1)];
            pb = row[1] * st.pitch[0];
            yb = row[0] * st.yaw[0];
            c2 = row[2];
            c3 = row[3];
        }
        let sign = |p: &mut Punch| if p.rand() < 0.5 { -1.0 } else { 1.0 };
        let sp = sign(self);
        let rp = self.rand();
        let sy = sign(self);
        let ry = self.rand();
        let sr = sign(self);
        let rr = self.rand();
        let pitch_rand = sp * lerp(c3 * st.pitch[2] * 0.5, c3 * st.pitch[1] * 0.5, rp);
        let yaw_rand = sy * lerp(c2 * st.yaw[2] * 0.5, c2 * st.yaw[1] * 0.5, ry);
        let roll = st.roll[0] + sr * lerp(st.roll[1], st.roll[2], rr);
        let kp = pb + pitch_rand;
        let ky = yb + yaw_rand;
        self.vel.x += st.pitch[3] * kp * p_scale * 20.0;
        self.ang.x += st.pitch[4] * kp * p_scale;
        self.vel.y += st.yaw[3] * ky * y_scale * 20.0;
        self.ang.y += st.yaw[4] * ky * y_scale;
        self.vel.z += st.roll[3] * roll * 20.0;
        self.ang.z += st.roll[4] * roll;
        // state for the next shot (PrimaryAttack FUN_1406fea80)
        let heat = (self.heat(st, now) + st.heat_per_shot).clamp(0.0, 1.0);
        self.heat_at_shot = heat;
        self.heat_hold_until = now + st.hold;
        if !rows.is_empty() {
            let n = rows.len() as f32;
            let mut v = (self.decayed(self.pattern_value, st, now) + 1.0).min(n);
            if v >= n - 1e-4 {
                v -= loop_offset as f32;
            }
            self.pattern_value = v;
        }
        self.last_fire = now;
    }

    /// Integrates the spring over `dt` (render frames; `now` = the weapon's clock for the heat).
    pub fn update(&mut self, st: &Settings, ads: f32, now: f32, dt: f32) {
        if self.ang.length_squared() <= 0.001 && self.vel.length_squared() <= 0.001 {
            self.ang = Vec3::ZERO;
            self.vel = Vec3::ZERO;
            return;
        }
        let heat = self.heat(st, now);
        let base = |n: &SpringSet| n.clone();
        let n = base(&st.normal);
        let h = st.hot.unwrap_or(n);
        let mut a = self.ang.to_array();
        let mut v = self.vel.to_array();
        for i in 0..3 {
            let k = lerp(lerp(n.k_hip[i], h.k_hip[i], heat), lerp(n.k_ads[i], h.k_ads[i], heat), ads);
            let c = lerp(lerp(n.c_hip[i], h.c_hip[i], heat), lerp(n.c_ads[i], h.c_ads[i], heat), ads);
            let (x, vv) = spring(a[i], v[i], k.max(0.0), c.max(0.0), dt);
            a[i] = x.clamp(-89.0, 89.0);
            v[i] = vv;
        }
        self.ang = Vec3::from(a);
        self.vel = Vec3::from(v);
    }

    /// The weapon's share w (the camera shows 1 - w).
    pub fn weapon_fraction(st: &Settings, ads: f32) -> f32 {
        lerp(st.wf_hip, st.wf_ads, ads)
    }

    /// Camera punch (pitch, yaw).
    pub fn camera(&self, st: &Settings, ads: f32) -> (f32, f32) {
        let k = 1.0 - Self::weapon_fraction(st, ads);
        (self.ang.x * k, self.ang.y * k)
    }

    /// What the gun adds on top of the camera (pitch, yaw).
    pub fn viewmodel(&self, st: &Settings, ads: f32) -> (f32, f32) {
        let k = Self::weapon_fraction(st, ads) * lerp(st.vm_hip, st.vm_ads, ads);
        (self.ang.x * k, self.ang.y * k)
    }

    /// Mouse input against the punch eats the punch first (FUN_140a33ff0). `d` = this frame's (pitch, yaw) mouse
    /// delta in degrees, returned with what's left for the eye angles.
    pub fn absorb(&mut self, st: &Settings, ads: f32, d: (f32, f32)) -> (f32, f32) {
        let w = Self::weapon_fraction(st, ads);
        let mut out = [d.0, d.1];
        for (axis, slot) in out.iter_mut().enumerate() {
            let p = if axis == 0 { self.ang.x } else { self.ang.y };
            let pc = (1.0 - w) * p;
            let dd = *slot;
            let pn = if pc > 1.19e-7 && dd < 1.19e-7 {
                (pc + dd).max(0.0)
            } else if pc < 1.19e-7 && dd > 1.19e-7 {
                (pc + dd).min(0.0)
            } else {
                pc
            };
            let delta = pn - pc;
            if axis == 0 {
                self.ang.x += delta;
            } else {
                self.ang.y += delta;
            }
            *slot -= delta;
        }
        (out[0], out[1])
    }
}
