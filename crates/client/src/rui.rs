//! The Apex HUD, drawn by Respawn's own UI code: the weapon panel (`ui/weapon_hud_v2`) and the crosshair
//! (`ui/crosshair_plus`, the R-99's from its RUI_CrosshairData) run in the game's compiled RUI through
//! apex-content-kit's rui-host (`tools/rui/bridge.mjs`, a Node child process). Their draw lists (widgets with the
//! engine's own transforms, styles, images and text) are drawn here by a port of the kit's `public/js/rui.js`, itself a
//! port of the game's `ui_ps` pixel shader (rui.wgsl).
//!
//! Live state (ammo, mag size, attachments, spread, ADS) is mapped to the RUI's args exactly like Flowstate's
//! cl_weapon_status.gnut / the kit's gamehud.js do. Each state's draw list is cached; when the loadout changes every
//! ammo count of the new mag is fetched ahead so the counter changes the same frame a bullet leaves. A host call takes
//! ~2-8 ms once warm (the first one maps a ~300 MB memory snapshot: ~9 s cold).

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Style {
    pub color: [[f32; 4]; 3],
    pub tint: [f32; 4],
    pub blend: f32,
    pub premul: f32,
    pub hue: f32,
    pub sat: f32,
    pub light: f32,
    pub font: u32,
    pub size: f32,
    pub aspect: f32,
    pub outline: f32,
    pub thicken: f32,
    pub blur: f32,
    pub baseline: f32,
    pub kerning: f32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ImgMeta {
    pub map: [f32; 4],
    pub rect: [f32; 4],
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Img {
    pub code: Option<i32>,
    pub white: bool,
    pub file: Option<String>,
    pub meta: Option<ImgMeta>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Widget {
    #[serde(rename = "type")]
    pub ty: u8,
    pub pos: [f32; 2],
    pub grad: [f32; 4],
    pub size: [f32; 2],
    #[serde(rename = "clipRect", default)]
    pub clip_rect: Option<[f32; 4]>,
    pub style: Style,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub align: f32,
    #[serde(default)]
    pub img0: Option<Img>,
    #[serde(default)]
    pub img1: Option<Img>,
    #[serde(default)]
    pub mins: Option<[f32; 2]>,
    #[serde(default)]
    pub maxs: Option<[f32; 2]>,
    #[serde(rename = "uvMin", default)]
    pub uv_min: Option<[f32; 2]>,
    #[serde(rename = "uvMax", default)]
    pub uv_max: Option<[f32; 2]>,
    #[serde(default)]
    pub flags: u32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DrawList {
    pub hidden: bool,
    pub screen: [f32; 2],
    pub widgets: Vec<Widget>,
}

#[derive(Deserialize)]
struct Answer {
    id: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    fonts: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    screen: [f32; 2],
    #[serde(default)]
    widgets: Vec<Widget>,
}

/// The Node bridge to the game's UI code.
pub struct Bridge {
    stdin: std::process::ChildStdin,
    rx: Receiver<Answer>,
    child: std::process::Child,
    next: u64,
    pub fonts_dir: Option<PathBuf>,
}

impl Bridge {
    /// Starts `node tools/rui/bridge.mjs` (found next to the working directory or the exe's repo).
    pub fn start() -> Result<Bridge, String> {
        let script = ["tools/rui/bridge.mjs".into(), std::env::current_exe().ok().and_then(|e| e.ancestors().nth(3).map(|r| r.join("tools/rui/bridge.mjs"))).unwrap_or_default()]
            .into_iter()
            .find(|p: &PathBuf| p.is_file())
            .ok_or("tools/rui/bridge.mjs not found")?;
        let mut cmd = std::process::Command::new("node");
        cmd.arg(&script).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("node: {e}"))?;
        let stdin = child.stdin.take().ok_or("bridge stdin")?;
        let stdout = child.stdout.take().ok_or("bridge stdout")?;
        let (tx, rx) = std::sync::mpsc::channel::<Answer>();
        std::thread::Builder::new()
            .name("rui-bridge".into())
            .spawn(move || {
                for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                    match serde_json::from_str::<Answer>(&line) {
                        Ok(a) => {
                            if tx.send(a).is_err() {
                                break;
                            }
                        }
                        Err(e) => log::warn!("rui bridge: bad answer ({e}): {}", &line[..line.len().min(200)]),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        // the first line says where the game fonts are
        let hello = rx.recv_timeout(std::time::Duration::from_secs(20)).map_err(|_| "rui bridge didn't start (node / apex-content-kit?)")?;
        Ok(Bridge { stdin, rx, child, next: 1, fonts_dir: hello.fonts.filter(|_| hello.ready).map(PathBuf::from) })
    }

    fn send(&mut self, mut q: serde_json::Value) -> Result<u64, String> {
        let id = self.next;
        self.next += 1;
        q["id"] = id.into();
        writeln!(self.stdin, "{q}").map_err(|e| format!("rui bridge: {e}"))?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Blocking request (headless screenshots).
    pub fn request_sync(&mut self, q: serde_json::Value) -> Result<DrawList, String> {
        let id = self.send(q)?;
        loop {
            let a = self.rx.recv_timeout(std::time::Duration::from_secs(60)).map_err(|_| "rui bridge timed out")?;
            if a.id == id {
                return to_list(a);
            }
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn to_list(a: Answer) -> Result<DrawList, String> {
    if !a.ok {
        return Err(a.error.unwrap_or_else(|| "rui request failed".into()));
    }
    Ok(DrawList { hidden: a.hidden, screen: a.screen, widgets: a.widgets })
}

/// Cached, asynchronous draw lists keyed by request (the request JSON is the key).
pub struct Hud {
    pub bridge: Option<Bridge>,
    cache: HashMap<String, Arc<DrawList>>,
    inflight: HashMap<u64, String>,
    asked: HashSet<String>,
    last: HashMap<&'static str, Arc<DrawList>>,
    /// per HUD slot (crosshair, weapon): the request on its way, and the newest one wanted meanwhile. One at a time
    /// per slot: sending every new spread value at once queued hundreds behind each other in the host, so the
    /// crosshair showed a state seconds old, stuck wide open (Kainz, 2026-11-18)
    slot_busy: HashMap<&'static str, u64>,
    slot_next: HashMap<&'static str, serde_json::Value>,
    pub errors: u32,
}

impl Hud {
    pub fn new() -> Hud {
        let bridge = match Bridge::start() {
            Ok(b) => {
                log::info!("HUD: Respawn's RUI through tools/rui/bridge.mjs (fonts {:?})", b.fonts_dir);
                Some(b)
            }
            Err(e) => {
                log::warn!("HUD: no game UI ({e}); the simple HUD is used instead");
                None
            }
        };
        Hud { bridge, cache: HashMap::new(), inflight: HashMap::new(), asked: HashSet::new(), last: HashMap::new(), slot_busy: HashMap::new(), slot_next: HashMap::new(), errors: 0 }
    }

    pub fn active(&self) -> bool {
        self.bridge.is_some()
    }

    /// Takes the answers that came in.
    pub fn poll(&mut self) {
        let Some(b) = &self.bridge else { return };
        let mut done = Vec::new();
        while let Ok(a) = b.rx.try_recv() {
            let id = a.id;
            let Some(key) = self.inflight.remove(&id) else { continue };
            done.push(id);
            match to_list(a) {
                Ok(l) => {
                    self.cache.insert(key, Arc::new(l));
                }
                Err(e) => {
                    // ask again later instead of never
                    self.asked.remove(&key);
                    self.errors += 1;
                    if self.errors < 5 {
                        log::warn!("HUD: {e}");
                    }
                }
            }
        }
        // a slot's request came back: send the newest one wanted meanwhile
        for id in done {
            let slots: Vec<&'static str> = self.slot_busy.iter().filter(|(_, v)| **v == id).map(|(k, _)| *k).collect();
            for s in slots {
                self.slot_busy.remove(s);
                if let Some(q) = self.slot_next.remove(s) {
                    self.send_slot(s, &q);
                }
            }
        }
    }

    fn send_slot(&mut self, slot: &'static str, q: &serde_json::Value) {
        let key = q.to_string();
        if self.cache.contains_key(&key) {
            return;
        }
        if let Some(b) = &mut self.bridge {
            if let Ok(id) = b.send(q.clone()) {
                self.inflight.insert(id, key.clone());
                self.asked.insert(key);
                self.slot_busy.insert(slot, id);
            }
        }
    }

    /// Asks for `q` unless cached or on its way.
    pub fn want(&mut self, q: &serde_json::Value) {
        let key = q.to_string();
        if self.cache.contains_key(&key) || self.asked.contains(&key) {
            return;
        }
        if let Some(b) = &mut self.bridge {
            if let Ok(id) = b.send(q.clone()) {
                self.inflight.insert(id, key.clone());
                self.asked.insert(key);
            }
        }
    }

    /// The draw list for `q` if it's in, else the last one shown in this `slot` (no flicker while it's fetched).
    pub fn get(&mut self, slot: &'static str, q: &serde_json::Value) -> Option<Arc<DrawList>> {
        let key = q.to_string();
        if !self.cache.contains_key(&key) && !self.asked.contains(&key) {
            if self.slot_busy.contains_key(slot) {
                self.slot_next.insert(slot, q.clone());
            } else {
                self.send_slot(slot, q);
            }
        }
        match self.cache.get(&key) {
            Some(l) => {
                self.last.insert(slot, l.clone());
                Some(l.clone())
            }
            None => self.last.get(slot).cloned(),
        }
    }

    /// Blocking fetch into the cache (headless).
    pub fn fetch_sync(&mut self, q: &serde_json::Value) -> Option<Arc<DrawList>> {
        let key = q.to_string();
        if let Some(l) = self.cache.get(&key) {
            return Some(l.clone());
        }
        let l = Arc::new(self.bridge.as_mut()?.request_sync(q.clone()).map_err(|e| log::warn!("HUD: {e}")).ok()?);
        self.cache.insert(key, l.clone());
        Some(l)
    }
}

// ---------------------------------------------------------------- renderer

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawU {
    view: [f32; 4],
    color0: [f32; 4],
    color1: [f32; 4],
    color2: [f32; 4],
    tint: [f32; 4],
    xf0: [f32; 4],
    xf1: [f32; 4],
    xf2: [f32; 4],
    map0: [f32; 4],
    map1: [f32; 4],
    /// x type (0 text, 1 image), y flags, z has mask, w clip0
    mode: [u32; 4],
    /// x blend, y premul, z desat, w thicken
    p0: [f32; 4],
    /// x outline, y text blur inverse
    p1: [f32; 4],
    pad: [f32; 12],
}
const DRAW_STRIDE: u64 = 256;
const _: () = assert!(std::mem::size_of::<DrawU>() == 256);

#[derive(Deserialize)]
struct Glyph {
    a: f32,
    #[serde(default)]
    b: Option<[f32; 4]>,
    #[serde(default)]
    r: Option<[f32; 4]>,
}

#[derive(Deserialize)]
struct FontMeta {
    em: f32,
    #[serde(rename = "sheetW")]
    sheet_w: f32,
    #[serde(rename = "sheetH")]
    sheet_h: f32,
    glyphs: HashMap<String, Glyph>,
}

struct Font {
    meta: FontMeta,
    view: Arc<wgpu::TextureView>,
}

pub struct Renderer {
    pipe: wgpu::RenderPipeline,
    ub: wgpu::Buffer,
    ub_cap: u64,
    ub_group: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    /// (texture, texture) -> its bind group, reused across frames
    tex_groups: HashMap<(wgpu::TextureView, wgpu::TextureView), wgpu::BindGroup>,
    ub_layout: wgpu::BindGroupLayout,
    samp: wgpu::Sampler,
    white: Arc<wgpu::TextureView>,
    textures: HashMap<String, Option<Arc<wgpu::TextureView>>>,
    fonts: HashMap<u32, Option<Font>>,
    fonts_dir: Option<PathBuf>,
    vbuf: wgpu::Buffer,
    vcap: u64,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, fonts_dir: Option<PathBuf>) -> Renderer {
        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("rui"), source: wgpu::ShaderSource::Wgsl(include_str!("rui.wgsl").into()) });
        let ub_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rui draw"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(DRAW_STRIDE) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let tex = |b: u32| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("rui tex"), entries: &[tex(0), tex(1)] });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("rui"), bind_group_layouts: &[Some(&ub_layout), Some(&tex_layout)], immediate_size: 0 });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
        // premultiplied: ONE / ONE_MINUS_SRC_ALPHA, like the kit (and the game's UI blend)
        let blend = wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::OVER };
        let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rui"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &sh, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 24, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &sh, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None,
            cache: None,
        });
        let samp = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("rui"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let ub_cap = DRAW_STRIDE * 256;
        let ub = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rui draws"), size: ub_cap, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let ub_group = Self::ub_group(device, &ub_layout, &ub, &samp);
        let white = Arc::new(
            device
                .create_texture_with_data(queue, &wgpu::TextureDescriptor { label: Some("rui white"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8UnormSrgb, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }, wgpu::util::TextureDataOrder::LayerMajor, &[255; 4])
                .create_view(&Default::default()),
        );
        let vcap = 24 * 6 * 4096;
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rui verts"), size: vcap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Renderer { pipe, ub, ub_cap, ub_group, tex_layout, tex_groups: HashMap::new(), ub_layout, samp, white, textures: HashMap::new(), fonts: HashMap::new(), fonts_dir, vbuf, vcap }
    }

    fn ub_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, ub: &wgpu::Buffer, samp: &wgpu::Sampler) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rui draws"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: ub, offset: 0, size: wgpu::BufferSize::new(DRAW_STRIDE) }) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(samp) },
            ],
        })
    }

    fn texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, file: &str, srgb: bool) -> Option<Arc<wgpu::TextureView>> {
        if let Some(t) = self.textures.get(file) {
            return t.clone();
        }
        let t = image::open(file).ok().map(|i| {
            let i = i.to_rgba8();
            let (w, h) = i.dimensions();
            let fmt = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
            Arc::new(
                device
                    .create_texture_with_data(queue, &wgpu::TextureDescriptor { label: Some("rui image"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: fmt, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }, wgpu::util::TextureDataOrder::LayerMajor, i.as_raw())
                    .create_view(&Default::default()),
            )
        });
        if t.is_none() {
            log::warn!("HUD: missing image {file}");
        }
        self.textures.insert(file.to_string(), t.clone());
        t
    }

    fn font(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: u32) -> bool {
        if !self.fonts.contains_key(&id) {
            let f = self.fonts_dir.clone().and_then(|d| {
                let meta: FontMeta = serde_json::from_str(&std::fs::read_to_string(d.join(format!("sdf-{id}.json"))).ok()?).ok()?;
                let view = self.texture(device, queue, &d.join(format!("sdf-{id}.png")).to_string_lossy(), false)?;
                Some(Font { meta, view })
            });
            self.fonts.insert(id, f);
        }
        self.fonts.get(&id).is_some_and(|f| f.is_some())
    }

    /// Draws `lists` over `target` (`size` = its pixels; the lists were laid out for that screen). `backdrop` = the
    /// blurred frame (the engine's code texture -4, the frosted plate backing).
    pub fn draw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: [u32; 2], lists: &[&DrawList], backdrop: Option<&wgpu::TextureView>) {
        let (sw, sh) = (size[0] as f32, size[1] as f32);
        let mut verts: Vec<f32> = Vec::new();
        // (vertex range, uniform, tex0, tex1, scissor)
        struct Call {
            v: std::ops::Range<u32>,
            u: DrawU,
            t0: TexRef,
            t1: TexRef,
            scissor: Option<[u32; 4]>,
        }
        #[derive(Clone)]
        enum TexRef {
            White,
            View(Arc<wgpu::TextureView>),
            Backdrop,
        }
        let mut calls: Vec<Call> = Vec::new();
        for list in lists {
            if list.hidden {
                continue;
            }
            let [lw, lh] = list.screen;
            let (kx, ky) = (sw / lw.max(1.0), sh / lh.max(1.0));
            for w in &list.widgets {
                let scissor = w.clip_rect.and_then(|r| {
                    let x0 = (r[0] * sw).floor().max(0.0);
                    let y0 = (r[1] * sh).floor().max(0.0);
                    let x1 = (r[2] * sw).ceil().min(sw);
                    let y1 = (r[3] * sh).ceil().min(sh);
                    (x1 > x0 && y1 > y0).then(|| [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32])
                });
                if w.clip_rect.is_some() && scissor.is_none() {
                    continue;
                }
                let st = &w.style;
                let mut u = style_uniform(st, [0.0, 0.0, sw, sh]);
                if w.ty == 1 {
                    let (Some(i0), i1) = (&w.img0, &w.img1) else { continue };
                    let blur0 = i0.code == Some(-4);
                    let blur1 = i1.as_ref().is_some_and(|i| i.code == Some(-4));
                    if (blur0 || blur1) && backdrop.is_none() {
                        continue;
                    }
                    if (i0.code.is_some() && !blur0) || i1.as_ref().is_some_and(|i| i.code.is_some() && !blur1) {
                        continue;
                    }
                    let t0 = if blur0 {
                        TexRef::Backdrop
                    } else if i0.white {
                        TexRef::White
                    } else {
                        match i0.file.as_deref().and_then(|f| self.texture(device, queue, f, true)) {
                            Some(t) => TexRef::View(t),
                            None => continue,
                        }
                    };
                    let t1 = match i1 {
                        None => None,
                        Some(_) if blur1 => Some(TexRef::Backdrop),
                        Some(i) if i.white => Some(TexRef::White),
                        Some(i) => match i.file.as_deref().and_then(|f| self.texture(device, queue, f, true)) {
                            Some(t) => Some(TexRef::View(t)),
                            None => continue,
                        },
                    };
                    u.mode = [1, (w.flags & 0xff) | if i0.white { 4 } else { 0 }, t1.is_some() as u32, 0];
                    let (mins, maxs) = (w.mins.unwrap_or([0.0, 0.0]), w.maxs.unwrap_or([1.0, 1.0]));
                    let (uv0, uv1) = (w.uv_min.unwrap_or([0.0, 0.0]), w.uv_max.unwrap_or([1.0, 1.0]));
                    // engine trim: image0's opaque-content rect in widget space (kit rui.js / RenderFunctions.cpp:161-176)
                    let m0 = i0.meta.as_ref();
                    let trim = m0.map(|m| {
                        let us = [nz(uv1[0] - uv0[0]), nz(uv1[1] - uv0[1])];
                        let mut t = [(m.rect[0] - uv0[0]) / us[0], (m.rect[1] - uv0[1]) / us[1], (m.rect[2] - uv0[0]) / us[0], (m.rect[3] - uv0[1]) / us[1]];
                        if w.flags & 16 != 0 {
                            t[0] = f32::NEG_INFINITY;
                            t[2] = f32::INFINITY;
                        }
                        if w.flags & 32 != 0 {
                            t[1] = f32::NEG_INFINITY;
                            t[3] = f32::INFINITY;
                        }
                        if us[0] < 0.0 {
                            t.swap(0, 2);
                        }
                        if us[1] < 0.0 {
                            t.swap(1, 3);
                        }
                        t
                    });
                    u.map0 = m0.map_or([1.0, 1.0, 0.0, 0.0], |m| m.map);
                    u.map1 = i1.as_ref().and_then(|i| i.meta.as_ref()).map_or([1.0, 1.0, 0.0, 0.0], |m| m.map);
                    let start = verts.len() as u32 / 6;
                    quad(&mut verts, w, uv0, uv1, [0.0, 0.0], [1.0, 1.0], mins, maxs, [lw * kx, lh * ky], trim);
                    let n = verts.len() as u32 / 6;
                    if n == start {
                        continue;
                    }
                    // backdrop slots sample by screen position
                    for k in start as usize..n as usize {
                        let (x, y) = (verts[k * 6], verts[k * 6 + 1]);
                        if blur0 {
                            verts[k * 6 + 2] = x / sw;
                            verts[k * 6 + 3] = y / sh;
                        }
                        if blur1 {
                            verts[k * 6 + 4] = x / sw;
                            verts[k * 6 + 5] = y / sh;
                        }
                    }
                    calls.push(Call { v: start..n, u, t0, t1: t1.unwrap_or(TexRef::White), scissor });
                } else if w.ty == 0 {
                    if !self.font(device, queue, st.font) {
                        continue;
                    }
                    let f = self.fonts[&st.font].as_ref().unwrap();
                    let start = verts.len() as u32 / 6;
                    layout_text(&mut verts, w, f, [lw * kx, lh * ky]);
                    let n = verts.len() as u32 / 6;
                    if n == start {
                        continue;
                    }
                    let size = st.size.max(1e-3);
                    let k = f.meta.em.max(1.0) / size;
                    u.mode = [0, 0, 0, 0];
                    u.p0[3] = k * st.thicken;
                    u.p1 = [k * st.outline, 1.0 / (k * st.blur).max(1e-6), 0.0, 0.0];
                    calls.push(Call { v: start..n, u, t0: TexRef::View(f.view.clone()), t1: TexRef::White, scissor });
                }
            }
        }
        if calls.is_empty() {
            return;
        }
        // upload
        let vbytes = (verts.len() * 4) as u64;
        if vbytes > self.vcap {
            self.vcap = vbytes.next_power_of_two();
            self.vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rui verts"), size: self.vcap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        }
        queue.write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&verts));
        let ubytes = calls.len() as u64 * DRAW_STRIDE;
        if ubytes > self.ub_cap {
            self.ub_cap = ubytes.next_power_of_two();
            self.ub = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rui draws"), size: self.ub_cap, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            self.ub_group = Self::ub_group(device, &self.ub_layout, &self.ub, &self.samp);
        }
        let us: Vec<DrawU> = calls.iter().map(|c| c.u).collect();
        queue.write_buffer(&self.ub, 0, bytemuck::cast_slice(&us));
        let white = self.white.clone();
        let view_of = |t: &TexRef| -> wgpu::TextureView {
            match t {
                TexRef::White => (*white).clone(),
                TexRef::View(v) => (**v).clone(),
                TexRef::Backdrop => backdrop.unwrap().clone(),
            }
        };
        // texture pair -> bind group, kept across frames (creating ~40 a frame cost ~0.2 ms of CPU, bench.rs)
        if self.tex_groups.len() > 512 {
            self.tex_groups.clear();
        }
        let groups: Vec<wgpu::BindGroup> = calls
            .iter()
            .map(|c| {
                let key = (view_of(&c.t0), view_of(&c.t1));
                self.tex_groups
                    .entry(key)
                    .or_insert_with_key(|(a, b)| {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("rui tex"),
                            layout: &self.tex_layout,
                            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(a) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(b) }],
                        })
                    })
                    .clone()
            })
            .collect();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rui") });
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.pipe);
            rp.set_vertex_buffer(0, self.vbuf.slice(..));
            for (i, c) in calls.iter().enumerate() {
                match c.scissor {
                    Some([x, y, w, h]) => rp.set_scissor_rect(x, y, w.min(size[0] - x), h.min(size[1] - y)),
                    None => rp.set_scissor_rect(0, 0, size[0], size[1]),
                }
                rp.set_bind_group(0, &self.ub_group, &[(i as u64 * DRAW_STRIDE) as u32]);
                rp.set_bind_group(1, &groups[i], &[]);
                rp.draw(c.v.clone(), 0..1);
            }
        }
        queue.submit([enc.finish()]);
    }
}

