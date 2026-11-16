// Auto exposure, Apex's algorithm (docs/apex-post.md section 1):
//  meter  = arithmetic mean of min(lum709, max_lum) over a centred spot (c_spotMeteringScreenRatio) of the
//           unexposed HDR frame (no log average, no histogram)
//  target = dead band: 1 inside [target_min, target_max], else just enough to put the average on the nearest edge,
//           times 2^compensation
//  adapt  = prev + (target - prev) * saturate(rate / max(prev, target)), clamped to [exposure_min, exposure_max]

struct State {
    exposure: f32,
    sum: atomic<u32>,      // fixed-point sum of clamped luminance
    count: atomic<u32>,
    avg: f32,
};

struct Params {
    rate: f32,             // c_adaptationRate already scaled by dt (>= 1000 snaps)
    target_min: f32,
    target_max: f32,
    compensation: f32,     // EV stops
    exposure_min: f32,
    exposure_max: f32,
    spot: f32,             // fraction of the screen metered, centred
    max_lum: f32,          // per-pixel clamp
};

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> st: State;
@group(0) @binding(2) var<uniform> pr: Params;

const GRID: vec2<u32> = vec2<u32>(128u, 72u);
const FIX: f32 = 4096.0;

@compute @workgroup_size(16, 8)
fn lum(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= GRID.x || id.y >= GRID.y) { return; }
    let dims = vec2<f32>(textureDimensions(hdr));
    let uv = ((vec2<f32>(id.xy) + 0.5) / vec2<f32>(GRID) - 0.5) * pr.spot + 0.5;
    let c = textureLoad(hdr, vec2<i32>(uv * dims), 0).rgb;
    let l = min(dot(max(c, vec3<f32>(0.0)), vec3<f32>(0.2126, 0.7152, 0.0722)), pr.max_lum);
    atomicAdd(&st.sum, u32(l * FIX));
    atomicAdd(&st.count, 1u);
}

@compute @workgroup_size(1)
fn adapt() {
    let n = max(atomicLoad(&st.count), 1u);
    let avg = max(f32(atomicLoad(&st.sum)) / FIX / f32(n), 1e-5);
    var e = 1.0;
    if (avg < pr.target_min) { e = pr.target_min / avg; } else if (avg > pr.target_max) { e = pr.target_max / avg; }
    let target_e = e * exp2(pr.compensation);
    let prev = st.exposure;
    var next: f32;
    if (prev <= 0.0 || pr.rate >= 1000.0) {
        next = target_e;
    } else {
        next = prev + (target_e - prev) * clamp(pr.rate / max(prev, target_e), 0.0, 1.0);
    }
    st.exposure = clamp(next, pr.exposure_min, pr.exposure_max);
    st.avg = avg;
    atomicStore(&st.sum, 0u);
    atomicStore(&st.count, 0u);
}
