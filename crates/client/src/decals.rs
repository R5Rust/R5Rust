//! Retail bullet-hole decals for impact marks (tools/build_decals.py -> `<assets>/decals/bullets`).
//!
//! A hit's surface (collision.bin v2: surfaceproperties id per triangle, `Trace::surface`) -> its gamematerial letter
//! (the map's surfaces.json) -> the decal family the impact table uses (decals.json: concrete, metal, wood, glass,
//! default) -> one of the family's variants, spun at random. Drawn as quads multiplying the lit surface (decals.wgsl).

use std::path::Path;

/// A GPU instance: (pos.xyz, size), (normal.xyz, alpha), (layer, angle, 0, 0).
pub type Instance = [f32; 12];

pub struct Decals {
    pub pipeline: wgpu::RenderPipeline,
    pub group: wgpu::BindGroup,
    layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    /// surfaceproperties id -> family index (into `families`)
    family_of_surface: [u8; 256],
    /// (layers, size) per family
    families: Vec<(Vec<u32>, f32)>,
    fallback: u8,
}

fn upload_array(device: &wgpu::Device, queue: &wgpu::Queue, img: &image::RgbaImage, size: u32, format: wgpu::TextureFormat, label: &str) -> wgpu::TextureView {
    let count = img.height() / size;
    let mips = 32 - size.leading_zeros();
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: count },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for layer in 0..count {
        let mut cur = image::imageops::crop_imm(img, 0, layer * size, size, size).to_image();
        let mut s = size;
        for mip in 0..mips {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: mip, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
                cur.as_raw(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * s), rows_per_image: Some(s) },
                wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
            );
            if s == 1 {
                break;
            }
            s /= 2;
            cur = image::imageops::resize(&cur, s, s, image::imageops::FilterType::Triangle);
        }
    }
    tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
}

fn make_pipeline(device: &wgpu::Device, pl: &wgpu::PipelineLayout, sh: &wgpu::ShaderModule, hdr: wgpu::TextureFormat, depth: wgpu::TextureFormat, samples: u32) -> wgpu::RenderPipeline {
    let attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
    // multiply: dst * src (the shader returns 1 where the decal doesn't cover); dst alpha kept
    let multiply = wgpu::BlendState {
        color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Dst, dst_factor: wgpu::BlendFactor::Zero, operation: wgpu::BlendOperation::Add },
        alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("decals"),
        layout: Some(pl),
        vertex: wgpu::VertexState {
            module: sh,
            entry_point: Some("vs_decal"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Instance, attributes: &attrs })],
        },
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState { constant: 16, slope_scale: 4.0, clamp: 0.001 },
        }),
        multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
        fragment: Some(wgpu::FragmentState {
            module: sh,
            entry_point: Some("fs_decal"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: hdr, blend: Some(multiply), write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

impl Decals {
    /// `dir` = the bake (col.png, nml.png, decals.json), `surfaces` = the map's surfaces.json.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dir: &Path,
        surfaces: &Path,
        frame_layout: &wgpu::BindGroupLayout,
        hdr: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Result<Decals, String> {
        let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("decals.json")).map_err(|e| format!("decals.json: {e}"))?).map_err(|e| e.to_string())?;
        let size = j["layer_size"].as_u64().unwrap_or(256) as u32;
        let names: Vec<String> = j["families"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let families: Vec<(Vec<u32>, f32)> = names
            .iter()
            .map(|n| {
                let f = &j["families"][n];
                (f["layers"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect()).unwrap_or_default(), f["size"].as_f64().unwrap_or(4.0) as f32)
            })
            .collect();
        let idx = |n: &str| names.iter().position(|x| x == n).map(|i| i as u8);
        let fallback = idx(j["fallback"].as_str().unwrap_or("default")).unwrap_or(0);
        let mut family_of_surface = [fallback; 256];
        match std::fs::read_to_string(surfaces) {
            Ok(s) => {
                let sj: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
                for (id, v) in sj.as_object().into_iter().flatten() {
                    let (Ok(id), Some(gm)) = (id.parse::<usize>(), v[1].as_str()) else { continue };
                    if let (true, Some(f)) = (id < 256, j["gamematerial"][gm].as_str().and_then(idx)) {
                        family_of_surface[id] = f;
                    }
                }
            }
            Err(_) => log::warn!("no {} (re-export collision): every impact uses the default decal", surfaces.display()),
        }

        let col = image::open(dir.join("col.png")).map_err(|e| format!("col.png: {e}"))?.to_rgba8();
        let nml = image::open(dir.join("nml.png")).map_err(|e| format!("nml.png: {e}"))?.to_rgba8();
        let col_v = upload_array(device, queue, &col, size, wgpu::TextureFormat::Rgba8UnormSrgb, "decal colour");
        let nml_v = upload_array(device, queue, &nml, size, wgpu::TextureFormat::Rgba8Unorm, "decal normal");
        let samp = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let tex_entry = |b: u32| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("decals"),
            entries: &[tex_entry(0), tex_entry(1), wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decals"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&col_v) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&nml_v) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&samp) },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("decals"), bind_group_layouts: &[Some(frame_layout), Some(&bgl)], immediate_size: 0 });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("decals"), source: wgpu::ShaderSource::Wgsl(include_str!("decals.wgsl").into()) });
        let pipeline = make_pipeline(device, &layout, &shader, hdr, depth, samples);
        log::info!("decals: {} families, {} layers", families.len(), col.height() / size);
        Ok(Decals { pipeline, group, layout, shader, family_of_surface, families, fallback })
    }

    /// New pipeline for a changed MSAA sample count.
    pub fn rebuild(&mut self, device: &wgpu::Device, hdr: wgpu::TextureFormat, depth: wgpu::TextureFormat, samples: u32) {
        self.pipeline = make_pipeline(device, &self.layout, &self.shader, hdr, depth, samples);
    }

    /// Weapon impact (pos.xyz, -, normal.xyz, alpha, surface, seed, ...) -> its decal instance.
    pub fn resolve(&self, i: &Instance) -> Instance {
        let surf = (i[8] as usize).min(255);
        let fam = self.family_of_surface[surf] as usize;
        let (layers, size) = self.families.get(fam).or_else(|| self.families.get(self.fallback as usize)).cloned().unwrap_or((vec![0], 4.0));
        let seed = i[9] as u32;
        let h = seed.wrapping_mul(0x9E37_79B9) ^ (seed >> 7).wrapping_mul(0x85EB_CA6B);
        let layer = layers[(h as usize) % layers.len().max(1)];
        let angle = ((h >> 8) & 0xFFFF) as f32 / 65535.0 * std::f32::consts::TAU;
        let scale = 0.85 + 0.3 * ((h >> 24) as f32 / 255.0);
        [i[0], i[1], i[2], size * scale, i[4], i[5], i[6], i[7], layer as f32, angle, 0.0, 0.0]
    }
}
