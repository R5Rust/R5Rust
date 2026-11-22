//! Experimental hardware ray-traced sun shadows (console `r_rtshadows 1`, off by default; RTX / any Vulkan GPU with
//! ray queries). Not retail Apex: Apex uses shadow cascades. Kept off the Video menu (CLAUDE.md: settings never change
//! shading), a toggle to try.
//!
//! Acceleration structures over what's already on the GPU: one BLAS for the opaque world (every shadow-casting opaque
//! draw of world.glb as a geometry), one BLAS per static-prop variant (its LOD0 opaque draws), and a TLAS with the world
//! plus every static prop instance. Alpha-tested foliage and blended surfaces aren't in it (wgpu ray queries only see
//! opaque geometry), so those cast no ray-traced shadow. The world shader (rt_on.wgsl) casts one ray per pixel towards
//! the sun in place of the cascades.

use crate::loader::LoadedWorld;

/// Device features this needs (requested when the adapter has them).
pub const FEATURES: wgpu::Features = wgpu::Features::EXPERIMENTAL_RAY_QUERY;

pub struct Rt {
    pub layout: wgpu::BindGroupLayout,
    pub group: wgpu::BindGroup,
    _blas: Vec<wgpu::Blas>,
    _tlas: wgpu::Tlas,
}

pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rt"),
        entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::AccelerationStructure { vertex_return: false }, count: None }],
    })
}

/// Builds the acceleration structures for `world` (a few hundred ms on a 2080 Ti) and their bind group.
pub fn build(device: &wgpu::Device, queue: &wgpu::Queue, world: &LoadedWorld) -> Result<Rt, String> {
    if !device.features().contains(FEATURES) {
        return Err("this GPU / backend has no ray queries (needs Vulkan or DX12 ray tracing)".into());
    }
    let t0 = std::time::Instant::now();
    // the world vertex buffer: 32-byte vertices, position (Float32x3) first; indices are absolute into it
    let verts = (world.vbuf.size() / 32) as u32;
    let size = |n: u32| wgpu::BlasTriangleGeometrySizeDescriptor {
        vertex_format: wgpu::VertexFormat::Float32x3,
        vertex_count: verts,
        index_format: Some(wgpu::IndexFormat::Uint32),
        index_count: Some(n),
        flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
    };
    // opaque, shadow-casting, not the 3D skybox, not sky material (pipe 6) or decal-biased (7+)
    let world_ranges: Vec<(u32, u32)> = world.draws.iter().filter(|d| !d.skybox && d.shadow && d.key / 2 == 0 && d.pipe < 6).map(|d| (d.first_index, d.index_count)).collect();
    let mut sets: Vec<Vec<(u32, u32)>> = vec![world_ranges];
    // per prop variant: its LOD0 opaque shadow draws (None = nothing opaque)
    let mut variant_set: Vec<Option<usize>> = Vec::new();
    if let Some(p) = &world.props {
        for v in &p.variants {
            let r: Vec<(u32, u32)> = v.lods.first().map(|l| l.draws.iter().filter(|d| d.shadow && d.key / 2 == 0 && d.pipe < 6).map(|d| (d.first_index, d.index_count)).collect()).unwrap_or_default();
            if r.is_empty() {
                variant_set.push(None);
            } else {
                variant_set.push(Some(sets.len()));
                sets.push(r);
            }
        }
    }
    let sizes: Vec<Vec<wgpu::BlasTriangleGeometrySizeDescriptor>> = sets.iter().map(|s| s.iter().map(|&(_, n)| size(n)).collect()).collect();
    let blas: Vec<wgpu::Blas> = sizes
        .iter()
        .map(|d| {
            device.create_blas(
                &wgpu::CreateBlasDescriptor { label: Some("rt blas"), flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE, update_mode: wgpu::AccelerationStructureUpdateMode::Build },
                wgpu::BlasGeometrySizeDescriptors::Triangles { descriptors: d.clone() },
            )
        })
        .collect();
    let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let mut instances: Vec<(usize, [f32; 12])> = vec![(0, identity)];
    if let Some(p) = &world.props {
        for inst in &p.instances {
            if let Some(Some(s)) = variant_set.get(inst.variant as usize) {
                let r = inst.gpu.rows;
                instances.push((*s, [r[0][0], r[0][1], r[0][2], r[0][3], r[1][0], r[1][1], r[1][2], r[1][3], r[2][0], r[2][1], r[2][2], r[2][3]]));
            }
        }
    }
    let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("rt tlas"),
        max_instances: instances.len() as u32,
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    });
    for (i, (s, xf)) in instances.iter().enumerate() {
        tlas[i] = Some(wgpu::TlasInstance::new(&blas[*s], *xf, 0, 0xFF));
    }
    let entries: Vec<wgpu::BlasBuildEntry> = blas
        .iter()
        .zip(&sets)
        .zip(&sizes)
        .map(|((b, set), sz)| wgpu::BlasBuildEntry {
            blas: b,
            geometry: wgpu::BlasGeometries::TriangleGeometries(
                set.iter()
                    .zip(sz)
                    .map(|(&(first, _), size)| wgpu::BlasTriangleGeometry {
                        size,
                        vertex_buffer: &world.vbuf,
                        first_vertex: 0,
                        vertex_stride: 32,
                        index_buffer: Some(&world.ibuf),
                        first_index: Some(first),
                        transform_buffer: None,
                        transform_buffer_offset: None,
                    })
                    .collect(),
            ),
        })
        .collect();
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rt build") });
    enc.build_acceleration_structures(entries.iter(), std::iter::once(&tlas));
    queue.submit([enc.finish()]);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let layout = layout(device);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("rt"), layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: tlas.as_binding() }] });
    let geoms: usize = sets.iter().map(Vec::len).sum();
    log::info!("rt: {} BLAS ({geoms} geometries), {} TLAS instances, built in {:.0} ms", blas.len(), instances.len(), t0.elapsed().as_secs_f64() * 1000.0);
    Ok(Rt { layout, group, _blas: blas, _tlas: tlas })
}
