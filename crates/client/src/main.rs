//! r5rust client (v1: single program, no server yet).
//! Loading screen -> Firing Range at the real spawn -> Apex movement (walk / sprint / crouch / slide / jump / mantle), fly (V).
//!
//! Screenshot mode (no window, renders offscreen and exits):
//!   r5rust --screenshot out.png [--frames 120] [--loadshot load.png] [--pos x,y,z] [--yaw d] [--pitch d]
//!          [--hold w,shift,space] [--fly]

mod binds;
mod bloom;
mod csm;
mod dds;
mod fog;
mod lighting;
mod texcache;
mod gfx;
mod loader;
mod menu;

use binds::{Action, Bind, Binds, InputState, UserSettings};
use gfx::{Camera, Gfx};
use menu::Menu;
use glam::Vec3;
use loader::{LoadedWorld, Progress};
use shared::assets::Config;
use shared::sim::{self, Input, Player};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Icon, Window, WindowId};

const TICK: f32 = 1.0 / 60.0;
/// Source / Apex m_yaw: degrees per mouse count at sensitivity 1.
const M_YAW: f32 = 0.022;

#[derive(Default)]
struct Args {
    screenshot: Option<PathBuf>,
    loadshot: Option<PathBuf>,
    frames: u32,
    pos: Option<Vec3>,
    yaw: Option<f32>,
    pitch: Option<f32>,
    hold: Vec<String>,
    fly: bool,
    debug: f32,
    fov: Option<f32>,
    exposure: Option<f32>,
    menu: bool,
    /// second pose "x,y,z,pitch,yaw" (feet): print the auto exposure curve stepping between --pos and it
    expo_curve: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args { frames: 60, ..Default::default() };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let next = |i: &mut usize| -> String {
        *i += 1;
        v.get(*i).cloned().unwrap_or_default()
    };
    while i < v.len() {
        match v[i].as_str() {
            "--screenshot" => a.screenshot = Some(next(&mut i).into()),
            "--loadshot" => a.loadshot = Some(next(&mut i).into()),
            "--frames" => a.frames = next(&mut i).parse().unwrap_or(60),
            "--pos" => {
                let p: Vec<f32> = next(&mut i).split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if p.len() == 3 {
                    a.pos = Some(Vec3::new(p[0], p[1], p[2]));
                }
            }
            "--yaw" => a.yaw = next(&mut i).parse().ok(),
            "--pitch" => a.pitch = next(&mut i).parse().ok(),
            "--hold" => a.hold = next(&mut i).split(',').map(|s| s.trim().to_ascii_lowercase()).collect(),
            "--fly" => a.fly = true,
            "--debug" => a.debug = next(&mut i).parse().unwrap_or(0.0),
            "--fov" => a.fov = next(&mut i).parse().ok(),
            "--exposure" => a.exposure = next(&mut i).parse().ok(),
            "--menu" => a.menu = true,
            "--expo-curve" => a.expo_curve = Some(next(&mut i)),
            other => log::warn!("unknown arg {other}"),
        }
        i += 1;
    }
    a
}

fn load_loadscreen(cfg: &Config) -> Option<image::RgbaImage> {
    let p = cfg.loadscreen()?;
    match image::open(&p) {
        Ok(i) => Some(i.to_rgba8()),
        Err(e) => {
            log::warn!("loadscreen {}: {e}", p.display());
            None
        }
    }
}

fn start_loader(cfg: &Config, gfx: &Gfx, progress: Arc<Progress>) -> Receiver<Result<LoadedWorld, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (cfg, device, queue, layout) = (cfg.clone(), gfx.device.clone(), gfx.queue.clone(), gfx.mat_layout.clone());
    std::thread::Builder::new()
        .name("loader".into())
        .spawn(move || {
            let _ = tx.send(loader::load(&cfg, &device, &queue, &layout, &progress));
        })
        .expect("spawn loader");
    rx
}

