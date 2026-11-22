// Ray-traced sun shadows (rt.rs, console r_rtshadows 1): one ray per pixel towards the sun through the opaque world
// and static props. Prepended after mat.wgsl with `enable wgpu_ray_query;` in front.
@group(3) @binding(0) var rt_tlas: acceleration_structure;

fn rt_sun(wp: vec3<f32>, nv: vec3<f32>) -> f32 {
    var rq: ray_query;
    // start a little off the surface (normal and sun side) so it doesn't hit itself
    let o = wp + nv * 1.5 + fr.to_sun.xyz * 0.5;
    rayQueryInitialize(&rq, rt_tlas, RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, 0xFFu, 0.0, 60000.0, o, fr.to_sun.xyz));
    rayQueryProceed(&rq);
    let hit = rayQueryGetCommittedIntersection(&rq);
    return select(1.0, 0.0, hit.kind != RAY_QUERY_INTERSECTION_NONE);
}
