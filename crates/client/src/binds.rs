//! Rebindable controls, Apex-style: every action has two slots, each a keyboard key, a mouse button or a wheel
//! direction. A bind lives in one slot only. Saved with the other settings in `r5rust_settings.toml`.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Crouch,
    Sprint,
    FlyToggle,
    FlyDown,
    Respawn,
    Fullscreen,
}

impl Action {
    pub const ALL: [Action; 11] = [
        Action::Forward,
        Action::Back,
        Action::Left,
        Action::Right,
        Action::Jump,
        Action::Crouch,
        Action::Sprint,
        Action::FlyToggle,
        Action::FlyDown,
        Action::Respawn,
        Action::Fullscreen,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Action::Forward => "Move Forward",
            Action::Back => "Move Back",
            Action::Left => "Move Left",
            Action::Right => "Move Right",
            Action::Jump => "Jump",
            Action::Crouch => "Crouch (Hold)",
            Action::Sprint => "Sprint",
            Action::FlyToggle => "Fly (toggle)",
            Action::FlyDown => "Fly Down",
            Action::Respawn => "Respawn",
            Action::Fullscreen => "Fullscreen",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Bind {
    Key(KeyCode),
    Mouse(MouseBtn),
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum MouseBtn {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

impl From<MouseButton> for MouseBtn {
    fn from(b: MouseButton) -> Self {
        match b {
            MouseButton::Left => MouseBtn::Left,
            MouseButton::Right => MouseBtn::Right,
            MouseButton::Middle => MouseBtn::Middle,
            MouseButton::Back => MouseBtn::Back,
            MouseButton::Forward => MouseBtn::Forward,
            MouseButton::Other(n) => MouseBtn::Other(n),
        }
    }
}

impl Bind {
    pub fn label(&self) -> String {
        match self {
            Bind::Key(k) => {
                let s = format!("{k:?}");
                let s = s.strip_prefix("Key").filter(|r| r.len() == 1).map(str::to_string).unwrap_or(s);
                let s = s.strip_prefix("Digit").map(str::to_string).unwrap_or(s);
                match s.as_str() {
                    "ShiftLeft" => "LSHIFT".into(),
                    "ShiftRight" => "RSHIFT".into(),
                    "ControlLeft" => "LCTRL".into(),
                    "ControlRight" => "RCTRL".into(),
                    "AltLeft" => "LALT".into(),
                    "AltRight" => "RALT".into(),
                    "Space" => "SPACE".into(),
                    _ => s.to_uppercase(),
                }
            }
            Bind::Mouse(MouseBtn::Left) => "MOUSE1".into(),
            Bind::Mouse(MouseBtn::Right) => "MOUSE2".into(),
            Bind::Mouse(MouseBtn::Middle) => "MOUSE3".into(),
            Bind::Mouse(MouseBtn::Back) => "MOUSE4".into(),
            Bind::Mouse(MouseBtn::Forward) => "MOUSE5".into(),
            Bind::Mouse(MouseBtn::Other(n)) => format!("MOUSE{}", n + 1),
            Bind::WheelUp => "MWHEELUP".into(),
            Bind::WheelDown => "MWHEELDOWN".into(),
        }
    }
}

/// In memory: action -> two slots. On disk: a list of named entries (TOML has no null, so empty slots are omitted).
#[derive(Clone, Debug)]
pub struct Binds {
    pub slots: Vec<(Action, [Option<Bind>; 2])>,
}

#[derive(Serialize, Deserialize)]
struct BindEntry {
    action: Action,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    slot1: Option<Bind>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    slot2: Option<Bind>,
}

impl Serialize for Binds {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let v: Vec<BindEntry> = self.slots.iter().map(|(a, [x, y])| BindEntry { action: *a, slot1: *x, slot2: *y }).collect();
        v.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Binds {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v: Vec<BindEntry> = Vec::deserialize(d)?;
        Ok(Binds { slots: v.into_iter().map(|e| (e.action, [e.slot1, e.slot2])).collect() })
    }
}

impl Default for Binds {
    fn default() -> Self {
        use KeyCode as K;
        let k = |c| Some(Bind::Key(c));
        Binds {
            slots: vec![
                (Action::Forward, [k(K::KeyW), None]),
                (Action::Back, [k(K::KeyS), None]),
                (Action::Left, [k(K::KeyA), None]),
                (Action::Right, [k(K::KeyD), None]),
                // the wheel on jump is what superglides / bhops are done with
                (Action::Jump, [k(K::Space), Some(Bind::WheelDown)]),
                (Action::Crouch, [k(K::ControlLeft), k(K::KeyC)]),
                (Action::Sprint, [k(K::ShiftLeft), None]),
                (Action::FlyToggle, [k(K::KeyV), None]),
                (Action::FlyDown, [k(K::KeyX), None]),
                (Action::Respawn, [k(K::KeyR), None]),
                (Action::Fullscreen, [k(K::F11), None]),
            ],
        }
    }
}

impl Binds {
    /// Fills in any action missing from a saved file (new actions added later).
    pub fn normalised(mut self) -> Self {
        let def = Binds::default();
        for (a, s) in def.slots {
            if !self.slots.iter().any(|(b, _)| *b == a) {
                self.slots.push((a, s));
            }
        }
        self.slots.sort_by_key(|(a, _)| Action::ALL.iter().position(|x| x == a).unwrap_or(99));
        self
    }

    pub fn get(&self, a: Action) -> [Option<Bind>; 2] {
        self.slots.iter().find(|(b, _)| *b == a).map(|(_, s)| *s).unwrap_or([None, None])
    }

    /// Puts `bind` in `action`'s slot, removing it from wherever else it was.
    pub fn assign(&mut self, action: Action, slot: usize, bind: Bind) {
        for (_, s) in self.slots.iter_mut() {
            for x in s.iter_mut() {
                if *x == Some(bind) {
                    *x = None;
                }
            }
        }
        if let Some((_, s)) = self.slots.iter_mut().find(|(b, _)| *b == action) {
            s[slot.min(1)] = Some(bind);
        }
    }

    pub fn clear(&mut self, action: Action, slot: usize) {
        if let Some((_, s)) = self.slots.iter_mut().find(|(b, _)| *b == action) {
            s[slot.min(1)] = None;
        }
    }

    pub fn actions_for(&self, bind: Bind) -> impl Iterator<Item = Action> + '_ {
        self.slots.iter().filter(move |(_, s)| s.contains(&Some(bind))).map(|(a, _)| *a)
    }
}

/// What's physically held right now, plus wheel notches waiting to be turned into one-tick presses.
#[derive(Default)]
pub struct InputState {
    pub held: HashSet<Bind>,
    /// pressed since the last tick (a tap shorter than a tick still counts once)
    pub tapped: HashSet<Bind>,
    wheel_up: u32,
    wheel_down: u32,
    /// the previous tick consumed a wheel press, so this one must be a release (keeps presses distinct)
    pulse_up_last: bool,
    pulse_down_last: bool,
    pulse_up_now: bool,
    pulse_down_now: bool,
}

impl InputState {
    pub fn wheel(&mut self, up: bool, notches: u32) {
        if up {
            self.wheel_up = (self.wheel_up + notches).min(8);
        } else {
            self.wheel_down = (self.wheel_down + notches).min(8);
        }
    }

    /// Call once per simulation tick before reading actions: a queued wheel notch becomes a press for exactly
    /// one tick, with a released tick between notches so each one is a separate press.
    pub fn begin_tick(&mut self) {
        self.pulse_up_now = !self.pulse_up_last && self.wheel_up > 0;
        if self.pulse_up_now {
            self.wheel_up -= 1;
        }
        self.pulse_up_last = self.pulse_up_now;
        self.pulse_down_now = !self.pulse_down_last && self.wheel_down > 0;
        if self.pulse_down_now {
            self.wheel_down -= 1;
        }
        self.pulse_down_last = self.pulse_down_now;
    }

    /// Held this tick (or tapped since the last one, or a wheel notch landing on this tick).
    pub fn down(&self, binds: &Binds, a: Action) -> bool {
        binds.get(a).iter().flatten().any(|b| match b {
            Bind::WheelUp => self.pulse_up_now,
            Bind::WheelDown => self.pulse_down_now,
            b => self.held.contains(b) || self.tapped.contains(b),
        })
    }

    /// A fresh press since the last tick (key/button down edge or a wheel notch).
    pub fn pressed(&self, binds: &Binds, a: Action) -> bool {
        binds.get(a).iter().flatten().any(|b| match b {
            Bind::WheelUp => self.pulse_up_now,
            Bind::WheelDown => self.pulse_down_now,
            b => self.tapped.contains(b),
        })
    }

    pub fn press(&mut self, b: Bind) {
        self.held.insert(b);
        self.tapped.insert(b);
    }

    pub fn release(&mut self, b: Bind) {
        self.held.remove(&b);
    }

    /// Call after the tick has read its input.
    pub fn end_tick(&mut self) {
        self.tapped.clear();
    }

    pub fn clear(&mut self) {
        *self = InputState::default();
    }
}

/// Everything the settings menu edits, persisted to `r5rust_settings.toml` in the working directory.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    pub binds: Binds,
    /// Apex sensitivity (Source m_yaw 0.022 deg/count)
    pub sensitivity: f32,
    /// horizontal FOV at 4:3, like Apex's slider
    pub fov: f32,
    pub invert_y: bool,
    /// Apex's mantle boost (playlist var mantle_boost_enabled, on by default)
    pub mantle_boost: bool,
    /// Apex's player_setting_autosprint: moving forward sprints without the sprint key (on in Kainz's R5F profile)
    pub auto_sprint: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        UserSettings { binds: Binds::default(), sensitivity: 1.6, fov: 100.0, invert_y: false, mantle_boost: true, auto_sprint: true }
    }
}

const SETTINGS_FILE: &str = "r5rust_settings.toml";

impl UserSettings {
    /// Saved settings, else defaults seeded from r5rust.toml's sensitivity/fov.
    pub fn load(sens: f32, fov: f32) -> Self {
        match std::fs::read_to_string(SETTINGS_FILE).ok().and_then(|s| toml::from_str::<UserSettings>(&s).map_err(|e| log::warn!("{SETTINGS_FILE}: {e}")).ok()) {
            Some(mut s) => {
                s.binds = s.binds.normalised();
                s
            }
            None => UserSettings { sensitivity: sens, fov, ..Default::default() },
        }
    }

    pub fn save(&self) {
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(SETTINGS_FILE, s) {
                    log::warn!("saving {SETTINGS_FILE}: {e}");
                }
            }
            Err(e) => log::warn!("serialising settings: {e}"),
        }
    }
}