/// Spawn point from meta.json (feet position), nudged out of the floor.
fn spawn(world: &LoadedWorld, args: &Args) -> (Player, f32, f32) {
    let s = world.meta.player_spawns.first().copied().unwrap_or([0.0, 0.0, 0.0, 0.0]);
    let pos = args.pos.unwrap_or(Vec3::new(s[0], s[1], s[2]));
    let pos = sim::find_free(&world.collision, pos);
    let mut p = Player::new(pos);
    // no weapon in hand = Apex's holstered move speed (pl+0x6134 ~1.15): R5Flowstate measures sprint 299 = 260 * 1.15,
    // crouch 92 = 80 * 1.15 (tools/r5f-probe)
    p.speed_scale = 1.15;
    p.flying = args.fly;
    (p, args.yaw.unwrap_or(s[3]), args.pitch.unwrap_or(0.0))
}

/// `keys` = held keys plus any key pressed since the last tick (a tap shorter than a tick still counts once).
fn input_from(keys: &HashSet<String>, jump_pressed: bool, yaw: f32, pitch: f32) -> Input {
    let k = |n: &str| keys.contains(n);
    let ax = |p: &str, n: &str| (k(p) as i32 - k(n) as i32) as f32;
    Input {
        forward: ax("w", "s"),
        right: ax("d", "a"),
        null_fwd: k("w") && k("s"),
        null_side: k("d") && k("a"),
        jump: k("space"),
        jump_pressed,
        sprint: k("shift"),
        crouch: k("ctrl"),
        down: k("ctrl"),
        yaw,
        pitch,
    }
}


// ---------------------------------------------------------------- screenshot mode

fn run_headless(cfg: Config, args: Args) -> Result<(), String> {
    let t0 = Instant::now();
    let gfx = Gfx::new(None, cfg.width, cfg.height, &cfg, load_loadscreen(&cfg))?;
    let progress = Arc::new(Progress(AtomicU32::new(0)));
    let rx = start_loader(&cfg, &gfx, progress.clone());
    if let Some(p) = &args.loadshot {
        gfx.screenshot(p, None, 0.35)?;
        println!("loadscreen -> {}", p.display());
    }
    let world = rx.recv().map_err(|e| e.to_string())??;
    let mut gfx = gfx;
    gfx.instant_exposure = true;
    gfx.forced_exposure = args.exposure;
    gfx.set_world(&world);
    let (mut player, yaw, pitch) = spawn(&world, &args);
    player.fly_speed = cfg.fly_speed;
    let keys: HashSet<String> = args.hold.iter().cloned().collect();
    // settle onto the floor for 30 ticks, then hold the keys for --frames ticks
    for _ in 0..30 {
        sim::simulate(&mut player, &input_from(&HashSet::new(), false, yaw, pitch), TICK, &world.collision);
    }
    let start = player.pos;
    let mut max_z = start.z;
    for _ in 0..args.frames {
        sim::simulate(&mut player, &input_from(&keys, false, yaw, pitch), TICK, &world.collision);
        max_z = max_z.max(player.pos.z);
    }
    println!("peak height above start: {:.0}u", max_z - start.z);
    let cam = Camera { pos: player.eye(), yaw, pitch, fov: cfg.fov, debug: args.debug };
    let tc = Instant::now();
    let nc = gfx.prepare(&cam, &world);
    gfx.wait();
    println!("shadow cascades: {nc} rendered in {:.1} ms", tc.elapsed().as_secs_f64() * 1000.0);
    let fu = gfx.frame_uniform(&cam, &world, args.frames as f32 * TICK);
    // GPU timing: a few warm-up frames, then average
    for _ in 0..5 {
        gfx.screenshot_free_draw(&world, &fu);
    }
    gfx.wait();
    let tg = Instant::now();
    let n = 30;
    for _ in 0..n {
        gfx.screenshot_free_draw(&world, &fu);
        gfx.wait();
    }
    let ms = tg.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let path = args.screenshot.clone().unwrap();
    if args.menu {
        let mut m = Menu::new_headless(&gfx.device, gfx.out_format(), UserSettings::load(cfg.sensitivity, cfg.fov));
        gfx.screenshot_with(&path, Some((&world, &fu)), 1.0, |dev, q, view, size| m.draw_headless(dev, q, view, size))?;
    } else {
        gfx.screenshot(&path, Some((&world, &fu)), 1.0)?;
    }
    println!("{}", world.stats);
    println!("chunks drawn: {} of {}", gfx.drawn.get(), world.draws.len());
    let (e, avg) = gfx.read_exposure();
    println!("auto exposure {e:.3} (spot mean lum {avg:.4})");
    println!("GPU: {}  {}x{}  {:.2} ms/frame ({:.0} fps)", gfx.adapter_name, gfx.width, gfx.height, ms, 1000.0 / ms);
    println!(
        "spawn ({:.0},{:.0},{:.0}) -> after {} ticks ({:.0},{:.0},{:.0}) ground={} fly={} moved {:.0}u",
        start.x, start.y, start.z, args.frames, player.pos.x, player.pos.y, player.pos.z, player.on_ground, player.flying, (player.pos - start).length()
    );
    println!("screenshot -> {}  (total {:.1}s)", path.display(), t0.elapsed().as_secs_f64());
    if let Some(b) = &args.expo_curve {
        let v: Vec<f32> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if v.len() != 5 {
            return Err("--expo-curve wants x,y,z,pitch,yaw".into());
        }
        let eye = Player::new(sim::find_free(&world.collision, Vec3::new(v[0], v[1], v[2]))).eye();
        let cam_b = Camera { pos: eye, yaw: v[4], pitch: v[3], fov: cfg.fov, debug: 0.0 };
        expo_curve(&mut gfx, &world, &cam, &cam_b);
    }
    Ok(())
}

