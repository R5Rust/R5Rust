//! Console commands and cvars (the console UI is `console.rs`).
//!
//! Three kinds of names:
//! - commands (`CMDS`): `noclip`, `setpos`, `bind`, `exec`, ...
//! - client cvars (`VARS`): settings and debug knobs; the ones the settings menu also has save to
//!   `r5rust_settings.toml` like the menu does
//! - movement cvars: every field of the sim's `Cvars` (Apex's own names and defaults, `sv_gravity`,
//!   `jump_graceperiodmax`, ...), live on the player; `reset <name>` restores the game's default. They last until
//!   the game closes (put them in `cfg/autoexec.cfg` to keep them).

use crate::binds::{Action, UserSettings};
use crate::console::{self, Kind};
use crate::App;
use glam::Vec3;

pub struct Cmd {
    pub name: &'static str,
    pub help: &'static str,
    run: fn(&mut App, &[String]),
}

pub struct Var {
    pub name: &'static str,
    pub help: &'static str,
    get: fn(&App) -> Option<String>,
    set: fn(&mut App, &str) -> Result<(), String>,
    default: &'static str,
}

fn out(t: impl AsRef<str>) {
    console::print(Kind::Info, t);
}
fn err(t: impl AsRef<str>) {
    console::print(Kind::Error, t);
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "0" | "false" | "off" | "no" => Ok(false),
        _ => Err(format!("want 0 or 1, got {v:?}")),
    }
}
fn parse_num<T: std::str::FromStr>(v: &str) -> Result<T, String> {
    v.trim().parse().map_err(|_| format!("not a number: {v:?}"))
}
fn b(v: bool) -> String {
    (v as u8).to_string()
}

pub const CMDS: &[Cmd] = &[
    Cmd { name: "help", help: "list commands, or explain one: help <name>", run: cmd_help },
    Cmd { name: "find", help: "search command and cvar names + help: find <text>", run: cmd_find },
    Cmd { name: "cmdlist", help: "every command", run: |_, _| list_cmds("") },
    Cmd { name: "cvarlist", help: "every cvar with its value (* = changed): cvarlist [prefix]", run: cmd_cvarlist },
    Cmd { name: "clear", help: "clear the console", run: |_, _| console::clear() },
    Cmd { name: "echo", help: "print text", run: |_, a| out(a[1..].join(" ")) },
    Cmd { name: "condump", help: "write the console to condump.txt", run: cmd_condump },
    Cmd { name: "exec", help: "run cfg/<name>.cfg (cfg/autoexec.cfg runs by itself after the map loads)", run: cmd_exec },
    Cmd { name: "alias", help: "alias <name> \"<commands>\"; alias alone lists them", run: cmd_alias },
    Cmd { name: "toggle", help: "flip a cvar: toggle <cvar> [v1 v2 ...] (default 0/1)", run: cmd_toggle },
    Cmd { name: "reset", help: "set a cvar back to its default: reset <cvar>", run: cmd_reset },
    Cmd { name: "bind", help: "bind <key> \"<commands>\" (key as the Controls tab names it: F5, MOUSE4, G); bind <key> shows it", run: cmd_bind },
    Cmd { name: "unbind", help: "unbind <key>", run: cmd_unbind },
    Cmd { name: "unbindall", help: "remove every console bind (the Controls tab binds stay)", run: cmd_unbindall },
    Cmd { name: "binds", help: "list the console binds and the Controls tab binds", run: cmd_binds },
    Cmd { name: "noclip", help: "fly through everything (toggle)", run: cmd_noclip },
    Cmd { name: "fly", help: "fly with collision (toggle, same as the Fly key)", run: |a, _| a.do_action(Action::FlyToggle) },
    Cmd { name: "setpos", help: "teleport (feet): setpos x y z", run: cmd_setpos },
    Cmd { name: "setang", help: "look: setang pitch yaw", run: cmd_setang },
    Cmd { name: "getpos", help: "print setpos/setang for where you are, and copy it", run: cmd_getpos },
    Cmd { name: "respawn", help: "back to the spawn point", run: |a, _| a.do_action(Action::Respawn) },
    Cmd { name: "kill", help: "same as respawn", run: |a, _| a.do_action(Action::Respawn) },
    Cmd { name: "mark", help: "log this spot to r5rust_marks.txt (same as F8)", run: |a, _| a.do_action(Action::Mark) },
    Cmd { name: "fullscreen", help: "toggle borderless fullscreen", run: |a, _| a.do_action(Action::Fullscreen) },
    Cmd { name: "attach", help: "attach <barrel|mag|optic|stock> [tier 1-4|c] [name: laser-sight, light-mag, hcog-classic, holo, standard-stock ...]", run: cmd_attach },
    Cmd { name: "detach", help: "detach <barrel|mag|optic|stock|all>", run: cmd_detach },
    Cmd { name: "weapon_slot", help: "switch slots at once (no holster / draw): weapon_slot 1 (gun) | 3 (melee / heirloom)", run: cmd_weapon_slot },
    Cmd { name: "weapon_probe", help: "simulate the gun at 240 Hz: weapon_probe ads|jump (logs a bone's camera-space path + the animator state each frame)", run: cmd_weapon_probe },
    Cmd { name: "sim_hold", help: "move the player holding keys for N ticks (screenshot tests): sim_hold w,space 20", run: |a, args| {
        let keys: std::collections::HashSet<String> = args.get(1).map(|k| k.split(',').map(|s| s.trim().to_ascii_lowercase()).collect()).unwrap_or_default();
        let n: u32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(20);
        let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
        // optional: stop once the player has been in a traversal for this many ticks
        let stop_in: Option<u32> = args.get(3).and_then(|v| v.parse().ok());
        let mut in_trav = 0u32;
        for i in 0..n {
            g.prev_eye = g.player.eye();
            let inp = crate::input_from(&keys, i == 0 && keys.contains("space"), g.yaw, g.pitch);
            shared::sim::simulate(&mut g.player, &inp, crate::TICK, &g.world.collision);
            if g.player.mantling() {
                in_trav += 1;
                if stop_in.is_some_and(|m| in_trav >= m) {
                    break;
                }
            }
        }
        g.sim_frozen = true;
        log::info!("sim_hold: {} at ({:.0}, {:.0}, {:.0}) traversal state {} cycle {:.2} ({in_trav} ticks traversing)", g.player.state_name(), g.player.pos.x, g.player.pos.y, g.player.pos.z, g.player.traversal.state, g.player.traversal.cycle);
        out(format!("{} after {n} ticks at ({:.0}, {:.0}, {:.0}) traversal state {} cycle {:.2}", g.player.state_name(), g.player.pos.x, g.player.pos.y, g.player.pos.z, g.player.traversal.state, g.player.traversal.cycle));
    } },
    Cmd { name: "mantle_pose", help: "show the mantle hands at a traversal state + cycle: mantle_pose <3 below|4 level|5 above|6 high|11 hang|12 jumpoff> [cycle 0..1], or mantle_pose off", run: |a, args| {
        let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
        if args.get(1).map(String::as_str) == Some("off") { g.proxy_debug = None; return; }
        let s: i32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(4);
        let c: f32 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0.5);
        g.proxy_debug = Some((s, c));
    } },
    Cmd { name: "anim_probe", help: "simulate the melee item at 240 Hz through a scenario and log frames where a bone jumps: anim_probe [sprint|deploy|deploysprint] [threshold units, default 1.5] [from seconds]", run: cmd_anim_probe },
    Cmd { name: "melee_pose", help: "jump the melee item to a pose (and its slot in hand): idle, draw, drawfirst, holster, inspect, attack, raise, sprint, or a sequence name [t seconds]", run: cmd_melee_pose },
    Cmd { name: "heirloom", help: "pick the melee slot's heirloom: heirloom wraith_kunai | octane_knife | bat | none", run: cmd_heirloom },
    Cmd { name: "weapon_pose", help: "jump the first-person weapon to a pose: hip, ads, draw, drawfirst, holster, reload, reload_empty, inspect, fire, sprint, sprintraise, lower, raise, or a sequence name [t seconds]", run: cmd_weapon_pose },
    Cmd { name: "give", help: "pick up an item as if it were on the ground in front of you: give <loot ref> [count] (give mp_weapon_r97, give bullet 60, give bullets_mag_l3)", run: cmd_give },
    Cmd { name: "inventory", help: "print the inventory (backpack, weapons, gear); inventory open|close shows / hides the TAB screen", run: cmd_inventory },
    Cmd { name: "use", help: "press use on what the crosshair is on (E): use [hold]", run: cmd_use },
    Cmd { name: "loot_near", help: "the ground loot around you, nearest first: loot_near [radius 300]", run: cmd_loot_near },
    Cmd { name: "infinite_ammo", help: "Firing Range infinite ammo: infinite_ammo 0|1 (no arg = toggle)", run: cmd_infinite_ammo },
    Cmd { name: "weapon_fire", help: "hold fire for N ticks (60/s) from where you stand, then let go for S ticks: weapon_fire [ticks 18] [settle 30]", run: cmd_weapon_fire },
    Cmd { name: "status", help: "map, GPU, resolution, frame rate", run: cmd_status },
    Cmd { name: "version", help: "build info", run: |_, _| out(format!("R5Rust {} ({} build)", env!("CARGO_PKG_VERSION"), if cfg!(debug_assertions) { "debug" } else { "release" })) },
    Cmd { name: "quit", help: "close the game", run: |a, _| a.quit = true },
    Cmd { name: "exit", help: "close the game", run: |a, _| a.quit = true },
];