fn nz(x: f32) -> f32 {
    if x == 0.0 { 1.0 } else { x }
}

/// The style's colours + the hue / lightness transform (kit rui.js colorXform: hue as a rotation about the grey axis,
/// lightness scales toward black or white) + blend terms.
fn style_uniform(st: &Style, view: [f32; 4]) -> DrawU {
    let a = st.hue * std::f32::consts::TAU;
    let (c, s) = (a.cos(), a.sin());
    let k: f32 = 1.0 / 3.0;
    let q = k.sqrt();
    let m = |i: usize, j: usize| if i == j { c + (1.0 - c) * k } else { (1.0 - c) * k + if (j + 3 - i) % 3 == 1 { -1.0 } else { 1.0 } * s * q };
    let scale = if st.light < 0.0 { 1.0 + st.light } else { 1.0 - st.light };
    let offset = st.light.max(0.0);
    let row = |i: usize| [m(i, 0) * scale, m(i, 1) * scale, m(i, 2) * scale, offset];
    DrawU {
        view,
        color0: st.color[0],
        color1: st.color[1],
        color2: st.color[2],
        tint: st.tint,
        xf0: row(0),
        xf1: row(1),
        xf2: row(2),
        map0: [1.0, 1.0, 0.0, 0.0],
        map1: [1.0, 1.0, 0.0, 0.0],
        mode: [0; 4],
        p0: [st.blend, st.premul, 1.0 - st.sat, 0.0],
        p1: [0.0; 4],
        pad: [0.0; 12],
    }
}