/// Auto exposure step response with the real meter + adaptation passes: settle (snap) at one pose, cut to the other,
/// print the exposure after every frame both ways, and check it against the engine formula run on the CPU.
fn expo_curve(gfx: &mut Gfx, world: &LoadedWorld, a: &Camera, b: &Camera) {
    gfx.instant_exposure = false;
    gfx.forced_exposure = None;
    let x = gfx.expo.clone();
    fn frame(gfx: &mut Gfx, world: &LoadedWorld, cam: &Camera) -> (f32, f32) {
        let fu = gfx.frame_uniform(cam, world, 0.0);
        gfx.screenshot_free_draw(world, &fu);
        gfx.read_exposure()
    }
    let settle = |gfx: &mut Gfx, cam: &Camera| {
        gfx.prepare(cam, world);
        gfx.snap_exposure();
        let mut r = (0.0, 0.0);
        for _ in 0..4 {
            r = frame(gfx, world, cam);
        }
        r
    };
    let (ea, la) = settle(gfx, a);
    let (eb, lb) = settle(gfx, b);
    println!("expo curve: pose A settles at {ea:.3} (spot lum {la:.4}), pose B at {eb:.3} (spot lum {lb:.4})");
    let (emin, emax) = (x.min_multiplier * x.min, (x.max_multiplier * x.max).max(x.min_multiplier * x.min));
    for (name, from, to) in [("A -> B", a, b), ("B -> A", b, a)] {
        let (e0, _) = settle(gfx, from);
        gfx.prepare(to, world);
        let mut curve = Vec::new();
        let mut cpu = e0;
        let mut worst = 0f32;
        for _ in 0..600 {
            let (e, avg) = frame(gfx, world, to);
            // the engine's step on the same metered average
            let k = if avg < x.target_min { x.target_min / avg } else if avg > x.target_max.max(x.target_min) { x.target_max.max(x.target_min) / avg } else { 1.0 };
            let t = k * x.compensation.exp2();
            cpu = (cpu + (t - cpu) * (x.rate / cpu.max(t)).clamp(0.0, 1.0)).clamp(emin, emax);
            worst = worst.max((cpu - e).abs());
            curve.push(e);
        }
        let end = *curve.last().unwrap();
        let reach = |f: f32| curve.iter().position(|&e| (e - e0).abs() >= f * (end - e0).abs() - 1e-4).map_or(curve.len(), |i| i + 1);
        let (f50, f90, f99) = (reach(0.5), reach(0.9), reach(0.99));
        println!("{name}: {e0:.3} -> {end:.3}; 50% after {f50} frames, 90% after {f90}, 99% after {f99}; |GPU - CPU formula| <= {worst:.5}");
        for fps in [60.0, 144.0, 240.0] {
            println!("    at {fps:>3} fps: 90% in {:.2} s, 99% in {:.2} s", f90 as f32 / fps, f99 as f32 / fps);
        }
        let line: Vec<String> = curve.iter().take(f99.max(8) + 2).map(|e| format!("{e:.3}")).collect();
        println!("    per frame: {}", line.join(" "));
    }
}

