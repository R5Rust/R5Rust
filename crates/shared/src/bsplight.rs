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
    /// image 0 of every sky page, RGBA16F: baked light as RGBM (rgb * (a * 3.984375 + 0.015625), what the world
    /// shader decodes); type-11 HDR pages store the raw value with a = 0.2470588 (RGBM scale 1.0)
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

/// BC7 blocks (`w x h`, both multiples of 4) to RGBA8.
pub fn bc7_to_rgba(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (bw, bh) = (w / 4, h / 4);
    for by in 0..bh {
        for bx in 0..bw {
            let i = (by * bw + bx) * 16;
            let Some(block) = src.get(i..i + 16) else { return out };
            let at = (by * 4 * w + bx * 4) * 4;
            bcdec_rs::bc7(block, &mut out[at..], w * 4);
        }
    }
    out
}

fn cast_vec<T: bytemuck::Pod>(b: &[u8]) -> Vec<T> {
    let sz = std::mem::size_of::<T>();
    b.chunks_exact(sz).map(bytemuck::pod_read_unaligned::<T>).collect()
}

/// Copies a `w x h` image with `bpp` bytes/texel and `src_stride` bytes/row into the atlas at x0.
/// f32 -> IEEE half bits (round to nearest even; no NaN inputs here).
pub fn f16_bits(f: f32) -> u16 {
    let x = f.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let e = ((x >> 23) & 0xFF) as i32 - 127 + 15;
    let m = x & 0x7F_FFFF;
    if e >= 31 {
        return sign | 0x7BFF; // clamp to the largest finite half
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = m | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let mid = 1 << (shift - 1);
        let r = half + ((rem > mid || (rem == mid && half & 1 == 1)) as u32);
        return sign | r as u16;
    }
    let half = ((e as u32) << 10) | (m >> 13);
    let rem = m & 0x1FFF;
    let r = half + ((rem > 0x1000 || (rem == 0x1000 && half & 1 == 1)) as u32);
    sign | r as u16
}

/// RGBA8 texels -> RGBA16F texels (x / 255).
fn rgba8_to_f16(src: &[u8]) -> Vec<u8> {
    src.iter().flat_map(|&b| f16_bits(b as f32 / 255.0).to_le_bytes()).collect()
}

/// RGB9E5 (9-bit mantissas, shared 5-bit exponent, bias 15) -> RGBA16F with a = 0.2470588: the world shader's RGBM
/// decode (a * 3.984375 + 0.015625) is then x1, so it reads the HDR value raw, like S30 does for type 9/10/11 pages.
fn rgb9e5_to_f16(src: &[u8]) -> Vec<u8> {
    let a = f16_bits(0.984375 / 3.984375).to_le_bytes();
    src.chunks_exact(4)
        .flat_map(|c| {
            let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            let s = 2f32.powi((v >> 27) as i32 - 15 - 9);
            let r = f16_bits((v & 0x1FF) as f32 * s).to_le_bytes();
            let g = f16_bits(((v >> 9) & 0x1FF) as f32 * s).to_le_bytes();
            let b = f16_bits(((v >> 18) & 0x1FF) as f32 * s).to_le_bytes();
            [r[0], r[1], g[0], g[1], b[0], b[1], a[0], a[1]]
        })
        .collect()
}

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
        let comp = lump(dir, 0x61);
        let mut at_c = 0usize;
        let rtl = lump(dir, 0x69);
        let mut sky0 = Atlas::new(aw, ah, 8);
        let mut sky1 = Atlas::new(aw, ah, 4);
        let (mut at_s, mut x) = (0usize, 0u32);
        for h in &headers {
            if h.kind == 11 {
                // Retail / S30 type 11 (R5Flowstate s30-port-kit docs/10-lightmaps.md): lump 0x62 holds per page plane 0
                // = RGB9E5 HDR baked light (4 B/texel) and plane 1 = RGBA8 (sky direction *2-1, sun visibility). The S30
                // engine sets the RGBM branch flag (0x8000) only for page types 1 / 4..8, so the retail world PS reads
                // type 11 raw. Until 2026-11-21 r5rust RGBM-decoded the BC7 copy in lump 0x61 instead: per texel 0.16-2.2x
                // the exact value (median 0.52). 0x61 stays the fallback when 0x62 is missing.
                let (w, hh) = (h.width as usize, h.height as usize);
                let n = w * hh;
                if at_s + 8 * n <= sky.len() {
                    blit(&mut sky0, x, &rgb9e5_to_f16(&sky[at_s..at_s + 4 * n]), h.width, h.height, w * 8, 8);
                    blit(&mut sky1, x, &sky[at_s + 4 * n..at_s + 8 * n], h.width, h.height, w * 4, 4);
                    at_s += 8 * n;
                } else if at_c + 2 * n <= comp.len() {
                    blit(&mut sky0, x, &rgba8_to_f16(&bc7_to_rgba(&comp[at_c..at_c + n], w, hh)), h.width, h.height, w * 8, 8);
                    blit(&mut sky1, x, &bc7_to_rgba(&comp[at_c + n..at_c + 2 * n], w, hh), h.width, h.height, w * 4, 4);
                }
                at_c += 2 * n;
                x += h.width;
                continue;
            }
            if h.kind != 1 && h.kind != 10 {
                return Err(format!("lightmap page type {} not supported", h.kind));
            }
            let (w, hh) = (h.width as usize, h.height as usize);
            let n = w * hh * 4;
            if at_s + 2 * n <= sky.len() {
                blit(&mut sky0, x, &rgba8_to_f16(&sky[at_s..at_s + n]), h.width, h.height, w * 8, 8);
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
        // retail (BSP v54, lightmap pages type 11) cubemap records are 24 bytes: origin i32x3, u32, u64 (S21: 16)
        let cube_rec = if headers.iter().any(|h| h.kind == 11) { 24 } else { 16 };
        let cubemaps = lump(dir, 0x2A)
            .chunks_exact(cube_rec)
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
    pub fn nearest_probe(&self, p: [f32; 3]) -> Option<ProbeRef> {
        let d2 = |r: &ProbeRef| (0..3).map(|k| (r.origin[k] - p[k]).powi(2)).sum::<f32>();
        self.probe_leaf(p).iter().min_by(|a, b| d2(a).total_cmp(&d2(b))).copied()
    }

    /// Index of the cubemap closest to `p` (stand-in for the world vertex CUBEMAP_ID the export dropped).
    pub fn nearest_cubemap(&self, p: [f32; 3]) -> u32 {
        let mut best = (f32::MAX, 0u32);
        for (i, c) in self.cubemaps.iter().enumerate() {
            let d = (0..3).map(|k| (c.origin[k] as f32 - p[k]).powi(2)).sum::<f32>();
            if d < best.0 {
                best = (d, i as u32);
            }
        }
        best.1
    }
}

const _: () = assert!(std::mem::size_of::<WorldLight>() == 112);
const _: () = assert!(std::mem::size_of::<LightProbe>() == 44);
const _: () = assert!(std::mem::size_of::<ProbeRef>() == 20);