/// A widget quad (kit rui.js quad()): corners in widget uv through the transform, image uv u0..u1, mask uv v0..v1,
/// clipped to mins..maxs and the content trim.
#[allow(clippy::too_many_arguments)]
fn quad(out: &mut Vec<f32>, w: &Widget, u0: [f32; 2], u1: [f32; 2], v0: [f32; 2], v1: [f32; 2], mins: [f32; 2], maxs: [f32; 2], screen: [f32; 2], trim: Option<[f32; 4]>) {
    let [sw, sh] = screen;
    let p = |u: f32, v: f32| [(w.pos[0] + u * w.grad[0] + v * w.grad[2]) * sw, (w.pos[1] + u * w.grad[1] + v * w.grad[3]) * sh];
    let (mut a0, mut b0, mut a1, mut b1) = (mins[0], mins[1], maxs[0], maxs[1]);
    if let Some(t) = trim {
        a0 = a0.max(t[0]);
        b0 = b0.max(t[1]);
        a1 = a1.min(t[2]);
        b1 = b1.min(t[3]);
    }
    if a1 <= a0 || b1 <= b0 {
        return;
    }
    for (u, v) in [(a0, b0), (a1, b0), (a1, b1), (a0, b0), (a1, b1), (a0, b1)] {
        let [x, y] = p(u, v);
        let tu = u0[0] + u * (u1[0] - u0[0]);
        let tv = u0[1] + v * (u1[1] - u0[1]);
        let mu = v0[0] + (u - mins[0]) / nz(maxs[0] - mins[0]) * (v1[0] - v0[0]);
        let mv = v0[1] + (v - mins[1]) / nz(maxs[1] - mins[1]) * (v1[1] - v0[1]);
        out.extend_from_slice(&[x, y, tu, tv, mu, mv]);
    }
}