// ---------------------------------------------------------------- windowed game

struct Game {
    world: LoadedWorld,
    player: Player,
    prev_eye: Vec3,
    yaw: f32,
    pitch: f32,
    accum: f32,
}

struct App {
    cfg: Config,
    args: Args,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    progress: Arc<Progress>,
    rx: Option<Receiver<Result<LoadedWorld, String>>>,
    game: Option<Game>,
    /// physical input state, read through the rebindable actions in the settings menu
    input: InputState,
    /// Esc settings menu (created with the window)
    menu: Option<Menu>,
    grabbed: bool,
    last: Instant,
    start: Instant,
    fps_t: Instant,
    fps_n: u32,
}

impl App {
    fn grab(&mut self, on: bool) {
        let Some(w) = &self.window else { return };
        if on {
            let ok = w.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined)).is_ok();
            w.set_cursor_visible(false);
            self.grabbed = ok;
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(true);
            self.grabbed = false;
        }
    }

    fn frame(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;

        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(Ok(world)) => {
                    let gfx = self.gfx.as_mut().unwrap();
                    gfx.set_world(&world);
                    let (mut player, yaw, pitch) = spawn(&world, &self.args);
                    player.fly_speed = self.cfg.fly_speed;
                    log::info!("spawned at {:?} yaw {yaw}", player.pos);
                    self.game = Some(Game { prev_eye: player.eye(), world, player, yaw, pitch, accum: 0.0 });
                    self.rx = None;
                }
                Ok(Err(e)) => {
                    log::error!("map load failed: {e}");
                    eprintln!("map load failed: {e}");
                    el.exit();
                    return;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    log::error!("loader thread died");
                    el.exit();
                    return;
                }
            }
        }

        let gfx = self.gfx.as_mut().unwrap();
        let (binds, fov, menu_open, mantle_boost, auto_sprint) = match &self.menu {
            Some(m) => (m.settings.binds.clone(), m.settings.fov, m.open, m.settings.mantle_boost, m.settings.auto_sprint),
            None => (Binds::default(), self.cfg.fov, false, true, true),
        };
        let frame = gfx.begin_frame();
        if let Some(g) = &mut self.game {
            g.player.cvars.mantle_boost_enabled = mantle_boost;
            g.accum += dt;
            while g.accum >= TICK {
                self.input.begin_tick();
                // the menu freezes input (the sim keeps running, like Apex's Esc menu)
                let input = if menu_open { Input { yaw: g.yaw, pitch: g.pitch, ..Default::default() } } else { bind_input(&self.input, &binds, auto_sprint, g.yaw, g.pitch) };
                self.input.end_tick();
                g.prev_eye = g.player.eye();
                sim::simulate(&mut g.player, &input, TICK, &g.world.collision);
                g.accum -= TICK;
            }
            let alpha = g.accum / TICK;
            let eye = g.prev_eye.lerp(g.player.eye(), alpha);
            let cam = Camera { pos: eye, yaw: g.yaw, pitch: g.pitch, fov, debug: self.args.debug };
            gfx.prepare(&cam, &g.world);
            let fu = gfx.frame_uniform(&cam, &g.world, self.start.elapsed().as_secs_f32());
            if let Some((_, view)) = &frame {
                gfx.draw(view, Some((&g.world, &fu)), 1.0);
            }
        } else if let Some((_, view)) = &frame {
            gfx.draw(view, None, self.progress.get());
        }
        if let Some((tex, view)) = frame {
            if let (Some(m), Some(w)) = (&mut self.menu, &self.window) {
                m.draw(w, &gfx.device, &gfx.queue, view_ref(&view), [gfx.width, gfx.height]);
            }
            gfx.end_frame(tex);
        }
        if self.menu.as_ref().is_some_and(|m| m.quit_requested) {
            el.exit();
        }

        self.fps_n += 1;
        if self.fps_t.elapsed().as_secs_f32() >= 1.0 {
            let fps = self.fps_n as f32 / self.fps_t.elapsed().as_secs_f32();
            if let (Some(w), Some(g)) = (&self.window, &self.game) {
                let p = g.player.pos;
                w.set_title(&format!(
                    "R5Rust  |  {fps:.0} fps  |  {:.0}, {:.0}, {:.0}  ang {:.1} {:.1}  |  {:.0} u/s  |  {}{}",
                    p.x,
                    p.y,
                    p.z,
                    g.pitch,
                    g.yaw,
                    g.player.vel.truncate().length(),
                    g.player.state_name(),
                    if self.menu.as_ref().is_some_and(|m| m.open) { "  |  settings (Esc to resume)" } else if self.grabbed { "  |  Esc: settings" } else { "  |  click to play" }
                ));
            }
            self.fps_n = 0;
            self.fps_t = Instant::now();
        }
    }
}

