//! Game audio: a small software mixer on cpal's output thread. The main thread sends commands over a channel (no
//! locks on the audio thread); voices play preloaded stereo sounds with volume, pitch (resampling), a loop region and
//! fade-outs. The weapon's sounds are retail Apex's Miles sources, picked per event the way retail's events pick them
//! (tools/build_sounds.py writes sounds.json + stereo WAVs).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// A decoded sound: interleaved stereo f32 at `rate`, with an optional loop region (frames).
pub struct Sound {
    pub data: Vec<f32>,
    pub rate: u32,
    pub looping: Option<(usize, usize)>,
}

impl Sound {
    pub fn frames(&self) -> usize {
        self.data.len() / 2
    }
}

/// Reads a 16-bit or float PCM WAV (1, 2 or 6 channels; 5.1 is folded down to stereo).
pub fn load_wav(path: &Path) -> Result<Sound, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(format!("{}: not a WAV", path.display()));
    }
    let (mut fmt, mut data) = (None, None);
    let mut o = 12;
    while o + 8 <= b.len() {
        let id = &b[o..o + 4];
        let n = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()) as usize;
        let body = &b[o + 8..(o + 8 + n).min(b.len())];
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        o += 8 + n + (n & 1);
    }
    let (fmt, data) = (fmt.ok_or("no fmt chunk")?, data.ok_or("no data chunk")?);
    let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
    let ch = u16::from_le_bytes([fmt[2], fmt[3]]) as usize;
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
    let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
    let samples: Vec<f32> = match (tag, bits) {
        (1, 16) | (0xFFFE, 16) => data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect(),
        (3, 32) | (0xFFFE, 32) => data.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
        _ => return Err(format!("{}: unsupported WAV format {tag}/{bits} bit", path.display())),
    };
    let frames = samples.len() / ch.max(1);
    let mut out = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let s = &samples[f * ch..f * ch + ch];
        let (l, r) = match ch {
            1 => (s[0], s[0]),
            2 => (s[0], s[1]),
            // FL FR FC LFE SL SR
            6 => {
                let c = s[2] * 0.707;
                (s[0] + c + s[4] * 0.707 + s[3] * 0.5, s[1] + c + s[5] * 0.707 + s[3] * 0.5)
            }
            _ => (s[0], s[1.min(ch - 1)]),
        };
        out.push(l);
        out.push(r);
    }
    Ok(Sound { data: out, rate, looping: None })
}

enum Cmd {
    Play { id: u64, snd: Arc<Sound>, vol: f32, pitch: f32, delay: f32, looped: bool },
    /// fade out over `fade` seconds; for a looping voice, `release` lets it play on past the loop end instead
    Stop { id: u64, fade: f32, release: bool },
    Volume(f32),
    SetVol { id: u64, vol: f32 },
}

struct Voice {
    id: u64,
    snd: Arc<Sound>,
    pos: f64,
    step: f64,
    vol: f32,
    delay: u64,
    looped: bool,
    fade: Option<(f32, f32)>,
}

/// Handle to a playing voice (stop a loop with it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Handle(pub u64);

pub struct Audio {
    tx: Sender<Cmd>,
    _stream: cpal::Stream,
    out_rate: u32,
    next: u64,
}

impl Audio {
    /// Opens the default output device; None (logged) when there is no audio device.
    pub fn new() -> Option<Audio> {
        let host = cpal::default_host();
        let dev = host.default_output_device()?;
        let cfg = match dev.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("audio: {e}");
                return None;
            }
        };
        let channels = cfg.channels() as usize;
        let out_rate = cfg.sample_rate();
        let (tx, rx) = channel::<Cmd>();
        let mut mix = Mixer { rx, voices: Vec::with_capacity(MAX_VOICES), out_rate: out_rate as f64, master: 1.0 };
        let err = |e| log::warn!("audio stream: {e}");
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => dev.build_output_stream(&cfg.config(), move |out: &mut [f32], _| mix.fill(out, channels), err, None),
            cpal::SampleFormat::I16 => {
                let mut buf = Vec::new();
                dev.build_output_stream(
                    &cfg.config(),
                    move |out: &mut [i16], _| {
                        buf.resize(out.len(), 0.0);
                        mix.fill(&mut buf, channels);
                        for (o, s) in out.iter_mut().zip(&buf) {
                            *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        }
                    },
                    err,
                    None,
                )
            }
            f => {
                log::warn!("audio: unsupported sample format {f:?}");
                return None;
            }
        };
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                log::warn!("audio: {e}");
                return None;
            }
        };
        if let Err(e) = stream.play() {
            log::warn!("audio: {e}");
            return None;
        }
        log::info!("audio: {} ch {} Hz ({:?})", channels, out_rate, cfg.sample_format());
        Some(Audio { tx, _stream: stream, out_rate, next: 1 })
    }

    pub fn play(&mut self, snd: &Arc<Sound>, vol: f32, pitch: f32, delay: f32, looped: bool) -> Handle {
        let id = self.next;
        self.next += 1;
        let _ = self.tx.send(Cmd::Play { id, snd: snd.clone(), vol, pitch, delay, looped });
        Handle(id)
    }

    pub fn stop(&mut self, h: Handle, fade: f32, release: bool) {
        let _ = self.tx.send(Cmd::Stop { id: h.0, fade, release });
    }

    pub fn set_volume(&mut self, v: f32) {
        let _ = self.tx.send(Cmd::Volume(v));
    }

    pub fn set_voice_volume(&mut self, h: Handle, vol: f32) {
        let _ = self.tx.send(Cmd::SetVol { id: h.0, vol });
    }

    pub fn rate(&self) -> u32 {
        self.out_rate
    }
}

