//! r5rust client (v1: single program, no server yet).
//! Loading screen -> Firing Range at the real spawn -> Apex movement (walk / sprint / crouch / slide / jump / mantle), fly (V).
//!
//! Screenshot mode (no window, renders offscreen and exits):
//!   r5rust --screenshot out.png [--frames 120] [--loadshot load.png] [--pos x,y,z] [--yaw d] [--pitch d]
//!          [--hold w,shift,space] [--fly]
//!   r5rust --screenshot DIR --poses list.txt [--debug 11|12] [--time]   (one PNG per "name x y z_eye pitch yaw" line)
//!   --graphics lowest|low|medium|high|ultra   video preset for this run (default: the saved settings)
//!
//!
//! In game, ` opens the developer console (console.rs, commands in commands.rs). Source-style launch options:
//!   -console          open it at start            +<command> [args]   run after the map loads (+noclip +cl_showfps 2)
//! With --screenshot they render the console into the shot (verification); R5_CONSOLE_INPUT=<text> types into it.

mod anim;
mod viewkick;
mod decals;
mod sway;
mod audio;
mod backdrop;
mod binds;
mod bloom;
mod commands;
mod console;
mod csm;
mod dds;
mod fx;
mod fog;
mod lighting;
mod loadout;
mod loot;
mod melee;
mod texcache;
mod gfx;
mod highlight;
mod rt;
mod bench;
mod prof;
mod graphics;
mod loader;
mod menu;
mod props;
mod rui;
mod weapon;
mod viewmodel;

use binds::{Action, Bind, Binds, InputState, UserSettings};
use gfx::{Camera, Gfx};
use menu::Menu;
use glam::Vec3;
use loader::{LoadedWorld, Progress};
use shared::assets::Config;
use shared::sim::{self, Input, Player};
use std::collections::{BTreeMap, HashSet};
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