impl App {
    /// Actions that fire on press rather than feeding the movement sim.
    fn instant_actions(&mut self, b: Bind) {
        let actions: Vec<Action> = match &self.menu {
            Some(m) => m.settings.binds.actions_for(b).collect(),
            None => Binds::default().actions_for(b).collect(),
        };
        for a in actions {
            match a {
                Action::FlyToggle => {
                    if let Some(g) = &mut self.game {
                        g.player.flying = !g.player.flying;
                        g.player.vel = Vec3::ZERO;
                    }
                }
                Action::Respawn => {
                    if let Some(g) = &mut self.game {
                        let (mut p, yaw, pitch) = spawn(&g.world, &Args::default());
                        p.fly_speed = self.cfg.fly_speed;
                        g.player = p;
                        g.prev_eye = g.player.eye();
                        g.yaw = yaw;
                        g.pitch = pitch;
                    }
                }
                Action::Mark => {
                    // F8: log exactly where you are so a movement/collision bug can be reproduced headless
                    if let Some(g) = &self.game {
                        let p = &g.player;
                        let line = format!(
                            "{}  pos {:.2},{:.2},{:.2}  ang {:.2} {:.2}  vel {:.1},{:.1},{:.1} ({:.0} u/s)  state {}  ground {}  flying {}
",
                            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
                            p.pos.x, p.pos.y, p.pos.z, g.pitch, g.yaw, p.vel.x, p.vel.y, p.vel.z, p.vel.truncate().length(),
                            p.state_name(), p.on_ground, p.flying
                        );
                        use std::io::Write;
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("r5rust_marks.txt") {
                            let _ = f.write_all(line.as_bytes());
                        }
                        log::info!("marked: {}", line.trim());
                        if let Some(w) = &self.window {
                            w.set_title(&format!("R5Rust  |  MARKED {:.0}, {:.0}, {:.0} -> r5rust_marks.txt", p.pos.x, p.pos.y, p.pos.z));
                        }
                    }
                }
                Action::Fullscreen => {
                    if let Some(w) = &self.window {
                        w.set_fullscreen(if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) });
                    }
                }
                _ => {}
            }
        }
    }
}

/// Movement input for one sim tick from the rebindable actions.
fn bind_input(st: &InputState, binds: &Binds, auto_sprint: bool, yaw: f32, pitch: f32) -> Input {
    let d = |a| st.down(binds, a);
    let ax = |p, n| (d(p) as i32 - d(n) as i32) as f32;
    Input {
        forward: ax(Action::Forward, Action::Back),
        right: ax(Action::Right, Action::Left),
        null_fwd: d(Action::Forward) && d(Action::Back),
        null_side: d(Action::Right) && d(Action::Left),
        jump: d(Action::Jump),
        jump_pressed: st.pressed(binds, Action::Jump),
        sprint: d(Action::Sprint) || auto_sprint,
        crouch: d(Action::Crouch),
        // fly mode: crouch or the Fly Down bind descends
        down: d(Action::FlyDown) || d(Action::Crouch),
        yaw,
        pitch,
        ..Default::default()
    }
}