fn settings(a: &App) -> Option<&UserSettings> {
    a.menu.as_ref().map(|m| &m.settings)
}
/// Edits the menu's settings and saves them, like closing the menu does.
fn with_settings(a: &mut App, f: impl FnOnce(&mut UserSettings) -> Result<(), String>) -> Result<(), String> {
    let m = a.menu.as_mut().ok_or("no settings yet")?;
    f(&mut m.settings)?;
    m.settings.graphics.preset = m.settings.graphics.detect_preset();
    m.settings.save();
    Ok(())
}

pub const VARS: &[Var] = &[
    Var {
        name: "sensitivity",
        help: "mouse sensitivity, Apex units (0.022 deg/count)",
        default: "1.6",
        get: |a| settings(a).map(|s| s.sensitivity.to_string()),
        set: |a, v| {
            let x: f32 = parse_num(v)?;
            with_settings(a, |s| Ok(s.sensitivity = x.clamp(0.01, 100.0)))
        },
    },
    Var {
        name: "fov",
        help: "horizontal field of view at 4:3, 70-120 like Apex",
        default: "100",
        get: |a| settings(a).map(|s| s.fov.to_string()),
        set: |a, v| {
            let x: f32 = parse_num(v)?;
            with_settings(a, |s| Ok(s.fov = x.clamp(70.0, 120.0)))
        },
    },
    Var {
        name: "m_invert",
        help: "invert mouse look up/down",
        default: "0",
        get: |a| settings(a).map(|s| b(s.invert_y)),
        set: |a, v| {
            let x = parse_bool(v)?;
            with_settings(a, |s| Ok(s.invert_y = x))
        },
    },
    Var {
        name: "cl_autosprint",
        help: "pushing forward sprints without the sprint key (Apex's Auto Sprint)",
        default: "1",
        get: |a| settings(a).map(|s| b(s.auto_sprint)),
        set: |a, v| {
            let x = parse_bool(v)?;
            with_settings(a, |s| Ok(s.auto_sprint = x))
        },
    },
    Var {
        name: "cl_toggle_ads",
        help: "the aim key toggles aiming instead of holding (Apex's ADS Toggle, Gameplay tab)",
        default: "0",
        get: |a| settings(a).map(|s| b(s.toggle_ads)),
        set: |a, v| {
            let x = parse_bool(v)?;
            with_settings(a, |s| Ok(s.toggle_ads = x))
        },
    },
    Var {
        name: "mantle_boost_enabled",
        help: "Apex's mantle boost (Gameplay tab)",
        default: "1",
        get: |a| settings(a).map(|s| b(s.mantle_boost)),
        set: |a, v| {
            let x = parse_bool(v)?;
            with_settings(a, |s| Ok(s.mantle_boost = x))
        },
    },
    Var {
        name: "fps_max",
        help: "frame rate cap, 0 = unlimited",
        default: "0",
        get: |a| settings(a).map(|s| s.graphics.fps_cap.to_string()),
        set: |a, v| {
            let x: u32 = parse_num(v)?;
            with_settings(a, |s| Ok(s.graphics.fps_cap = if x == 0 { 0 } else { x.clamp(20, 1000) }))
        },
    },
    Var {
        name: "mat_vsync",
        help: "wait for the display's refresh",
        default: "0",
        get: |a| settings(a).map(|s| b(s.graphics.vsync)),
        set: |a, v| {
            let x = parse_bool(v)?;
            with_settings(a, |s| Ok(s.graphics.vsync = x))
        },
    },
    Var {
        name: "mat_antialias",
        help: "MSAA samples: 1 (off) or 4",
        default: "4",
        get: |a| settings(a).map(|s| s.graphics.msaa.to_string()),
        set: |a, v| {
            let x: u32 = parse_num(v)?;
            with_settings(a, |s| Ok(s.graphics.msaa = if x >= 4 { 4 } else { 1 }))
        },
    },
    Var {
        name: "mat_anisotropic",
        help: "texture filtering: 1 (off), 2, 4, 8, 16",
        default: "16",
        get: |a| settings(a).map(|s| s.graphics.anisotropic.to_string()),
        set: |a, v| {
            let x: u16 = parse_num(v)?;
            let x = [1, 2, 4, 8, 16].into_iter().rev().find(|&n| n <= x.max(1)).unwrap_or(1);
            with_settings(a, |s| Ok(s.graphics.anisotropic = x as _))
        },
    },
    Var {
        name: "r_renderscale",
        help: "3D resolution in percent of the window, 50-100",
        default: "100",
        get: |a| settings(a).map(|s| s.graphics.render_scale.to_string()),
        set: |a, v| {
            let x: u32 = parse_num(v)?;
            with_settings(a, |s| Ok(s.graphics.render_scale = x.clamp(50, 100) as _))
        },
    },
    Var {
        name: "r_stretch",
        help: "stretched resolution like Apex's: r_stretch 1300x1070 renders at that size and stretches it to the window; 0 = off",
        default: "0",
        get: |a| settings(a).map(|s| if s.graphics.stretch[0] == 0 { "0".to_string() } else { format!("{} {}", s.graphics.stretch[0], s.graphics.stretch[1]) }),
        set: |a, v| {
            let n: Vec<u32> = v.split(|c: char| c == ' ' || c == 'x' || c == ',').filter_map(|t| t.trim().parse().ok()).collect();
            let st = match n.as_slice() {
                [0] | [] => [0, 0],
                [w, h] if *w >= 320 && *h >= 240 => [(*w).min(7680), (*h).min(4320)],
                _ => return Err("r_stretch <width>x<height>, or 0 for off".into()),
            };
            with_settings(a, |s| Ok(s.graphics.stretch = st))
        },
    },
    Var {
        name: "cl_showfps",
        help: "frame rate overlay: 0 off, 1 fps, 2 fps + frame time graph",
        default: "0",
        get: |a| settings(a).map(|s| s.show_fps.to_string()),
        set: |a, v| {
            let x: u8 = parse_num(v)?;
            with_settings(a, |s| Ok(s.show_fps = x.min(2)))
        },
    },
    Var {
        name: "cl_showpos",
        help: "position / angles / velocity overlay",
        default: "0",
        get: |a| settings(a).map(|s| b(s.show_pos)),
        set: |a, v| {
            let x = parse_bool(v)?;
            with_settings(a, |s| Ok(s.show_pos = x))
        },
    },
    Var {
        name: "host_timescale",
        help: "game speed: 0.5 = half speed (the sim stays 60 ticks per game second)",
        default: "1",
        get: |a| Some(a.timescale.to_string()),
        set: |a, v| {
            a.timescale = parse_num::<f32>(v)?.clamp(0.05, 10.0);
            Ok(())
        },
    },
    Var {
        name: "volume",
        help: "master volume 0..1 (the weapon's sounds)",
        default: "1",
        get: |a| a.game.as_ref().and_then(|g| g.sfx.as_ref()).map(|s| s.volume.to_string()),
        set: |a, v| {
            let x = parse_num::<f32>(v)?;
            match a.game.as_mut().and_then(|g| g.sfx.as_mut()) {
                Some(s) => {
                    s.set_volume(x);
                    Ok(())
                }
                None => Err("no sound (no audio device, or run tools/build_sounds.py)".into()),
            }
        },
    },
    Var {
        name: "mat_debug",
        help: "debug view: 0 off, 1 sun visibility, 2 baked indirect, 3 albedo, 11 material colours, 12 distance",
        default: "0",
        get: |a| Some(a.args.debug.to_string()),
        set: |a, v| {
            a.args.debug = parse_num(v)?;
            Ok(())
        },
    },
    Var {
        name: "r_lod",
        help: "static prop LOD: -1 auto (by distance), 0 = full detail, 1, 2, ... forces that LOD",
        default: "-1",
        get: |a| a.gfx.as_ref().map(|g| g.lod.force.to_string()),
        set: |a, v| {
            let x: i32 = parse_num(v)?;
            a.gfx.as_mut().ok_or("no renderer")?.lod.force = x.max(-1);
            Ok(())
        },
    },
    Var {
        name: "r_lod_switch_scale",
        help: "Apex's LOD distance scale (Model Detail sets it: High 1, Medium 0.8, Low 0.6); -1 = follow Model Detail",
        default: "-1",
        get: |a| a.gfx.as_ref().map(|g| g.lod.switch_scale.map_or("-1".into(), |s| s.to_string())),
        set: |a, v| {
            let x: f32 = parse_num(v)?;
            a.gfx.as_mut().ok_or("no renderer")?.lod.switch_scale = (x > 0.0).then_some(x.min(100.0));
            Ok(())
        },
    },
    Var {
        name: "mat_set_dress_level",
        help: "Apex's set-dress level: static props whose model tier (0 always, 1 grass, 2 small rocks / conduits / signs) is above it are not drawn; 1 = what retail shows",
        default: "1",
        get: |a| a.gfx.as_ref().map(|g| g.lod.dress_level.to_string()),
        set: |a, v| {
            let x: f32 = parse_num(v)?;
            a.gfx.as_mut().ok_or("no renderer")?.lod.dress_level = x.clamp(0.0, 255.0) as u8;
            Ok(())
        },
    },
    Var {
        name: "fadedistscale",
        help: "Apex's fadeDistScale: multiplies every static prop's fade distance (1 = retail, the default; 0 = never fade or size-cull)",
        default: "1",
        get: |a| a.gfx.as_ref().map(|g| g.lod.fade_scale.to_string()),
        set: |a, v| {
            let x: f32 = parse_num(v)?;
            a.gfx.as_mut().ok_or("no renderer")?.lod.fade_scale = x.clamp(0.0, 1000.0);
            Ok(())
        },
    },
    Var {
        name: "r_rtshadows",
        help: "EXPERIMENTAL hardware ray-traced sun shadows (RTX): one ray per pixel instead of the shadow cascades. Not retail's look; foliage casts none",
        default: "0",
        get: |a| a.gfx.as_ref().map(|g| b(g.rt_on)),
        set: |a, v| {
            let x = parse_bool(v)?;
            let world = a.game.as_ref().map(|g| &g.world);
            a.gfx.as_mut().ok_or("no renderer")?.set_rt(x, world)
        },
    },
    Var {
        name: "r_drawstaticprops",
        help: "draw static props (0 hides them: perf and debugging)",
        default: "1",
        get: |a| a.gfx.as_ref().map(|g| b(g.lod.draw_props)),
        set: |a, v| {
            let x = parse_bool(v)?;
            a.gfx.as_mut().ok_or("no renderer")?.lod.draw_props = x;
            Ok(())
        },
    },
    Var {
        name: "viewmodel_fov",
        help: "first-person weapon FOV (horizontal at 4:3), 0 = follow your FOV setting",
        default: "58",
        get: |a| Some(a.vm_fov.to_string()),
        set: |a, v| {
            a.vm_fov = parse_num::<f32>(v)?.clamp(0.0, 150.0);
            Ok(())
        },
    },
    Var {
        name: "fly_speed",
        help: "fly / noclip speed in units per second (sprint = 3x)",
        default: "1200",
        get: |a| Some(a.cfg.fly_speed.to_string()),
        set: |a, v| {
            a.cfg.fly_speed = parse_num::<f32>(v)?.max(1.0);
            if let Some(g) = &mut a.game {
                g.player.fly_speed = a.cfg.fly_speed;
            }
            Ok(())
        },
    },
];