const MAX_VOICES: usize = 64;

struct Mixer {
    rx: Receiver<Cmd>,
    voices: Vec<Voice>,
    out_rate: f64,
    master: f32,
}

impl Mixer {
    fn fill(&mut self, out: &mut [f32], channels: usize) {
        while let Ok(c) = self.rx.try_recv() {
            match c {
                Cmd::Play { id, snd, vol, pitch, delay, looped } => {
                    if self.voices.len() >= MAX_VOICES {
                        // drop the oldest one-shot
                        if let Some(i) = self.voices.iter().position(|v| !v.looped) {
                            self.voices.remove(i);
                        } else {
                            continue;
                        }
                    }
                    let step = snd.rate as f64 / self.out_rate * pitch as f64;
                    let delay = (delay.max(0.0) as f64 * self.out_rate) as u64;
                    self.voices.push(Voice { id, snd, pos: 0.0, step, vol, delay, looped, fade: None });
                }
                Cmd::Stop { id, fade, release } => {
                    for v in self.voices.iter_mut().filter(|v| v.id == id) {
                        // play on through the end of the loop and its tail, but only when the source has a tail after
                        // the loop region: retail fire loops loop the whole file (R-99: 3.6 s), so playing on would keep
                        // "firing" for seconds after the trigger is released; those stop and the end event plays
                        let tail = v.snd.looping.is_some_and(|(_, le)| le + 64 < v.snd.data.len() / 2);
                        if release && tail {
                            v.looped = false;
                        } else {
                            let per = 1.0 / (fade.max(0.001) * self.out_rate as f32);
                            v.fade = Some((1.0, per));
                        }
                    }
                }
                Cmd::Volume(v) => self.master = v,
                Cmd::SetVol { id, vol } => {
                    for v in self.voices.iter_mut().filter(|v| v.id == id) {
                        v.vol = vol.max(1e-6);
                    }
                }
            }
        }
        out.fill(0.0);
        let frames = out.len() / channels.max(1);
        for v in &mut self.voices {
            let d = &v.snd.data;
            let n = d.len() / 2;
            let (ls, le) = v.snd.looping.unwrap_or((0, n));
            for f in 0..frames {
                if v.delay > 0 {
                    v.delay -= 1;
                    continue;
                }
                let i = v.pos as usize;
                if i + 1 >= n || (v.looped && i + 1 >= le) {
                    if v.looped && le > ls + 1 {
                        v.pos -= (le - ls) as f64;
                        continue;
                    }
                    v.vol = 0.0;
                    break;
                }
                let t = (v.pos - i as f64) as f32;
                let l = d[i * 2] + (d[i * 2 + 2] - d[i * 2]) * t;
                let r = d[i * 2 + 1] + (d[i * 2 + 3] - d[i * 2 + 1]) * t;
                let mut g = v.vol;
                if let Some((a, per)) = &mut v.fade {
                    *a -= *per;
                    if *a <= 0.0 {
                        v.vol = 0.0;
                        break;
                    }
                    g *= *a;
                }
                out[f * channels] += l * g;
                if channels > 1 {
                    out[f * channels + 1] += r * g;
                }
                v.pos += v.step;
            }
        }
        self.voices.retain(|v| v.vol > 0.0);
        // soft clip, master volume
        for s in out.iter_mut() {
            let x = *s * self.master;
            *s = if x.abs() < 0.8 { x } else { x.signum() * (0.8 + 0.2 * ((x.abs() - 0.8) / 0.2).tanh()) };
        }
    }
}