fn view_ref(v: &wgpu::TextureView) -> &wgpu::TextureView {
    v
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // square icon (tools/make_icon.py): small = title bar, big = taskbar / Alt-Tab
        let icon = |px: u32| {
            image::load_from_memory(include_bytes!("../../../assets/icon.png")).ok().and_then(|i| {
                let i = i.resize_exact(px, px, image::imageops::FilterType::Lanczos3).to_rgba8();
                Icon::from_rgba(i.into_raw(), px, px).ok()
            })
        };
        let mut attrs = Window::default_attributes()
            .with_title("R5Rust  |  loading Firing Range")
            .with_inner_size(winit::dpi::PhysicalSize::new(self.cfg.width, self.cfg.height))
            .with_window_icon(icon(32));
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_taskbar_icon(icon(256));
        }
        if self.cfg.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        let size = window.inner_size();
        let gfx = match Gfx::new(Some(window.clone()), size.width, size.height, &self.cfg, load_loadscreen(&self.cfg)) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("graphics init failed: {e}");
                el.exit();
                return;
            }
        };
        self.rx = Some(start_loader(&self.cfg, &gfx, self.progress.clone()));
        let settings = UserSettings::load(self.cfg.sensitivity, self.cfg.fov);
        self.menu = Some(Menu::new(&window, &gfx.device, gfx.out_format(), settings));
        self.gfx = Some(gfx);
        self.window = Some(window);
        self.grab(true);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Esc toggles the settings menu (unless the menu is waiting for a key to bind, where it cancels)
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed && !ke.repeat && ke.physical_key == PhysicalKey::Code(KeyCode::Escape) {
                let listening = self.menu.as_ref().is_some_and(|m| m.open && m.is_listening());
                if !listening {
                    if let Some(m) = &mut self.menu {
                        m.toggle();
                        let open = m.open;
                        self.input.clear();
                        self.grab(!open);
                    }
                    return;
                }
            }
        }
        let is_input = matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } | WindowEvent::CursorMoved { .. });
        if let (Some(m), Some(w)) = (&mut self.menu, &self.window) {
            if m.on_window_event(w, &event) && is_input {
                return;
            }
        }
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gfx {
                    g.resize(s.width, s.height);
                }
            }
            WindowEvent::Focused(false) => {
                self.input.clear();
                self.grab(false);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.grabbed {
                    let y = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                    };
                    if y != 0.0 {
                        let up = y > 0.0;
                        self.input.wheel(up, y.abs().round().max(1.0) as u32);
                        self.instant_actions(if up { Bind::WheelUp } else { Bind::WheelDown });
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if !self.grabbed {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        self.grab(true);
                    }
                    return;
                }
                let b = Bind::Mouse(button.into());
                if state == ElementState::Pressed {
                    self.input.press(b);
                    self.instant_actions(b);
                } else {
                    self.input.release(b);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let b = Bind::Key(code);
                if event.state == ElementState::Pressed {
                    if !event.repeat {
                        self.input.press(b);
                        self.instant_actions(b);
                    }
                } else {
                    self.input.release(b);
                }
            }
            WindowEvent::RedrawRequested => self.frame(el),
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: winit::event::DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if !self.grabbed || self.menu.as_ref().is_some_and(|m| m.open) {
                return;
            }
            let (sens, inv) = self.menu.as_ref().map(|m| (m.settings.sensitivity, m.settings.invert_y)).unwrap_or((self.cfg.sensitivity, false));
            if let Some(g) = &mut self.game {
                let s = sens * M_YAW;
                g.yaw = (g.yaw - delta.0 as f32 * s).rem_euclid(360.0);
                g.pitch = (g.pitch + delta.1 as f32 * s * if inv { -1.0 } else { 1.0 }).clamp(-89.0, 89.0);
            }
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")).init();
    let mut cfg = Config::load();
    let args = parse_args();
    if let Some(f) = args.fov {
        cfg.fov = f;
    }
    if args.screenshot.is_some() {
        if let Err(e) = run_headless(cfg, args) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    let el = EventLoop::new().expect("event loop");
    el.set_control_flow(ControlFlow::Poll);
    let now = Instant::now();
    let mut app = App {
        cfg,
        args,
        window: None,
        gfx: None,
        progress: Arc::new(Progress(AtomicU32::new(0))),
        rx: None,
        game: None,
        input: InputState::default(),
        menu: None,
        grabbed: false,
        last: now,
        start: now,
        fps_t: now,
        fps_n: 0,
    };
    if let Err(e) = el.run_app(&mut app) {
        eprintln!("event loop: {e}");
    }
}