// ---------------------------------------------------------------- movement cvars (sim Cvars, by serde)

fn movement_defaults() -> serde_json::Map<String, serde_json::Value> {
    match serde_json::to_value(shared::settings::CVARS) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => Default::default(),
    }
}

/// Fields the client sets every frame from the settings (changing them here would be undone).
const MOVEMENT_SKIP: &[&str] = &["mantle_boost_enabled"];

fn movement_get(a: &App, name: &str) -> Option<String> {
    let g = a.game.as_ref()?;
    let v = serde_json::to_value(g.player.cvars).ok()?;
    v.get(name).map(json_str)
}

fn json_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Bool(x) => b(*x),
        serde_json::Value::Number(n) => n.as_f64().map(|f| if f.fract() == 0.0 && f.abs() < 1e15 { format!("{}", f as i64) } else { format!("{}", f as f32) }).unwrap_or_else(|| n.to_string()),
        o => o.to_string(),
    }
}

fn movement_set(a: &mut App, name: &str, value: &str) -> Result<(), String> {
    let g = a.game.as_mut().ok_or("the map hasn't loaded yet")?;
    let mut v = serde_json::to_value(g.player.cvars).map_err(|e| e.to_string())?;
    let slot = v.get_mut(name).ok_or("unknown")?;
    *slot = match slot {
        serde_json::Value::Bool(_) => serde_json::Value::Bool(parse_bool(value)?),
        serde_json::Value::Number(n) if n.is_f64() => serde_json::json!(parse_num::<f64>(value)?),
        _ => serde_json::json!(parse_num::<f64>(value)? as i64),
    };
    g.player.cvars = serde_json::from_value(v).map_err(|e| e.to_string())?;
    Ok(())
}

