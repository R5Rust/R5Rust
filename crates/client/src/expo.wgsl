// Auto exposure: average log2 luminance of the HDR frame, then adapt the exposure towards key / average.
// state: x = current exposure (f32 bits), y = log-lum sum (fixed point, i32), z = sample count, w = last avg (f32 bits)

struct State {
    exposure: f32,
    sum: atomic<i32>,
    count: atomic<u32>,
    avg_log: f32,
};

struct Params {
    dt: f32,        // seconds since last frame (>= 1 snaps instantly)
    key: f32,       // target average luminance after exposure
    min_e: f32,
    max_e: f32,
};

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> st: State;
@group(0) @binding(2) var<uniform> pr: Params;

const GRID: vec2<u32> = vec2<u32>(128u, 72u);
const FIX: f32 = 256.0;

@compute @workgroup_size(16, 8)
fn lum(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= GRID.x || id.y >= GRID.y) { return; }
    let dims = vec2<f32>(textureDimensions(hdr));
    // centre-weighted like most game metering: ignore the outer 10%
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(GRID);
    let p = vec2<i32>((uv * 0.8 + 0.1) * dims);
    let c = textureLoad(hdr, p, 0).rgb;
    let l = log2(max(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-4));
    atomicAdd(&st.sum, i32(clamp(l, -14.0, 14.0) * FIX));
    atomicAdd(&st.count, 1u);
}

@compute @workgroup_size(1)
fn adapt() {
    let n = max(atomicLoad(&st.count), 1u);
    let avg_log = f32(atomicLoad(&st.sum)) / FIX / f32(n);
    let target_e = clamp(pr.key / exp2(avg_log), pr.min_e, pr.max_e);
    var e = st.exposure;
    if (e <= 0.0 || pr.dt >= 1.0) {
        e = target_e;
    } else {
        // brightening is slower than darkening, like an eye
        let rate = select(1.2, 3.0, target_e < e);
        e = e + (target_e - e) * (1.0 - exp(-pr.dt * rate));
    }
    st.exposure = e;
    st.avg_log = avg_log;
    atomicStore(&st.sum, 0);
    atomicStore(&st.count, 0u);
}