/// A controller curve on a layer's volume (Miles graph): dB at each x (AmmoClipPct = % of the mag left).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Ctrl {
    pub ctrl: String,
    pub points: Vec<(f32, f32)>,
}

impl Ctrl {
    pub fn db_at(&self, x: f32) -> f32 {
        let p = &self.points;
        if p.is_empty() {
            return 0.0;
        }
        if x <= p[0].0 {
            return p[0].1;
        }
        for w in p.windows(2) {
            if x <= w[1].0 {
                let k = (x - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
                return w[0].1 + (w[1].1 - w[0].1) * k;
            }
        }
        p[p.len() - 1].1
    }
}

/// One layer of a Miles event (tools/build_sounds.py): pick one file by weight, volume / pitch / delay ranges.
#[derive(Clone, serde::Deserialize)]
pub struct Layer {
    pub files: Vec<String>,
    #[serde(default)]
    pub weights: Vec<f32>,
    #[serde(default)]
    pub vol_db: (f32, f32),
    #[serde(default)]
    pub pitch_st: (f32, f32),
    #[serde(default)]
    pub delay: (f32, f32),
    #[serde(default)]
    pub ctrl: Option<Ctrl>,
}

#[derive(serde::Deserialize)]
struct SrcInfo {
    #[serde(rename = "loop")]
    looping: (usize, usize),
}

#[derive(serde::Deserialize)]
struct BankFile {
    events: HashMap<String, Vec<Layer>>,
    #[serde(default)]
    sources: HashMap<String, SrcInfo>,
}

/// An item's sound events (sounds.json), keyed by the Miles event name in lower case.
pub struct Bank {
    pub sounds: HashMap<String, Arc<Sound>>,
    pub events: HashMap<String, Vec<Layer>>,
    seed: u32,
}

/// A started layer: its voice, and its controller curve + base dB (volume follows the controller).
pub struct Playing {
    pub handle: Handle,
    pub ctrl: Option<Ctrl>,
    pub base_db: f32,
}

fn db(x: f32) -> f32 {
    10f32.powf(x / 20.0)
}

impl Bank {
    pub fn load(dir: &Path) -> Result<Bank, String> {
        let txt = std::fs::read_to_string(dir.join("sounds.json")).map_err(|e| format!("{}: {e}", dir.join("sounds.json").display()))?;
        let f: BankFile = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
        let mut sounds = HashMap::new();
        for layers in f.events.values() {
            for l in layers {
                for file in &l.files {
                    if !sounds.contains_key(file) {
                        let mut s = load_wav(&dir.join(file))?;
                        if let Some(i) = f.sources.get(file) {
                            let (a, b) = i.looping;
                            s.looping = (b > a + 1).then_some((a, b.min(s.frames())));
                        }
                        sounds.insert(file.clone(), Arc::new(s));
                    }
                }
            }
        }
        log::info!("sounds {}: {} events, {} sources", dir.display(), f.events.len(), sounds.len());
        Ok(Bank { sounds, events: f.events, seed: 0x1234_5678 })
    }

    fn rnd(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / 16_777_216.0
    }

    pub fn has(&self, event: &str) -> bool {
        self.events.contains_key(&event.to_ascii_lowercase())
    }