/// Every name the console knows, with its one-line help (Tab completion).
pub fn names() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = CMDS.iter().map(|c| (c.name.to_string(), c.help.to_string())).collect();
    v.extend(VARS.iter().map(|c| (c.name.to_string(), c.help.to_string())));
    for (k, d) in movement_defaults() {
        if !MOVEMENT_SKIP.contains(&k.as_str()) {
            v.push((k, format!("movement cvar, default {}", json_str(&d))));
        }
    }
    v
}

// ---------------------------------------------------------------- running lines

const MAX_DEPTH: u32 = 8;

impl App {
    /// Runs one console line (`;`-separated commands).
    pub fn exec(&mut self, line: &str) {
        self.exec_depth(line, 0);
    }

    fn exec_depth(&mut self, line: &str, depth: u32) {
        if depth > MAX_DEPTH {
            err("too many nested exec/alias calls");
            return;
        }
        for cmd in console::split_commands(line) {
            let args = console::tokenize(&cmd);
            if args.is_empty() {
                continue;
            }
            self.run_args(&args, depth);
        }
    }

    fn run_args(&mut self, args: &[String], depth: u32) {
        let name = args[0].to_ascii_lowercase();
        if let Some(body) = self.aliases.get(&name).cloned() {
            return self.exec_depth(&body, depth + 1);
        }
        if name == "exec" {
            return self.exec_file(args.get(1).map(String::as_str).unwrap_or(""), depth);
        }
        if let Some(c) = CMDS.iter().find(|c| c.name == name) {
            return (c.run)(self, args);
        }
        if let Some(v) = VARS.iter().find(|v| v.name == name) {
            match args.get(1) {
                None => out(format!("{} = \"{}\"  (default \"{}\")  {}", v.name, (v.get)(self).unwrap_or_default(), v.default, v.help)),
                Some(val) => match (v.set)(self, val) {
                    Ok(()) => out(format!("{} = \"{}\"", v.name, (v.get)(self).unwrap_or_default())),
                    Err(e) => err(format!("{}: {e}", v.name)),
                },
            }
            return;
        }
        let defaults = movement_defaults();
        if let Some(d) = defaults.get(&name).filter(|_| !MOVEMENT_SKIP.contains(&name.as_str())) {
            match args.get(1) {
                None => match movement_get(self, &name) {
                    Some(cur) => out(format!("{name} = \"{cur}\"  (default \"{}\")  movement cvar", json_str(d))),
                    None => out(format!("{name}: default \"{}\" (the map hasn't loaded yet)", json_str(d))),
                },
                Some(val) => match movement_set(self, &name, val) {
                    Ok(()) => out(format!("{name} = \"{}\"", movement_get(self, &name).unwrap_or_default())),
                    Err(e) => err(format!("{name}: {e}")),
                },
            }
            return;
        }
        err(format!("Unknown command \"{}\" (try help or find)", args[0]));
    }

    fn exec_file(&mut self, name: &str, depth: u32) {
        if name.is_empty() {
            return err("exec <name>: runs cfg/<name>.cfg");
        }
        let file = if name.ends_with(".cfg") { name.to_string() } else { format!("{name}.cfg") };
        let mut dirs = vec![std::path::PathBuf::from("cfg")];
        if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("cfg"))) {
            dirs.push(d);
        }
        let Some(path) = dirs.iter().map(|d| d.join(&file)).find(|p| p.is_file()) else {
            return err(format!("exec: couldn't find cfg/{file}"));
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                console::print(Kind::Good, format!("exec {}", path.display()));
                for line in text.lines() {
                    self.exec_depth(line, depth + 1);
                }
            }
            Err(e) => err(format!("exec {}: {e}", path.display())),
        }
    }

    /// Runs whatever the console collected this frame.
    pub fn run_console(&mut self) {
        let lines = self.console.as_mut().map(|c| std::mem::take(&mut c.submitted)).unwrap_or_default();
        for l in lines {
            self.exec(&l);
        }
    }
}

// ---------------------------------------------------------------- commands

fn cmd_help(_: &mut App, a: &[String]) {
    if let Some(n) = a.get(1) {
        let n = n.to_ascii_lowercase();
        match names().into_iter().find(|(k, _)| *k == n) {
            Some((k, h)) => out(format!("{k}: {h}")),
            None => err(format!("no command or cvar called {n}")),
        }
        return;
    }
    out("Type a command, or a cvar name to see it (add a value to set it). ; chains commands, quotes group words.");
    out("Tab completes, Up/Down = history, PgUp/PgDn scroll. cvarlist lists every cvar, find <text> searches.");
    list_cmds("");
}

