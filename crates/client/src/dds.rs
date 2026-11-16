//! Minimal DDS reader for the game's block-compressed textures (DX10 header, any array size, full mip chains).

pub struct Dds {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub layers: u32,
    pub format: wgpu::TextureFormat,
    /// layer-major: every mip of layer 0, then layer 1, ...
    pub data: Vec<u8>,
}

fn block_bytes(f: wgpu::TextureFormat) -> u32 {
    use wgpu::TextureFormat as F;
    match f {
        F::Bc1RgbaUnorm | F::Bc1RgbaUnormSrgb | F::Bc4RUnorm => 8,
        _ => 16,
    }
}

pub fn load(path: &std::path::Path) -> Result<Dds, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    if b.len() < 148 || &b[0..4] != b"DDS " || &b[84..88] != b"DX10" {
        return Err(format!("{}: not a DX10 DDS", path.display()));
    }
    let (height, width, mips) = (u(12), u(16), u(28).max(1));
    let (dxgi, layers) = (u(128), u(140).max(1));
    use wgpu::TextureFormat as F;
    let format = match dxgi {
        95 => F::Bc6hRgbUfloat,
        96 => F::Bc6hRgbFloat,
        98 => F::Bc7RgbaUnorm,
        99 => F::Bc7RgbaUnormSrgb,
        71 => F::Bc1RgbaUnorm,
        72 => F::Bc1RgbaUnormSrgb,
        _ => return Err(format!("{}: DXGI format {dxgi} not handled", path.display())),
    };
    Ok(Dds { width, height, mips, layers, format, data: b[148..].to_vec() })
}

pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, d: &Dds, label: &str) -> wgpu::Texture {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: d.layers },
        mip_level_count: d.mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: d.format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bb = block_bytes(d.format);
    let mut at = 0usize;
    for layer in 0..d.layers {
        for m in 0..d.mips {
            let (w, h) = ((d.width >> m).max(1), (d.height >> m).max(1));
            let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
            let n = (bw * bh * bb) as usize;
            if at + n > d.data.len() {
                log::warn!("{label}: DDS shorter than its header says");
                return tex;
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: m, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
                &d.data[at..at + n],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bw * bb), rows_per_image: Some(bh) },
                wgpu::Extent3d { width: bw * 4, height: bh * 4, depth_or_array_layers: 1 },
            );
            at += n;
        }
    }
    tex
}
