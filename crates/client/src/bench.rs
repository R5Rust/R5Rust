//! `r5rust --bench <seconds> [--graphics preset]`: the game's per-frame work, headless, along a fixed path through
//! the range (racks, loot rows, the open range), at 240 fps game time. Prints CPU per section, the frame-time spread
//! and the throughput with CPU and GPU overlapping like the windowed game (no present / vsync). R5_PROF=1 adds the
//! GPU split of `Gfx::draw` (prof.rs). Measure before and after any performance change.

use std::time::Instant;

use glam::Vec3;

/// Feet positions the camera runs through (looped), with the view yaw it holds there.
const PATH: [(f32, f32, f32, f32); 7] = [
    (-7300.0, 200.0, -18.0, 90.0),
    (-7300.0, 1000.0, -18.0, 90.0),
    (-7370.0, 1250.0, -18.0, 0.0),
    (-7000.0, 1250.0, -18.0, 270.0),
    (-7000.0, 400.0, -18.0, 0.0),
    (-6300.0, 400.0, -18.0, 180.0),
    (-7300.0, 200.0, -18.0, 90.0),
];
/// Sprint speed (u/s), so shadow cascades re-render like they do in play.
const SPEED: f32 = 300.0;
const DT: f32 = 1.0 / 240.0;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThread() -> isize;
    fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
}

/// CPU cycles this thread has actually run (Windows QueryThreadCycleTime): unlike wall time, time the thread spends
/// preempted by other programs doesn't count, so A/B numbers hold up on a busy desktop.
pub fn thread_cycles() -> u64 {
    #[cfg(windows)]
    unsafe {
        let mut c = 0u64;
        QueryThreadCycleTime(GetCurrentThread(), &mut c);
        c
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Cycles per millisecond of this thread, measured by spinning 50 ms (max of 3 tries: least preempted).
pub fn cycles_per_ms() -> f64 {
    (0..3)
        .map(|_| {
            let (t, c) = (Instant::now(), thread_cycles());
            while t.elapsed().as_millis() < 50 {
                std::hint::spin_loop();
            }
            (thread_cycles() - c) as f64 / (t.elapsed().as_secs_f64() * 1000.0)
        })
        .fold(0.0, f64::max)
}

/// Per-section CPU time in thread cycles, reported as ms (cycles_per_ms).
pub struct Sections {
    names: Vec<&'static str>,
    sum: Vec<f64>,
    last: u64,
    cpm: f64,
}

impl Sections {
    pub fn new(cpm: f64) -> Sections {
        Sections { names: Vec::new(), sum: Vec::new(), last: thread_cycles(), cpm }
    }
    pub fn start(&mut self) {
        self.last = thread_cycles();
    }
    pub fn lap(&mut self, name: &'static str) {
        let now = thread_cycles();
        let ms = (now - self.last) as f64 / self.cpm;
        self.last = now;
        match self.names.iter().position(|n| *n == name) {
            Some(i) => self.sum[i] += ms,
            None => {
                self.names.push(name);
                self.sum.push(ms);
            }
        }
    }
    pub fn report(&self, frames: usize) -> String {
        let f = frames.max(1) as f64;
        self.names.iter().zip(&self.sum).map(|(n, s)| format!("{n} {:.3}", s / f)).collect::<Vec<_>>().join("  ")
    }
    pub fn total(&self) -> f64 {
        self.sum.iter().sum()
    }
}

/// The camera at path distance `d`: feet position and yaw (yaw eases toward the next point's).
pub fn path_at(d: f32) -> (Vec3, f32) {
    let segs: Vec<f32> = PATH.windows(2).map(|w| Vec3::new(w[1].0 - w[0].0, w[1].1 - w[0].1, 0.0).length()).collect();
    let total: f32 = segs.iter().sum();
    let mut d = d.rem_euclid(total);
    for (i, &len) in segs.iter().enumerate() {
        if d <= len {
            let t = d / len;
            let (a, b) = (PATH[i], PATH[i + 1]);
            let p = Vec3::new(a.0, a.1, a.2).lerp(Vec3::new(b.0, b.1, b.2), t);
            let mut dy = (b.3 - a.3).rem_euclid(360.0);
            if dy > 180.0 {
                dy -= 360.0;
            }
            return (p, a.3 + dy * t.min(0.3) / 0.3);
        }
        d -= len;
    }
    (Vec3::new(PATH[0].0, PATH[0].1, PATH[0].2), PATH[0].3)
}

/// `--bench <s> --window`: the real windowed frame loop (present, menu, console, HUD) along PATH. The window opens
/// inactive and never grabs the cursor, so it can run on a desktop someone is using.
pub struct WinBench {
    pub secs: f32,
    dist: f32,
    /// measuring since (after a 3 s warm-up once the map is in)
    since: Option<Instant>,
    warm: Option<Instant>,
    frame_ms: Vec<f64>,
    busy_ms: Vec<f64>,
    cycles: u64,
    cpm: f64,
    sec: Sections,
}

impl WinBench {
    pub fn new(secs: f32) -> WinBench {
        let cpm = cycles_per_ms();
        WinBench { secs, dist: 0.0, since: None, warm: None, frame_ms: Vec::new(), busy_ms: Vec::new(), cycles: 0, cpm, sec: Sections::new(cpm) }
    }
    /// Frame start (map loaded): where the camera is now.
    /// CPU time since the previous lap goes to `name` (while measuring).
    pub fn lap(&mut self, name: &'static str) {
        if self.since.is_some() {
            self.sec.lap(name);
        } else {
            self.sec.start();
        }
    }
    pub fn step(&mut self, dt: f32) -> (Vec3, f32) {
        self.cycles = thread_cycles();
        self.dist += SPEED * dt;
        let now = Instant::now();
        let warm = *self.warm.get_or_insert(now);
        if self.since.is_none() && now.duration_since(warm).as_secs_f32() > 3.0 {
            self.since = Some(now);
        }
        path_at(self.dist)
    }
    /// Frame end: record it; true when done.
    pub fn end(&mut self, dt: f32) -> bool {
        let Some(s) = self.since else { return false };
        self.frame_ms.push(dt as f64 * 1000.0);
        self.busy_ms.push((thread_cycles() - self.cycles) as f64 / self.cpm);
        s.elapsed().as_secs_f32() >= self.secs
    }
    pub fn report(&mut self) -> String {
        let n = self.frame_ms.len().max(1) as f64;
        let avg = self.frame_ms.iter().sum::<f64>() / n;
        let busy = self.busy_ms.iter().sum::<f64>() / n;
        let (p50, p95, p99, max) = percentiles(&mut self.frame_ms);
        format!("winbench: {} frames, {avg:.3} ms/frame avg ({:.0} fps), p50 {p50:.3} p95 {p95:.3} p99 {p99:.3} max {max:.2} ms; main thread busy {busy:.3} ms/frame
winbench sections (ms/frame): {}", self.frame_ms.len(), 1000.0 / avg, self.sec.report(self.frame_ms.len()))
    }
}

pub fn percentiles(v: &mut [f64]) -> (f64, f64, f64, f64) {
    v.sort_by(|a, b| a.total_cmp(b));
    let at = |p: f64| v[((v.len() as f64 - 1.0) * p) as usize];
    (at(0.5), at(0.95), at(0.99), *v.last().unwrap_or(&0.0))
}

pub fn frames_for(secs: f32) -> usize {
    (secs / DT) as usize
}

pub fn dt() -> f32 {
    DT
}

pub fn speed() -> f32 {
    SPEED
}