fn list_cmds(filter: &str) {
    for c in CMDS.iter().filter(|c| c.name.contains(filter)) {
        out(format!("  {:<12} {}", c.name, c.help));
    }
}

fn cmd_find(_: &mut App, a: &[String]) {
    let Some(q) = a.get(1).map(|s| s.to_ascii_lowercase()) else { return err("find <text>") };
    let hits: Vec<_> = names().into_iter().filter(|(n, h)| n.contains(&q) || h.to_ascii_lowercase().contains(&q)).collect();
    if hits.is_empty() {
        out(format!("nothing matches {q:?}"));
    }
    for (n, h) in hits {
        out(format!("  {n:<34} {h}"));
    }
}

fn cmd_cvarlist(a: &mut App, args: &[String]) {
    let p = args.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let mut n = 0;
    for v in VARS.iter().filter(|v| v.name.starts_with(&p)) {
        let cur = (v.get)(a).unwrap_or_default();
        let changed = cur.parse::<f64>().ok().zip(v.default.parse::<f64>().ok()).map_or(cur != v.default, |(x, y)| x != y);
        out(format!("{} {:<34} {:<10} {}", if changed { "*" } else { " " }, v.name, cur, v.help));
        n += 1;
    }
    for (k, d) in movement_defaults().into_iter().filter(|(k, _)| k.starts_with(&p) && !MOVEMENT_SKIP.contains(&k.as_str())) {
        let cur = movement_get(a, &k).unwrap_or_else(|| json_str(&d));
        out(format!("{} {:<34} {:<10} movement", if cur != json_str(&d) { "*" } else { " " }, k, cur));
        n += 1;
    }
    out(format!("{n} cvars"));
}

fn cmd_condump(_: &mut App, _: &[String]) {
    match std::fs::write("condump.txt", console::dump()) {
        Ok(()) => console::print(Kind::Good, "console -> condump.txt"),
        Err(e) => err(format!("condump: {e}")),
    }
}

fn cmd_exec(a: &mut App, args: &[String]) {
    a.exec_file(args.get(1).map(String::as_str).unwrap_or(""), 0);
}

fn cmd_alias(a: &mut App, args: &[String]) {
    match (args.get(1), args.get(2)) {
        (None, _) => {
            if a.aliases.is_empty() {
                out("no aliases");
            }
            for (k, v) in &a.aliases {
                out(format!("  {k} = \"{v}\""));
            }
        }
        (Some(n), None) => out(a.aliases.get(&n.to_ascii_lowercase()).map(|v| format!("{n} = \"{v}\"")).unwrap_or_else(|| format!("no alias {n}"))),
        (Some(n), Some(_)) => {
            let n = n.to_ascii_lowercase();
            if CMDS.iter().any(|c| c.name == n) || VARS.iter().any(|v| v.name == n) {
                return err(format!("{n} is already a command or cvar"));
            }
            a.aliases.insert(n, args[2..].join(" "));
        }
    }
}

fn cvar_value(a: &App, name: &str) -> Option<String> {
    VARS.iter().find(|v| v.name == name).and_then(|v| (v.get)(a)).or_else(|| movement_get(a, name))
}

fn cmd_toggle(a: &mut App, args: &[String]) {
    let Some(n) = args.get(1).map(|s| s.to_ascii_lowercase()) else { return err("toggle <cvar> [values...]") };
    let vals: Vec<String> = if args.len() > 2 { args[2..].to_vec() } else { vec!["0".into(), "1".into()] };
    let Some(cur) = cvar_value(a, &n) else { return err(format!("toggle: no cvar {n}")) };
    let same = |x: &str| x == cur || x.parse::<f64>().ok().zip(cur.parse::<f64>().ok()).is_some_and(|(p, q)| p == q);
    let next = vals.iter().position(|v| same(v)).map_or(0, |i| (i + 1) % vals.len());
    a.run_args(&[n, vals[next].clone()], 0);
}

fn cmd_reset(a: &mut App, args: &[String]) {
    let Some(n) = args.get(1).map(|s| s.to_ascii_lowercase()) else { return err("reset <cvar>") };
    if let Some(v) = VARS.iter().find(|v| v.name == n) {
        return a.run_args(&[n, v.default.to_string()], 0);
    }
    match movement_defaults().get(&n) {
        Some(d) => a.run_args(&[n.clone(), json_str(d)], 0),
        None => err(format!("reset: no cvar {n}")),
    }
}

fn cmd_bind(a: &mut App, args: &[String]) {
    let Some(key) = args.get(1).map(|k| k.to_ascii_uppercase()) else { return err("bind <key> \"<commands>\"") };
    if args.len() == 2 {
        let cur = settings(a).and_then(|s| s.console_binds.get(&key).cloned());
        return out(match cur {
            Some(c) => format!("{key} = \"{c}\""),
            None => format!("{key} is not bound in the console"),
        });
    }
    let cmd = args[2..].join(" ");
    let _ = with_settings(a, |s| Ok(drop(s.console_binds.insert(key.clone(), cmd.clone()))));
    out(format!("{key} = \"{cmd}\""));
}

fn cmd_unbind(a: &mut App, args: &[String]) {
    let Some(key) = args.get(1).map(|k| k.to_ascii_uppercase()) else { return err("unbind <key>") };
    let mut had = false;
    let _ = with_settings(a, |s| Ok(had = s.console_binds.remove(&key).is_some()));
    out(if had { format!("unbound {key}") } else { format!("{key} had no console bind (Controls tab binds are edited in Esc > Controls)") });
}

fn cmd_unbindall(a: &mut App, _: &[String]) {
    let _ = with_settings(a, |s| Ok(s.console_binds.clear()));
    out("console binds cleared");
}

fn cmd_binds(a: &mut App, _: &[String]) {
    let Some(s) = settings(a) else { return };
    out("console binds:");
    if s.console_binds.is_empty() {
        out("  (none)");
    }
    for (k, v) in &s.console_binds {
        out(format!("  {k:<12} \"{v}\""));
    }
    out("Controls tab:");
    for act in Action::ALL {
        let keys: Vec<String> = s.binds.get(act).iter().flatten().map(|b| b.label()).collect();
        out(format!("  {:<24} {}", act.label(), if keys.is_empty() { "-".into() } else { keys.join(", ") }));
    }
}

fn cmd_noclip(a: &mut App, _: &[String]) {
    let Some(g) = &mut a.game else { return err("the map hasn't loaded yet") };
    let on = !(g.player.flying && g.player.noclip);
    g.player.flying = on;
    g.player.noclip = on;
    g.player.vel = Vec3::ZERO;
    out(if on { "noclip ON" } else { "noclip OFF" });
}

