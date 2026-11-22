//! Developer console, Quake / Source style: ` (rebindable, Controls tab) drops it down from the top of the screen.
//! Type a command or a cvar name (alone prints it, with a value sets it); `;` chains commands, quotes group words.
//! Tab completes and cycles, Up/Down walk the history (kept in `r5rust_console_history.txt`). The game's log
//! (info / warn / error) is mirrored here. The commands and cvars themselves are in `commands.rs`.
//!
//! Also draws the always-on overlays the console turns on: `cl_showfps` (top right) and `cl_showpos` (top left).

use egui::{Align2, Color32, FontId, Id, Key, Modifiers, Order, Rect, RichText, Stroke, TextStyle, pos2, vec2};
use std::collections::VecDeque;
use std::sync::Mutex;
use winit::event::WindowEvent;
use winit::window::Window;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// command output
    Info,
    /// log::info / debug lines
    Log,
    Warn,
    Error,
    /// the echoed command line
    Input,
    Good,
}

impl Kind {
    fn color(self) -> Color32 {
        match self {
            Kind::Info => Color32::from_rgb(0xd8, 0xdb, 0xe0),
            Kind::Log => Color32::from_rgb(0x8c, 0x93, 0x9c),
            Kind::Warn => Color32::from_rgb(0xf0, 0xb4, 0x4c),
            Kind::Error => Color32::from_rgb(0xff, 0x6b, 0x5b),
            Kind::Input => Color32::from_rgb(0x86, 0xb8, 0xff),
            Kind::Good => Color32::from_rgb(0x8f, 0xd1, 0x8f),
        }
    }
}

const ACCENT: Color32 = Color32::from_rgb(0xff, 0x5a, 0x36);
const MAX_LINES: usize = 4000;
const HISTORY_FILE: &str = "r5rust_console_history.txt";
const MAX_HISTORY: usize = 200;

struct Line {
    kind: Kind,
    text: String,
}

/// Scrollback, global so the logger and the commands can both write to it.
static LINES: Mutex<VecDeque<Line>> = Mutex::new(VecDeque::new());

fn lines() -> std::sync::MutexGuard<'static, VecDeque<Line>> {
    LINES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Appends to the console (each `\n` starts a new line).
pub fn print(kind: Kind, text: impl AsRef<str>) {
    let mut l = lines();
    for t in text.as_ref().split('\n') {
        if l.len() >= MAX_LINES {
            l.pop_front();
        }
        l.push_back(Line { kind, text: t.trim_end_matches('\r').to_string() });
    }
}

pub fn clear() {
    lines().clear();
}

