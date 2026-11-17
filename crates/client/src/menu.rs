//! Esc menu (placeholder look): Controls (two binds per action, Apex-style) and Mouse & View settings.
//! egui drawn over the finished frame; while it's open the game gets no input and the cursor is free.

use crate::binds::{Action, Bind, UserSettings};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Controls,
    Mouse,
    Gameplay,
}

pub struct Menu {
    ctx: egui::Context,
    state: Option<egui_winit::State>,
    renderer: egui_wgpu::Renderer,
    pub open: bool,
    tab: Tab,
    /// waiting for an input to put in this action's slot
    listening: Option<(Action, usize)>,
    pub settings: UserSettings,
    dirty: bool,
    pub quit_requested: bool,
}

impl Menu {
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat, settings: UserSettings) -> Self {
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        let state = egui_winit::State::new(ctx.clone(), egui::ViewportId::ROOT, window, Some(window.scale_factor() as f32), None, None);
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        Menu { ctx, state: Some(state), renderer, open: false, tab: Tab::Controls, listening: None, settings, dirty: false, quit_requested: false }
    }

    /// Window-less menu for the headless screenshot mode (verification only).
    pub fn new_headless(device: &wgpu::Device, format: wgpu::TextureFormat, settings: UserSettings) -> Self {
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        let tab = match std::env::var("R5_MENU_TAB").as_deref() {
            Ok("mouse") => Tab::Mouse,
            Ok("gameplay") => Tab::Gameplay,
            _ => Tab::Controls,
        };
        Menu { ctx, state: None, renderer, open: true, tab, listening: None, settings, dirty: false, quit_requested: false }
    }

    pub fn is_listening(&self) -> bool {
        self.listening.is_some()
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.listening = None;
        if !self.open && self.dirty {
            self.settings.save();
            self.dirty = false;
        }
    }

    /// Feeds a window event to the menu. Returns true when the menu used it (the game must ignore it).
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        if !self.open {
            return false;
        }
        if let Some((action, slot)) = self.listening {
            let bind = match event {
                WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) => {
                        self.listening = None;
                        return true;
                    }
                    PhysicalKey::Code(c) => Some(Bind::Key(c)),
                    _ => None,
                },
                WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => Some(Bind::Mouse((*button).into())),
                WindowEvent::MouseWheel { delta, .. } => {
                    let y = match delta {
                        MouseScrollDelta::LineDelta(_, y) => *y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    };
                    if y > 0.0 {
                        Some(Bind::WheelUp)
                    } else if y < 0.0 {
                        Some(Bind::WheelDown)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(b) = bind {
                self.settings.binds.assign(action, slot, b);
                self.listening = None;
                self.dirty = true;
                return true;
            }
            if matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }) {
                return true;
            }
        }
        if let Some(st) = &mut self.state {
            let _ = st.on_window_event(window, event);
        }
        true
    }

    fn ui(&mut self) {
        let ctx = self.ctx.clone();
        egui::Area::new(egui::Id::unique("dim")).fixed_pos(egui::pos2(0.0, 0.0)).order(egui::Order::Background).show(&ctx, |ui| {
            let r = ctx.content_rect();
            ui.painter().rect_filled(r, 0.0, egui::Color32::from_black_alpha(150));
        });
        egui::Window::new("SETTINGS")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .default_width(560.0)
            .show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Controls, "Controls");
                    ui.selectable_value(&mut self.tab, Tab::Mouse, "Mouse & View");
                    ui.selectable_value(&mut self.tab, Tab::Gameplay, "Gameplay");
                });
                ui.separator();
                match self.tab {
                    Tab::Controls => self.controls_tab(ui),
                    Tab::Mouse => self.mouse_tab(ui),
                    Tab::Gameplay => self.gameplay_tab(ui),
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Resume  (Esc)").clicked() {
                        self.toggle();
                    }
                    if ui.button("Quit game").clicked() {
                        self.quit_requested = true;
                    }
                });
            });
    }

    fn controls_tab(&mut self, ui: &mut egui::Ui) {
        ui.label("Click a slot, then press a key, mouse button or scroll the wheel. Right-click clears. Esc cancels.");
        ui.add_space(6.0);
        egui::Grid::new("binds").num_columns(3).spacing([12.0, 6.0]).striped(true).show(ui, |ui| {
            ui.strong("Action");
            ui.strong("Key 1");
            ui.strong("Key 2");
            ui.end_row();
            for a in Action::ALL {
                ui.label(a.label());
                let slots = self.settings.binds.get(a);
                for (i, b) in slots.iter().enumerate() {
                    let listening = self.listening == Some((a, i));
                    let text = if listening { "PRESS A KEY...".to_string() } else { b.map(|b| b.label()).unwrap_or_else(|| "-".into()) };
                    let resp = ui.add_sized([150.0, 22.0], egui::Button::new(text).selected(listening));
                    if resp.clicked() {
                        self.listening = Some((a, i));
                    }
                    if resp.secondary_clicked() {
                        self.settings.binds.clear(a, i);
                        self.listening = None;
                        self.dirty = true;
                    }
                }
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        if ui.button("Reset controls to defaults").clicked() {
            self.settings.binds = crate::binds::Binds::default();
            self.listening = None;
            self.dirty = true;
        }
    }

    fn gameplay_tab(&mut self, ui: &mut egui::Ui) {
        if ui.checkbox(&mut self.settings.auto_sprint, "Auto Sprint").changed() {
            self.dirty = true;
        }
        ui.label("Like Apex's Auto Sprint option: pushing forward sprints without holding the sprint key.");
        ui.add_space(8.0);
        if ui.checkbox(&mut self.settings.mantle_boost, "Mantle Boost").changed() {
            self.dirty = true;
        }
        ui.label("Jump in the sweet spot at the end of a mantle to launch out of it. With it off, every jump during a mantle is Apex's normal jump-off / hop-back, and superglides (crouch + jump at the end of the mantle) work.");
    }

    fn mouse_tab(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        let mut changed = false;
        egui::Grid::new("mouse").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Sensitivity (Apex units)");
            changed |= ui.add(egui::Slider::new(&mut s.sensitivity, 0.1..=10.0).step_by(0.05)).changed();
            ui.end_row();
            ui.label("Field of view");
            changed |= ui.add(egui::Slider::new(&mut s.fov, 70.0..=120.0).step_by(1.0)).changed();
            ui.end_row();
            ui.label("Invert look");
            changed |= ui.checkbox(&mut s.invert_y, "").changed();
            ui.end_row();
        });
        ui.label("Sensitivity uses Source/Apex's 0.022 deg per count, so your in-game Apex number transfers 1:1.");
        if changed {
            self.dirty = true;
        }
    }

    /// Runs the UI and draws it over `target` (already holding the finished frame).
    pub fn draw(&mut self, window: &Window, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2]) {
        if !self.open {
            return;
        }
        let input = match &mut self.state {
            Some(st) => st.take_egui_input(window),
            None => egui::RawInput::default(),
        };
        let out = self.ctx.clone().run_ui(input, |_| self.ui());
        if let Some(st) = &mut self.state {
            st.handle_platform_output(window, out.platform_output.clone());
        }
        self.render(device, queue, target, size, out);
    }

    /// Headless: lays out and draws the menu into `target` (screenshot verification).
    pub fn draw_headless(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2]) {
        // run a second of frames so the window's fade-in animation completes, then draw the last one
        let mut last = None;
        for i in 0..30 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32))),
                time: Some(i as f64 / 30.0),
                predicted_dt: 1.0 / 30.0,
                ..Default::default()
            };
            let out = self.ctx.clone().run_ui(input, |_| self.ui());
            for (id, deltas) in &out.textures_delta.set {
                for delta in deltas.iter() {
                    self.renderer.update_texture(device, queue, *id, delta);
                }
            }
            last = Some(out);
        }
        if let Some(mut out) = last {
            out.textures_delta.set.clear();
            self.render(device, queue, target, size, out);
        }
    }

    fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], out: egui::FullOutput) {
        let ctx = self.ctx.clone();
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: out.pixels_per_point };
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas.iter() {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("menu") });
        let extra = self.renderer.update_buffers(device, queue, &mut enc, &prims, &screen);
        {
            let rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("menu"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut rp = rp.forget_lifetime();
            self.renderer.render(&mut rp, &prims, &screen);
        }
        queue.submit(extra.into_iter().chain(std::iter::once(enc.finish())));
        for id in &out.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}