fn nums(args: &[String], n: usize) -> Option<Vec<f32>> {
    let v: Vec<f32> = args.iter().skip(1).take(n).filter_map(|s| s.parse().ok()).collect();
    (v.len() == n).then_some(v)
}

fn cmd_setpos(a: &mut App, args: &[String]) {
    let Some(v) = nums(args, 3) else { return err("setpos x y z") };
    let Some(g) = &mut a.game else { return err("the map hasn't loaded yet") };
    g.player.pos = Vec3::new(v[0], v[1], v[2]);
    g.player.vel = Vec3::ZERO;
    g.prev_eye = g.player.eye();
}

fn cmd_setang(a: &mut App, args: &[String]) {
    let Some(v) = nums(args, 2) else { return err("setang pitch yaw") };
    let Some(g) = &mut a.game else { return err("the map hasn't loaded yet") };
    g.pitch = v[0].clamp(-89.0, 89.0);
    g.yaw = v[1].rem_euclid(360.0);
}

fn cmd_getpos(a: &mut App, _: &[String]) {
    let Some(g) = &a.game else { return err("the map hasn't loaded yet") };
    let p = g.player.pos;
    let line = format!("setpos {:.2} {:.2} {:.2}; setang {:.2} {:.2}", p.x, p.y, p.z, g.pitch, g.yaw);
    out(&line);
    if let Some(c) = &mut a.console {
        c.clipboard = Some(line);
    }
}

/// default attachment per point (the R-99's: kit data.js ids)
fn default_attachment(point: &str) -> Option<&'static str> {
    Some(match point {
        "barrel" => "laser-sight",
        "mag" => "light-mag",
        "sight" | "optic" => "hcog-classic",
        "grip" | "stock" => "standard-stock",
        _ => return None,
    })
}

fn cmd_attach(a: &mut App, args: &[String]) {
    let Some(w) = a.game.as_mut().and_then(|g| g.weapon.as_mut()) else { return err("no weapon") };
    let Some(point) = args.get(1).map(|s| s.to_ascii_lowercase()) else { return err("attach <barrel|mag|optic|stock> [tier] [name]") };
    let Some(def) = default_attachment(&point) else { return err(format!("no attachment point {point}")) };
    let tier = match args.get(2).map(|s| s.to_ascii_lowercase()) {
        Some(t) if t == "c" => 5,
        Some(t) => t.parse::<u8>().unwrap_or(3).clamp(1, 5),
        None => 3,
    };
    let id = args.get(3).cloned().unwrap_or_else(|| def.to_string());
    match w.attach(&point, Some(crate::weapon::Attachment { id: id.clone(), tier })) {
        Ok(()) => out(format!("{point}: {id} tier {tier}, mag {} / {}", w.ammo, w.clip())),
        Err(e) => err(e),
    }
}

fn cmd_detach(a: &mut App, args: &[String]) {
    let Some(w) = a.game.as_mut().and_then(|g| g.weapon.as_mut()) else { return err("no weapon") };
    let point = args.get(1).map_or("all".to_string(), |s| s.to_ascii_lowercase());
    let points: Vec<&str> = if point == "all" { vec!["barrel", "mag", "sight", "grip"] } else { vec![point.as_str()] };
    for p in points {
        if let Err(e) = w.attach(p, None) {
            return err(e);
        }
    }
    out(format!("detached {point}, mag {} / {}", w.ammo, w.clip()));
}

fn cmd_weapon_slot(a: &mut App, args: &[String]) {
    let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
    let melee = match args.get(1).map(String::as_str) {
        Some("1") => false,
        Some("3") => true,
        _ => return err("weapon_slot 1 | 3"),
    };
    g.loadout.pending = None;
    g.loadout.quick = false;
    if melee {
        g.loadout.slot = crate::loadout::Slot::Melee;
        if let (Some(m), Some(vm)) = (g.melee.as_mut(), a.gfx.as_ref().and_then(|x| x.melee_vm.as_ref())) {
            m.debug_pose(vm, "idle", 0.0);
        }
    } else {
        g.loadout.slot = crate::loadout::Slot::Gun;
        if let (Some(w), Some(vm)) = (g.weapon.as_mut(), a.gfx.as_ref().and_then(|x| x.viewmodel.as_ref())) {
            w.debug_pose(vm, "hip", 0.0);
        }
    }
}

fn cmd_melee_pose(a: &mut App, args: &[String]) {
    let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
    let Some(m) = g.melee.as_mut() else { return err("no melee item (run tools/build_melee.py)") };
    let Some(vm) = a.gfx.as_ref().and_then(|x| x.melee_vm.as_ref()) else { return err("no melee viewmodel") };
    let t = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    m.debug_pose(vm, args.get(1).map_or("idle", String::as_str), t);
    g.loadout.slot = crate::loadout::Slot::Melee;
    g.loadout.pending = None;
    g.loadout.quick = false;
}

fn cmd_weapon_probe(a: &mut App, args: &[String]) {
    use crate::weapon::WeaponInput;
    let scen = args.get(1).map_or("ads", String::as_str).to_string();
    let bone_name = args.get(2).cloned().unwrap_or_else(|| "ja_c_propGun".into());
    let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
    let Some(w) = g.weapon.as_mut() else { return err("no weapon") };
    let Some(vm) = a.gfx.as_ref().and_then(|x| x.viewmodel.as_ref()) else { return err("no viewmodel") };
    let Some(bone) = vm.bone_index(&bone_name) else { return err(format!("no bone {bone_name}")) };
    let base = WeaponInput { on_ground: true, ..Default::default() };
    let steps: Vec<(f32, WeaponInput)> = match scen.as_str() {
        "jump" => vec![(1.0, base), (0.5, WeaponInput { on_ground: false, ..base }), (1.0, base)],
        "spray" => vec![(1.0, base), (1.2, WeaponInput { fire: true, ..base }), (2.0, base)],
        "spray_ads" => vec![(1.0, WeaponInput { ads: true, ..base }), (1.2, WeaponInput { fire: true, ads: true, ..base }), (2.0, WeaponInput { ads: true, ..base })],
        _ => vec![(1.0, base), (0.6, WeaponInput { ads: true, ..base }), (0.6, base)],
    };
    w.debug_pose(vm, "hip", 0.0);
    w.end_debug();
    let (dt, tick) = (1.0 / 240.0, 1.0 / 60.0);
    let mut acc = 0.0;
    let mut pose = Vec::new();
    let mut t = 0.0f32;
    let eye = g.player.eye();
    for (secs, inp) in steps {
        for _ in 0..(secs / dt) as usize {
            acc += dt;
            while acc >= tick {
                w.tick(tick, inp, eye, g.yaw, g.pitch, &g.world.collision);
                acc -= tick;
            }
            w.pose(vm, dt, &mut pose);
            let p = vm.attachments(&pose, glam::Mat3A::IDENTITY, glam::Affine3A::IDENTITY, &[bone])[0].0;
            if t >= 0.9 {
                log::info!("weapon_probe t {t:.4} ads {:.3} punch {:.2} {:.2} {:.2} pos {:.3} {:.3} {:.3} | {}", w.ads, w.punch.ang.x, w.punch.ang.y, w.punch.ang.z, p.x, p.y, p.z, w.anim.describe(vm));
            }
            t += dt;
        }
    }
    out(format!("weapon_probe {scen}: logged {bone_name} (r5rust log)"));
}