    /// Plays a Miles event; `ctrl` gives controller values (AmmoClipPct). Looping = every layer loops its source.
    pub fn play(&mut self, audio: &mut Audio, event: &str, looped: bool, ctrl: &dyn Fn(&str) -> Option<f32>) -> Vec<Playing> {
        let Some(layers) = self.events.get(&event.to_ascii_lowercase()).cloned() else { return Vec::new() };
        let mut out = Vec::new();
        for l in &layers {
            if l.files.is_empty() {
                continue;
            }
            let total: f32 = if l.weights.len() == l.files.len() { l.weights.iter().sum() } else { 0.0 };
            let pick = if total > 0.0 {
                let mut r = self.rnd() * total;
                l.weights.iter().position(|w| {
                    r -= w;
                    r <= 0.0
                }).unwrap_or(0)
            } else {
                ((self.rnd() * l.files.len() as f32) as usize).min(l.files.len() - 1)
            };
            let vol = l.vol_db.0 + (l.vol_db.1 - l.vol_db.0) * self.rnd();
            let st = l.pitch_st.0 + (l.pitch_st.1 - l.pitch_st.0) * self.rnd();
            let delay = l.delay.0 + (l.delay.1 - l.delay.0) * self.rnd();
            let Some(s) = self.sounds.get(&l.files[pick]) else { continue };
            let c = l.ctrl.as_ref().and_then(|c| ctrl(&c.ctrl).map(|x| c.db_at(x))).unwrap_or(0.0);
            let h = audio.play(s, db(vol + c), 2f32.powf(st / 12.0), delay, looped);
            out.push(Playing { handle: h, ctrl: l.ctrl.clone(), base_db: vol });
        }
        out
    }
}

/// The names of an item's sound events in its weapon settings (retail keys).
#[derive(Default, Clone)]
pub struct SoundNames {
    pub fire_first: Option<String>,
    pub fire_loop: Option<String>,
    pub fire_last: Option<String>,
    pub fire_each: Option<String>,
    pub low_ammo: Option<String>,
    pub dryfire: Option<String>,
    pub ads_in: Option<String>,
    pub ads_out: Option<String>,
}

impl SoundNames {
    pub fn from_settings(s: &serde_json::Value) -> SoundNames {
        let g = |k: &str| s[k].as_str().filter(|v| !v.is_empty()).map(|v| v.to_ascii_lowercase());
        SoundNames {
            fire_first: g("burst_or_looping_fire_sound_start_1p").or_else(|| g("fire_sound_1_player_1p")),
            fire_loop: g("burst_or_looping_fire_sound_middle_1p"),
            fire_last: g("burst_or_looping_fire_sound_end_1p"),
            fire_each: g("fire_sound_2_player_1p"),
            low_ammo: g("low_ammo_sound_name_1"),
            dryfire: g("sound_dryfire"),
            ads_in: g("sound_zoom_in"),
            ads_out: g("sound_zoom_out"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ZipSound {
    Mount,
    Start,
    Stop,
}

/// The first-person sounds: the gun's and the melee item's banks, weapon events (fire loop lifetime, ADS, dry fire)
/// and sequence events (AE_CL_PLAYSOUND...), with the low-ammo controller following the mag.
pub struct Sfx {
    pub audio: Audio,
    pub gun: Option<(Bank, SoundNames)>,
    pub melee: Option<Bank>,
    /// player movement sounds (assets <root>/player/sounds: the zipline events, tools/build_sounds.py)
    pub player: Option<Bank>,
    /// the zipline ride loop while it plays
    zip_loop: Vec<Playing>,
    looping: Vec<Playing>,
    /// sounds started by the current sequence (cut when it's interrupted: inspect, reload)
    seq: Vec<Playing>,
    /// % of the mag left (Miles' AmmoClipPct)
    ammo_pct: f32,
    /// master volume 0..1 (console `volume`)
    pub volume: f32,
}

impl Sfx {
    pub fn new() -> Option<Sfx> {
        let audio = Audio::new()?;
        Some(Sfx { audio, gun: None, melee: None, player: None, zip_loop: Vec::new(), looping: Vec::new(), seq: Vec::new(), ammo_pct: 100.0, volume: 1.0 })
    }

    /// `dir` = the item's folder (weapon.json + sounds/).
    pub fn load_gun(&mut self, dir: &Path) {
        let wj: serde_json::Value = std::fs::read_to_string(dir.join("weapon.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        match Bank::load(&dir.join("sounds")) {
            Ok(b) => self.gun = Some((b, SoundNames::from_settings(&wj["settings"]))),
            Err(e) => log::warn!("weapon sounds: {e} (run tools/build_sounds.py)"),
        }
    }

    pub fn load_melee(&mut self, dir: &Path) {
        self.melee = match Bank::load(&dir.join("sounds")) {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!("melee sounds: {e} (run tools/build_sounds.py)");
                None
            }
        };
    }

    pub fn load_player(&mut self, dir: &Path) {
        self.player = match Bank::load(&dir.join("sounds")) {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!("player sounds: {e} (run tools/build_sounds.py on <assets>/player)");
                None
            }
        };
    }

    /// Zipline sound events, as R5F S30's _zipline.gnut plays them for the local player: mount =
    /// player_zipline_attach (CodeCallback_ZiplineMount), ride start = player_zipline_loop (CodeCallback_ZiplineStart),
    /// stop = the loop and attach cut + player_zipline_detach (CodeCallback_ZiplineStop).
    pub fn zipline(&mut self, ev: ZipSound) {
        let Some(b) = self.player.as_mut() else { return };
        match ev {
            ZipSound::Mount => {
                b.play(&mut self.audio, "player_zipline_attach", false, &|_| None);
            }
            ZipSound::Start => {
                self.zip_loop = b.play(&mut self.audio, "player_zipline_loop", true, &|_| None);
            }
            ZipSound::Stop => {
                for p in std::mem::take(&mut self.zip_loop) {
                    self.audio.stop(p.handle, 0.05, false);
                }
                b.play(&mut self.audio, "player_zipline_detach", false, &|_| None);
            }
        }
    }

    fn play_gun(&mut self, name: Option<String>, looped: bool) -> Vec<Playing> {
        let pct = self.ammo_pct;
        match (&mut self.gun, name) {
            (Some((b, _)), Some(n)) => b.play(&mut self.audio, &n, looped, &|c| (c == "AmmoClipPct").then_some(pct)),
            _ => Vec::new(),
        }
    }

    /// One weapon event. `ammo` = rounds left after it, `clip` = the mag size.
    pub fn gun_event(&mut self, e: &crate::weapon::Event, ammo: u32, clip: u32) {
        use crate::weapon::Event as E;
        self.ammo_pct = if clip > 0 { ammo as f32 * 100.0 / clip as f32 } else { 100.0 };
        let names = match &self.gun {
            Some((_, n)) => n.clone(),
            None => return,
        };
        match *e {
            E::Shot { first } => {
                if first {
                    self.stop_loop(false);
                    self.play_gun(names.fire_first.clone(), false);
                    self.looping = self.play_gun(names.fire_loop.clone(), true);
                } else if names.fire_loop.is_none() {
                    self.play_gun(names.fire_first.clone(), false);
                }
                self.play_gun(names.fire_each.clone(), false);
            }
            E::SprayEnd => {
                self.stop_loop(true);
                self.play_gun(names.fire_last.clone(), false);
                // retail's low-ammo ring when a burst ends on a low mag
                if self.ammo_pct < 25.0 {
                    self.play_gun(names.low_ammo.clone(), false);
                }
            }
            E::DryFire => {
                self.play_gun(names.dryfire.clone(), false);
            }
            E::Reload { .. } | E::Inspect => {
                self.stop_loop(false);
            }
            E::AdsIn => {
                self.play_gun(names.ads_in.clone(), false);
            }
            E::AdsOut => {
                self.play_gun(names.ads_out.clone(), false);
            }
            E::Impact { .. } => {}
        }
    }

    /// A sequence event of the gun (`melee` = false) or the melee item.
    pub fn anim_event(&mut self, e: &crate::viewmodel::AnimEvent, melee: bool) {
        let name = match e.kind.as_str() {
            "AE_CL_PLAYSOUND" | "AE_SV_PLAYSOUND" | "AE_CL_PLAYSOUND_FOR_TYPE" => e.opt.to_ascii_lowercase(),
            "AE_CL_PLAYSOUND_FOR_ARCHETYPE" => format!("human_{}", e.opt.to_ascii_lowercase()),
            _ => return,
        };
        let pct = self.ammo_pct;
        let bank = if melee { self.melee.as_mut() } else { self.gun.as_mut().map(|(b, _)| b) };
        if let Some(b) = bank {
            let p = b.play(&mut self.audio, &name, false, &|c| (c == "AmmoClipPct").then_some(pct));
            self.seq.extend(p);
        }
    }

    /// The low-ammo layers of a running fire loop follow the mag (call every frame).
    pub fn update(&mut self, ammo: u32, clip: u32) {
        self.ammo_pct = if clip > 0 { ammo as f32 * 100.0 / clip as f32 } else { 100.0 };
        for p in &self.looping {
            if let Some(c) = &p.ctrl {
                if c.ctrl == "AmmoClipPct" {
                    self.audio.set_voice_volume(p.handle, db(p.base_db + c.db_at(self.ammo_pct)));
                }
            }
        }
    }

    fn stop_loop(&mut self, release: bool) {
        for p in std::mem::take(&mut self.looping) {
            self.audio.stop(p.handle, 0.04, release);
        }
    }

    /// The current sequence was cut short (inspect / reload interrupted, weapon switched): fade its sounds.
    pub fn cut_sequence(&mut self) {
        for p in std::mem::take(&mut self.seq) {
            self.audio.stop(p.handle, 0.08, false);
        }
    }

    /// Forget sequence sounds that have run their course (they're not cut later).
    pub fn sequence_done(&mut self) {
        self.seq.clear();
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.audio.set_volume(self.volume);
    }
}