/// Text glyph quads (kit rui.js drawText): pen layout in the widget's box, baseline at the box middle + 0.8 em.
fn layout_text(out: &mut Vec<f32>, w: &Widget, f: &Font, screen: [f32; 2]) {
    let st = &w.style;
    let size = st.size;
    let aspect = if st.aspect == 0.0 { 1.0 } else { st.aspect };
    let raw = w.text.as_deref().unwrap_or("");
    // inline syntax: `0-`3 style switches, %% = %, %token% (key binds / inline images) dropped
    let mut text = String::new();
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '`' if it.peek().is_some_and(|d| ('0'..='3').contains(d)) => {
                it.next();
            }
            '%' => {
                if it.peek() == Some(&'%') {
                    it.next();
                    text.push('%');
                } else {
                    for d in it.by_ref() {
                        if d == '%' {
                            break;
                        }
                    }
                }
            }
            c => text.push(c),
        }
    }
    if text.trim().is_empty() {
        return;
    }
    let mut pen = 0.0;
    let mut glyphs = Vec::new();
    for ch in text.chars() {
        match f.meta.glyphs.get(&(ch as u32).to_string()) {
            None => pen += 0.3 * size * aspect,
            Some(g) => {
                if let (Some(b), Some(r)) = (g.b, g.r) {
                    glyphs.push((pen, b, r));
                }
                pen += (g.a + st.kerning) * size * aspect;
            }
        }
    }
    let (bw, bh) = (w.size[0], w.size[1]);
    if bw == 0.0 || bh == 0.0 {
        return;
    }
    let ox = (bw - pen) * w.align;
    let base = (bh - size) / 2.0 + size * 0.8 + st.baseline;
    let [sw, sh] = screen;
    let to = |lx: f32, ly: f32| [(w.pos[0] + (lx / bw) * w.grad[0] + (ly / bh) * w.grad[2]) * sw, (w.pos[1] + (lx / bw) * w.grad[1] + (ly / bh) * w.grad[3]) * sh];
    let (iw, ih) = (f.meta.sheet_w, f.meta.sheet_h);
    for (x0, b, r) in glyphs {
        let (l, rr) = (ox + x0 + b[0] * size * aspect, ox + x0 + b[2] * size * aspect);
        let (t, bb) = (base + b[1] * size, base + b[3] * size);
        let p = [to(l, t), to(rr, t), to(rr, bb), to(l, t), to(rr, bb), to(l, bb)];
        let uv = [[r[0], r[1]], [r[0] + r[2], r[1]], [r[0] + r[2], r[1] + r[3]], [r[0], r[1]], [r[0] + r[2], r[1] + r[3]], [r[0], r[1] + r[3]]];
        for k in 0..6 {
            out.extend_from_slice(&[p[k][0], p[k][1], uv[k][0] / iw, uv[k][1] / ih, 0.0, 0.0]);
        }
    }
}

/// Where the RUI assets are, for a quick check before starting the bridge.
pub fn available() -> bool {
    Path::new("C:/Code/apex/apex-content-kit/tools/rui-host/bin/host.exe").is_file()
}