fn cmd_anim_probe(a: &mut App, args: &[String]) {
    use crate::melee::MeleeInput;
    let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
    let Some(m) = g.melee.as_mut() else { return err("no melee item") };
    let Some(vm) = a.gfx.as_ref().and_then(|x| x.melee_vm.as_ref()) else { return err("no melee viewmodel") };
    let scen = args.get(1).map_or("sprint", String::as_str);
    let thr: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.5);
    let from: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let idle = MeleeInput { attack: false, inspect: false, sprinting: false, crouched: false, air: false, sliding: false, lowered: false, speed: 0.0 };
    let run = MeleeInput { speed: 190.0, ..idle };
    let sprint = MeleeInput { sprinting: true, speed: 300.0, ..idle };
    // (seconds, input)
    let steps: Vec<(f32, MeleeInput)> = match scen {
        "deploy" => vec![(3.0, idle), (1.0, run)],
        "deploysprint" => vec![(3.0, sprint), (1.0, idle)],
        _ => vec![(1.0, idle), (0.6, run), (2.5, sprint), (1.5, idle), (2.5, sprint), (0.4, run), (1.0, idle)],
    };
    if scen.starts_with("deploy") {
        m.deploy(vm);
    }
    let dt = 1.0 / 240.0;
    let tick = 1.0 / 60.0;
    let mut acc = 0.0;
    let mut pose = Vec::new();
    let all: Vec<usize> = (0..vm.bone_count()).collect();
    let mut prev: Option<Vec<glam::Vec3>> = None;
    let mut t = 0.0f32;
    let mut lines = Vec::new();
    for (secs, inp) in steps {
        let n = (secs / dt) as usize;
        for _ in 0..n {
            acc += dt;
            while acc >= tick {
                m.tick(tick, inp, vm);
                acc -= tick;
            }
            m.pose(vm, dt, &mut pose);
            let pts: Vec<glam::Vec3> = vm.attachments(&pose, glam::Mat3A::IDENTITY, glam::Affine3A::IDENTITY, &all).into_iter().map(|p| p.0).collect();
            if let Some(pv) = &prev {
                let (bi, d) = pts.iter().zip(pv).map(|(a, b)| a.distance(*b)).enumerate().fold((0, 0.0f32), |acc, (i, d)| if d > acc.1 { (i, d) } else { acc });
                if d > thr && t >= from && lines.len() < 60 {
                    lines.push(format!("t {t:6.3} jump {d:5.2} bone {} | {}", vm.bone_name(bi), m.anim.describe(vm)));
                }
            }
            prev = Some(pts);
            t += dt;
        }
    }
    if lines.is_empty() {
        out(format!("anim_probe {scen}: no bone moved more than {thr} units in one 240 Hz frame"));
    }
    for l in lines {
        log::info!("anim_probe {l}");
        out(l);
    }
}

fn cmd_heirloom(a: &mut App, args: &[String]) {
    let id = match args.get(1).map(String::as_str) {
        None => return out(format!("heirloom = {:?}", a.menu.as_ref().map(|m| m.settings.heirloom.clone()).unwrap_or_default())),
        Some("none") | Some("") => "",
        Some(x) => x,
    };
    if !crate::HEIRLOOMS.iter().any(|(i, _)| *i == id) {
        return err(format!("unknown heirloom {id}: {}", crate::HEIRLOOMS.iter().map(|(i, _)| if i.is_empty() { "none" } else { i }).collect::<Vec<_>>().join(", ")));
    }
    let (Some(g), Some(gfx)) = (a.game.as_mut(), a.gfx.as_mut()) else { return err("the map hasn't loaded yet") };
    g.melee = crate::load_melee(gfx, &a.cfg, id);
    if let (Some(s), Some(m)) = (&mut g.sfx, &g.melee) {
        s.load_melee(&a.cfg.assets_root().join(crate::MELEE_DIR).join(&m.id));
    }
    if let Some(m) = a.menu.as_mut() {
        m.settings.heirloom = id.to_string();
    }
    out(format!("melee slot: {}", g.melee.as_ref().map_or("nothing (not baked)".to_string(), |m| m.id.clone())));
}

fn cmd_weapon_pose(a: &mut App, args: &[String]) {
    let Some(w) = a.game.as_mut().and_then(|g| g.weapon.as_mut()) else { return err("no weapon (run tools/build_weapon.py)") };
    let Some(vm) = a.gfx.as_ref().and_then(|x| x.viewmodel.as_ref()) else { return err("no weapon viewmodel") };
    let t = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    w.debug_pose(vm, args.get(1).map_or("hip", String::as_str), t);
}

fn cmd_give(a: &mut App, args: &[String]) {
    let Some(r) = args.get(1) else { return err("give <loot ref> [count]") };
    let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
    let Some(l) = g.loot.as_mut() else { return err(r"no loot data (install step 10: tools\install.ps1 -Only loot)") };
    let db = l.db.clone();
    let Some(id) = db.get(r) else { return err(format!("no loot item {r}")) };
    let it = db.item(id);
    let count = args.get(2).and_then(|c| c.parse().ok()).unwrap_or(it.per_drop);
    let weapon = (it.kind == shared::loot::Kind::MainWeapon).then(|| shared::loot::Weapon::new(&db, id));
    let view = shared::loot::inventory::GroundView { item: id, count, weapon: weapon.as_ref(), shield: None };
    let mut p = l.inv.pickup(&db, &view, shared::loot::UseKind::Tap);
    if p.needs_hold {
        p = l.inv.pickup(&db, &view, shared::loot::UseKind::Hold);
    }
    let (eye, dir) = (g.player.eye(), shared::sim::forward_vec(g.yaw, g.pitch));
    l.throw(p.drops.clone(), eye, dir);
    let msg = format!("{}: took {}{}", crate::loot::name(&db, id), p.taken, p.message.map(|m| format!(" ({m})")).unwrap_or_default());
    if p.switch_to.is_some() {
        // straight into the hands (no holster / draw), so a console line can be followed by a pose or a screenshot
        g.loadout = crate::loadout::Loadout { slot: crate::loadout::Slot::Gun, pending: None, quick: false };
    }
    g.sync_r99();
    if p.switch_to.is_some() {
        if let Some(w) = g.weapon.as_mut() {
            w.draw = None;
        }
    }
    out(msg);
}

