//! Material textures from the extracted assets, block-compressed (BC1/4/5/7) with a full mip chain and cached on
//! disk next to the assets, so a second launch just reads + uploads. Keeps the whole map's
//! albedo/normal/gloss/spec/AO/emissive set in a few hundred MB of VRAM.

use image::RgbaImage;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Albedo,
    Normal,
    Gloss,
    Spec,
    Ao,
    Emissive,
    /// detail albedo: RGB around 0.5 = neutral, read linearly (the shader overlays it in gamma-2 space)
    Detail,
}

impl Kind {
    pub fn format(self) -> wgpu::TextureFormat {
        use wgpu::TextureFormat as F;
        match self {
            Kind::Albedo => F::Bc7RgbaUnormSrgb,
            Kind::Normal => F::Bc5RgUnorm,
            Kind::Gloss | Kind::Ao => F::Bc4RUnorm,
            Kind::Spec | Kind::Emissive => F::Bc1RgbaUnormSrgb,
            Kind::Detail => F::Bc1RgbaUnorm,
        }
    }
    fn block_bytes(self) -> usize {
        match self {
            Kind::Albedo | Kind::Normal => 16,
            _ => 8,
        }
    }
    fn id(self) -> u32 {
        self as u32
    }
}

pub struct Encoded {
    pub width: u32,
    pub height: u32,
    pub kind: Kind,
    pub mips: Vec<Vec<u8>>,
}

fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn cache_path(cache: &Path, rel: &str, kind: Kind, cap: u32) -> PathBuf {
    cache.join(format!("{:016x}.r5tc", hash(&format!("{rel}|{}|{cap}|{VERSION}", kind.id()))))
}

fn read_cache(p: &Path, kind: Kind) -> Option<Encoded> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).ok()?;
    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    if b.len() < 20 || &b[0..4] != b"R5TC" || u(16) != kind.id() {
        return None;
    }
    let (w, h, n) = (u(4), u(8), u(12) as usize);
    let mut at = 20;
    let mut mips = Vec::with_capacity(n);
    for _ in 0..n {
        let len = u(at) as usize;
        at += 4;
        mips.push(b.get(at..at + len)?.to_vec());
        at += len;
    }
    Some(Encoded { width: w, height: h, kind, mips })
}

fn write_cache(p: &Path, e: &Encoded) {
    let mut b = Vec::new();
    b.extend_from_slice(b"R5TC");
    for v in [e.width, e.height, e.mips.len() as u32, e.kind.id()] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for m in &e.mips {
        b.extend_from_slice(&(m.len() as u32).to_le_bytes());
        b.extend_from_slice(m);
    }
    let tmp = p.with_extension("tmp");
    if std::fs::File::create(&tmp).and_then(|mut f| f.write_all(&b)).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
}

/// Pads to a multiple of 4 (edge-clamped) so the block encoder sees whole blocks.
fn pad4(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let (pw, ph) = (w.div_ceil(4) * 4, h.div_ceil(4) * 4);
    if pw == w && ph == h {
        return img.clone();
    }
    RgbaImage::from_fn(pw, ph, |x, y| *img.get_pixel(x.min(w - 1), y.min(h - 1)))
}

fn encode_level(img: &RgbaImage, kind: Kind) -> Vec<u8> {
    let p = pad4(img);
    let (w, h) = p.dimensions();
    let raw = p.as_raw();
    match kind {
        Kind::Albedo => intel_tex_2::bc7::compress_blocks(&intel_tex_2::bc7::alpha_ultra_fast_settings(), &intel_tex_2::RgbaSurface { data: raw, width: w, height: h, stride: w * 4 }),
        Kind::Spec | Kind::Emissive | Kind::Detail => intel_tex_2::bc1::compress_blocks(&intel_tex_2::RgbaSurface { data: raw, width: w, height: h, stride: w * 4 }),
        Kind::Normal => {
            let rg: Vec<u8> = raw.chunks_exact(4).flat_map(|c| [c[0], c[1]]).collect();
            intel_tex_2::bc5::compress_blocks(&intel_tex_2::RgSurface { data: &rg, width: w, height: h, stride: w * 2 })
        }
        Kind::Gloss | Kind::Ao => {
            let r: Vec<u8> = raw.chunks_exact(4).map(|c| c[0]).collect();
            intel_tex_2::bc4::compress_blocks(&intel_tex_2::RSurface { data: &r, width: w, height: h, stride: w })
        }
    }
}

