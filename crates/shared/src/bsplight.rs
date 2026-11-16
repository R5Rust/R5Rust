//! Raw lighting lumps of the map's rBSP (v51), copied next to world.glb as `bsp/<lump hex>.bin`.
//! Everything here is lossless: the client decodes in the shader, the way the game does.
//!
//! Lightmap atlas layout = r5-web-client's export-map: pages side by side in header order, top-aligned,
//! so world.glb's TEXCOORD_1 indexes these atlases directly.

use std::path::Path;

pub struct LmHeader {
    pub kind: u8,
    pub compressed_kind: u8,
    pub width: u32,
    pub height: u32,
}

/// One RGBA8 atlas.
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Atlas {
    fn new(width: u32, height: u32, bpp: usize) -> Self {
        Atlas { width, height, rgba: vec![0; width as usize * height as usize * bpp] }
    }
}

/// Source-style dworldlight_t as Respawn ships it (112 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WorldLight {
    pub origin: [f32; 3],
    /// colour * brightness, linear
    pub intensity: [f32; 3],
    pub normal: [f32; 3],
    pub shadow_offset: [f32; 3],
    pub unk_f: f32,
    /// 1 point, 2 spot, 3 sun (skylight), 5 sky ambient
    pub emit_type: u32,
    pub style: u32,
    pub stopdot: f32,
    pub stopdot2: f32,
    pub exponent: f32,
    pub radius: f32,
    pub constant_attn: f32,
    pub linear_attn: f32,
    pub quadratic_attn: f32,
    pub flags: u32,
    pub texinfo: u32,
    pub owner: u32,
    pub extra: [u32; 3],
}

/// 44-byte v51 light probe.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LightProbe {
    pub ambient_sh: [i16; 12],
    pub sky_dir_sun_vis: [i16; 4],
    pub static_light_weights: [u8; 4],
    pub static_light_indexes: [i16; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ProbeTreeNode {
    pub flags: u32,
    pub data: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ProbeRef {
    pub origin: [f32; 3],
    pub probe: u32,
    pub cube_index: u16,
    pub unknown: u16,
}

pub struct Cubemap {
    pub origin: [i32; 3],
    pub guid: u32,
}

pub struct BspLighting {
    pub headers: Vec<LmHeader>,
    /// image 0 of every sky page (RGBM baked light)
    pub sky0: Atlas,
    /// image 1 of every sky page
    pub sky1: Atlas,
    /// raw lump 0x69: per page, R32 light IDs (w*h) then BC5 visibility (w x 2h)
    pub rtl_raw: Vec<u8>,
    pub rtl_index: Vec<u16>,
    pub lights: Vec<WorldLight>,
    pub probes: Vec<LightProbe>,
    pub probe_tree: Vec<ProbeTreeNode>,
    pub probe_refs: Vec<ProbeRef>,
    pub cubemaps: Vec<Cubemap>,
}

fn lump(dir: &Path, id: u16) -> Vec<u8> {
    std::fs::read(dir.join(format!("{id:04X}.bin"))).unwrap_or_default()
}

fn cast_vec<T: bytemuck::Pod>(b: &[u8]) -> Vec<T> {
    let sz = std::mem::size_of::<T>();
    b.chunks_exact(sz).map(bytemuck::pod_read_unaligned::<T>).collect()
}

/// Copies a `w x h` image with `bpp` bytes/texel and `src_stride` bytes/row into the atlas at x0.
fn blit(dst: &mut Atlas, x0: u32, src: &[u8], w: u32, h: u32, src_stride: usize, bpp: usize) {
    let dw = dst.width as usize;
    for y in 0..h as usize {
        let s = &src[y * src_stride..y * src_stride + w as usize * bpp];
        let d = (y * dw + x0 as usize) * bpp;
        dst.rgba[d..d + w as usize * bpp].copy_from_slice(s);
    }
}

impl BspLighting {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let hb = lump(dir, 0x53);
        if hb.is_empty() {
            return Err(format!("no lightmap headers in {}", dir.display()));
        }
        let headers: Vec<LmHeader> = hb
            .chunks_exact(8)
            .map(|c| LmHeader { kind: c[0], compressed_kind: c[1], width: u16::from_le_bytes([c[4], c[5]]) as u32, height: u16::from_le_bytes([c[6], c[7]]) as u32 })
            .collect();
        let aw: u32 = headers.iter().map(|h| h.width).sum();
        let ah: u32 = headers.iter().map(|h| h.height).max().unwrap_or(1);

        let sky = lump(dir, 0x62);
        let rtl = lump(dir, 0x69);
        let mut sky0 = Atlas::new(aw, ah, 4);
        let mut sky1 = Atlas::new(aw, ah, 4);
        let (mut at_s, mut x) = (0usize, 0u32);
        for h in &headers {
            if h.kind != 1 && h.kind != 10 {
                return Err(format!("lightmap page type {} not supported", h.kind));
            }
            let (w, hh) = (h.width as usize, h.height as usize);
            let n = w * hh * 4;
            if at_s + 2 * n <= sky.len() {
                blit(&mut sky0, x, &sky[at_s..at_s + n], h.width, h.height, w * 4, 4);
                blit(&mut sky1, x, &sky[at_s + n..at_s + 2 * n], h.width, h.height, w * 4, 4);
            }
            at_s += 2 * n;
            x += h.width;
        }

        let rtl_index = cast_vec::<u16>(&lump(dir, 0x7A));
        let lights = cast_vec::<WorldLight>(&lump(dir, 0x36));
        let probes = cast_vec::<LightProbe>(&lump(dir, 0x65));
        let probe_tree = cast_vec::<ProbeTreeNode>(&lump(dir, 0x67));
        let probe_refs = cast_vec::<ProbeRef>(&lump(dir, 0x68));
        let cubemaps = lump(dir, 0x2A)
            .chunks_exact(16)
            .map(|c| {
                let i = |k: usize| i32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
                Cubemap { origin: [i(0), i(4), i(8)], guid: i(12) as u32 }
            })
            .collect();
        Ok(BspLighting { headers, sky0, sky1, rtl_raw: rtl, rtl_index, lights, probes, probe_tree, probe_refs, cubemaps })
    }

    /// Probe references whose k-d tree leaf contains `p` (the game then picks/blends among them).
    pub fn probe_leaf(&self, p: [f32; 3]) -> &[ProbeRef] {
        let mut i = 0usize;
        for _ in 0..64 {
            let Some(n) = self.probe_tree.get(i) else { return &[] };
            let kind = n.flags & 3;
            let first = (n.flags >> 2) as usize;
            if kind == 3 {
                let end = (first + n.data as usize).min(self.probe_refs.len());
                return &self.probe_refs[first.min(end)..end];
            }
            let split = f32::from_bits(n.data);
            i = first + if p[kind as usize] > split { 1 } else { 0 };
        }
        &[]
    }

    /// Nearest probe in the leaf containing `p` (index into `probes`), if any.
    pub fn nearest_probe(&self, p: [f32; 3]) -> Option<u32> {
        let d2 = |r: &ProbeRef| (0..3).map(|k| (r.origin[k] - p[k]).powi(2)).sum::<f32>();
        self.probe_leaf(p).iter().min_by(|a, b| d2(a).total_cmp(&d2(b))).map(|r| r.probe)
    }
}

const _: () = assert!(std::mem::size_of::<WorldLight>() == 112);
const _: () = assert!(std::mem::size_of::<LightProbe>() == 44);
const _: () = assert!(std::mem::size_of::<ProbeRef>() == 20);