fn cmd_inventory(a: &mut App, args: &[String]) {
    // `inventory open|close`: the TAB screen (headless screenshots of it)
    if let (Some(v), Some(l)) = (args.get(1), a.game.as_mut().and_then(|g| g.loot.as_mut())) {
        l.open = v == "open";
    }
    let Some(l) = a.game.as_ref().and_then(|g| g.loot.as_ref()) else { return err("no inventory") };
    let (db, inv) = (&l.db, &l.inv);
    let name = |i: shared::loot::ItemId| crate::loot::name(db, i);
    out(format!("hands {:?}, shield {}, infinite ammo {}", inv.hands, inv.shield, inv.infinite_ammo));
    for (k, v) in [("helmet", inv.helmet), ("armor", inv.armor), ("knockdown", inv.incap), ("backpack", inv.bag)] {
        out(format!("  {k:10} {}", v.map(name).unwrap_or_else(|| "-".into())));
    }
    for (i, w) in inv.weapons.iter().enumerate() {
        match w {
            Some(w) => out(format!(
                "  weapon {} {} mag {}/{} reserve {} [{}]",
                i + 1,
                name(w.item),
                w.clip,
                w.clip_size(db),
                inv.reserve(db, i).unwrap_or(0),
                w.atts.iter().map(|(p, &a)| format!("{p}: {}", name(a))).collect::<Vec<_>>().join(", ")
            )),
            None => out(format!("  weapon {} -", i + 1)),
        }
    }
    out(format!("  backpack {}/{}: {}", inv.backpack.len(), inv.capacity(db), inv.backpack.iter().map(|s| format!("{} x{}", name(s.item), s.count)).collect::<Vec<_>>().join(", ")));
}

fn cmd_use(a: &mut App, args: &[String]) {
    let how = if args.get(1).is_some_and(|v| v == "hold") { shared::loot::UseKind::Hold } else { shared::loot::UseKind::Tap };
    let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
    let (eye, dir) = (g.player.eye(), shared::sim::forward_vec(g.yaw, g.pitch));
    let Some(l) = g.loot.as_mut() else { return err("no loot") };
    let Some((name, p)) = l.use_target(eye, dir, how) else { return out("nothing to use there") };
    out(format!("{name}: took {}{}{}", p.taken, if p.needs_hold { " (needs a hold)" } else { "" }, p.message.map(|m| format!(" ({m})")).unwrap_or_default()));
    if p.switch_to.is_some() {
        g.loadout = crate::loadout::Loadout { slot: crate::loadout::Slot::Gun, pending: None, quick: false };
        g.sync_r99();
        if let Some(w) = g.weapon.as_mut() {
            w.draw = None;
        }
    }
}

fn cmd_loot_near(a: &mut App, args: &[String]) {
    let r: f32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(300.0);
    let Some(g) = a.game.as_ref() else { return err("the map hasn't loaded yet") };
    let Some(l) = g.loot.as_ref() else { return err("no loot") };
    let eye = g.player.eye();
    let mut v: Vec<_> = l.world.items.iter().filter(|i| (i.pos - eye).length() < r).collect();
    v.sort_by(|x, y| (x.pos - eye).length().total_cmp(&(y.pos - eye).length()));
    let target = l.world.target(&l.db, eye, shared::sim::forward_vec(g.yaw, g.pitch));
    for i in v.iter().take(24) {
        out(format!(
            "{}{} x{} at {:.0} {:.0} {:.0} ({:.0} u)",
            if Some(i.id) == target { "> " } else { "  " },
            crate::loot::name(&l.db, i.item),
            i.count,
            i.pos.x,
            i.pos.y,
            i.pos.z,
            (i.pos - eye).length()
        ));
    }
}

fn cmd_infinite_ammo(a: &mut App, args: &[String]) {
    let Some(l) = a.game.as_mut().and_then(|g| g.loot.as_mut()) else { return err("no inventory") };
    l.inv.infinite_ammo = match args.get(1).map(String::as_str) {
        Some("1") => true,
        Some("0") => false,
        _ => !l.inv.infinite_ammo,
    };
    out(format!("infinite ammo {}", if l.inv.infinite_ammo { "on" } else { "off" }));
}

fn cmd_weapon_fire(a: &mut App, args: &[String]) {
    let ticks: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(18);
    // ticks after letting go (the last bullets land); 0 = stop mid-spray (screenshots of the flash / tracers)
    let settle: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let wi = crate::weapon::WeaponInput { fire: true, on_ground: true, ..Default::default() };
    let Some(g) = a.game.as_mut() else { return err("the map hasn't loaded yet") };
    if g.weapon.is_none() {
        return err("no weapon (run tools/build_weapon.py)");
    }
    let (p0, y0) = (g.pitch, g.yaw);
    for n in 0..ticks + settle {
        let g = a.game.as_mut().unwrap();
        let w = g.weapon.as_mut().unwrap();
        let (dp, dy) = w.tick(crate::TICK, if n < ticks { wi } else { Default::default() }, g.player.eye(), g.yaw, g.pitch, &g.world.collision);
        g.pitch = (g.pitch + dp).clamp(-89.0, 89.0);
        g.yaw = (g.yaw + dy).rem_euclid(360.0);
        // the viewmodel + effects as frames would see them
        crate::step_viewmodel(a, crate::TICK);
    }
    let g = a.game.as_ref().unwrap();
    let w = g.weapon.as_ref().unwrap();
    out(format!("fired {} shots, {} impacts, ammo {}, recoil moved the view {:.2} pitch {:.2} yaw", w.shots_total, w.impacts.len(), w.ammo, g.pitch - p0, g.yaw - y0));
}

fn cmd_status(a: &mut App, _: &[String]) {
    out(format!("map      {}", a.cfg.map));
    out(format!("assets   {}", a.cfg.assets_root.clone().unwrap_or_else(|| "(default)".into())));
    if let Some(gfx) = &a.gfx {
        out(format!("gpu      {}", gfx.adapter_name));
        out(format!("window   {}x{}  game {}x{}  render scale {}%  msaa {}x", gfx.width, gfx.height, gfx.game_screen()[0], gfx.game_screen()[1], gfx.settings.render_scale, gfx.settings.msaa));
    }
    if let Some(c) = &a.console {
        out(format!("frame    {:.0} fps, {:.2} ms", 1000.0 / c.hud.frame_ms.max(0.001), c.hud.frame_ms));
    }
    match &a.game {
        Some(g) => {
            out(format!("world    {}", g.world.stats));
            out(format!("player   {}  {:.0} u/s{}", g.player.state_name(), g.player.vel.truncate().length(), if g.player.noclip { "  noclip" } else { "" }));
        }
        None => out("world    loading"),
    }
}
