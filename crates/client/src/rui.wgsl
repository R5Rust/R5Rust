// Apex UI pixel shader (ui_ps), as ported by apex-content-kit public/js/rui.js:
//  * images: 4-tap supersample, optional mask image lerping color1 -> color2 then multiplied (or added, flag bit 0)
//  * text:   SDF distance = tex * 12.75 - 10 (+ thicken), 5-tap supersample, outline band color2 -> color1
//  * output: colour transform 3x4, tint, desaturate, rgb premultiplied by coverage * c0.a * tint.a * (premul*(1-a)+a),
//            alpha *= blend (blend 0 = additive under ONE / ONE_MINUS_SRC_ALPHA)
// Rendered into an sRGB target: blending happens in linear light like the game's UI.

struct D {
    view: vec4<f32>,
    color0: vec4<f32>,
    color1: vec4<f32>,
    color2: vec4<f32>,
    tint: vec4<f32>,
    xf0: vec4<f32>,
    xf1: vec4<f32>,
    xf2: vec4<f32>,
    map0: vec4<f32>,
    map1: vec4<f32>,
    mode: vec4<u32>,   // x type (0 text, 1 image), y flags, z has mask, w clip0
    p0: vec4<f32>,     // x blend, y premul, z desat, w thicken
    p1: vec4<f32>,     // x outline, y text blur inverse
};
@group(0) @binding(0) var<uniform> d: D;
@group(0) @binding(1) var samp: sampler;
@group(1) @binding(0) var tex0: texture_2d<f32>;
@group(1) @binding(1) var tex1: texture_2d<f32>;

struct VO {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec4<f32>,
};

@vertex
fn vs(@location(0) p: vec2<f32>, @location(1) uv: vec4<f32>) -> VO {
    var o: VO;
    let q = (p - d.view.xy) / d.view.zw;
    o.pos = vec4<f32>(q.x * 2.0 - 1.0, 1.0 - q.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

fn tap4(t: texture_2d<f32>, uv: vec2<f32>, wrap_u: bool, wrap_v: bool, map: vec4<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    var o = array<vec2<f32>, 4>(-dx * 0.25 - dy * 0.25, -dx * 0.25 + dy * 0.25, dx * 0.25 - dy * 0.25, dx * 0.25 + dy * 0.25);
    var s = vec4<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        var p = uv + o[i];
        p.x = select(clamp(p.x, 0.0, 1.0), p.x - floor(p.x), wrap_u);
        p.y = select(clamp(p.y, 0.0, 1.0), p.y - floor(p.y), wrap_v);
        s += textureSampleLevel(t, samp, p * map.xy + map.zw, 0.0);
    }
    return s * 0.25;
}

@fragment
fn fs(i: VO) -> @location(0) vec4<f32> {
    var rgb: vec3<f32>;
    var a: f32;
    var c0a = d.color0.a;
    var cov = 1.0;
    var premul = d.p0.y;
    let flags = d.mode.y;
    // derivatives first, in uniform control flow
    let dx = dpdx(i.uv.xy);
    let dy = dpdy(i.uv.xy);
    let mdx = dpdx(i.uv.zw);
    let mdy = dpdy(i.uv.zw);
    if (d.mode.x == 0u) {
        let uv = i.uv.xy;
        let ts = vec2<f32>(textureDimensions(tex0, 0));
        let gx = dx * ts;
        let gy = dy * ts;
        let px = inverseSqrt(max(max(dot(gx, gx), dot(gy, gy)), 1e-12)); // screen pixels per atlas texel
        let sharp_screen = px * 2.236068;
        let sharp = min(sharp_screen, d.p1.y);
        let thicken = d.p0.w;
        let outline = d.p1.x;
        let d0 = textureSampleLevel(tex0, samp, uv, 0.0).r * 12.75 + thicken - 10.0;
        var cv: vec3<f32>; // main, outline, outline band
        if ((d0 + outline) * sharp < -1.0) {
            cv = vec3<f32>(0.0);
        } else if (d0 * sharp > 1.0) {
            cv = vec3<f32>(1.0);
        } else {
            let band = clamp(d0 / max(outline, 0.0) + 1.0, 0.0, 1.0);
            var m = clamp(d0 * sharp + 0.5, 0.0, 1.0);
            var o = clamp((d0 + outline) * sharp + 0.5, 0.0, 1.0);
            let taps = array<vec2<f32>, 4>(uv - dx * 0.4 - dy * 0.2, uv - dx * 0.2 + dy * 0.4, uv + dx * 0.2 - dy * 0.4, uv + dx * 0.4 + dy * 0.2);
            for (var k = 0; k < 4; k++) {
                let dd = textureSampleLevel(tex0, samp, taps[k], 0.0).r * 12.75 + thicken - 10.0;
                m += clamp(dd * sharp + 0.5, 0.0, 1.0);
                o += clamp((dd + outline) * sharp_screen + 0.5, 0.0, 1.0);
            }
            cv = vec3<f32>(m * 0.2, o * 0.2, band);
        }
        let oc = d.color2 + cv.z * (d.color1 - d.color2);
        let om = (cv.y - cv.x) * oc.a;
        let mm = cv.x * d.color0.a;
        rgb = d.color0.rgb * mm + oc.rgb * om;
        a = om + mm;
        c0a = 1.0;
        rgb = select(vec3<f32>(0.0), rgb / a, a > 0.0);
        premul = 0.0;
    } else {
        var t0 = tap4(tex0, i.uv.xy, (flags & 16u) != 0u, (flags & 32u) != 0u, d.map0, dx, dy);
        if (d.mode.w != 0u && (any(i.uv.xy < vec2<f32>(0.0)) || any(i.uv.xy > vec2<f32>(1.0)))) {
            t0 = vec4<f32>(0.0);
        }
        t0.a = clamp(t0.a + f32((flags & 4u) != 0u), 0.0, 1.0);
        if (d.mode.z != 0u) {
            var m = tap4(tex1, i.uv.zw, (flags & 64u) != 0u, (flags & 128u) != 0u, d.map1, mdx, mdy);
            m.a = clamp(m.a + f32((flags & 8u) != 0u), 0.0, 1.0);
            let mc = d.color1 + m * (d.color2 - d.color1);
            t0 = select(t0 * mc, t0 + mc, (flags & 1u) != 0u);
        }
        rgb = d.color0.rgb * t0.rgb;
        a = t0.a;
    }
    let pk = premul * (1.0 - a);
    let keep = 1.0 - pk;
    let pf = pk + a;
    let x = vec3<f32>(dot(d.xf0.xyz, rgb) + keep * d.xf0.w, dot(d.xf1.xyz, rgb) + keep * d.xf1.w, dot(d.xf2.xyz, rgb) + keep * d.xf2.w);
    let af = cov * c0a * d.tint.a;
    var c = d.tint.rgb * x;
    if (d.p0.z != 0.0) {
        let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
        c = mix(c, vec3<f32>(l), d.p0.z);
    }
    return vec4<f32>(c * pf * af, d.p0.x * af * a);
}