/// Loads `<root>/<rel>` as `kind`, from the cache if present, else decodes + encodes + caches it.
pub fn load(root: &Path, cache: &Path, rel: &str, kind: Kind, cap: u32) -> Option<Encoded> {
    // key includes the source's size + mtime, so a re-exported image is re-encoded
    let stamp = std::fs::metadata(root.join(rel)).ok().map(|m| format!("{}:{:?}", m.len(), m.modified().ok())).unwrap_or_default();
    let cp = cache_path(cache, &format!("{rel}|{stamp}"), kind, cap);
    if let Some(e) = read_cache(&cp, kind) {
        return Some(e);
    }
    let img = match image::open(root.join(rel)) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            log::warn!("texture {rel}: {e}");
            return None;
        }
    };
    let (w, h) = img.dimensions();
    let m = w.max(h);
    let mut cur = if m > cap {
        let s = cap as f32 / m as f32;
        image::imageops::resize(&img, ((w as f32 * s) as u32).max(4), ((h as f32 * s) as u32).max(4), image::imageops::FilterType::Triangle)
    } else {
        img
    };
    // BC needs a mip-0 size divisible by 4
    let (w0, h0) = cur.dimensions();
    if w0 % 4 != 0 || h0 % 4 != 0 {
        cur = image::imageops::resize(&cur, w0.div_ceil(4) * 4, h0.div_ceil(4) * 4, image::imageops::FilterType::Triangle);
    }
    let (w0, h0) = cur.dimensions();
    let mut mips = vec![encode_level(&cur, kind)];
    loop {
        let (w, h) = cur.dimensions();
        if w == 1 && h == 1 {
            break;
        }
        cur = image::imageops::resize(&cur, (w / 2).max(1), (h / 2).max(1), image::imageops::FilterType::Triangle);
        mips.push(encode_level(&cur, kind));
    }
    let e = Encoded { width: w0, height: h0, kind, mips };
    let _ = std::fs::create_dir_all(cache);
    write_cache(&cp, &e);
    Some(e)
}

/// How many top mips to drop for Video > Texture Quality (`picmip` = (wanted, floor)): at most `wanted`, never taking
/// the longest side below `floor`, and only onto a level BC can start at (both sides a multiple of 4).
pub fn base_level(e: &Encoded, (wanted, floor): (u32, u32)) -> usize {
    let mut s = 0usize;
    while (s as u32) < wanted && s + 1 < e.mips.len() {
        let (w, h) = (e.width >> (s + 1), e.height >> (s + 1));
        if w.max(h) < floor || w == 0 || h == 0 || w % 4 != 0 || h % 4 != 0 {
            break;
        }
        s += 1;
    }
    s
}

/// Uploads the chain from mip `skip` down. Returns the texture and its size in bytes.
pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, e: &Encoded, skip: usize) -> (wgpu::Texture, u64) {
    let skip = skip.min(e.mips.len() - 1);
    let (bw0, bh0) = (e.width >> skip, e.height >> skip);
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("material"),
        size: wgpu::Extent3d { width: bw0, height: bh0, depth_or_array_layers: 1 },
        mip_level_count: (e.mips.len() - skip) as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: e.kind.format(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut bytes = 0u64;
    for (lvl, data) in e.mips[skip..].iter().enumerate() {
        bytes += data.len() as u64;
        let w = (bw0 >> lvl).max(1);
        let h = (bh0 >> lvl).max(1);
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: lvl as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bw * e.kind.block_bytes() as u32), rows_per_image: Some(bh) },
            wgpu::Extent3d { width: bw * 4, height: bh * 4, depth_or_array_layers: 1 },
        );
    }
    (tex, bytes)
}