pub(crate) const TICK: f32 = 1.0 / 60.0;
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
    /// batch screenshots: a text file of "name x y z_eye pitch yaw" lines, one PNG each into the --screenshot directory
    poses: Option<PathBuf>,
    /// --poses: also time each pose (GPU ms/frame over 30 frames)
    time: bool,
    /// --bench <seconds>: the game's frame loop headless along a fixed path (bench.rs)
    bench: f32,
    /// with --bench: run it in the real window (bench.rs WinBench)
    window: bool,
    /// video preset override (--graphics)
    graphics: Option<graphics::Preset>,
    /// `-console`: open the developer console at start
    console: bool,
    /// `+cmd args`: console commands to run once the map has loaded
    plus: Vec<String>,
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
        // `+cmd a b` collects words until the next +cmd or --flag (negative numbers like -5200 stay arguments)
        if let Some(c) = v[i].strip_prefix('+') {
            a.plus.push(c.to_string());
            i += 1;
            continue;
        }
        if !a.plus.is_empty() && !v[i].starts_with("--") && v[i] != "-console" {
            let last = a.plus.last_mut().unwrap();
            last.push(' ');
            last.push_str(&if v[i].contains(char::is_whitespace) { format!("\"{}\"", v[i]) } else { v[i].clone() });
            i += 1;
            continue;
        }
        match v[i].as_str() {
            "-console" | "--console" => a.console = true,
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
            "--poses" => a.poses = Some(next(&mut i).into()),
            "--time" => a.time = true,
            "--bench" => a.bench = next(&mut i).parse().unwrap_or(20.0),
            "--window" => a.window = true,
            "--graphics" => {
                let v = next(&mut i);
                a.graphics = graphics::Preset::parse(&v);
                if a.graphics.is_none() {
                    log::warn!("--graphics {v}: want lowest, low, medium, high or ultra");
                }
            }
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
    let (cfg, device, queue) = (cfg.clone(), gfx.device.clone(), gfx.queue.clone());
    let tq = gfx.settings.texture_quality;
    std::thread::Builder::new()
        .name("loader".into())
        .spawn(move || {
            let _ = tx.send(loader::load(&cfg, tq, &device, &queue, &progress));
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
pub(crate) fn input_from(keys: &HashSet<String>, jump_pressed: bool, yaw: f32, pitch: f32) -> Input {
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
        use_pressed: k("e"),
        yaw,
        pitch,
    }
}


// ---------------------------------------------------------------- screenshot mode

/// Saved video settings, or the --graphics preset.
fn graphics_for(args: &Args, saved: &UserSettings) -> graphics::GraphicsSettings {
    match args.graphics {
        Some(p) => graphics::GraphicsSettings::preset(p),
        None => saved.graphics.clone(),
    }
}

fn run_headless(cfg: Config, args: Args) -> Result<(), String> {
    let t0 = Instant::now();
    let gs = graphics_for(&args, &UserSettings::load(cfg.sensitivity, cfg.fov));
    println!("video: {:?}", gs);
    let gfx = Gfx::new(None, cfg.width, cfg.height, &cfg, &gs, load_loadscreen(&cfg))?;
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
    gfx.raw_post = (args.debug > 10.5 && args.debug < 12.5) || std::env::var_os("R5_RAWPOST").is_some();
    // headless shots have no console: honour `+fadedistscale x` here so A/B renders work
    for c in &args.plus {
        if let Some(x) = c.strip_prefix("fadedistscale ").and_then(|v| v.trim().parse::<f32>().ok()) {
            gfx.lod.fade_scale = x.clamp(0.0, 1000.0);
        }
        if let Some(x) = c.strip_prefix("r_lod ").and_then(|v| v.trim().parse::<i32>().ok()) {
            gfx.lod.force = x;
        }
        if let Some(x) = c.strip_prefix("mat_set_dress_level ").and_then(|v| v.trim().parse::<u8>().ok()) {
            gfx.lod.dress_level = x;
        }
    }
    if let Some(list) = &args.poses {
        return shoot_poses(&mut gfx, &world, &args, &cfg, list, t0);
    }
    if args.bench > 0.0 {
        return bench_game(cfg, args, gfx, world);
    }
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
    if args.console || !args.plus.is_empty() {
        return console_shot(cfg, args, gfx, world, player, yaw, pitch, &path, t0);
    }
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

/// Headless `-console` / `+cmd`: runs the commands through a window-less App, then renders the frame with the
/// console (if `-console`) and the overlays on top.
#[allow(clippy::too_many_arguments)]
/// `--bench <s>`: the windowed frame loop's work (loot ticks, viewmodel, prepare with shadow cascades, draw, the RUI
/// HUD, the console overlay) headless, moving along bench.rs's path. Numbers to compare before / after a change.
fn bench_game(cfg: Config, args: Args, gfx: Gfx, world: LoadedWorld) -> Result<(), String> {
    let mut gfx = gfx;
    gfx.instant_exposure = false;
    let weapon = load_weapon(&mut gfx, &cfg);
    let fx = weapon.as_ref().and_then(|_| load_fx(&mut gfx, &cfg));
    let settings = UserSettings::load(cfg.sensitivity, cfg.fov);
    let melee = weapon.as_ref().and_then(|_| load_melee(&mut gfx, &cfg, &settings.heirloom));
    let (show_fps, show_pos, fov) = (settings.show_fps, settings.show_pos, settings.fov);
    let mut con = console::Console::new(None, &gfx.device, gfx.out_format(), commands::names());
    let (player, yaw, pitch) = spawn(&world, &args);
    let mut g = Game { prev_eye: player.eye(), prev_trav: (-1, 0.0, 0.0), zip_prompt: None, world, player, yaw, pitch, accum: 0.0, weapon, vm_pose: Vec::new(), proxy_state: -1, proxy_t: 0.0, proxy_debug: None, sim_frozen: false, proxy_model: None, proxy_corr: (Vec3::ZERO, 0.0), sway: Default::default(), sway_melee: false, ads_latch: false, fx, vm_attach: None, sfx: None, melee, loadout: Default::default(), loot: None, r99_stash: None };
    g.start_loot(&gfx, &cfg);
    // R5_RT=1: with the experimental ray-traced sun shadows (rt.rs)
    if std::env::var_os("R5_RT").is_some_and(|v| v != "0") {
        gfx.set_rt(true, Some(&g.world))?;
    }
    let mut hud = if rui::available() { Some(rui::Hud::new()).filter(|h| h.active()) } else { None };
    let mut rr = hud.as_ref().map(|h| rui::Renderer::new(&gfx.device, &gfx.queue, gfx.out_format(), h.bridge.as_ref().and_then(|b| b.fonts_dir.clone())));
    gfx.hud_backdrop_on = hud.is_some();
    let mut prefetched = String::new();
    let target = gfx
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("bench"),
            size: wgpu::Extent3d { width: gfx.width, height: gfx.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gfx.out_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let frames = bench::frames_for(args.bench);
    println!("bench: {frames} frames at {}x{} (game {:?}), {:?}, HUD {}", gfx.width, gfx.height, gfx.game_screen(), gfx.settings, if hud.is_some() { "rui" } else { "off" });
    let (mut dist, mut t) = (0.0f32, 0.0f32);
    let mut run = |gfx: &mut Gfx, g: &mut Game, sec: &mut bench::Sections, frame_ms: &mut Vec<f64>| {
        let f0 = Instant::now();
        sec.start();
        dist += bench::speed() * bench::dt();
        t += bench::dt();
        let (feet, yaw) = bench::path_at(dist);
        g.accum += bench::dt();
        while g.accum >= TICK {
            g.prev_eye = g.player.eye();
            g.player.pos = feet;
            g.yaw = yaw;
            let (eye, dir) = (g.player.eye(), shared::sim::forward_vec(g.yaw, g.pitch));
            if let Some(l) = &mut g.loot {
                l.tick(TICK, eye, dir, false, false, &g.world.collision);
            }
            g.sync_r99();
            g.sync_ground_loot();
            g.accum -= TICK;
        }
        sec.lap("ticks");
        let eye = g.prev_eye.lerp(g.player.eye(), g.accum / TICK);
        let cam = Camera { pos: eye, yaw: g.yaw, pitch: g.pitch, fov, debug: 0.0 };
        let ui = g.loot.as_ref().map(|l| loot_overlay(l, gfx, &cam, &g.world.collision));
        sec.lap("loot overlay");
        update_viewmodel(gfx, g, &cam, VIEWMODEL_FOV, bench::dt());
        sec.lap("viewmodel");
        gfx.prepare(&cam, &g.world);
        sec.lap("prepare+csm");
        let fu = gfx.frame_uniform(&cam, &g.world, t);
        gfx.draw(&target, Some((&g.world, &fu)), 1.0);
        sec.lap("draw");
        if let (Some(h), Some(r)) = (hud.as_mut(), rr.as_mut()) {
            let lists = hud_lists(h, &mut prefetched, g, fov, gfx.game_screen(), prompt_keys(&binds::Binds::default()));
            let refs: Vec<&rui::DrawList> = lists.iter().map(|l| l.as_ref()).collect();
            r.draw(&gfx.device, &gfx.queue, &target, [gfx.width, gfx.height], &refs, gfx.backdrop.as_ref().map(|b| &b.a_view));
        }
        sec.lap("hud");
        con.frame_time(bench::dt());
        con.loot = ui.unwrap_or_default();
        con.draw_headless(&gfx.device, &gfx.queue, &target, [gfx.width, gfx.height], show_fps, show_pos);
        sec.lap("console");
        let _ = gfx.device.poll(wgpu::PollType::Poll);
        frame_ms.push(f0.elapsed().as_secs_f64() * 1000.0);
    };
    // warm-up: a second of frames (pipelines, first cascade renders, texture residency)
    let cpm = bench::cycles_per_ms();
    let (mut sec, mut frame_ms) = (bench::Sections::new(cpm), Vec::new());
    for _ in 0..240 {
        run(&mut gfx, &mut g, &mut sec, &mut frame_ms);
    }
    gfx.wait();
    if let Some(p) = &gfx.prof {
        let _ = p.report();
    }
    let (mut sec, mut frame_ms) = (bench::Sections::new(cpm), Vec::with_capacity(frames));
    let csm_start = gfx.csm_renders();
    gfx.submit_ms.set([0.0; 2]);
    let t0 = Instant::now();
    for _ in 0..frames {
        run(&mut gfx, &mut g, &mut sec, &mut frame_ms);
        if let Some(p) = &gfx.prof {
            p.collect(&gfx.device);
        }
    }
    gfx.wait();
    let wall = t0.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    let cpu_avg = frame_ms.iter().sum::<f64>() / frames as f64;
    let (p50, p95, p99, max) = bench::percentiles(&mut frame_ms);
    println!("bench: {wall:.3} ms/frame throughput ({:.0} fps), CPU frame avg {cpu_avg:.3} p50 {p50:.3} p95 {p95:.3} p99 {p99:.3} max {max:.2} ms", 1000.0 / wall);
    println!("bench CPU sections (ms/frame): {}  [draw: encoder finish {:.3}, submit {:.3}]", sec.report(frames), gfx.submit_ms.get()[0] / cpm / frames as f64, gfx.submit_ms.get()[1] / cpm / frames as f64);
    println!("bench CPU busy (thread cycles, ignores preemption): {:.3} ms/frame ({:.0} fps if CPU-bound)", sec.total() / frames as f64, frames as f64 * 1000.0 / sec.total());
    println!("bench shadow cascade renders: {} in {frames} frames", gfx.csm_renders() - csm_start);
    if let Some(p) = &gfx.prof {
        println!("bench GPU (R5_PROF, serialised frames): {}", p.report());
    }
    Ok(())
}

fn console_shot(cfg: Config, args: Args, gfx: Gfx, world: LoadedWorld, player: Player, yaw: f32, pitch: f32, path: &std::path::Path, t0: Instant) -> Result<(), String> {
    let mut gfx = gfx;
    // the weapon in hand, already drawn (screenshots of poses: +weapon_pose)
    let weapon = load_weapon(&mut gfx, &cfg).map(|mut w| {
        if let Some(vm) = gfx.viewmodel.as_ref() {
            w.debug_pose(vm, "hip", 0.0);
        }
        w
    });
    let fx = weapon.as_ref().and_then(|_| load_fx(&mut gfx, &cfg));
    // screenshots are silent
    let sfx = None;
    let heirloom = UserSettings::load(cfg.sensitivity, cfg.fov).heirloom;
    let melee = weapon.as_ref().and_then(|_| load_melee(&mut gfx, &cfg, &heirloom));
    let settings = UserSettings::load(cfg.sensitivity, cfg.fov);
    let menu = Menu::new_headless(&gfx.device, gfx.out_format(), settings);
    let mut con = console::Console::new(None, &gfx.device, gfx.out_format(), commands::names());
    con.open = args.console;
    let game = {
        let mut g = Game { prev_eye: player.eye(), prev_trav: (-1, 0.0, 0.0), zip_prompt: None, world, player, yaw, pitch, accum: 0.0, weapon, vm_pose: Vec::new(), proxy_state: -1, proxy_t: 0.0, proxy_debug: None, sim_frozen: false, proxy_model: None, proxy_corr: (Vec3::ZERO, 0.0), sway: Default::default(), sway_melee: false, ads_latch: false, fx, vm_attach: None, sfx, melee, loadout: Default::default(), loot: None, r99_stash: None };
        g.start_loot(&gfx, &cfg);
        g
    };
    let now = Instant::now();
    let mut app = App {
        cfg,
        args,
        window: None,
        gfx: Some(gfx),
        progress: Arc::new(Progress(AtomicU32::new(0))),
        rx: None,
        game: Some(game),
        input: InputState::default(),
        menu: Some(menu),
        grabbed: false,
        last: now,
        start: now,
        fps_t: now,
        fps_n: 0,
        next_frame: now,
        console: Some(con),
        aliases: BTreeMap::new(),
        timescale: 1.0,
        quit: false,
        vm_fov: VIEWMODEL_FOV,
        rui: None,
        rui_r: None,
        rui_prefetched: String::new(),
        inv_was_open: false,
        winbench: None,
    };
    for c in app.args.plus.clone() {
        console::print(console::Kind::Input, format!("] {c}"));
        app.exec(&c);
    }
    // a second of sim so +noclip / +setpos / timescale changes show up in the readout
    let g = app.game.as_mut().unwrap();
    if !g.sim_frozen {
        for _ in 0..60 {
            g.prev_eye = g.player.eye();
            sim::simulate(&mut g.player, &Input { yaw: g.yaw, pitch: g.pitch, ..Default::default() }, TICK, &g.world.collision);
        }
    }
    let (show_fps, show_pos) = app.menu.as_ref().map_or((0, false), |m| (m.settings.show_fps, m.settings.show_pos));
    let fov = app.menu.as_ref().map_or(app.cfg.fov, |m| m.settings.fov);
    let (g, gfx, con) = (app.game.as_mut().unwrap(), app.gfx.as_mut().unwrap(), app.console.as_mut().unwrap());
    let base_fov = fov;
    let fov = g.weapon.as_ref().map_or(fov, |w| ads_fov(fov, w));
    let eye = proxy_frame(gfx, g, g.player.eye(), 0.0, 1.0).unwrap_or(g.player.eye());
    let cam = Camera { pos: eye, yaw: g.yaw, pitch: g.pitch, fov, debug: app.args.debug };
    update_viewmodel(gfx, g, &cam, if app.vm_fov > 0.0 { app.vm_fov } else { base_fov }, 0.0);
    g.sync_ground_loot();
    let g = &*g;
    gfx.prepare(&cam, &g.world);
    let fu = gfx.frame_uniform(&cam, &g.world, 1.0);
    gfx.screenshot_free_draw(&g.world, &fu);
    gfx.wait();
    let t = Instant::now();
    for _ in 0..20 {
        gfx.screenshot_free_draw(&g.world, &fu);
        gfx.wait();
    }
    let ms = t.elapsed().as_secs_f32() * 1000.0 / 20.0;
    for _ in 0..100 {
        con.frame_time(ms / 1000.0 * (0.9 + 0.2 * rand_ish()));
    }
    con.hud = console::Hud {
        frame_ms: ms,
        pos: Some(g.player.pos.to_array()),
        ang: [g.pitch, g.yaw],
        vel: g.player.vel.to_array(),
        state: format!("{}{}", g.player.state_name(), if g.player.noclip { " noclip" } else { "" }),
        weapon: None,
    };
    if let Some(l) = &g.loot {
        con.loot = loot_overlay(l, gfx, &cam, &g.world.collision);
    }
    if let Ok(t) = std::env::var("R5_CONSOLE_INPUT") {
        con.set_input(&t);
    }
    // the game's own HUD (rui.rs): crosshair + weapon panel for this state, fetched synchronously
    // laid out at the game resolution (stretched res: 1300x1070), drawn stretched over the whole frame
    let screen = gfx.game_screen();
    let mut hud = if rui::available() { Some(rui::Hud::new()) } else { None };
    let mut lists = Vec::new();
    let mut rr = None;
    // with the inventory: the weapon panel is the inventory's loadout, the crosshair the R-99's when it's in hand
    let in_hand = g.weapon.as_ref().filter(|w| w.inspect.is_none());
    let qs: Vec<serde_json::Value> = match (&g.loot, in_hand) {
        (Some(l), w) if !l.open => w.filter(|_| !g.loadout.melee_shown()).map(|w| crosshair_request(w, false, base_fov, screen)).into_iter().chain([l.hud_request(screen)]).collect(),
        (Some(_), _) => Vec::new(),
        (None, Some(w)) if g.loadout.melee_shown() => vec![hud_request(w, w.ammo, screen)],
        (None, Some(w)) => vec![crosshair_request(w, false, base_fov, screen), hud_request(w, w.ammo, screen)],
        (None, None) => Vec::new(),
    };
    let zp = zipline_prompt(gfx, g, &cam);
    if let Some(h) = hud.as_mut().filter(|h| h.active() && (!qs.is_empty() || zp.is_some())) {
        for q in qs {
            if let Some(l) = h.fetch_sync(&q) {
                lists.push(l);
            }
        }
        // the zipline use prompt (flyout) when a line is in reach
        if let Some(to) = zp {
            let (u, p) = prompt_keys(&binds::Binds::default());
            let q = serde_json::json!({"kind": "flyout", "ui": "zipline_flyout", "screen": screen, "keys": {"use": u, "ping": p}});
            if let Some(l) = h.fetch_sync(&q) {
                lists.push(std::sync::Arc::new(flyout_with_line(&l, to, screen)));
            }
        }
        rr = Some(rui::Renderer::new(&gfx.device, &gfx.queue, gfx.out_format(), h.bridge.as_ref().and_then(|b| b.fonts_dir.clone())));
        gfx.hud_backdrop_on = true;
    } else {
        con.hud.weapon = weapon_hud(g, fov, gfx.height);
    }
    let refs: Vec<&rui::DrawList> = lists.iter().map(|l| l.as_ref()).collect();
    let gfx = &*gfx;
    gfx.screenshot_with(path, Some((&g.world, &fu)), 1.0, |dev, q, view, size| {
        if let Some(r) = rr.as_mut() {
            r.draw(dev, q, view, size, &refs, gfx.backdrop.as_ref().map(|b| &b.a_view));
        }
        con.draw_headless(dev, q, view, size, show_fps, show_pos)
    })?;
    drop(hud);
    println!("console shot -> {}  ({ms:.2} ms/frame GPU, total {:.1}s)", path.display(), t0.elapsed().as_secs_f64());
    Ok(())
}

/// Cheap jitter for the headless frame time graph.
fn rand_ish() -> f32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static S: AtomicU32 = AtomicU32::new(12345);
    let x = S.fetch_add(0x9E37_79B9, Ordering::Relaxed).wrapping_mul(2_654_435_761);
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// `--poses`: one screenshot per "name x y z_eye pitch yaw" line, at exactly that eye position (no collision nudge),
/// with one map load. Used by the z-fighting scan (tools/zscan.py).
fn shoot_poses(gfx: &mut Gfx, world: &LoadedWorld, args: &Args, cfg: &Config, list: &std::path::Path, t0: Instant) -> Result<(), String> {
    let dir = args.screenshot.clone().unwrap_or_else(|| PathBuf::from("."));
    let text = std::fs::read_to_string(list).map_err(|e| format!("{}: {e}", list.display()))?;
    let mut n = 0;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let v: Vec<f32> = f.iter().skip(1).filter_map(|x| x.parse().ok()).collect();
        if f.len() != 6 || v.len() != 5 {
            return Err(format!("bad pose line {line:?} (want: name x y z_eye pitch yaw)"));
        }
        let cam = Camera { pos: Vec3::new(v[0], v[1], v[2]), yaw: v[4], pitch: v[3], fov: args.fov.unwrap_or(cfg.fov), debug: args.debug };
        gfx.prepare(&cam, world);
        let fu = gfx.frame_uniform(&cam, world, 0.0);
        gfx.screenshot_free_draw(world, &fu);
        if args.time {
            // same method as the single --screenshot timing: warm up, then GPU-synchronous frames
            for _ in 0..5 {
                gfx.screenshot_free_draw(world, &fu);
            }
            gfx.wait();
            let t = Instant::now();
            let frames = 30;
            for _ in 0..frames {
                gfx.screenshot_free_draw(world, &fu);
                gfx.wait();
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            // CPU = encoding + submit only; pipelined = frames back to back like the game (CPU and GPU overlap)
            let (mut cpu, t) = (0.0, Instant::now());
            for _ in 0..frames {
                let c = Instant::now();
                gfx.screenshot_free_draw(world, &fu);
                cpu += c.elapsed().as_secs_f64();
            }
            gfx.wait();
            let piped = t.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            let cpu = cpu * 1000.0 / frames as f64;
            // per-pass split (R5_PROF=1): GPU-synchronous frames, each read back
            let split = gfx.prof.is_some().then(|| {
                for _ in 0..frames {
                    gfx.screenshot_free_draw(world, &fu);
                    if let Some(p) = &gfx.prof {
                        p.collect(&gfx.device);
                    }
                }
                gfx.prof.as_ref().map(|p| p.report()).unwrap_or_default()
            });
            let casc: Vec<String> = gfx.time_cascades(cam.pos, world).iter().map(|m| format!("{m:.2}")).collect();
            println!(
                "pose {}: {:.2} ms/frame ({:.0} fps), pipelined {:.2} ms ({:.0} fps), CPU {:.2} ms, {} chunks + {} props in {} draws / {:.2} M tris, cascade re-render ms [{}]",
                f[0], ms, 1000.0 / ms, piped, 1000.0 / piped, cpu, gfx.drawn.get(), gfx.drawn_props.get().0, gfx.drawn_props.get().1, gfx.drawn_tris.get() as f64 / 1e6, casc.join(" ")
            );
            if let Some(s) = split {
                println!("  {s}");
            }
        }
        gfx.screenshot(&dir.join(format!("{}.png", f[0])), Some((world, &fu)), 1.0)?;
        n += 1;
    }
    println!("{n} poses -> {}  (total {:.1}s)", dir.display(), t0.elapsed().as_secs_f64());
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
    /// the traversal (state, animation cycle, sim time) before the last tick: the climb view and hands are
    /// interpolated between ticks like the eye (the sim steps at 60 Hz; unblended they stair-stepped)
    prev_trav: (i32, f32, f64),
    /// the zipline use prompt this frame: the attach point's screen position (0..1, game screen), None = no prompt
    zip_prompt: Option<Option<[f32; 2]>>,
    yaw: f32,
    pitch: f32,
    accum: f32,
    /// the R-99 (weapon.rs), None when its assets aren't extracted
    weapon: Option<weapon::Weapon>,
    vm_pose: Vec<(Vec3, glam::Quat)>,
    /// first-person proxy (mantle hands): the traversal state it is showing and the seconds spent in it
    proxy_state: i32,
    proxy_t: f32,
    /// console mantle_pose: show the proxy at (traversal state, cycle) without a traversal
    pub(crate) proxy_debug: Option<(i32, f32)>,
    /// console sim_hold ran: screenshot mode keeps the player where it left them (no settle ticks)
    pub(crate) sim_frozen: bool,
    /// this frame's proxy placement (rig space -> world), set by `proxy_frame` while traversing
    proxy_model: Option<glam::Affine3A>,
    /// the traversal view's offset from the eye, decaying over 0.2 s once the traversal ends (retail 0x15c0..)
    proxy_corr: (Vec3, f32),
    /// viewmodel sway / bob state (sway.rs), and whether the melee item drove it last frame
    sway: sway::Sway,
    sway_melee: bool,
    /// toggle-ADS state (settings.toggle_ads)
    ads_latch: bool,
    /// weapon effects (fx.rs), None without the baked effects
    fx: Option<fx::Fx>,
    /// the viewmodel's muzzle_flash and shell attachments this frame (view space): (pos, rotation)
    vm_attach: Option<[(Vec3, glam::Mat3); 2]>,
    /// the weapon's sounds (audio.rs), None headless or without an audio device / baked sounds
    sfx: Option<audio::Sfx>,
    /// the melee slot (empty hands or the heirloom picked in Settings > Mods), None when not baked
    melee: Option<melee::Melee>,
    /// which slot is in hand, switching, quick melee (loadout.rs)
    loadout: loadout::Loadout,
    /// the inventory and the range's loot (loot.rs); None without the loot data (then the R-99 is simply in hand)
    pub(crate) loot: Option<loot::Loot>,
    /// the R-99 while it isn't the item in hand (in the inventory but holstered / another slot, or not picked up)
    pub(crate) r99_stash: Option<weapon::Weapon>,
}

impl Game {
    /// The inventory + the range's loot: the player starts with the Firing Range's spawn kit and no weapon (the R-99's
    /// viewmodel waits in the stash until one is picked up), the melee slot in hand.
    fn start_loot(&mut self, gfx: &Gfx, cfg: &Config) {
        self.loot = loot::Loot::load(cfg);
        if self.loot.is_some() {
            self.r99_stash = self.weapon.take();
            self.loadout = loadout::Loadout { slot: loadout::Slot::Melee, pending: None, quick: false };
            if let (Some(m), Some(vm)) = (&mut self.melee, gfx.melee_vm.as_ref()) {
                m.deploy(vm);
            }
        }
    }

    /// Ground loot -> the props' runtime instances (drawn and shadowed with the map's props).
    pub(crate) fn sync_ground_loot(&mut self) {
        let (eye, dir) = (self.player.eye(), shared::sim::forward_vec(self.yaw, self.pitch));
        let (Some(l), Some(props)) = (&mut self.loot, &mut self.world.props) else { return };
        let bsp = &self.world.probe_lookup;
        let mut out = std::mem::take(&mut props.dynamic);
        let mut hl = std::mem::take(&mut props.highlight);
        l.ground_instances(props, |p| match bsp.nearest_probe(p.to_array()) {
            Some(r) => (r.probe & 0xFFFFFF) | ((r.cube_index as u32 & 0xFF) << 24),
            None => 0xFFFFFF | (bsp.nearest_cubemap(p.to_array()) << 24),
        }, eye, dir, &mut out, &mut hl);
        props.dynamic = out;
        props.highlight = hl;
    }

    /// Puts the R-99's viewmodel in hand when the active inventory slot holds one (its mag and extended mag from the
    /// inventory), and back in the stash when it doesn't.
    pub(crate) fn sync_r99(&mut self) {
        let Some(l) = &self.loot else { return };
        let want = l.r99_in_hand();
        if want && self.weapon.is_none() {
            let Some(mut w) = self.r99_stash.take() else { return };
            let s = l.inv.active().unwrap_or(0);
            if let Some(iw) = &l.inv.weapons[s] {
                w.attachments.mag = iw.atts.get("mag").map(|&a| weapon::Attachment { id: "light-mag".into(), tier: l.db.item(a).tier });
                w.ammo = iw.clip.min(w.clip());
            }
            w.reload = None;
            if self.loadout.slot == loadout::Slot::Gun && self.loadout.pending.is_none() {
                w.deploy();
            }
            self.weapon = Some(w);
        } else if !want && self.weapon.is_some() {
            self.r99_stash = self.weapon.take();
        }
    }
}

/// Retail weapon in the hands: tools/build_weapon.py output under the assets root.
const WEAPON_DIR: &str = "weapons/mp_weapon_r97";

/// Loads the first-person weapon onto the GPU and its settings; logs and returns None if not extracted.
fn load_weapon(gfx: &mut Gfx, cfg: &Config) -> Option<weapon::Weapon> {
    let root = cfg.assets_root();
    let dir = root.join(WEAPON_DIR);
    if !dir.join("view.bin").is_file() {
        log::warn!("no first-person weapon: run tools/build_weapon.py ({} missing)", dir.join("view.bin").display());
        return None;
    }
    match (gfx.load_viewmodel(&root, &dir), weapon::Stats::load(&dir.join("weapon.json"))) {
        (Ok(vm), Ok(stats)) => {
            gfx.viewmodel = Some(vm);
            Some(weapon::Weapon::new(stats))
        }
        (Err(e), _) | (_, Err(e)) => {
            log::warn!("weapon: {e}");
            None
        }
    }
}

/// Melee items (tools/build_melee.py): `melee/<id>/` under the assets root.
pub(crate) const MELEE_DIR: &str = "melee";

/// The heirlooms Settings > Mods can pick: (id, name). "" = empty hands.
pub(crate) const HEIRLOOMS: [(&str, &str); 4] = [("", "None (fists)"), ("wraith_kunai", "Wraith - Kunai"), ("octane_knife", "Octane - Butterfly Knife"), ("bat", "Grand Slam (bat)")];

/// The melee slot's item: the heirloom `id` (empty hands when "" or not baked). Loads its viewmodel into `gfx`.
pub(crate) fn load_melee(gfx: &mut Gfx, cfg: &Config, id: &str) -> Option<melee::Melee> {
    let root = cfg.assets_root();
    let pick = if !id.is_empty() && root.join(MELEE_DIR).join(id).join("view.bin").is_file() {
        id
    } else {
        if !id.is_empty() {
            log::warn!("heirloom {id} not baked (tools/build_melee.py): empty hands");
        }
        "emptyhand"
    };
    let dir = root.join(MELEE_DIR).join(pick);
    if !dir.join("view.bin").is_file() {
        log::warn!("no melee item: run tools/build_melee.py ({} missing)", dir.join("view.bin").display());
        gfx.melee_vm = None;
        return None;
    }
    match (gfx.load_viewmodel(&root, &dir), melee::MeleeStats::load(&dir.join("weapon.json"))) {
        (Ok(vm), Ok(stats)) => {
            let m = melee::Melee::new(pick, stats, &vm);
            gfx.melee_vm = Some(vm);
            Some(m)
        }
        (Err(e), _) | (_, Err(e)) => {
            log::warn!("melee {pick}: {e}");
            None
        }
    }
}

/// The weapon effects (tools/build_fx.py output next to the weapon): GPU side into `gfx`, the simulation returned.
/// The first-person proxy arms (tools/build_arms.py): retail's first_person_proxy, playing pov_mp_core's traversal
/// sequences (mantle hands on the ledge) while the held item is put away.
fn load_proxy(gfx: &mut Gfx, cfg: &Config) {
    let root = cfg.assets_root();
    let dir = root.join("arms").join("pov_mp_core");
    if !dir.join("view.bin").is_file() {
        log::warn!("no mantle hands: run tools/build_arms.py ({} missing)", dir.display());
        return;
    }
    match gfx.load_viewmodel(&root, &dir) {
        Ok(vm) => gfx.proxy_vm = Some(vm),
        Err(e) => log::warn!("mantle hands: {e}"),
    }
}

/// Retail's mantle activity for a traversal state (sim/traversal.rs: 3 below, 4 level, 5 above, 6 high, +4 crouched,
/// 11 hanging, 12 jumping off).
fn mantle_activity(state: i32) -> &'static str {
    match state {
        3 => "ACT_VM_MANTLE_BELOW",
        5 => "ACT_VM_MANTLE_ABOVE",
        6 => "ACT_VM_MANTLE_HIGH",
        7 => "ACT_VM_MANTLE_CROUCH_BELOW",
        8 => "ACT_VM_MANTLE_CROUCH_LEVEL",
        9 => "ACT_VM_MANTLE_CROUCH_ABOVE",
        10 => "ACT_VM_MANTLE_CROUCH_HIGH",
        11 => "ACT_VM_MANTLE_IDLE",
        12 => "ACT_VM_MANTLE_JUMPOFF",
        _ => "ACT_VM_MANTLE_LEVEL",
    }
}

/// Mantle hands this frame, if the player is in a traversal and the proxy is baked. Returns whether it drew.
/// The proxy's pose for a traversal state + the sim's cycle into `g.vm_pose`.
fn proxy_pose(vm: &viewmodel::Viewmodel, g: &mut Game, state: i32, sim_cycle: f32, dt: f32) -> bool {
    if state != g.proxy_state {
        g.proxy_state = state;
        g.proxy_t = 0.0;
    } else {
        g.proxy_t += dt;
    }
    // the highest-weight variant (the _NEW_linear ones carry 999)
    let Some(si) = vm.pick_seq(mantle_activity(state), &[], 0.5) else { return false };
    if g.proxy_t == 0.0 {
        log::debug!("mantle hands: state {state} -> {} (cycle {:.2})", vm.seqs[si].name, sim_cycle);
    }
    let dur = vm.seqs[si].duration.max(1e-3);
    let cycle = match state {
        11 => (g.proxy_t / dur).fract(),
        12 => (g.proxy_t / dur).min(0.999),
        // the climbs follow the sim's own animation cycle (traversal.cycle, pl+0x2234)
        _ => sim_cycle.clamp(0.0, 0.999),
    };
    let mut pose = vm.bind_pose();
    vm.sample_seq(si, cycle, &|_| 0.0, &mut pose);
    g.vm_pose = pose;
    true
}

/// Retail's traversal view (C_PredictedFirstPersonProxy::GetTraversalViewPosition, S21 0x1406668a0): the proxy is
/// anchored with its `ref` attachment (jx_c_start) on the ledge point, facing the traversal forward, and the view
/// moves to its `camera` attachment (jx_c_camera); `camera_base` (jx_c_pov) only takes the body's turn out of the
/// view angles (the clips keep it level, so the eye angles stay). The view lerps in from the eye over
/// traversal_viewLerpInDuration 0.15 s (dropping the forward part of the gap), and can't move more than
/// (corner - start) . fwd + half hull - 10 forward of the start. Returns the view origin; sets g.proxy_model.
fn proxy_frame(gfx: &Gfx, g: &mut Game, eye: Vec3, dt: f32, alpha: f32) -> Option<Vec3> {
    g.proxy_model = None;
    if g.proxy_debug.is_some() || !g.player.mantling() || g.player.flying {
        if g.proxy_debug.is_none() {
            g.proxy_state = -1;
        }
        // after the traversal: the view's offset decays back onto the eye over 0.2 s
        if g.proxy_corr.1 > 0.0 {
            let k = (g.proxy_corr.1 / 0.2).clamp(0.0, 1.0);
            g.proxy_corr.1 -= dt;
            return Some(eye + g.proxy_corr.0 * k);
        }
        return None;
    }
    let vm = gfx.proxy_vm.as_ref()?;
    let t = g.player.traversal;
    // between ticks: the cycle and the clock blend from the previous tick (same state only; a new state starts fresh)
    let (ps, pc, ptime) = g.prev_trav;
    let same = ps == t.state && pc <= t.cycle;
    let cycle = if same { pc + (t.cycle - pc) * alpha } else { t.cycle };
    let now = if same { (ptime + (g.player.sim_time() - ptime) * alpha as f64) as f32 } else { g.player.sim_time() as f32 };
    if !proxy_pose(vm, g, t.state, cycle, dt) {
        return None;
    }
    let (rf, cm) = (vm.bone_index("jx_c_start")?, vm.bone_index("jx_c_camera")?);
    let reff = vm.bone_world(&g.vm_pose, rf);
    let camb = vm.bone_world(&g.vm_pose, cm);
    // the ledge frame: ref-local x = left, y = up, z = forward (the rig is X left / Z up / -Y forward)
    let fwd = Vec3::new(t.ledge_forward.x, t.ledge_forward.y, 0.0).normalize_or(Vec3::X);
    let up = Vec3::Z;
    let left = up.cross(fwd);
    let ledge = glam::Affine3A::from_mat3_translation(glam::Mat3::from_cols(left, up, fwd), t.ledge_point);
    let model = ledge * reff.inverse();
    let mut cam = model.transform_point3(Vec3::from(camb.translation));
    // lerp in from the eye (traversal_viewLerpInDuration 0.15); a mantle (type 0) drops the gap's forward part
    let w = (1.0 - (now - t.start_time) / 0.15).clamp(0.0, 1.0);
    let mut off = eye - cam;
    let d = off.dot(fwd);
    if d > 0.0 {
        off -= fwd * d;
    }
    cam += off * w;
    // never past the start column + half hull - 10 along the forward
    let (mn, mx) = g.player.hull();
    let lim = (t.corner - t.start).dot(fwd) + (mx.x - mn.x) * 0.5 - 10.0;
    let dd = (cam - t.start).dot(fwd);
    if lim < dd {
        cam -= fwd * (dd - lim);
    }
    if let (Some(l), Some(r)) = (vm.bone_index("def_l_wrist"), vm.bone_index("def_r_wrist")) {
        let lw = model.transform_point3(Vec3::from(vm.bone_world(&g.vm_pose, l).translation));
        let rw = model.transform_point3(Vec3::from(vm.bone_world(&g.vm_pose, r).translation));
        log::debug!("traversal view: eye {eye:.1} cam {cam:.1} ledge {:.1} fwd {fwd:.2} wrists rel cam {:.1} {:.1}", t.ledge_point, lw - cam, rw - cam);
    }
    g.proxy_model = Some(model);
    g.proxy_corr = (cam - eye, 0.2);
    Some(cam)
}

fn update_proxy(gfx: &mut Gfx, g: &mut Game, cam: &Camera, view: glam::Mat3A, vm_fov: f32, dt: f32) -> bool {
    gfx.vm_proxy = false;
    let model = g.proxy_model;
    let debug = g.proxy_debug;
    if model.is_none() && debug.is_none() {
        return false;
    }
    let Some(vm) = &gfx.proxy_vm else { return false };
    if model.is_none() {
        // console mantle_pose: no ledge, pinned to the eye like a weapon
        let (state, cycle) = debug.unwrap_or((4, 0.5));
        if !proxy_pose(vm, g, state, cycle, dt) {
            return false;
        }
    }
    let probe = match g.world.probe_lookup.nearest_probe(cam.pos.to_array()) {
        Some(r) => (r.probe & 0xFFFFFF) | ((r.cube_index as u32 & 0xFF) << 24),
        None => 0xFFFFFF | (g.world.probe_lookup.nearest_cubemap(cam.pos.to_array()) << 24),
    };
    // anchored in the world, the proxy is a world entity (C_FirstPersonProxy : C_BaseAnimating): the world FOV;
    // the console's pinned pose keeps the viewmodel's
    let clip = gfx.viewmodel_clip(cam, if model.is_some() { cam.fov } else { vm_fov });
    match model {
        // anchored on the ledge, drawn relative to the traversal view
        Some(m) => {
            let rel = glam::Affine3A::from_translation(-cam.pos) * m;
            if let Some(l) = vm.bone_index("def_l_wrist") {
                let p = rel.transform_point3(Vec3::from(vm.bone_world(&g.vm_pose, l).translation));
                log::debug!("proxy upload: wrist rel {p:.1} clip {:.3}", clip.project_point3(p));
            }
            vm.upload_world(&gfx.queue, &g.vm_pose, rel, clip, probe)
        }
        None => vm.upload(&gfx.queue, &g.vm_pose, view, glam::Affine3A::IDENTITY, clip, probe),
    }
    gfx.vm_proxy = true;
    gfx.vm_visible = true;
    g.vm_attach = None;
    weapon_feedback(gfx, g, cam, clip, gfx.view_proj(cam).0, dt);
    true
}

/// Retail bullet-hole decals (tools/build_decals.py) + the map's surface table (export-map surfaces.json).
fn load_decals(gfx: &mut Gfx, cfg: &Config) {
    let dir = cfg.assets_root().join("decals").join("bullets");
    if !dir.join("decals.json").is_file() {
        log::warn!("no bullet decals: run tools/build_decals.py ({} missing)", dir.display());
        return;
    }
    let surfaces = cfg.collision_bin().with_file_name("surfaces.json");
    let (hdr, depth, samples) = gfx.world_targets();
    match decals::Decals::load(&gfx.device, &gfx.queue, &dir, &surfaces, gfx.frame_layout(), hdr, depth, samples) {
        Ok(d) => gfx.decals = Some(d),
        Err(e) => log::warn!("bullet decals: {e}"),
    }
}

fn load_fx(gfx: &mut Gfx, cfg: &Config) -> Option<fx::Fx> {
    load_decals(gfx, cfg);
    load_proxy(gfx, cfg);
    let dir = cfg.assets_root().join(WEAPON_DIR).join("fx");
    if !dir.join("layers.bin").is_file() {
        log::warn!("no weapon effects: run tools/build_fx.py ({} missing)", dir.join("layers.bin").display());
        return None;
    }
    let (hdr, depth, samples) = gfx.world_targets();
    match fx::FxGpu::load(&gfx.device, &gfx.queue, &dir, hdr, depth, samples) {
        Ok((gpu, sim)) => {
            gfx.fx = Some(gpu);
            Some(sim)
        }
        Err(e) => {
            log::warn!("weapon effects: {e}");
            None
        }
    }
}

/// The first-person weapon's FOV (horizontal at 4:3). 58 picked by Kainz from renders at 70/58/50/42 next to his retail R-99
/// hipfire screenshot (2026-11-18).
const VIEWMODEL_FOV: f32 = 58.0;

/// Player FOV while aiming: zoom_fov scaled by the player's FOV / 70 (Apex's default ADS FOV scaling).
/// The viewmodel FOV for this frame: retail draws the gun at a fixed FOV and zooms it by the same ratio as the world
/// (SetUpView FUN_140838830: tan(vm/2) = tan(world/2) * tan(35) / tan(hip/2)), so in ADS the gun grows with the zoom.
/// `vm_hip` = our hipfire viewmodel FOV (console viewmodel_fov, matched to retail by eye).
fn zoomed_vm_fov(vm_hip: f32, world: f32, hip: f32) -> f32 {
    let half = |f: f32| (f.to_radians() * 0.5).tan();
    (half(vm_hip) * half(world) / half(hip).max(1e-4)).atan().to_degrees() * 2.0
}

fn ads_fov(fov: f32, w: &weapon::Weapon) -> f32 {
    let a = w.ads_view;
    let k = a * a * (3.0 - 2.0 * a);
    fov + (w.stats.zoom_fov * fov / 70.0 - fov) * k
}

/// The HUD's weapon readout: ammo, reload progress, crosshair gap from the spread cone at the current FOV.
fn weapon_hud(g: &Game, fov: f32, height: u32) -> Option<console::WeaponHud> {
    let w = g.weapon.as_ref()?;
    if g.player.flying || w.inspect.is_some() {
        return None;
    }
    let f = ads_fov(fov, w);
    let vfov = 2.0 * ((f.to_radians() * 0.5).tan() * 0.75).atan();
    let gap = (w.spread.to_radians() * 0.5).tan() / (vfov * 0.5).tan() * height as f32 * 0.5;
    let reload = w.reload.map(|(t, empty)| t / if empty { w.stats.reload_empty } else { w.stats.reload });
    Some(console::WeaponHud { name: w.stats.name.clone(), ammo: w.ammo, clip: w.stats.clip, reload, gap_px: gap, ads: w.ads })
}

/// The weapon panel request for the RUI (rui.rs / tools/rui/bridge.mjs): the live ammo, mag size and attachments.
fn hud_request(w: &weapon::Weapon, ammo: u32, screen: [u32; 2]) -> serde_json::Value {
    serde_json::json!({ "kind": "hud", "screen": screen, "weapon": "r99", "mag": ammo, "magMax": w.clip(), "tier": 1, "attachments": w.attachments.json() })
}

/// The crosshair request: the R-99's RUI_CrosshairData args (weapon_spread, player_zoomFrac, player_fov, ...),
/// quantised so nearby states share a cached draw list. weapon_spread is the cone in RADIANS: crosshair_plus puts
/// the ticks at 30 + spread * 540 * 90 / fov px (1080p), and Kainz's retail R-99 hipfire shot (gap ~25 px at fov 110)
/// matches 2 deg = 0.0349; degrees put the ticks at the screen edges.
fn crosshair_request(w: &weapon::Weapon, sprinting: bool, fov: f32, screen: [u32; 2]) -> serde_json::Value {
    let q = |x: f32, s: f32| (x / s).round() * s;
    serde_json::json!({ "kind": "crosshair", "ui": "crosshair_plus", "screen": screen, "args": {
        "adjustedSpread": q(w.spread.to_radians(), 0.001), "adsFrac": q(w.ads, 0.05), "playerFov": fov.round(),
        "isSprinting": sprinting, "isReloading": w.reload.is_some(), "teamColor": [1.0, 1.0, 1.0] } })
}

/// The RUI draw lists for this frame (crosshair, weapon panel), asking for whatever isn't cached yet. When the
/// loadout changes every ammo count of the new mag is asked for at once, so the counter never waits on a host call.
/// The zipline use prompt: S30 shows ui/zipline_flyout while a zipline is in reach (the same search the use key
/// runs, sim/zipline.rs find_mount), anchored at the attach point. Some(None) = shown, the point is off screen.
fn zipline_prompt(gfx: &Gfx, g: &Game, cam: &Camera) -> Option<Option<[f32; 2]>> {
    if g.player.flying || g.player.zip.active() || g.player.mantling() {
        return None;
    }
    let f = shared::sim::forward_vec(g.yaw, g.pitch);
    let (_, q, _) = shared::sim::zipline::find_mount(&g.player, &g.world.collision, f, g.player.sim_time() as f32)?;
    let (vp, _) = gfx.view_proj(cam);
    let c = vp * (q - cam.pos).extend(1.0);
    if c.w <= 1e-3 {
        return Some(None);
    }
    Some(Some([c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5]))
}

/// The flyout's leader line: from its anchor dot to the projected attach point, a 2-unit white quad (retail's line).
fn flyout_with_line(l: &rui::DrawList, to: Option<[f32; 2]>, screen: [u32; 2]) -> rui::DrawList {
    let mut out = l.clone();
    let (Some(to), Some(dot)) = (to, l.widgets.iter().find(|w| w.ty == 2 && w.size == [8.0, 8.0])) else { return out };
    let (sw, sh) = (screen[0] as f32, screen[1] as f32);
    let from = [(dot.pos[0] + dot.grad[0] * 0.5 + dot.grad[2] * 0.5) * sw, (dot.pos[1] + dot.grad[1] * 0.5 + dot.grad[3] * 0.5) * sh];
    let to = [to[0] * sw, to[1] * sh];
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 4.0 {
        return out;
    }
    let t = 2.0 * sh / 1080.0;
    let (px, py) = (-dy / len * t, dx / len * t);
    let mut w = dot.clone();
    w.pos = [(from[0] - px * 0.5) / sw, (from[1] - py * 0.5) / sh];
    w.grad = [dx / sw, dy / sh, px / sw, py / sh];
    w.size = [len, t];
    // under the labels
    out.widgets.insert(0, w);
    out
}

/// The use / ping keys as the prompts show them (first bind of each).
fn prompt_keys(b: &binds::Binds) -> (String, String) {
    let k = |a| b.get(a).iter().flatten().next().map_or("?".to_string(), |x| x.label());
    (k(binds::Action::Use), k(binds::Action::Ping))
}

fn hud_lists(hud: &mut rui::Hud, prefetched: &mut String, g: &Game, fov: f32, screen: [u32; 2], keys: (String, String)) -> Vec<std::sync::Arc<rui::DrawList>> {
    hud.poll();
    let mut out = hud_lists_inner(hud, prefetched, g, fov, screen);
    if let Some(to) = g.zip_prompt {
        let q = serde_json::json!({"kind": "flyout", "ui": "zipline_flyout", "screen": screen, "keys": {"use": keys.0, "ping": keys.1}});
        if let Some(l) = hud.get("flyout", &q) {
            out.push(std::sync::Arc::new(flyout_with_line(&l, to, screen)));
        }
    }
    out
}

fn hud_lists_inner(hud: &mut rui::Hud, prefetched: &mut String, g: &Game, fov: f32, screen: [u32; 2]) -> Vec<std::sync::Arc<rui::DrawList>> {
    if let Some(l) = &g.loot {
        return loot_hud_lists(hud, prefetched, g, l, fov, screen);
    }
    let Some(w) = &g.weapon else { return Vec::new() };
    if g.player.flying || w.inspect.is_some() || g.melee.as_ref().is_some_and(|m| m.inspecting() && g.loadout.melee_shown()) {
        return Vec::new();
    }
    let key = format!("{}|{:?}|{:?}", w.clip(), w.attachments, screen);
    if *prefetched != key {
        for n in (0..=w.clip()).rev() {
            hud.want(&hud_request(w, n, screen));
        }
        *prefetched = key;
    }
    let mut out = Vec::new();
    if !g.loadout.melee_shown() {
        if let Some(l) = hud.get("crosshair", &crosshair_request(w, g.player.sprinting, fov, screen)) {
            out.push(l);
        }
    }
    if let Some(l) = hud.get("weapon", &hud_request(w, w.ammo, screen)) {
        out.push(l);
    }
    out
}

/// Slot keys (1 / 2), the use key and TAB for this tick; returns whether a weapon slot was asked for (the loadout then
/// brings the gun up).
fn loot_hands(g: &mut Game, keys: [bool; 2], use_p: bool, use_d: bool, tab: bool) -> bool {
    let (eye, dir) = (g.player.eye(), shared::sim::forward_vec(g.yaw, g.pitch));
    let Some(l) = &mut g.loot else { return false };
    if tab {
        l.open = !l.open;
    }
    let mut want = false;
    for (i, &k) in keys.iter().enumerate() {
        if k && l.inv.weapons[i].is_some() {
            l.inv.hands = shared::loot::Hands::Weapon(i);
            l.inv.last_weapon = i;
            want = true;
        }
    }
    let ev = l.tick(TICK, eye, dir, use_p && !l.open, use_d && !l.open, &g.world.collision);
    if ev.switch_to.is_some() {
        want = true;
    }
    g.sync_r99();
    g.sync_ground_loot();
    want
}

/// The test UI's view of the loot: item labels (projected, only the ones in sight), the use prompt, the inventory.
fn loot_overlay(l: &loot::Loot, gfx: &Gfx, cam: &Camera, coll: &shared::collision::Collision) -> console::LootUi {
    let (vp, _) = gfx.view_proj(cam);
    let dir = shared::sim::forward_vec(cam.yaw, cam.pitch);
    let target = l.world.target(&l.db, cam.pos, dir);
    // items with a ground model show it (retail draws no labels: the use prompt names the targeted one), so only the
    // model-less ones are projected and ray-tested
    let mut near: Vec<&shared::loot::GroundItem> = l.world.items.iter().filter(|g| !l.has_model(g.item) && (g.pos - cam.pos).length() < 650.0).collect();
    near.sort_by(|a, b| (a.pos - cam.pos).length().total_cmp(&(b.pos - cam.pos).length()));
    let mut labels = Vec::new();
    for g in near.into_iter().take(90) {
        let p = g.pos + Vec3::new(0.0, 0.0, 6.0);
        let clip = vp * (p - cam.pos).extend(1.0);
        if clip.w <= 0.0 {
            continue;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.x.abs() > 1.0 || ndc.y.abs() > 1.0 {
            continue;
        }
        let to = p - cam.pos;
        let d = to.length();
        if d > 8.0 && coll.ray(cam.pos, to / d, d - 6.0).is_some() {
            continue;
        }
        let it = l.db.item(g.item);
        let name = loot::name(&l.db, g.item);
        let name = if it.kind.stacks() && g.count > 1 { format!("{name} x{}", g.count) } else { name };
        labels.push(console::LootLabel { x: ndc.x * 0.5 + 0.5, y: 0.5 - ndc.y * 0.5, name, tier: it.tier, targeted: Some(g.id) == target });
    }
    console::LootUi {
        labels,
        prompt: if l.open { None } else { l.prompt(cam.pos, dir) },
        prompt_icon: if l.open { None } else { l.target_item(cam.pos, dir).and_then(|i| l.icon(i)) },
        hold: l.hold_frac(),
        message: l.message.as_ref().map(|m| m.0.clone()),
        inv: l.open.then(|| inventory_view(l)),
    }
}

fn inventory_view(l: &loot::Loot) -> console::InvView {
    let db = &l.db;
    let inv = &l.inv;
    let slot = |id: Option<shared::loot::ItemId>| id.map(|i| console::InvSlot { name: loot::name(db, i), tier: db.item(i).tier, count: 1, icon: l.icon(i) });
    let weapon = |i: usize| {
        inv.weapons[i].as_ref().map(|w| console::InvWeapon {
            name: loot::name(db, w.item),
            icon: l.icon(w.item),
            tier: db.item(w.item).tier,
            clip: w.clip,
            clip_max: w.clip_size(db),
            reserve: if inv.infinite_ammo { "inf".into() } else { inv.reserve(db, i).unwrap_or(0).to_string() },
            atts: db
                .item(w.item)
                .supported
                .iter()
                .map(|p| (p.clone(), w.atts.get(p).map(|&a| console::InvSlot { name: loot::name(db, a), tier: db.item(a).tier, count: 1, icon: l.icon(a) })))
                .collect(),
            active: inv.active() == Some(i),
        })
    };
    console::InvView {
        gear: vec![
            ("Helmet".into(), slot(inv.helmet)),
            ("Body shield".into(), slot(inv.armor)),
            ("Knockdown shield".into(), slot(inv.incap)),
            ("Backpack".into(), slot(inv.bag)),
        ],
        shield: (inv.shield, inv.armor.map_or(0, |a| shared::loot::inventory::armor_shield(db.item(a).tier))),
        weapons: [weapon(0), weapon(1)],
        backpack: inv.backpack.iter().map(|s| console::InvSlot { name: loot::name(db, s.item), tier: db.item(s.item).tier, count: s.count, icon: l.icon(s.item) }).collect(),
        capacity: inv.capacity(db),
        infinite: inv.infinite_ammo,
    }
}

/// The weapon panel (and crosshair) from the inventory: two weapon tabs, the active one's mag + reserve + attachments.
/// When the loadout changes every mag count of the active weapon is asked for at once, so the counter never waits.
fn loot_hud_lists(hud: &mut rui::Hud, prefetched: &mut String, g: &Game, l: &loot::Loot, fov: f32, screen: [u32; 2]) -> Vec<std::sync::Arc<rui::DrawList>> {
    if g.player.flying || l.open || g.weapon.as_ref().is_some_and(|w| w.inspect.is_some()) || g.melee.as_ref().is_some_and(|m| m.inspecting() && g.loadout.melee_shown()) {
        return Vec::new();
    }
    let q = l.hud_request(screen);
    let active = l.inv.active();
    let mut key_q = q.clone();
    if let Some(a) = active {
        key_q["slots"][a]["mag"] = serde_json::Value::Null;
    }
    let key = key_q.to_string();
    if *prefetched != key {
        if let Some(a) = active {
            let max = q["slots"][a]["magMax"].as_u64().unwrap_or(0);
            for n in (0..=max).rev() {
                let mut v = q.clone();
                v["slots"][a]["mag"] = serde_json::json!(n);
                hud.want(&v);
            }
        }
        *prefetched = key;
    }
    let mut out = Vec::new();
    if let Some(w) = g.weapon.as_ref().filter(|_| !g.loadout.melee_shown()) {
        if let Some(c) = hud.get("crosshair", &crosshair_request(w, g.player.sprinting, fov, screen)) {
            out.push(c);
        }
    }
    if let Some(p) = hud.get("weapon", &q) {
        out.push(p);
    }
    out
}

/// Poses the viewmodel for this frame and uploads it (eye light probe, its own projection).
/// m_fov: the player's FOV setting (unzoomed): Apex's viewmodel follows it, so a wider FOV shows a smaller gun.
fn update_viewmodel(gfx: &mut Gfx, g: &mut Game, cam: &Camera, vm_fov: f32, dt: f32) {
    let view = glam::Mat3A::from_rotation_z(cam.yaw.to_radians()) * glam::Mat3A::from_rotation_y(cam.pitch.to_radians());
    // traversal (mantle): the proxy arms replace the held item, which keeps simulating (lowered) out of view
    if update_proxy(gfx, g, cam, view, vm_fov, dt) {
        return;
    }
    // the melee slot on screen: pose + upload its viewmodel, the gun keeps simulating out of view
    gfx.vm_melee = g.loadout.melee_shown();
    if gfx.vm_melee {
        let (Some(m), Some(vm)) = (&mut g.melee, &gfx.melee_vm) else {
            gfx.vm_visible = false;
            weapon_feedback(gfx, g, cam, gfx.viewmodel_clip(cam, vm_fov), gfx.view_proj(cam).0, dt);
            return;
        };
        m.pose(vm, dt, &mut g.vm_pose);
        let sway = sway_frame(&mut g.sway, &mut g.sway_melee, true, &m.stats.sway, &g.player, cam, 0.0, dt);
        let probe = match g.world.probe_lookup.nearest_probe(cam.pos.to_array()) {
            Some(r) => (r.probe & 0xFFFFFF) | ((r.cube_index as u32 & 0xFF) << 24),
            None => 0xFFFFFF | (g.world.probe_lookup.nearest_cubemap(cam.pos.to_array()) << 24),
        };
        let clip = gfx.viewmodel_clip(cam, vm_fov);
        vm.upload(&gfx.queue, &g.vm_pose, view, sway, clip, probe);
        gfx.vm_visible = !g.player.flying;
        if let Some(w) = &g.weapon {
            gfx.impacts = w.impact_instances();
        }
        g.vm_attach = None;
        weapon_feedback(gfx, g, cam, clip, gfx.view_proj(cam).0, dt);
        return;
    }
    let (Some(w), Some(vm)) = (&mut g.weapon, &gfx.viewmodel) else {
        gfx.vm_visible = false;
        return;
    };
    w.pose(vm, dt, &mut g.vm_pose);
    // the gun shows its weapon share of the punch on top of the camera's (x vmScale)
    let (vp, vy) = w.punch.viewmodel(&w.stats.viewkick, w.ads_view);
    let view = glam::Mat3A::from_rotation_z((cam.yaw + vy).to_radians()) * glam::Mat3A::from_rotation_y((cam.pitch + vp).to_radians());
    let sway = sway_frame(&mut g.sway, &mut g.sway_melee, false, &w.stats.sway, &g.player, cam, w.ads_view, dt);
    let probe = match g.world.probe_lookup.nearest_probe(cam.pos.to_array()) {
        Some(r) => (r.probe & 0xFFFFFF) | ((r.cube_index as u32 & 0xFF) << 24),
        None => 0xFFFFFF | (g.world.probe_lookup.nearest_cubemap(cam.pos.to_array()) << 24),
    };
    let clip = gfx.viewmodel_clip(cam, vm_fov);
    vm.upload(&gfx.queue, &g.vm_pose, view, sway, clip, probe);
    gfx.vm_visible = !g.player.flying;
    gfx.impacts = w.impact_instances();
    // the muzzle and ejection port as drawn; the muzzle as seen on screen, put back into the world at the same view
    // depth, is where this frame's bullets' tracers start
    let (world_clip, _) = gfx.view_proj(cam);
    g.vm_attach = match (vm.bone_index("muzzle_flash"), vm.bone_index("shell")) {
        (Some(m), Some(s)) => {
            let a = vm.attachments(&g.vm_pose, view, sway, &[m, s]);
            Some([a[0], a[1]])
        }
        _ => None,
    };
    if let Some([(m, _), _]) = g.vm_attach {
        let fwd = shared::sim::forward_vec(cam.yaw, cam.pitch);
        let ndc = clip.project_point3(m);
        let d = m.dot(fwd).max(1.0);
        // reverse-infinite projection: ndc z = near / depth (world near plane 4)
        w.muzzle_off = world_clip.inverse().project_point3(Vec3::new(ndc.x, ndc.y, 4.0 / d));
    }
    weapon_feedback(gfx, g, cam, clip, world_clip, dt);
}

/// One frame of viewmodel sway / bob (sway.rs) for the item on screen; restarts when the item changes.
#[allow(clippy::too_many_arguments)]
fn sway_frame(s: &mut sway::Sway, last_melee: &mut bool, melee: bool, st: &sway::Settings, p: &Player, cam: &Camera, ads: f32, dt: f32) -> glam::Affine3A {
    if *last_melee != melee {
        s.reset();
        *last_melee = melee;
    }
    if p.flying || dt <= 0.0 {
        return glam::Affine3A::IDENTITY;
    }
    s.update(st, &sway::Frame { dt, vel: p.vel, angles: Vec3::new(cam.pitch, cam.yaw, 0.0), ads, on_ground: p.on_ground })
}

/// One frame of the viewmodel + effects without drawing (console `weapon_fire`, which steps them every tick).
pub(crate) fn step_viewmodel(a: &mut App, dt: f32) {
    let fov = a.menu.as_ref().map_or(a.cfg.fov, |m| m.settings.fov);
    let vm_fov = if a.vm_fov > 0.0 { a.vm_fov } else { fov };
    let debug = a.args.debug;
    let (Some(g), Some(gfx)) = (a.game.as_mut(), a.gfx.as_mut()) else { return };
    let f = g.weapon.as_ref().map_or(fov, |w| ads_fov(fov, w));
    let cam = Camera { pos: g.player.eye(), yaw: g.yaw, pitch: g.pitch, fov: f, debug };
    update_viewmodel(gfx, g, &cam, zoomed_vm_fov(vm_fov, f, fov), dt);
}

/// This frame's weapon events -> effects (fx.rs); advances them and hands the frame's sprites and shells to gfx.
fn weapon_feedback(gfx: &mut Gfx, g: &mut Game, cam: &Camera, view_clip: glam::Mat4, world_clip: glam::Mat4, dt: f32) {
    let Some(w) = &mut g.weapon else { return };
    let events = std::mem::take(&mut w.events);
    let anim = std::mem::take(&mut w.anim_events);
    let melee_anim = g.melee.as_mut().map(|m| std::mem::take(&mut m.anim_events)).unwrap_or_default();
    if let Some(s) = &mut g.sfx {
        for e in &events {
            if matches!(e, weapon::Event::Shot { first: true } | weapon::Event::AdsIn | weapon::Event::Reload { .. }) {
                s.cut_sequence();
            }
            s.gun_event(e, w.ammo, w.clip());
        }
        for e in &anim {
            s.anim_event(e, false);
        }
        for e in &melee_anim {
            s.anim_event(e, true);
        }
        s.update(w.ammo, w.clip());
    }
    // the magazine bodygroup (reload sequences hide / show it)
    if let Some(vm) = &gfx.viewmodel {
        for e in &anim {
            match e.kind.as_str() {
                "AE_WPN_CLIPBODYGROUP_HIDE" => vm.set_group_visible("clip", false),
                "AE_WPN_CLIPBODYGROUP_SHOW" => vm.set_group_visible("clip", true),
                _ => {}
            }
        }
        if w.reload.is_none() {
            vm.set_group_visible("clip", true);
        }
    }
    let Some(fx) = &mut g.fx else { return };
    let fwd = shared::sim::forward_vec(cam.yaw, cam.pitch);
    let right = shared::sim::forward_vec(cam.yaw - 90.0, 0.0);
    let up = right.cross(fwd).normalize();
    for e in events {
        match e {
            weapon::Event::Shot { .. } => {
                if let Some([(m, mr), (s, sr)]) = g.vm_attach {
                    // the attachment axis that points down the barrel
                    let axes = [mr.x_axis, mr.y_axis, mr.z_axis];
                    let a = axes.iter().copied().max_by(|a, b| a.dot(fwd).abs().total_cmp(&b.dot(fwd).abs())).unwrap_or(fwd);
                    let dir = if a.dot(fwd) < 0.0 { -a } else { a };
                    if std::env::var_os("R5_FX_LOG").is_some() {
                        log::info!("muzzle {m:?} axes x {:?} y {:?} z {:?} fwd {fwd:?} -> {dir:?}; shell {s:?}", mr.x_axis, mr.y_axis, mr.z_axis);
                    }
                    fx.muzzle_flash(m, dir, up);
                    fx.shell(s, right, up, fwd, sr);
                }
            }
            weapon::Event::Impact { pos, normal } => fx.impact(pos, normal),
            _ => {}
        }
    }
    fx.tick(dt);
    let f = &mut gfx.fx_frame;
    f.world.clear();
    f.view.clear();
    f.shells.clear();
    fx.sprites(fx::Space::World, cam.pos, &mut f.world);
    fx.sprites(fx::Space::View, cam.pos, &mut f.view);
    fx.shell_instances(&mut f.shells);
    // tracers: every bullet in flight, drawn from the gun and closing onto its real path over its first 0.1 s; a
    // short streak behind the bullet that never reaches back past the first 48 units from the muzzle
    for p in &w.projectiles {
        let speed = p.vel.length().max(1.0);
        let dir = p.vel / speed;
        let travelled = p.age * speed;
        let origin = p.pos - dir * travelled;
        let at = |d: f32| origin + dir * d + p.muzzle_off * (1.0 - d / (0.1 * speed)).max(0.0);
        let tail_d = (travelled - (speed * 0.012).min(260.0)).max(48.0);
        if travelled > tail_d + 4.0 {
            fx.tracer(at(tail_d), at(travelled), cam.pos, &mut f.world);
        }
    }
    // R5_FX=noworld / noview / noshells: leave a group out (checking effects in screenshots)
    match std::env::var("R5_FX").as_deref() {
        Ok("noworld") => f.world.clear(),
        Ok("noview") => f.view.clear(),
        Ok("noshells") => f.shells.clear(),
        _ => {}
    }
    f.world_clip = world_clip;
    f.view_clip = view_clip;
    f.right = right;
    f.up = up;
    let l = &g.world.light;
    f.to_sun = Vec3::from(l.sun_dir).normalize_or(Vec3::Z);
    f.sun = l.sun_color;
    f.sky = l.sky_color;
    f.shell_layer = fx.shell_layer();
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
    /// when the next frame may start (Video > FPS cap)
    next_frame: Instant,
    /// ` developer console (created with the window)
    console: Option<console::Console>,
    /// console `alias` definitions
    aliases: BTreeMap<String, String>,
    /// console `host_timescale`
    timescale: f32,
    /// console `quit`
    quit: bool,
    /// console `viewmodel_fov`: the first-person weapon's FOV (horizontal at 4:3); 0 = follow the player FOV
    vm_fov: f32,
    /// the game's own HUD code (rui.rs) and its renderer; None = the simple egui HUD
    rui: Option<rui::Hud>,
    rui_r: Option<rui::Renderer>,
    /// the loadout the HUD's ammo counts were last prefetched for
    rui_prefetched: String,
    /// the inventory screen was open last frame (the mouse is let go while it is)
    inv_was_open: bool,
    /// --bench --window
    winbench: Option<bench::WinBench>,
}

impl App {
    /// The inventory screen's clicks (console.rs) and the mouse while it's open.
    fn loot_actions(&mut self) {
        let actions = self.console.as_mut().map(|c| std::mem::take(&mut c.loot_actions)).unwrap_or_default();
        let open = self.game.as_ref().and_then(|g| g.loot.as_ref()).is_some_and(|l| l.open);
        if let Some(g) = &mut self.game {
            let (eye, dir) = (g.player.eye(), shared::sim::forward_vec(g.yaw, g.pitch));
            if let Some(l) = &mut g.loot {
                for a in actions {
                    match a {
                        console::InvAction::DropStack(i, all) => {
                            let n = if all { u32::MAX } else { 1 };
                            if let Some(d) = l.inv.drop_stack(i, n) {
                                l.throw(vec![d], eye, dir);
                            }
                        }
                        console::InvAction::DropWeapon(i) => {
                            let drops = l.inv.drop_weapon(&l.db.clone(), i);
                            l.throw(drops, eye, dir);
                        }
                        console::InvAction::Detach(i, p) => {
                            let drops = l.inv.detach(&l.db.clone(), i, &p);
                            l.throw(drops, eye, dir);
                        }
                        console::InvAction::Infinite => l.inv.infinite_ammo = !l.inv.infinite_ammo,
                        console::InvAction::Close => l.open = false,
                    }
                }
            }
            g.sync_r99();
            // the last gun dropped: the melee item comes up
            let melee_now = g.loot.as_ref().is_some_and(|l| l.inv.hands == shared::loot::Hands::Melee);
            if melee_now && g.loadout.slot == loadout::Slot::Gun && g.loadout.pending.is_none() {
                let vm = self.gfx.as_ref().and_then(|x| x.melee_vm.as_ref());
                g.loadout.request(loadout::Slot::Melee, g.weapon.as_mut(), g.melee.as_mut(), vm);
            }
        }
        let open = open && self.game.as_ref().and_then(|g| g.loot.as_ref()).is_some_and(|l| l.open);
        if open != self.inv_was_open {
            self.inv_was_open = open;
            self.input.clear();
            let menu_open = self.menu.as_ref().is_some_and(|m| m.open);
            self.grab(!open && !menu_open);
        }
    }

    fn grab(&mut self, on: bool) {
        let Some(w) = &self.window else { return };
        // a benchmark window never takes the mouse
        let on = on && self.winbench.is_none();
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
                    let weapon = load_weapon(gfx, &self.cfg);
                    let fx = weapon.as_ref().and_then(|_| load_fx(gfx, &self.cfg));
                    let heirloom = self.menu.as_ref().map(|m| m.settings.heirloom.clone()).unwrap_or_default();
                    let melee = weapon.as_ref().and_then(|_| load_melee(gfx, &self.cfg, &heirloom));
                    let sfx = weapon.as_ref().and_then(|_| {
                        let mut s = audio::Sfx::new()?;
                        s.load_gun(&self.cfg.assets_root().join(WEAPON_DIR));
                        if let Some(m) = &melee {
                            s.load_melee(&self.cfg.assets_root().join(MELEE_DIR).join(&m.id));
                        }
                        s.load_player(&self.cfg.assets_root().join("player"));
                        Some(s)
                    });
                    self.game = Some(Game { prev_eye: player.eye(), prev_trav: (-1, 0.0, 0.0), zip_prompt: None, world, player, yaw, pitch, accum: 0.0, weapon, vm_pose: Vec::new(), proxy_state: -1, proxy_t: 0.0, proxy_debug: None, sim_frozen: false, proxy_model: None, proxy_corr: (Vec3::ZERO, 0.0), sway: Default::default(), sway_melee: false, ads_latch: false, fx, vm_attach: None, sfx, melee, loadout: Default::default(), loot: None, r99_stash: None });
                    if let Some(g) = &mut self.game {
                        g.start_loot(gfx, &self.cfg);
                    }
                    self.rx = None;
                    // like Quake / Source: cfg/autoexec.cfg, once the map (and so the movement cvars) exists
                    if ["cfg/autoexec.cfg"].iter().any(|p| std::path::Path::new(p).is_file()) || std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("cfg/autoexec.cfg").is_file())).unwrap_or(false) {
                        self.exec("exec autoexec");
                    }
                    for c in self.args.plus.clone() {
                        self.exec(&c);
                    }
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

        self.run_console();
        let console_open = self.console.as_ref().is_some_and(|c| c.open);
        // Settings > Mods picked another heirloom: swap the melee item (and its sounds) in place
        if let Some(id) = self.menu.as_mut().filter(|m| m.heirloom_changed).map(|m| {
            m.heirloom_changed = false;
            m.settings.heirloom.clone()
        }) {
            if let (Some(g), Some(gfx)) = (&mut self.game, &mut self.gfx) {
                g.melee = load_melee(gfx, &self.cfg, &id);
                if let (Some(s), Some(m)) = (&mut g.sfx, &g.melee) {
                    s.load_melee(&self.cfg.assets_root().join(MELEE_DIR).join(&m.id));
                }
                if g.loadout.melee_shown() {
                    if let (Some(m), Some(vm)) = (&mut g.melee, &gfx.melee_vm) {
                        m.deploy(vm);
                    }
                }
            }
        }
        let (show_fps, show_pos) = self.menu.as_ref().map_or((0, false), |m| (m.settings.show_fps, m.settings.show_pos));
        let gfx = self.gfx.as_mut().unwrap();
        let (binds, fov, menu_open, mantle_boost, auto_sprint, toggle_ads) = match &self.menu {
            Some(m) => (m.settings.binds.clone(), m.settings.fov, m.open || console_open, m.settings.mantle_boost, m.settings.auto_sprint, m.settings.toggle_ads),
            None => (Binds::default(), self.cfg.fov, console_open, true, true, false),
        };
        // video settings changed in the menu: apply live
        if let Some(m) = &self.menu {
            if m.settings.graphics != gfx.settings {
                gfx.apply_settings(&m.settings.graphics, self.game.as_ref().map(|g| &g.world));
            }
        }
        let fps_cap = gfx.settings.fps_cap;
        let mut loot_ui: Option<console::LootUi> = None;
        if let Some(b) = &mut self.winbench {
            b.lap("events+console cmds");
        }
        let frame = gfx.begin_frame();
        if let Some(b) = &mut self.winbench {
            b.lap("acquire");
        }
        if let (Some(g), Some(b)) = (&mut self.game, &mut self.winbench) {
            let (feet, yaw) = b.step(dt);
            g.player.pos = feet;
            g.player.vel = Vec3::ZERO;
            g.prev_eye = g.player.eye();
            g.yaw = yaw;
            g.pitch = 0.0;
        }
        if let Some(g) = &mut self.game {
            g.player.cvars.mantle_boost_enabled = mantle_boost;
            g.accum += dt * self.timescale;
            while g.accum >= TICK {
                self.input.begin_tick();
                // the menu freezes input (the sim keeps running, like Apex's Esc menu)
                let mut input = if menu_open { Input { yaw: g.yaw, pitch: g.pitch, ..Default::default() } } else { bind_input(&self.input, &binds, auto_sprint && sim::auto_sprint(&g.player), g.yaw, g.pitch) };
                let mut wi = weapon::WeaponInput::default();
                let mut mi = melee::MeleeInput::default();
                if !menu_open && !g.player.flying {
                    // toggle ADS: the aim key flips it; pressing sprint drops out of aim (Apex)
                    if !toggle_ads || self.input.pressed(&binds, Action::Sprint) {
                        g.ads_latch = false;
                    } else if self.input.pressed(&binds, Action::Ads) {
                        g.ads_latch = !g.ads_latch;
                    }
                    let mut hi = loadout::HandsInput {
                        slot_gun: self.input.pressed(&binds, Action::WeaponSlot1),
                        slot_melee: self.input.pressed(&binds, Action::HolsterSlot),
                        melee: self.input.pressed(&binds, Action::Melee),
                        fire: self.input.down(&binds, Action::Fire),
                        ads: if toggle_ads { g.ads_latch } else { self.input.down(&binds, Action::Ads) },
                        reload: self.input.pressed(&binds, Action::Reload),
                        inspect: self.input.pressed(&binds, Action::Inspect),
                    };
                    if g.loot.is_some() {
                        let keys = [self.input.pressed(&binds, Action::WeaponSlot1), self.input.pressed(&binds, Action::WeaponSlot2)];
                        let (use_p, use_d) = (self.input.pressed(&binds, Action::Use), self.input.down(&binds, Action::Use));
                        let tab = self.input.pressed(&binds, Action::Inventory);
                        hi.slot_gun = loot_hands(g, keys, use_p, use_d, tab);
                    }
                    // a weapon switch / holster / melee drops toggle-ADS: coming back to the gun starts at the hip
                    // (it used to come back aimed: the latch survived the swap)
                    if hi.slot_gun || hi.slot_melee || hi.melee || g.loadout.pending.is_some() || g.loadout.slot != loadout::Slot::Gun {
                        g.ads_latch = false;
                        hi.ads = false;
                    }
                    let (w_in, m_in) = g.loadout.tick(hi, &mut g.weapon, &mut g.melee, gfx.melee_vm.as_ref());
                    wi = w_in;
                    mi = m_in;
                    if let Some(l) = &mut g.loot {
                        if g.loadout.slot == loadout::Slot::Melee && g.loadout.pending.is_none() {
                            l.inv.hands = shared::loot::Hands::Melee;
                        }
                    }
                } else if let Some(l) = &mut g.loot {
                    // flying / menu: the range's loot still respawns
                    let (eye, dir) = (g.player.eye(), shared::sim::forward_vec(g.yaw, g.pitch));
                    l.tick(TICK, eye, dir, false, false, &g.world.collision);
                }
                self.input.end_tick();
                // aiming, firing and swinging stop a sprint (Apex); the item in hand sets the run speed
                if wi.fire || wi.ads || g.melee.as_ref().is_some_and(|m| m.attacking()) {
                    input.sprint = false;
                }
                g.player.speed_scale = g.loadout.speed_scale(g.weapon.as_ref(), g.melee.as_ref());
                g.prev_eye = g.player.eye();
                g.prev_trav = (g.player.traversal.state, g.player.traversal.cycle, g.player.sim_time());
                sim::simulate(&mut g.player, &input, TICK, &g.world.collision);
                // zipline cues -> the S30 script's 1p sounds (attach on mount, loop on start, detach on stop)
                let ev = std::mem::take(&mut g.player.zip.events);
                if let Some(s) = g.sfx.as_mut() {
                    use shared::sim::zipline::{ZIP_EV_MOUNT, ZIP_EV_START, ZIP_EV_STOP};
                    if ev & ZIP_EV_MOUNT != 0 {
                        s.zipline(audio::ZipSound::Mount);
                    }
                    if ev & ZIP_EV_START != 0 {
                        s.zipline(audio::ZipSound::Start);
                    }
                    if ev & ZIP_EV_STOP != 0 {
                        s.zipline(audio::ZipSound::Stop);
                    }
                }
                if let Some(w) = &mut g.weapon {
                    wi.sprinting = g.player.sprinting;
                    wi.crouched = g.player.ducked;
                    wi.on_ground = g.player.on_ground;
                    wi.moving = g.player.vel.truncate().length() > 50.0;
                    wi.sliding = g.player.sliding;
                    // the game lowers held items while mantling / wall climbing (WeaponLowerMask)
                    wi.lowered = g.player.mantling() || g.player.on_wall() || g.player.zip.active();
                    wi.speed = g.player.vel.truncate().length();
                    // the reload takes from the inventory: the backpack's ammo of this type (or the gun's own reserve)
                    let slot = g.loot.as_ref().and_then(|l| l.inv.active());
                    if let (Some(l), Some(s)) = (&g.loot, slot) {
                        w.reserve = if l.inv.infinite_ammo { None } else { l.inv.reserve(&l.db, s) };
                    }
                    let before = w.reserve;
                    let (dp, dy) = w.tick(TICK, wi, g.player.eye(), g.yaw, g.pitch, &g.world.collision);
                    g.pitch = (g.pitch + dp).clamp(-89.0, 89.0);
                    g.yaw = (g.yaw + dy).rem_euclid(360.0);
                    if let (Some(l), Some(s)) = (&mut g.loot, slot) {
                        if let (Some(b), Some(a)) = (before, w.reserve) {
                            let used = b.saturating_sub(a);
                            if used > 0 {
                                let item = l.inv.weapons[s].as_ref().map(|iw| iw.item);
                                match item.and_then(|i| l.db.ammo_of(i)) {
                                    Some(ammo) => {
                                        l.inv.remove(ammo, used);
                                    }
                                    None => {
                                        if let Some(iw) = l.inv.weapons[s].as_mut() {
                                            iw.reserve = iw.reserve.saturating_sub(used);
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(iw) = l.inv.weapons[s].as_mut() {
                            iw.clip = w.ammo;
                        }
                    }
                }
                if let (Some(m), Some(vm)) = (&mut g.melee, &gfx.melee_vm) {
                    mi.sprinting = g.player.sprinting;
                    mi.crouched = g.player.ducked;
                    mi.air = !g.player.on_ground;
                    mi.sliding = g.player.sliding;
                    mi.lowered = g.player.mantling() || g.player.on_wall() || g.player.zip.active();
                    mi.speed = g.player.vel.truncate().length();
                    m.tick(TICK, mi, vm);
                }
                g.accum -= TICK;
            }
            let alpha = g.accum / TICK;
            let mut eye = g.prev_eye.lerp(g.player.eye(), alpha);
            // mantles: the view follows the first-person proxy's animated camera (retail GetTraversalViewPosition)
            if let Some(v) = proxy_frame(gfx, g, eye, dt, alpha) {
                eye = v;
            }
            let base_fov = fov;
            let fov = g.weapon.as_ref().map_or(fov, |w| ads_fov(fov, w));
            // the view punch (recoil): the camera shows (1 - weaponFraction) of it; the aim (g.yaw / g.pitch) never moves
            let (pp, py) = g.weapon.as_ref().map_or((0.0, 0.0), |w| w.punch.camera(&w.stats.viewkick, w.ads_view));
            let cam = Camera { pos: eye, yaw: g.yaw + py, pitch: (g.pitch + pp).clamp(-89.0, 89.0), fov, debug: self.args.debug };
            let vm_fov = if self.vm_fov > 0.0 { self.vm_fov } else { base_fov };
            if let Some(b) = &mut self.winbench {
                b.lap("ticks");
            }
            loot_ui = g.loot.as_ref().map(|l| loot_overlay(l, gfx, &cam, &g.world.collision));
            g.zip_prompt = zipline_prompt(gfx, g, &cam);
            update_viewmodel(gfx, g, &cam, zoomed_vm_fov(vm_fov, fov, base_fov), dt);
            if let Some(b) = &mut self.winbench {
                b.lap("overlay+viewmodel");
            }
            gfx.prepare(&cam, &g.world);
            if let Some(b) = &mut self.winbench {
                b.lap("prepare+csm");
            }
            let fu = gfx.frame_uniform(&cam, &g.world, self.start.elapsed().as_secs_f32());
            if let Some((_, view)) = &frame {
                gfx.draw(view, Some((&g.world, &fu)), 1.0);
            }
        } else if let Some((_, view)) = &frame {
            gfx.draw(view, None, self.progress.get());
        }
        if let Some(b) = &mut self.winbench {
            b.lap("draw");
        }
        if let (Some((_, view)), Some(g), Some(hud), Some(r)) = (&frame, &self.game, &mut self.rui, &mut self.rui_r) {
            let keys = self.menu.as_ref().map_or_else(|| prompt_keys(&binds::Binds::default()), |m| prompt_keys(&m.settings.binds));
            let lists = hud_lists(hud, &mut self.rui_prefetched, g, fov, gfx.game_screen(), keys);
            let refs: Vec<&rui::DrawList> = lists.iter().map(|l| l.as_ref()).collect();
            r.draw(&gfx.device, &gfx.queue, view, [gfx.width, gfx.height], &refs, gfx.backdrop.as_ref().map(|b| &b.a_view));
        }
        if let Some((tex, view)) = frame {
            if let Some(b) = &mut self.winbench {
                b.lap("hud");
            }
            if let (Some(m), Some(w)) = (&mut self.menu, &self.window) {
                m.draw(w, &gfx.device, &gfx.queue, view_ref(&view), [gfx.width, gfx.height]);
            }
            if let (Some(c), Some(w)) = (&mut self.console, &self.window) {
                c.frame_time(dt);
                c.hud = console::Hud {
                    frame_ms: dt * 1000.0,
                    pos: self.game.as_ref().map(|g| g.player.pos.to_array()),
                    ang: self.game.as_ref().map_or([0.0; 2], |g| [g.pitch, g.yaw]),
                    vel: self.game.as_ref().map_or([0.0; 3], |g| g.player.vel.to_array()),
                    state: self.game.as_ref().map_or(String::new(), |g| {
                        format!("{}{}", g.player.state_name(), if g.player.noclip { " noclip" } else if g.player.flying { " fly" } else { "" })
                    }),
                    weapon: if self.rui.as_ref().is_some_and(|h| h.active()) { None } else { self.game.as_ref().and_then(|g| weapon_hud(g, fov, gfx.height)) },
                };
                c.loot = loot_ui.unwrap_or_default();
                c.draw(w, &gfx.device, &gfx.queue, view_ref(&view), [gfx.width, gfx.height], show_fps, show_pos);
            }
            if let Some(b) = &mut self.winbench {
                b.lap("menu+console");
            }
            gfx.end_frame(tex);
            if let Some(b) = &mut self.winbench {
                b.lap("present");
            }
        }
        self.loot_actions();
        if let (Some(b), true) = (&mut self.winbench, self.game.is_some()) {
            if b.end(dt) {
                println!("{}", b.report());
                self.quit = true;
            }
        }
        if self.quit || self.menu.as_ref().is_some_and(|m| m.quit_requested) {
            el.exit();
        }
        if fps_cap > 0 {
            // sleep most of the wait, spin the last ~1.5 ms (Windows sleeps overshoot)
            let period = std::time::Duration::from_secs_f64(1.0 / fps_cap as f64);
            let now = Instant::now();
            self.next_frame = if self.next_frame + period < now { now } else { self.next_frame };
            let left = self.next_frame.saturating_duration_since(now);
            if left > std::time::Duration::from_micros(1500) {
                std::thread::sleep(left - std::time::Duration::from_micros(1500));
            }
            while Instant::now() < self.next_frame {
                std::hint::spin_loop();
            }
            self.next_frame += period;
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
            self.do_action(a);
        }
        // console `bind <key> <commands>`
        if let Some(cmd) = self.menu.as_ref().and_then(|m| m.settings.console_binds.get(&b.label()).cloned()) {
            self.exec(&cmd);
        }
    }

    fn set_console(&mut self, open: bool) {
        if let Some(c) = &mut self.console {
            if c.open != open {
                c.toggle();
            }
        }
        self.input.clear();
        let menu_open = self.menu.as_ref().is_some_and(|m| m.open);
        self.grab(!open && !menu_open);
    }

    pub fn do_action(&mut self, a: Action) {
        {
            match a {
                Action::FlyToggle => {
                    if let Some(g) = &mut self.game {
                        g.player.flying = !g.player.flying;
                        g.player.noclip = false;
                        g.player.vel = Vec3::ZERO;
                    }
                }
                Action::Console => {
                    let open = self.console.as_ref().is_some_and(|c| c.open);
                    self.set_console(!open);
                }
                Action::Respawn => {
                    if let Some(g) = &mut self.game {
                        let (mut p, yaw, pitch) = spawn(&g.world, &Args::default());
                        p.fly_speed = self.cfg.fly_speed;
                        // console-set movement cvars survive a respawn
                        p.cvars = g.player.cvars;
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
    // the S21 client's KeyState (r5apex_dx12 0x140900e40): a key that went down this tick counts 0.5, one pressed
    // and released within the tick (a wheel notch) 0.25, held 1.0
    let ks = |a| {
        let held = binds.get(a).iter().flatten().any(|b| st.held.contains(b));
        match (held, st.pressed(binds, a)) {
            (true, true) => 0.5,
            (true, false) => 1.0,
            (false, true) => 0.25,
            (false, false) => 0.0,
        }
    };
    let ax = |p, n| ks(p) - ks(n);
    Input {
        forward: ax(Action::Forward, Action::Back),
        right: ax(Action::Right, Action::Left),
        null_fwd: d(Action::Forward) && d(Action::Back),
        null_side: d(Action::Right) && d(Action::Left),
        jump: d(Action::Jump),
        jump_pressed: st.pressed(binds, Action::Jump),
        use_pressed: st.pressed(binds, Action::Use),
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
        if self.cfg.fullscreen && self.winbench.is_none() {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        if self.winbench.is_some() {
            // don't take focus from whoever is using the desktop
            attrs = attrs.with_active(false);
        }
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        // started from the launcher the window once opened parked at (-32000, -32000) with a 198x56 size (Windows'
        // minimized spot): bring a window that isn't on any monitor back onto the primary one, at its real size
        if !self.cfg.fullscreen {
            window.set_minimized(false);
            let on_screen = window.outer_position().ok().is_some_and(|p| {
                window.available_monitors().any(|m| {
                    let (mp, ms) = (m.position(), m.size());
                    p.x + 64 > mp.x && p.y + 16 > mp.y && p.x < mp.x + ms.width as i32 - 64 && p.y < mp.y + ms.height as i32 - 16
                })
            });
            if !on_screen {
                if let Some(m) = window.primary_monitor().or_else(|| window.available_monitors().next()) {
                    let (mp, ms) = (m.position(), m.size());
                    let (w, h) = (self.cfg.width.min(ms.width), self.cfg.height.min(ms.height));
                    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
                    window.set_outer_position(winit::dpi::PhysicalPosition::new(mp.x + (ms.width.saturating_sub(w) / 2) as i32, mp.y + (ms.height.saturating_sub(h) / 2) as i32));
                    log::warn!("window opened off-screen; moved onto {:?}", m.name());
                }
            }
        }
        let size = window.inner_size();
        let settings = UserSettings::load(self.cfg.sensitivity, self.cfg.fov);
        let gs = graphics_for(&self.args, &settings);
        let gfx = match Gfx::new(Some(window.clone()), size.width, size.height, &self.cfg, &gs, load_loadscreen(&self.cfg)) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("graphics init failed: {e}");
                el.exit();
                return;
            }
        };
        self.rx = Some(start_loader(&self.cfg, &gfx, self.progress.clone()));
        let mut settings = settings;
        settings.graphics = gs;
        self.menu = Some(Menu::new(&window, &gfx.device, gfx.out_format(), settings));
        self.console = Some(console::Console::new(Some(&window), &gfx.device, gfx.out_format(), commands::names()));
        if rui::available() {
            let hud = rui::Hud::new();
            if hud.active() {
                self.rui_r = Some(rui::Renderer::new(&gfx.device, &gfx.queue, gfx.out_format(), hud.bridge.as_ref().and_then(|b| b.fonts_dir.clone())));
                self.rui = Some(hud);
            }
        }
        let mut gfx = gfx;
        gfx.hud_backdrop_on = self.rui.is_some();
        self.gfx = Some(gfx);
        self.window = Some(window);
        self.grab(true);
        if self.args.console {
            self.set_console(true);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let listening = self.menu.as_ref().is_some_and(|m| m.open && m.is_listening());
        let console_open = self.console.as_ref().is_some_and(|c| c.open);
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed && !ke.repeat && !listening {
                if let PhysicalKey::Code(code) = ke.physical_key {
                    // the console key (Controls tab, default `) opens / closes it from anywhere, Esc closes it
                    let console_key = self.menu.as_ref().map_or(Binds::default(), |m| m.settings.binds.clone()).get(Action::Console).contains(&Some(Bind::Key(code)));
                    if console_key || (console_open && code == KeyCode::Escape) {
                        self.set_console(!console_open && console_key);
                        return;
                    }
                }
            }
        }
        let is_input = matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } | WindowEvent::CursorMoved { .. });
        let inv_open = self.game.as_ref().and_then(|g| g.loot.as_ref()).is_some_and(|l| l.open);
        if console_open || inv_open {
            if let (Some(c), Some(w)) = (&mut self.console, &self.window) {
                if c.on_window_event(w, &event) && is_input {
                    return;
                }
            }
        }
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
            WindowEvent::Focused(false) if !self.console.as_ref().is_some_and(|c| c.open) => {
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
            if !self.grabbed || self.menu.as_ref().is_some_and(|m| m.open) || self.console.as_ref().is_some_and(|c| c.open) {
                return;
            }
            let (sens, inv) = self.menu.as_ref().map(|m| (m.settings.sensitivity, m.settings.invert_y)).unwrap_or((self.cfg.sensitivity, false));
            if let Some(g) = &mut self.game {
                // ADS: Apex's default per-zoom sensitivity keeps the same feel at the zoomed FOV (tan ratio)
                let fov = self.menu.as_ref().map_or(self.cfg.fov, |m| m.settings.fov);
                let zoom = g.weapon.as_ref().map_or(1.0, |w| {
                    let half = |f: f32| (f.to_radians() * 0.5).tan();
                    half(ads_fov(fov, w)) / half(fov)
                });
                let s = sens * M_YAW * zoom;
                let mut d = (delta.1 as f32 * s * if inv { -1.0 } else { 1.0 }, -(delta.0 as f32) * s);
                // moving against the recoil eats the punch before it turns the aim (retail FUN_140a33ff0)
                if let Some(w) = &mut g.weapon {
                    let (st, ads) = (w.stats.viewkick.clone(), w.ads_view);
                    d = w.punch.absorb(&st, ads, d);
                }
                g.yaw = (g.yaw + d.1).rem_euclid(360.0);
                g.pitch = (g.pitch + d.0).clamp(-89.0, 89.0);
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
    // env_logger, mirrored into the console
    console::init_logging(env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")));
    let mut cfg = Config::load();
    // content registry (legends, weapons, maps, ... + mods): only reported for now, nothing reads it yet
    let content = shared::registry::Content::load_default();
    log::info!("content: {}", content.summary());
    for e in &content.errors {
        log::warn!("content: {e}");
    }
    match content.map_by_bsp(&cfg.map) {
        Some((_, id, m)) => log::info!("map {id} ({})", m.name),
        None => log::warn!("map {} isn't in content/maps", cfg.map),
    }
    let args = parse_args();
    if let Some(f) = args.fov {
        cfg.fov = f;
    }
    if args.screenshot.is_some() || (args.bench > 0.0 && !args.window) {
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
        next_frame: now,
        console: None,
        aliases: BTreeMap::new(),
        timescale: 1.0,
        quit: false,
        vm_fov: VIEWMODEL_FOV,
        rui: None,
        rui_r: None,
        rui_prefetched: String::new(),
        inv_was_open: false,
        winbench: None,
    };
    if app.args.bench > 0.0 && app.args.window {
        app.winbench = Some(bench::WinBench::new(app.args.bench));
    }
    if let Err(e) = el.run_app(&mut app) {
        eprintln!("event loop: {e}");
    }
}