/// The whole scrollback as text (`condump`).
pub fn dump() -> String {
    lines().iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------- log capture

/// env_logger, plus a copy of every record it lets through into the console.
struct Tee {
    inner: env_logger::Logger,
}

impl log::Log for Tee {
    fn enabled(&self, m: &log::Metadata) -> bool {
        self.inner.enabled(m)
    }
    fn log(&self, r: &log::Record) {
        if !self.inner.matches(r) {
            return;
        }
        self.inner.log(r);
        let (kind, text) = match r.level() {
            log::Level::Error => (Kind::Error, format!("{}: {}", r.target(), r.args())),
            log::Level::Warn => (Kind::Warn, format!("{}: {}", r.target(), r.args())),
            _ => (Kind::Log, r.args().to_string()),
        };
        print(kind, text);
    }
    fn flush(&self) {
        self.inner.flush()
    }
}

/// Installs `builder`'s logger with the console tee.
pub fn init_logging(mut builder: env_logger::Builder) {
    let inner = builder.build();
    let max = inner.filter();
    if log::set_boxed_logger(Box::new(Tee { inner })).is_ok() {
        log::set_max_level(max);
    }
}

// ---------------------------------------------------------------- parsing

/// Splits a console line into commands on `;` outside quotes, dropping `//` comments.
pub fn split_commands(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            '/' if !quoted && chars.peek() == Some(&'/') => break,
            ';' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// One command's words; double quotes group (and are removed).
pub fn tokenize(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in cmd.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            _ => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------- the console

/// What the overlays show; the game fills it in every frame.
#[derive(Default, Clone)]
pub struct Hud {
    pub frame_ms: f32,
    pub pos: Option<[f32; 3]>,
    pub ang: [f32; 2],
    pub vel: [f32; 3],
    pub state: String,
    /// the weapon readout (crosshair, ammo); None = nothing in hand
    pub weapon: Option<WeaponHud>,
}

#[derive(Default, Clone)]
pub struct WeaponHud {
    pub name: String,
    pub ammo: u32,
    pub clip: u32,
    /// 0..1 while reloading
    pub reload: Option<f32>,
    /// crosshair gap from the centre, pixels (the spread cone at this FOV)
    pub gap_px: f32,
    /// 0..1 aim: the crosshair fades out as the sights come up
    pub ads: f32,
}

/// A ground item's label (test UI): position in 0..1 of the screen.
#[derive(Clone, Default)]
pub struct LootLabel {
    pub x: f32,
    pub y: f32,
    pub name: String,
    pub tier: u8,
    pub targeted: bool,
}

#[derive(Clone, Default)]
pub struct InvSlot {
    pub name: String,
    pub tier: u8,
    pub count: u32,
    pub icon: Option<std::path::PathBuf>,
}

#[derive(Clone, Default)]
pub struct InvWeapon {
    pub name: String,
    pub icon: Option<std::path::PathBuf>,
    pub tier: u8,
    pub clip: u32,
    pub clip_max: u32,
    pub reserve: String,
    /// attachment point -> what's on it
    pub atts: Vec<(String, Option<InvSlot>)>,
    pub active: bool,
}

/// The inventory screen's contents (TAB).
#[derive(Clone, Default)]
pub struct InvView {
    pub gear: Vec<(String, Option<InvSlot>)>,
    /// current / max shield
    pub shield: (u32, u32),
    pub weapons: [Option<InvWeapon>; 2],
    pub backpack: Vec<InvSlot>,
    pub capacity: usize,
    pub infinite: bool,
}

/// The loot's test UI for this frame (main.rs loot_overlay).
#[derive(Clone, Default)]
pub struct LootUi {
    pub labels: Vec<LootLabel>,
    pub prompt: Option<String>,
    pub prompt_icon: Option<std::path::PathBuf>,
    /// hold-to-swap progress 0..1
    pub hold: Option<f32>,
    pub message: Option<String>,
    pub inv: Option<InvView>,
}

/// A click on the inventory screen (main.rs App::loot_actions applies it).
#[derive(Clone, Debug)]
pub enum InvAction {
    /// backpack stack, whole stack?
    DropStack(usize, bool),
    DropWeapon(usize),
    /// weapon slot, attachment point
    Detach(usize, String),
    Infinite,
    Close,
}

fn tier_color(t: u8) -> egui::Color32 {
    let [r, g, b] = crate::loot::tier_color(t);
    egui::Color32::from_rgb(r, g, b)
}

pub struct Console {
    ctx: egui::Context,
    /// None in the headless screenshot mode
    state: Option<egui_winit::State>,
    renderer: egui_wgpu::Renderer,
    pub open: bool,
    input: String,
    history: Vec<String>,
    /// index into `history` while walking it with Up/Down
    hist_pos: Option<usize>,
    /// completion candidates: name, one-line help
    names: Vec<(String, String)>,
    /// Tab cycling: the prefix typed before the first Tab, and the match shown now
    tab: Option<(String, usize)>,
    /// command lines entered since the game last took them
    pub submitted: Vec<String>,
    /// text a command wants on the clipboard (getpos)
    pub clipboard: Option<String>,
    pub hud: Hud,
    /// the loot's labels / prompt / inventory screen, and the screen's clicks for the game
    pub loot: LootUi,
    pub loot_actions: Vec<InvAction>,
    frame_times: VecDeque<f32>,
    fps_shown: (f32, f32),
    fps_acc: (f32, u32),
    cursor_to_end: bool,
    /// loot icons loaded so far (None: the file is missing or unreadable)
    icons: std::collections::HashMap<std::path::PathBuf, Option<egui::TextureHandle>>,
}

/// An icon drawn `h` points tall at its own aspect.
#[derive(Clone, Copy)]
struct Icon {
    id: egui::TextureId,
    aspect: f32,
}

impl Icon {
    fn image(self, h: f32) -> egui::Image<'static> {
        egui::Image::new((self.id, vec2(h * self.aspect, h)))
    }
}

impl Console {
    pub fn new(window: Option<&Window>, device: &wgpu::Device, format: wgpu::TextureFormat, names: Vec<(String, String)>) -> Self {
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        let state = window.map(|w| egui_winit::State::new(ctx.clone(), egui::ViewportId::ROOT, w, Some(w.scale_factor() as f32), None, None));
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        let history = std::fs::read_to_string(HISTORY_FILE).map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()).unwrap_or_default();
        Console {
            ctx,
            state,
            renderer,
            open: false,
            input: String::new(),
            history,
            hist_pos: None,
            names,
            tab: None,
            submitted: Vec::new(),
            clipboard: None,
            hud: Hud::default(),
            loot: LootUi::default(),
            loot_actions: Vec::new(),
            frame_times: VecDeque::new(),
            fps_shown: (0.0, 0.0),
            fps_acc: (0.0, 0),
            cursor_to_end: false,
            icons: Default::default(),
        }
    }

    /// The texture of a loot icon, loaded on first use.
    fn icon(&mut self, path: &Option<std::path::PathBuf>) -> Option<Icon> {
        let path = path.as_ref()?;
        if !self.icons.contains_key(path) {
            let tex = image::open(path).ok().map(|img| {
                let img = img.to_rgba8();
                let size = [img.width() as usize, img.height() as usize];
                let c = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                self.ctx.load_texture(path.to_string_lossy(), c, egui::TextureOptions::LINEAR)
            });
            self.icons.insert(path.clone(), tex);
        }
        self.icons[path].as_ref().map(|t| Icon { id: t.id(), aspect: t.aspect_ratio() })
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.tab = None;
        self.hist_pos = None;
    }

    /// Feeds a window event while the console is open. Returns true when it was input (the game must ignore it).
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        let inv = self.loot.inv.is_some();
        if !self.open && !inv {
            return false;
        }
        let mouse = matches!(event, WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } | WindowEvent::CursorMoved { .. } | WindowEvent::CursorLeft { .. });
        if !self.open && !mouse {
            return false;
        }
        if let Some(st) = &mut self.state {
            let _ = st.on_window_event(window, event);
        }
        true
    }

    /// Per-frame timing for `cl_showfps` (call once per presented frame).
    pub fn frame_time(&mut self, dt: f32) {
        self.frame_times.push_back(dt * 1000.0);
        while self.frame_times.len() > 240 {
            self.frame_times.pop_front();
        }
        // the number itself updates 4x a second, like the title bar, so it's readable
        self.fps_acc.0 += dt;
        self.fps_acc.1 += 1;
        if self.fps_acc.0 >= 0.25 {
            let ms = self.fps_acc.0 * 1000.0 / self.fps_acc.1 as f32;
            self.fps_shown = (1000.0 / ms, ms);
            self.fps_acc = (0.0, 0);
        }
    }

    fn push_history(&mut self, line: &str) {
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
            if self.history.len() > MAX_HISTORY {
                self.history.remove(0);
            }
            let _ = std::fs::write(HISTORY_FILE, self.history.join("\n") + "\n");
        }
    }

    /// Names starting with the first word typed (only while the first word is still being typed).
    fn matches(&self, prefix: &str) -> Vec<usize> {
        let p = prefix.to_ascii_lowercase();
        if p.is_empty() || p.contains(char::is_whitespace) {
            return Vec::new();
        }
        let mut m: Vec<usize> = (0..self.names.len()).filter(|&i| self.names[i].0.starts_with(&p)).collect();
        m.sort_by(|&a, &b| self.names[a].0.cmp(&self.names[b].0));
        m
    }

    /// Draws the console (when open) and the overlays over `target`.
    pub fn draw(&mut self, window: &Window, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], show_fps: u8, show_pos: bool) {
        let loot = !self.loot.labels.is_empty() || self.loot.prompt.is_some() || self.loot.message.is_some() || self.loot.inv.is_some();
        if !self.open && show_fps == 0 && !show_pos && self.hud.weapon.is_none() && !loot {
            return;
        }
        let Some(st) = &mut self.state else { return };
        let input = st.take_egui_input(window);
        let ctx = self.ctx.clone();
        let out = ctx.run_ui(input, |_| self.ui(&ctx, show_fps, show_pos));
        if let Some(st) = &mut self.state {
            st.handle_platform_output(window, out.platform_output.clone());
        }
        crate::menu::paint(&mut self.renderer, &self.ctx, device, queue, target, size, out);
    }

    /// Headless screenshot mode: lays out a few frames (focus, scrolling) and draws the last one into `target`.
    pub fn draw_headless(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], show_fps: u8, show_pos: bool) {
        let mut last = None;
        for i in 0..10 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(size[0] as f32, size[1] as f32))),
                time: Some(i as f64 / 30.0),
                predicted_dt: 1.0 / 30.0,
                ..Default::default()
            };
            let ctx = self.ctx.clone();
            let out = ctx.run_ui(input, |_| self.ui(&ctx, show_fps, show_pos));
            for (id, deltas) in &out.textures_delta.set {
                for delta in deltas.iter() {
                    self.renderer.update_texture(device, queue, *id, delta);
                }
            }
            last = Some(out);
        }
        if let Some(mut out) = last {
            out.textures_delta.set.clear();
            crate::menu::paint(&mut self.renderer, &self.ctx, device, queue, target, size, out);
        }
    }

    /// Types `text` into the input line (headless screenshots of the completion list).
    pub fn set_input(&mut self, text: &str) {
        self.input = text.to_string();
    }

    fn ui(&mut self, ctx: &egui::Context, show_fps: u8, show_pos: bool) {
        if let Some(t) = self.clipboard.take() {
            ctx.copy_text(t);
        }
        // while the console is down the fps readout moves under it, and the position readout (covered, and you
        // can't move anyway) hides
        let top = if self.open { Self::console_rect(ctx).bottom() + 10.0 } else { ctx.content_rect().top() + 10.0 };
        if show_fps > 0 {
            self.fps_overlay(ctx, show_fps, top);
        }
        if show_pos && !self.open {
            self.pos_overlay(ctx);
        }
        if let Some(w) = self.hud.weapon.clone() {
            weapon_hud(ctx, &w);
        }
        self.loot_ui(ctx);
        if self.open {
            self.console_ui(ctx);
        }
    }

    /// The loot's test UI: labels on ground items, the use prompt (+ hold ring), messages, the inventory screen.
    fn loot_ui(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, Id::unique("loot-labels")));
        let font = egui::FontId::proportional(12.0);
        for l in &self.loot.labels {
            let pos = egui::pos2(screen.left() + l.x * screen.width(), screen.top() + l.y * screen.height());
            let col = tier_color(l.tier);
            let r = painter.text(pos, egui::Align2::CENTER_BOTTOM, &l.name, font.clone(), if l.targeted { egui::Color32::WHITE } else { col });
            painter.rect_filled(r.expand(2.0), 2.0, egui::Color32::from_black_alpha(if l.targeted { 170 } else { 110 }));
            painter.text(pos, egui::Align2::CENTER_BOTTOM, &l.name, font.clone(), if l.targeted { egui::Color32::WHITE } else { col });
            painter.rect_filled(egui::Rect::from_min_size(egui::pos2(r.left() - 2.0, r.bottom() + 2.0), vec2(r.width() + 4.0, 2.0)), 0.0, col);
        }
        let c = screen.center();
        let prompt_icon = self.icon(&self.loot.prompt_icon.clone());
        if let Some(p) = &self.loot.prompt {
            let at = egui::pos2(c.x, c.y + 70.0);
            if let Some(ic) = prompt_icon {
                let h = 44.0;
                let r = egui::Rect::from_center_size(egui::pos2(at.x, at.y - 18.0 - h * 0.5), vec2(h * ic.aspect, h));
                painter.rect_filled(r.expand(4.0), 3.0, egui::Color32::from_black_alpha(120));
                painter.image(ic.id, r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            let r = painter.text(at, egui::Align2::CENTER_CENTER, p, egui::FontId::proportional(17.0), egui::Color32::WHITE);
            painter.rect_filled(r.expand2(vec2(10.0, 5.0)), 3.0, egui::Color32::from_black_alpha(160));
            painter.text(at, egui::Align2::CENTER_CENTER, p, egui::FontId::proportional(17.0), egui::Color32::WHITE);
            if let Some(h) = self.loot.hold {
                let bar = egui::Rect::from_min_size(egui::pos2(r.left(), r.bottom() + 7.0), vec2(r.width() * h, 3.0));
                painter.rect_filled(bar, 0.0, egui::Color32::from_rgb(255, 210, 60));
            }
        }
        if let Some(m) = &self.loot.message {
            let at = egui::pos2(c.x, c.y + 110.0);
            let r = painter.text(at, egui::Align2::CENTER_CENTER, m, egui::FontId::proportional(15.0), egui::Color32::from_rgb(255, 120, 100));
            painter.rect_filled(r.expand2(vec2(8.0, 4.0)), 3.0, egui::Color32::from_black_alpha(150));
            painter.text(at, egui::Align2::CENTER_CENTER, m, egui::FontId::proportional(15.0), egui::Color32::from_rgb(255, 120, 100));
        }
        let Some(inv) = self.loot.inv.clone() else { return };
        let mut icons: std::collections::HashMap<std::path::PathBuf, Icon> = Default::default();
        let slots = inv.gear.iter().filter_map(|g| g.1.as_ref()).chain(inv.backpack.iter()).chain(inv.weapons.iter().flatten().flat_map(|w| w.atts.iter().filter_map(|a| a.1.as_ref())));
        let paths: Vec<_> = slots.filter_map(|s| s.icon.clone()).chain(inv.weapons.iter().flatten().filter_map(|w| w.icon.clone())).collect();
        for p in paths {
            if let Some(ic) = self.icon(&Some(p.clone())) {
                icons.insert(p, ic);
            }
        }
        let icon_of = |p: &Option<std::path::PathBuf>| p.as_ref().and_then(|p| icons.get(p).copied());
        let mut actions = Vec::new();
        egui::Window::new("Inventory")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_width(880.0)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    // equipment
                    ui.vertical(|ui| {
                        ui.set_width(210.0);
                        ui.heading("Equipment");
                        for (label, item) in &inv.gear {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(label).weak());
                                match item {
                                    Some(it) => {
                                        if let Some(ic) = icon_of(&it.icon) {
                                            ui.add(ic.image(22.0));
                                        }
                                        ui.label(egui::RichText::new(&it.name).color(tier_color(it.tier)))
                                    }
                                    None => ui.label(egui::RichText::new("empty").weak()),
                                };
                            });
                        }
                        ui.label(format!("Shield {} / {}", inv.shield.0, inv.shield.1));
                        ui.add_space(8.0);
                        let mut inf = inv.infinite;
                        if ui.checkbox(&mut inf, "Infinite ammo (Firing Range)").changed() {
                            actions.push(InvAction::Infinite);
                        }
                    });
                    ui.separator();
                    // weapons
                    ui.vertical(|ui| {
                        ui.set_width(300.0);
                        ui.heading("Weapons");
                        for (i, w) in inv.weapons.iter().enumerate() {
                            ui.group(|ui| {
                                ui.set_width(285.0);
                                match w {
                                    None => {
                                        ui.label(egui::RichText::new(format!("{}  empty", i + 1)).weak());
                                    }
                                    Some(w) => {
                                        if let Some(ic) = icon_of(&w.icon) {
                                            ui.add(ic.image(44.0));
                                        }
                                        ui.horizontal(|ui| {
                                            let t = format!("{}  {}{}", i + 1, w.name, if w.active { "  (in hand)" } else { "" });
                                            ui.label(egui::RichText::new(t).strong().color(tier_color(w.tier)));
                                            if ui.small_button("Drop").clicked() {
                                                actions.push(InvAction::DropWeapon(i));
                                            }
                                        });
                                        ui.label(format!("Mag {} / {}   Reserve {}", w.clip, w.clip_max, w.reserve));
                                        for (p, a) in &w.atts {
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(p).weak());
                                                match a {
                                                    Some(a) => {
                                                        if let Some(ic) = icon_of(&a.icon) {
                                                            ui.add(ic.image(20.0));
                                                        }
                                                        ui.label(egui::RichText::new(&a.name).color(tier_color(a.tier)));
                                                        if ui.small_button("x").on_hover_text("take off into the backpack").clicked() {
                                                            actions.push(InvAction::Detach(i, p.clone()));
                                                        }
                                                    }
                                                    None => {
                                                        ui.label(egui::RichText::new("-").weak());
                                                    }
                                                }
                                            });
                                        }
                                    }
                                }
                            });
                        }
                    });
                    ui.separator();
                    // backpack
                    ui.vertical(|ui| {
                        ui.set_width(300.0);
                        ui.heading(format!("Backpack {} / {}", inv.backpack.len(), inv.capacity));
                        egui::Grid::new("backpack").num_columns(3).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
                            for (i, s) in inv.backpack.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    if let Some(ic) = icon_of(&s.icon) {
                                        ui.add(ic.image(22.0));
                                    }
                                    ui.label(egui::RichText::new(&s.name).color(tier_color(s.tier)));
                                });
                                ui.label(format!("x{}", s.count));
                                ui.horizontal(|ui| {
                                    if s.count > 1 && ui.small_button("Drop 1").clicked() {
                                        actions.push(InvAction::DropStack(i, false));
                                    }
                                    if ui.small_button("Drop").clicked() {
                                        actions.push(InvAction::DropStack(i, true));
                                    }
                                });
                                ui.end_row();
                            }
                            for _ in inv.backpack.len()..inv.capacity {
                                ui.label(egui::RichText::new("empty").weak());
                                ui.label("");
                                ui.label("");
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Close (TAB)").clicked() {
                        actions.push(InvAction::Close);
                    }
                    ui.label(egui::RichText::new("E picks up, hold E swaps a weapon. Keys still move you while this is open.").weak());
                });
            });
        self.loot_actions.extend(actions);
    }

    fn console_rect(ctx: &egui::Context) -> Rect {
        let screen = ctx.content_rect();
        let h = (screen.height() * 0.45).max(220.0).min(screen.height());
        Rect::from_min_size(screen.min, vec2(screen.width(), h))
    }

    fn console_ui(&mut self, ctx: &egui::Context) {
        let rect = Self::console_rect(ctx);
        let mono = TextStyle::Monospace;
        let input_id = Id::unique("console_input");

        // keys the text box would otherwise eat or ignore
        let (up, down, tab, enter, page_up, page_down) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Tab),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::PageUp),
                i.key_pressed(Key::PageDown),
            )
        });
        if up || down {
            self.tab = None;
            if !self.history.is_empty() {
                let n = self.history.len();
                self.hist_pos = match (self.hist_pos, up) {
                    (None, true) => Some(n - 1),
                    (None, false) => None,
                    (Some(i), true) => Some(i.saturating_sub(1)),
                    (Some(i), false) if i + 1 < n => Some(i + 1),
                    (Some(_), false) => None,
                };
                self.input = self.hist_pos.map(|i| self.history[i].clone()).unwrap_or_default();
                self.cursor_to_end = true;
            }
        }
        if tab {
            let (prefix, next) = match &self.tab {
                Some((p, i)) => (p.clone(), i + 1),
                None => (self.input.trim_start().to_string(), 0),
            };
            let m = self.matches(&prefix);
            if !m.is_empty() {
                let k = next % m.len();
                self.input = format!("{} ", self.names[m[k]].0);
                self.tab = Some((prefix, k));
                self.cursor_to_end = true;
            }
        }
        if self.cursor_to_end {
            if let Some(mut st) = egui::TextEdit::load_state(ctx, input_id) {
                let c = egui::text::CCursor::new(self.input.chars().count());
                st.cursor.set_char_range(Some(egui::text::CCursorRange::one(c)));
                st.store(ctx, input_id);
            }
            self.cursor_to_end = false;
        }

        egui::Area::new(Id::unique("console")).fixed_pos(rect.min).order(Order::Foreground).show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            let p = ui.painter();
            p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(9, 11, 14, 238));
            p.line_segment([pos2(rect.left(), rect.bottom() - 1.0), pos2(rect.right(), rect.bottom() - 1.0)], Stroke::new(2.0, ACCENT));
            let inner = rect.shrink2(vec2(14.0, 8.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("R5RUST CONSOLE").strong().color(ACCENT).size(12.0));
                    ui.label(RichText::new("`  close    Tab  complete    Up/Down  history    help  commands    find <text>  search").size(11.0).color(Kind::Log.color()));
                });
                ui.add_space(4.0);
                let row_h = ui.text_style_height(&mono);
                let input_h = row_h + 14.0;
                let list_h = (ui.available_height() - input_h).max(row_h);
                let n = lines().len();
                let mut sa = egui::ScrollArea::vertical().id_salt("console_scroll").max_height(list_h).auto_shrink([false, false]).stick_to_bottom(true);
                if page_up || page_down {
                    let off = ctx.data(|d| d.get_temp::<f32>(Id::unique("console_off"))).unwrap_or(0.0);
                    let step = list_h * 0.8 * if page_up { -1.0 } else { 1.0 };
                    sa = sa.vertical_scroll_offset((off + step).max(0.0));
                }
                let out = sa.show_rows(ui, row_h, n, |ui, range| {
                    let l = lines();
                    for line in l.range(range) {
                        ui.add(egui::Label::new(RichText::new(&line.text).text_style(mono.clone()).color(line.kind.color())).truncate());
                    }
                });
                ctx.data_mut(|d| d.insert_temp(Id::unique("console_off"), out.state.clamped_offset().y));
                ui.add_space(4.0);
                ui.painter().line_segment([pos2(inner.left(), ui.cursor().top()), pos2(inner.right(), ui.cursor().top())], Stroke::new(1.0, Color32::from_white_alpha(18)));
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("]").text_style(mono.clone()).color(ACCENT));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .id(input_id)
                            .font(mono.clone())
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY)
                            .lock_focus(true),
                    );
                    if resp.changed() {
                        self.tab = None;
                        self.hist_pos = None;
                    }
                    if enter {
                        let line = self.input.trim().to_string();
                        self.input.clear();
                        self.tab = None;
                        self.hist_pos = None;
                        if !line.is_empty() {
                            print(Kind::Input, format!("] {line}"));
                            self.push_history(&line);
                            self.submitted.push(line);
                        }
                    }
                    if !resp.has_focus() {
                        resp.request_focus();
                    }
                });
            });
        });

        // completions under the console, like CS2: name + what it does
        let first = self.input.trim_start();
        let (word, rest) = first.split_once(char::is_whitespace).unwrap_or((first, ""));
        let rows: Vec<(String, String, bool)> = if !rest.is_empty() || first.ends_with(' ') {
            self.names.iter().filter(|(n, _)| n.eq_ignore_ascii_case(word)).map(|(n, h)| (n.clone(), h.clone(), false)).collect()
        } else {
            let prefix = self.tab.as_ref().map(|(p, _)| p.clone()).unwrap_or_else(|| word.to_string());
            let sel = self.tab.as_ref().map(|(_, i)| *i);
            self.matches(&prefix).into_iter().enumerate().take(12).map(|(k, i)| (self.names[i].0.clone(), self.names[i].1.clone(), sel == Some(k))).collect()
        };
        if !rows.is_empty() {
            egui::Area::new(Id::unique("console_complete")).fixed_pos(pos2(rect.left() + 14.0, rect.bottom() + 2.0)).order(Order::Foreground).show(ctx, |ui| {
                egui::Frame::NONE.fill(Color32::from_rgba_unmultiplied(9, 11, 14, 238)).stroke(Stroke::new(1.0, Color32::from_white_alpha(24))).inner_margin(6.0).corner_radius(4.0).show(ui, |ui| {
                    egui::Grid::new("complete_grid").num_columns(2).spacing([18.0, 2.0]).show(ui, |ui| {
                        for (name, help, sel) in rows {
                            let c = if sel { ACCENT } else { Kind::Info.color() };
                            ui.label(RichText::new(name).text_style(TextStyle::Monospace).color(c));
                            ui.label(RichText::new(help).size(12.0).color(Kind::Log.color()));
                            ui.end_row();
                        }
                    });
                });
            });
        }
    }

    fn fps_overlay(&mut self, ctx: &egui::Context, mode: u8, top: f32) {
        let screen = ctx.content_rect();
        let (fps, ms) = self.fps_shown;
        let text = format!("{fps:.0} fps  {ms:.2} ms");
        let font = FontId::monospace(14.0);
        let p = ctx.layer_painter(egui::LayerId::new(Order::Background, Id::unique("showfps")));
        let anchor = pos2(screen.right() - 12.0, top);
        let galley = p.layout_no_wrap(text, font, Color32::WHITE);
        let r = Align2::RIGHT_TOP.anchor_size(anchor, galley.size()).expand2(vec2(8.0, 4.0));
        p.rect_filled(r, 3.0, Color32::from_black_alpha(150));
        p.galley(r.min + vec2(8.0, 4.0), galley, Color32::WHITE);
        if mode >= 2 && self.frame_times.len() > 1 {
            // frame time graph, 240 frames; the line marks 2x the average (spikes stand out)
            let g = Rect::from_min_size(pos2(screen.right() - 12.0 - 240.0, r.bottom() + 6.0), vec2(240.0, 60.0));
            p.rect_filled(g, 3.0, Color32::from_black_alpha(150));
            let avg = self.frame_times.iter().sum::<f32>() / self.frame_times.len() as f32;
            let top = (avg * 3.0).max(1.0);
            let pts: Vec<egui::Pos2> = self.frame_times.iter().enumerate().map(|(i, &t)| pos2(g.left() + i as f32, g.bottom() - (t / top).min(1.0) * g.height())).collect();
            let y2 = g.bottom() - (2.0 / 3.0) * g.height();
            p.line_segment([pos2(g.left(), y2), pos2(g.right(), y2)], Stroke::new(1.0, Color32::from_white_alpha(30)));
            p.add(egui::Shape::line(pts, Stroke::new(1.0, Color32::from_rgb(0x8f, 0xd1, 0x8f))));
            let max = self.frame_times.iter().cloned().fold(0.0, f32::max);
            p.text(pos2(g.left() + 4.0, g.top() + 2.0), Align2::LEFT_TOP, format!("max {max:.1} ms"), FontId::monospace(11.0), Color32::from_white_alpha(160));
        }
    }

    fn pos_overlay(&self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let h = &self.hud;
        let text = match h.pos {
            Some(p) => {
                let [vx, vy, vz] = h.vel;
                format!(
                    "pos: {:>9.2} {:>9.2} {:>9.2}\nang: {:>9.2} {:>9.2}\nvel: {:>9.2} {:>9.2} {:>9.2}\nspd: {:>9.1} u/s  ({:.1} 3D)\n     {}",
                    p[0], p[1], p[2], h.ang[0], h.ang[1], vx, vy, vz, (vx * vx + vy * vy).sqrt(), (vx * vx + vy * vy + vz * vz).sqrt(), h.state
                )
            }
            None => "loading".into(),
        };
        let p = ctx.layer_painter(egui::LayerId::new(Order::Background, Id::unique("showpos")));
        let galley = p.layout_no_wrap(text, FontId::monospace(13.0), Color32::WHITE);
        let r = Rect::from_min_size(pos2(screen.left() + 12.0, screen.top() + 10.0), galley.size()).expand2(vec2(8.0, 5.0));
        p.rect_filled(r, 3.0, Color32::from_black_alpha(150));
        p.galley(r.min + vec2(8.0, 5.0), galley, Color32::WHITE);
    }
}

/// Crosshair (Apex's default: a dot and four ticks that spread with the weapon's cone) and the ammo counter.
fn weapon_hud(ctx: &egui::Context, w: &WeaponHud) {
    let screen = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::new(Order::Background, Id::unique("weapon_hud")));
    let c = screen.center();
    let a = ((1.0 - w.ads * 2.0).clamp(0.0, 1.0) * 255.0) as u8;
    if a > 0 && w.reload.is_none() {
        let col = Color32::from_rgba_unmultiplied(255, 255, 255, a);
        let shadow = Color32::from_rgba_unmultiplied(0, 0, 0, a / 2);
        let gap = w.gap_px.clamp(3.0, screen.height() * 0.3);
        let len = 7.0;
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let a0 = c + vec2(dx, dy) * gap;
            let a1 = c + vec2(dx, dy) * (gap + len);
            p.line_segment([a0 + vec2(1.0, 1.0), a1 + vec2(1.0, 1.0)], Stroke::new(2.0, shadow));
            p.line_segment([a0, a1], Stroke::new(2.0, col));
        }
        p.circle_filled(c + vec2(0.5, 0.5), 1.8, shadow);
        p.circle_filled(c, 1.5, col);
    }
    // ammo, bottom right like the game's weapon panel
    let right = screen.right() - 40.0;
    let bottom = screen.bottom() - 36.0;
    let low = w.ammo * 4 <= w.clip;
    let ammo_col = if w.ammo == 0 { Color32::from_rgb(0xff, 0x5b, 0x4b) } else if low { Color32::from_rgb(0xf0, 0xb4, 0x4c) } else { Color32::WHITE };
    let big = p.layout_no_wrap(format!("{}", w.ammo), FontId::proportional(40.0), ammo_col);
    let small = p.layout_no_wrap(format!("/ {}", w.clip), FontId::proportional(18.0), Color32::from_white_alpha(170));
    let name = p.layout_no_wrap(w.name.to_uppercase(), FontId::proportional(14.0), Color32::from_white_alpha(200));
    let small_pos = pos2(right - small.size().x, bottom - small.size().y - 6.0);
    let big_pos = pos2(small_pos.x - 8.0 - big.size().x, bottom - big.size().y);
    let name_pos = pos2(right - name.size().x, big_pos.y - name.size().y - 2.0);
    let bg = Rect::from_min_max(pos2(big_pos.x - 14.0, name_pos.y - 8.0), pos2(right + 14.0, bottom + 8.0));
    p.rect_filled(bg, 4.0, Color32::from_black_alpha(110));
    p.galley(name_pos, name, Color32::WHITE);
    p.galley(big_pos, big, Color32::WHITE);
    p.galley(small_pos, small, Color32::WHITE);
    if let Some(f) = w.reload {
        let bar = Rect::from_min_size(pos2(bg.left(), bg.bottom() + 4.0), vec2(bg.width() * f.clamp(0.0, 1.0), 3.0));
        p.rect_filled(bar, 1.0, ACCENT);
        let t = p.layout_no_wrap("RELOADING".into(), FontId::proportional(13.0), Color32::from_white_alpha(220));
        p.galley(pos2(c.x - t.size().x * 0.5, c.y + 40.0), t, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        assert_eq!(split_commands("setpos 1 2 3; setang 0 90 // comment; nope"), vec!["setpos 1 2 3", "setang 0 90"]);
        assert_eq!(split_commands(r#"bind f5 "noclip; echo hi"; echo x"#), vec![r#"bind f5 "noclip; echo hi""#, "echo x"]);
        assert_eq!(tokenize(r#"bind f5 "noclip; echo hi""#), vec!["bind", "f5", "noclip; echo hi"]);
        assert_eq!(tokenize(r#"echo "" x"#), vec!["echo", "", "x"]);
    }
}
