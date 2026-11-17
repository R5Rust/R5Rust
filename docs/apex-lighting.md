# Apex (S21 / R5) world + static-prop lighting, from the shipped shaders

The source is the compiled DXBC in `D:\R5Rust\assets\shaders\programs`, disassembled with `d3dcompiler_47!D3DDisassemble`.
The RDEF is not stripped, so every cbuffer field and resource name below is the game's own name.
Scratch dumps live in the session scratchpad, `...\scratchpad\shader-re\`. `L###` refers to a line number there:

| Role | Set (material `shaderSet`) | PS | VS | Dump |
|---|---|---|---|---|
| world `wldc`, col+nml+gls+spc+ao, vertex-colour tint | `0x7793C3C015EAE573` (steel_corrugated…) | `0x393C8DECDE9ABE41` (`_0`; `_1` = the same plus a dither-fade `discard`) | `0x834C6E518B33674` | `WORLD_PS_wldc_0x393C8DECDE9ABE41_code.asm` |
| world `wldc`, 2-layer blend (`_bm`) | `0x8998A6E6030B0098` | `0x286DCA7655784859` | `0x68C175CD8879399` | `0x286DCA7655784859_0.asm` |
| static prop `rgdp` (top set, 1548 mats), and the top `rgdc` set (same PS, different VS) | `0x9BB652D63E2C4B0C`, `0x2C55D5AA7EF3372C` | `0x9996FB2101AB4F75` (`_0` plain, `_1` dither, `_2/_3` instanced via `t75`) | `0x8815107E59BA114B` | `PROP_PS_rgdp_0x9996FB2101AB4F75_code.asm` |

**`.msw` set layout** (37 bytes): `"MSW" 03`, `u8 01`, `u64 pixelProgramGuid @5`, `u64 vertexProgramGuid @13`, then `09 00 00 00 00 03 ...`.
Confidence is high: 4671 sets resolve to a PIXEL+VERTEX pair this way. 1620 sets have no PS in the extraction, including
the example `0xDADC20606BD5124F`, whose PS `0xCD574C666A5484E1` is missing.

**Material texture slot N is bound to register tN.** The shader's RDEF names that register, for example
`steel_corrugated…_wldc.json` slots 0-4 = `albedoTexture normalTexture glossTexture specTexture aoTexture` = t0-t4.

Confidence tags: **[H]** read directly from instructions or names · **[M]** strong inference · **[L]** guess or calibration hint · **unknown**.

---

## 0. Conventions used by every shader [H]

- All lighting is in **camera-relative world space**. `worldPos = v.camRelPos + c_cameraOrigin` (L51).
  `viewRay = normalize(camRelPos)` points from the camera to the pixel, and `V = -viewRay`.
- **No 1/π anywhere.** Lambert is `albedo * NdotL * lightColor`, and specular is `π·D·Vis·F·NdotL·lightColor`
  (D's 1/π is dropped). Light colours and lightmaps are in "π-folded" irradiance units.
- Perceptual roughness `r = max(1 - gloss, 0.04)`, `α = r²`, `α2 = α²` (L66-77).
- Accumulators are packed to f16 (`f32tof16`, clamped to ±65504). That only affects precision, so it can be ignored.
- `lum(c) = dot(c, float3(0.2126, 0.7152, 0.0722))`.

## 1. Material inputs and texture channels

```hlsl
albedo   = t0.rgb * c_L0_albedoTint(cb0[22].yzw) * vtxColor.rgb;   // L55,64,65 [H]
gloss    = t2.r;              r = max(1 - gloss, 0.04);              // L66-67,75 [H] gls = linear smoothness
F0       = t3.rgb;            // specular colour used directly as F0 (no metalness)  L68 [H]
ao       = floor(t4.r*255)/255;  // multiplies INDIRECT DIFFUSE only            L69-71,1029-1041 [H]
// normal map: 2 channels, NO z reconstruction, implicit z = 1           L56-63 [H]
n2  = t1.rg * 2.867661 - 1.439453;     // = (byte - 128) / 88.92  -> zero at 128/255, slope range ±1.44
N   = normalize(n2.x * T + n2.y * B + Nvtx);    // T,B,Nvtx unnormalised interpolants; no green flip in the PS
```

- **`_nml`**: only `.rg` are read, so the texture is BC5-class [M]. Any green-flip convention lives in the VS binormal sign
  (§8), not in the PS [H].
- **`_col` sRGB-ness is unknown** from the shader, because it is sampled as `float4` and used linearly. Use the DXGI
  format in the texture asset. In practice albedo must reach the shader linear, so expect an `_SRGB` format [M].
- **Vertex colour** (world VS): `rgb = pow(|c|, 2.2)` (L in VS: `log/mul 2.2/exp`), `a` raw [H]. The plain world shader
  multiplies albedo by `rgb`. The blend shader uses only `a`, as the blend driver. Props have no vertex colour [H].
- **`_bm` / layerBlendTexture** (blend shader `0x286D…`, `c_layerBlendRamp` = cb0[8].w) [H]:
  ```hlsl
  blend = saturate(ramp * (t_bm.r - 1) + vtxColor.a * (ramp + 1));
  albedo = lerp(t0.rgb*c_L0_albedoTint, t1.rgb*c_L1_albedoTint(cb0[25].yzw), blend);
  N      = lerp(N_layer0, N_layer1, blend);   // each layer normalised first; gloss/spec/ao lerp the same way [M]
  ```
- The base world variant `0xE156…` (dev_gray etc.) is the same code without the AO texture or the vertex-colour tint [H].

### 1.1 Which UV each texture uses [H]
Every texture is sampled with either the raw mesh UV (`v0.xy` = TEXCOORD0 / 16384 from the VS, no transform) or a
`c_uvN` transform (N = 1..5, `CBufUberStatic`: `c_uvNRotScaleX/Y` + `c_uvNTranslate`, `uv = dot(p + T, RS)`) of
either the mesh UV or world position `p = (x, -y)`. **The routing differs per PS**, so `export-map/build_materials.py`
reads it from each material's own DXBC (`uv_route` in materials.json: slot -> `[N, "planar"|"mesh"]`, N = 0 untransformed):

| PS | layer 0 (alb/nml/gls) | layer 1 | layerBlend mask | detail / detailNormal | detail2 / detailNormal2 |
|---|---|---|---|---|---|
| `0x6DDE96D0617FFC89` sand_desert_01_sand_desert_02 (range floor) | uv1 planar | uv1 planar | uv1 planar | uv2 planar | - |
| `0x2FBB46BF988A98AC` sand_desert_02_dirt_desert_05 (road floor) | uv2 planar | uv2 planar | uv1 planar | uv3 planar | uv4 planar |
| `0xF96FF9520440F420` sand_desert_02_to_s2s_metal_trims | uv1 planar | mesh | mesh | uv2 planar | - |
| `0x937D8E4BC98DD7A9` sand_desert_02_bm_transparent (overlay) | uv1 planar | - | uv1 planar | uv2 planar | - |
| plain world / props (`0x393C…`, `0x355A…`, …) | mesh | - | - | - | - |

r5rust used to sample layer 1 and the mask on the mesh UV and layer 0 through uv1 for every blend, which stretched the
road floor's gravel dirt (`dirt_desert_05`) into smooth brown bands. 345 of 434 materials get a route; the rest
(PS not in the extraction) keep that old default.
Some foliage/scrolling PSes read `c_uvN` in **rotation form** (`sincos(c_uvNRotScaleX.x)`): RotScaleX = (angle,
scaleU), RotScaleY = (centre as two packed f16, scaleV), Translate = scroll speed. build_materials.py rewrites those as
the equivalent matrix (scroll dropped); read as a matrix, `tree_fir_large_leaves_01`'s identity became `(v, v)` and
the pine branches rendered as streaked solid cards.

**Detail albedo** (blend PSes above) [H]: Photoshop overlay in gamma-2 space, `d = saturate(det - 0.5/255)`,
`s = sqrt(albedo)`, `r = any(d < 0.5) ? 2*d*s : 1 - 2*(1-d)*(1-s)`, `albedo = r*r * tint`; 128/255 is neutral, the
texture is UNORM. detail2 is overlaid the same way when `c_uberFeatureFlags & 4`, else `albedo2 * detail2 * 2`.

### 1.2 Translucent overlays (`alphaMode` BLEND with a mask)
`0x937D8E4BC98DD7A9` (transparent layer blend, e.g. `sand_desert_02_bm_transparent`) [H]:
```hlsl
blend   = saturate(ramp * (mask - 1) + vtxAlpha * (ramp + 1));
opacity = (1 - blend) * albedo.a * c_opacity * instanceAlpha;     // colour is premultiplied by it before output
rough   = max(lerp(1 - gloss, 1, blend), 0.04);
F0      = c_L0_perfSpecColor;                                     // no spec texture
```
Old-style `_col/_nml/_gls/_det/_nml/_bm` overlays (`road_tracks_dry_03_blend_2_trans`, PS `0x19865A3619FC6126`;
`sand_desert_erosion_01_bm_transparent`, PS `0xBED6B8166A731D51`) are not in the extraction, and their
CBufUberStatic exports empty [M]. They have the same slot layout as `0x937D…`, and the same opacity rule matches
R5Flowstate: at the `patch1` probe pose (dirt road under the catwalk) the road's column-luma profile correlates 0.88
with the real shot using `1 - blend` and -0.27 using `blend`; mean road colour ours (196,174,142) vs real (197,175,142).
At `patch2` (erosion sand) the region medians match only with `1 - blend` (147/175/179 vs real 147/173/178).
With no constants, F0 is the dielectric 0.04 (a missing perf spec used to default to 1.0: white mirrors) and ramp 3.
Overlays whose mask slot is only a guess (an unnamed 4th texture: `sand_blend_firing_range`, the decals) keep
`opacity = blend`, the closer rule on `map00`; that one is unverified [L].

## 2. Lightmaps (world only)

### UV [H]
VS: `lmTexel = TEXCOORD1(uint2) / 64` (6-bit sub-texel). PS (L84-88):
```hlsl
uint2 dims = RealTimeLightIDs.GetDimensions();      // = the page's w,h (e.g. 1024x928)
float2 lmUV = min(lmTexel / min(dims, 1024), 0.999999);
```

### Sky page (lump 0x62 type 1 = two RGBA8 images per page, compressed 0x61 type 8 = two 16B-block BC textures) [H]
```hlsl
float4 lm0 = lightmapTexture0.Sample(bilinear, lmUV);   // t41
float4 lm1 = lightmapTexture1.Sample(bilinear, lmUV);   // t42
float3 bakedIrr = lm0.rgb * (lm0.a * 3.984375 + 0.015625);     // RGBM, range [1/64, 4]    L110-111
float3 skyVec   = lm1.rgb * 2.007874 - 1.0;                     // 2.007874 = 255/127 -> byte/127 - 1  L91
float  sunVisBaked = lm1.a;                                     // L124,177-180
// directional combine with the normal-mapped N:
float NdotNv  = saturate(dot(N, Nvtx_normalized));              // L112
float len2    = dot(skyVec, skyVec), len = sqrt(len2), NdS = dot(N, skyVec);
float skyTerm = saturate((max(len2, NdS) + NdS) * len * 0.5);   // L113-119
float3 indirectDiffuse = bakedIrr * NdotNv + skyTerm * c_skyColor;   // L120-121
```
- The exporter's assumption is **confirmed for image0**. Image1 is not a pure direction: **rgb is a sky-visibility-scaled
  bent normal**, signed, whose length encodes how much sky is visible, and **a = baked sun visibility** [H].
- `bakedIrr` is the bounce plus static non-RTL lights. Its only "directionality" is the `N·Nvtx` normal-map darkening.
  There is no per-direction lightmap basis [H].
- Texture-format sRGB flags are unknown. Treat both images as linear UNORM. RGBM in sRGB would be unusual [M].

### Real-time-light page (lump 0x69) [H for shader side; M for byte layout]
Per page: **`R32_UINT` light-ID image w×h (4wh bytes), then a BC5 visibility image w×2h** (2·padded bytes).
`rbsp::rtl_page_bytes` = `2*(padded+2wh)` matches this. The order is confirmed from data: in page 0 the first 4wh bytes
have a top byte of 0..6 (7 groups = 441/63 entries in 0x7A), and 82% of texels have all four slots empty.

```hlsl
uint id = RealTimeLightIDs.Load(int3(lmUV * dims, 0));   // point, L94-97
//   bits 24-31: group g   (0x7A base = g*63)
//   bits 0-23 : four 6-bit slots s0..s3 (bits 6k..6k+5), 63 = empty
float2 visA = RealTimeLightVisibilities.Sample(bilinear, lmUV * float2(1, 0.5)).rg;            // top half    -> s0,s1
float2 visB = RealTimeLightVisibilities.Sample(bilinear, lmUV * float2(1, 0.5) + float2(0,0.5)).rg; // bottom -> s2,s3
// (visA zeroed if (id & 0xFFF)==0xFFF, visB if (id & 0xFFF000)==0xFFF000 - skip optimisation)
for (k = 0; k < 4; k++) {                                    // L653-882
    uint s = (id >> (6*k)) & 63;  if (s == 63) continue;
    uint li = s_realTimeLightIDs[g*63 + s] + 32;             // lump 0x7A value + 32 (L665-668)
    HardwareLight L = s_globalLights[li];
    float vis = (L.propertyFlags & 32) ? 1.0 : vis4[k];      // dynamic-shadowed lights ignore baked vis (L673-674)
    // ... evaluate L (section 4) with extra factor vis
}
```
- **0x7A values index the list of worldlights that have lump flag `0x8`, in lump order** [H by data]. That is 404 lights
  on staging_mu1, including the sun as entry 0. The referenced types come out as spot/point, and the probe
  `static_light_indexes` max is 403 in the same space.
  `s_globalLights[32 + i]` = that light, so slots 0-31 are reserved for dynamic lights [M].
- This path runs when `c_branchFlags` (cb3[37].w, offset 604) **bit 10 == 0** (L92). With bit 10 set, the world uses the
  clustered list instead (§4.3).

## 3. Sun (world and props)

### Shadow term [H] (world L177-300, prop L143-269)
```hlsl
float sun = sunVisBaked;            // world: lm1.a ; prop: (packedSunData >> 16) / 32768
if (sun > 0) {                      // whole sun term skipped when baked visibility == 0
  if (c_csm.enableShadows) {
    float3 p = camRelPos + Nvtx * c_shadowBias;                      // normal offset (cb0[12].y)
    float3 s = shadowRelConst + p.x*shadowRelForX + p.y*shadowRelForY + p.z*shadowRelForZ;  // ortho, z = depth
    float2 c0 = s.xy;  float4 c12 = s.xyxy*normCoordsScale12 + normCoordsBias12;
    float2 cS = s.xy*normCoordsScaleBiasStatic.xy + .zw;
    // per-cascade fade weight: w = sat(|c.x|*cascadeWeightScale + cascadeWeightBias) * sat(|c.y|*... ) for c0,c1,c2,static
    // pick the first cascade with w>0 (0,1,2, else static); atlas uv = c*normToAtlasCoordsScale{0|12} + Bias
    float csm    = PCF3x3(CSMDepthAtlas, uv, s.z);                    // tent: corners 1/16, edges 1/8, centre 1/4
    float statSh = PCF3x3(staticShadowTexture, cS*0.5+0.5,
                          dot(p, shadowRelToZStatic.xyz) + shadowRelToZStatic.w + c_shadowBiasStatic*worldUnitSizeInDepthStatic);
    float cur    = inDynamicCascade ? csm : statSh;
    float next   = sun;              // baked value if nothing further out
    if (w_cur < 1) next = (cascade 0/1 && normCoordsScale12.x != 0) ? PCF(next cascade) : (cascade 2 && static present ? statSh : sun);
    sun = lerp(next, cur, w_cur);    // outside all cascades and static: sun stays = baked
    if (cascade3LogicData == 1 && max(|c3|) <= 1)          // c3 = s.xy*normCoordsScaleBias3.xy+.zw
        sun = min(sun, PCF3x3(CSMDepthAtlas, c3*normToAtlasCoordsScale3+Bias3, dot(p, shadowRelToZ3.xyz)+shadowRelToZ3.w));
  }
  // props only: sun *= (packedSunData & 0xFFFF) / 32768;             (prop L147-149, 269)
  sun *= saturate(dot(N, c_sunDir));
}
```
So inside CSM range the cascade **replaces** the baked sun visibility (it does not multiply it). The baked value is used
as a gate (0 = no sun at all) and as the far fallback [H].

### Who casts into the cascades (2026-11-16) [H data]

Because the cascade replaces the bake, one wrong caster blacks out a sunlit area. The static prop record's flag byte
(`StaticProp::unknown_1` in r5ed's `sprp.rs`, Source `dstaticprop_t::flags`) decides it. Measured on staging_mu1
against the map's own CSM caster tree (lumps 0x63 nodes / 0x64 obj refs; a ref `>= first_static_prop_index` (0x7B) is a
static prop):

| bit | Source name | on this map | in the CSM caster tree | r5rust |
|---|---|---|---|---|
| 0x01 | FLAG_FADES | 392 props | 170 | casts |
| 0x02 | USE_LIGHTING_ORIGIN | 630 | 269 | casts |
| 0x04 | (Source: NO_DRAW; here on 11198/11198 props, so not that) | all | 6097 | casts |
| **0x10** | **NO_SHADOW** | 1308: decal models, the giant `grave_wall_gasket*` / `live_fire_col20_pan*` / `fs_girder_beam` set dressing at z 14-16k | **0** | **never casts** |
| 0x20 | SCREEN_SPACE_FADE | 1115: grass, pebble clusters, small rocks | 881 | casts |
| 0x40 | (Respawn) 3D skybox only | 1: the vista `mdl/vistas/eden_se.rmdl` (flags 0x54) | 0 | skybox pass, never casts (docs/skybox.md) |

The bake agrees: the range floor under the gaskets is sunlit in lm1.a. Not every prop without 0x10 is in the tree
(61% are; the rest are presumably culled by size or extent), so r5rust casts all of them rather than guess.
World meshes: bit 0x10 of `Mesh.flags` (0x00100210 decals, 0x00100a10 blended decals, 0x00103210 water) marks
decal-class meshes, which r5rust also keeps out of the cascades. The world's own shadow geometry is a separate,
much smaller set (lumps 0x7C-0x7F: 45k triangles against 248k opaque render triangles), so that part is the Source
convention (decals and overlays never cast) rather than proven from the data.

`export-map` carries this into world.glb as material extras `noShadow` / `skybox` (geometry with either gets its
own primitives: group keys `|ns`, `|sky`); `crates/client/src/csm.rs` skips `DrawCmd::shadow == false`.

### Sun shading [H] (world L304-372)
```hlsl
float3 sunCol = c_sunColor * cloudMaskTexture.Sample(bilinear,
                 c_cloudRelConst + wp.x*c_cloudRelForX + wp.y*c_cloudRelForY + wp.z*c_cloudRelForZ).rgb;  // wp = camRelPos
Ldir = c_sunDir;   lightAtten = sun /* includes NdotL */;   -> BRDF of section 5, no specularIntensity, no sphere normalisation
```
- `c_sunColor` (cb3 off 276) / `c_sunDir` (off 288, points *toward* the sun) / `c_skyColor` (off 256) are per-camera
  constants. Their source is [M]: worldlight type 3 `intensity` (1.508, 1.411, 1.183) is the sun colour,
  `-normal` (0.529, -0.344, 0.776) is `c_sunDir`, matching lump 0x05 and 0x7B, and type 5 `intensity`
  (0.220, 0.491, 0.905) is `c_skyColor`.
- Cloud mask: its texture and matrix come from the level or script, which is unknown. Use 1.0.

## 4. Real-time lights

### 4.1 `HardwareLight` (StructuredBuffer `s_globalLights`, stride 128) [H names/offsets]
| off | field | use |
|---|---|---|
| 0 | `int propertyFlags` | bit 4: world clustered loop requires it · 8: inverse-square falloff · 16: spot cone · 32: has shadow map (and ignores baked vis) |
| 4 | `float spotExpSel` | cone curve: `lerp(t, t², spotExpSel)` |
| 16 | `float3 color` | |
| 28 | `highlightSizeOrVfogBoost` | unused here |
| 32 | `float3 pos` | when `rcpMaxRadius == 0` (prop static-light loop only) `pos` = direction, atten = 1 |
| 44 | `float rcpMaxRadius` | |
| 48 | `float emitterRadius` | >0 means a sphere light (representative point) |
| 52 | `float specularIntensity` | spec multiplier |
| 56/60 | `attenLinear`, `attenQuadratic` | polynomial falloff in x = d/R |
| 64 | `float3 spotDir`, 76 `spotBias` | `t = saturate(dot(spotDir, Lto) + spotBias)` |
| 80/96 | `spotAxisX`/`spotAxisY`, 92 `shadowBias` | shadow projection |
| 112 | `float2 shadowMapUVOffset` | atlas offset in `t_shadowMaps` |

### 4.2 Attenuation, cone, shadow [H] (world L427-556 / L675-799)
```hlsl
float3 Lv = L.pos - worldPos;  float d2 = dot(Lv,Lv);  float3 Ld = Lv*rsqrt(d2);   // diffuse/NdotL direction
float3 Ls = Ld; float sphereNorm = 1;
if (L.emitterRadius > 0) {                            // Karis representative point
   float3 c2r = dot(Lv,R)*R - Lv;  Ls = normalize(Lv + c2r*saturate(L.emitterRadius/length(c2r)));
   sphereNorm = sq(α / saturate(L.emitterRadius*rsqrt(d2)*0.5 + α));
}
float atten;
if (flags & 8) { float x = d2*sq(L.rcpMaxRadius); atten = sq(max(1 - x*x, 0)) / (d2 + 1); }  // windowed 1/(d²+1)
else { float x2 = min(sq(L.rcpMaxRadius)*max(d2,1), 1);   float x = sqrt(x2);              // x = d/R, d>=1
       atten = saturate(1 + L.attenLinear*x + L.attenQuadratic*x2); }
if (flags & 16) { float t = saturate(dot(L.spotDir, Ld) + L.spotBias); atten *= t + L.spotExpSel*(t*t - t); }
if (atten != 0 && (flags & 32)) {
   float depth = sqrt(d2) - L.shadowBias;  float k = 1/dot(L.spotDir, Ld);
   float2 uv = L.shadowMapUVOffset + float2(dot(L.spotAxisX,Ld), dot(L.spotAxisY,Ld))*k;
   // c_spotShadowMode (cb3 off 744): 0 = VSM moments in t_shadowMaps.rg:
   //   var = max(rcpMaxRadius*c_minShadowVariance, m2-m1²); t = max(depth*rcpMaxRadius - m1,0); sh = var/(var+t²)
   // 1 = PCF compare vs depth*rcpMaxRadius - c_pcfShadowBias; c_pcfShadowKernelWidth<2: 1 tap, else 3x3 [1 2 1]²/16
   // other: 0
   atten *= sh;
}
lightAtten = atten * vis * saturate(dot(N, Ld));       // vis = baked RTL vis (world) or probe weight (props)
// -> BRDF (section 5) with diffuse dir Ld, specular dir Ls, extra spec factor L.specularIntensity*sphereNorm, colour L.color
```

### 4.3 Which lights reach which surface [H]
- **World, normal path** (`c_branchFlags` bit 10 = 0): the ≤4 RTL slots of §2, plus the per-instance dynamic list
  `c_modelInst.lighting.packedLightData.w`: up to 4 bytes, each `& 127` = global light index, loop until the word is 0
  (L887-1026). No flag-4 test.
- **World, clustered path** (bit 10 = 1 and bit 2 = 1, L381-642): `cluster = (floor(py/ppcY)*dimX + floor(px/ppcX))*dimZ + slice`,
  `slice = min(floor(max(log(w/zSliceFirstFar)/zSlicePowerRatioLog + 1, 0)), dimZ-1)`, where (px,py,w) = SV_Position.
  `b_lightClusterData[cluster]` holds the first index in its low 20 bits and the count in its high 12.
  `b_lightIndexBigList[i]` gives the light. **Lights without propertyFlags bit 4 are skipped on world.** Shadow maps apply,
  no baked vis.
- **Props**: clustered (same, no bit-4 test) **or** the probe's static lights + dynamic list (§6).

### 4.4 Mapping lump 0x36 `WorldLight` (112 B) → `HardwareLight`
The shader only sees the GPU struct; the engine conversion is not visible. Checked against all 496 lights on staging_mu1:

| word | lead's name | finding | conf |
|---|---|---|---|
| 0-2 | origin | → `pos` | M |
| 3-5 | intensity | → `color` (no evidence of extra scaling) | M |
| 6-8 | normal | spot: `spotDir = -normal / (stopdot - stopdot2)`, `spotBias = -stopdot2/(stopdot - stopdot2)` (Source cone remapped to the shader's `saturate(dot+bias)` form); sun: `c_sunDir = -normal` | M |
| 12 | float 1.0/2.0/3.0 | unknown (bsp_tool's v3 layout has `viscluster` int here; Apex stores a float) | unknown |
| 13 | emittype | 1 point, 2 spot (→ flag 16), 3 sun, 5 sky ambient | H (data) |
| 15/16/17 | stopdot/stopdot2/exponent | exponent is 0 on points and 1 on spots → `spotExpSel` probably `exponent-1` clamped to 0..1, so S21 spots are linear | L |
| 18 | radius | → `rcpMaxRadius = 1/radius` | M |
| 19 | constant = 1.0 | the shader hardcodes the 1 | H |
| 20/21 | linear/quadratic | **stored pre-divided**: on every light `linear*R` and `quadratic*R²` are exact constants (-1.952/0.952, -0.048/-0.952, -1/0), all giving 0 at d=R. So `attenLinear = linear*radius`, `attenQuadratic = quadratic*radius²` | M-H |
| 22 | flags | **0x8 = has an RTL slot / realtime** (exactly the 404-entry 0x7A index space) [H]. **0x20 ⇒ inverse-square** (`propertyFlags 8`): every 0x20 light has intensity 3.8e3-4e5, every other light ≤ 20 [M]. 0x4, 0x10, 0x40, 0x80, 0x2000 (sky) unknown | |
| 25 | 0 / 20 / 150 | candidate `emitterRadius` | L |
| 26/27 | 1.0 / 0.1-2.0 | candidates `specularIntensity` / vfog boost | L |

Shadow-mapped (`propertyFlags 32`) lights come from runtime script/entities; nothing in the lump marks them [M].
A from-scratch renderer can set 32 = 0 for every map light and rely on the baked RTL visibility.

## 5. BRDF (identical in world, prop, sun and point paths) [H]

```hlsl
// per light: Ld (diffuse/NdotL dir), Ls (spec dir), A = lightAtten (incl. NdotL), C = colour
float NoV = saturate(dot(N, V)), NoL = saturate(dot(N, Ld));
float3 Hd = normalize(Ld + V);  float NoHd = saturate(dot(N, Hd));
float facing = saturate(0.5 + 0.5*dot(Ld, V));
float rough  = facing*(0.9375 - 0.416667*facing) * (NoHd + 0.5) / max(NoHd, 0.05) * (1 - lum(F0));
float smooth = 1.05 * sq(1 - 0.5*pow5(1 - NoL));
float  diff  = lerp(smooth, rough, α) * A;                      // Hammon-17-style diffuse, Respawn constants
float  multi = 0.379271 * α * (1 - lum(F0)) * A;                // multiple-scatter, multiplied by albedo again at the end
float3 Hs = normalize(Ls + V) /* N if zero */;  float NoH = saturate(dot(N,Hs)), NoLs = saturate(dot(N,Ls));
float3 F  = F0 + (1 - F0) * pow5(1 - saturate(dot(Ls, Hs)));
float  dd = sq(NoH*NoH*(α2 - 1) + 1);
float  vis= max(lerp(2*NoV*NoLs, NoV + NoLs, α), 0.00125);
float3 spec = F * (α2 / (2*vis*dd)) * A * specScale;            // specScale = L.specularIntensity*sphereNorm (1 for sun)
diffAcc += diff*C;  multiAcc += multi*C;  specAcc += spec*C;
```
`α2/(2·vis·dd)` is `π · D_GGX · V_SmithGGXCorrelated(Hammon approx)`, with Schlick F and no F90 term.

### Environment specular [H] (world L126-176, prop L91-142)
```hlsl
float3 R = viewRay + 2*NoV*N;                                    // reflect(viewRay, N)
float mipA = 22.6*r^4 - 44.9*r^3 + 26.8*r^2 + 3.69*r - 0.185;
float mipB = -11*r^4 + 23.5*r^3 - 11.4*r^2 + 3.55*r - 0.0882;
float mip  = clamp(mipA + sq(1-NoV)*(mipB - mipA), 0, 8);         // r = perceptual roughness
float3 env = IndirectSpecularTextureCubeArray.SampleLevel(trilinear, float4(R, cubeIndex), mip).rgb;
float3 Fe  = F0 + (1-F0)*pow5(1-NoV);
float  k   = 1 - 0.5*saturate(2*r - 1);
float  horizon = saturate(1 + 1.3*dot(R, Nvtx));
float  occ = saturate((lum(indirectBaked) + len(skyVec)*lum(c_skyColor)) * g_cubemapSamples[cubeIndex].ambient) * horizon;
float3 envSpec = env * Fe * k * occ * (4*NoV*NoV) / (2*max(2*NoV*lerp(NoV, 1, α), 0.00125)) * c_envMapLightScale;
//            = env*Fe*k*occ*NoV/lerp(NoV,1,α) away from the clamp
```
- `indirectBaked` is `bakedIrr` (world, the RGBM *before* `N·Nvtx`), or the SH L0 `(SH[0].w, SH[1].w, SH[2].w)` for props
  [H]. `skyVec` is `lm1.rgb` or `c_modelInst.lighting.skyDir` [H].
- **`g_cubemapSamples[i].ambient` = lump 0x2B `CUBEMAPS_AMBIENT_RCP`**, one float per cubemap. It is a reciprocal
  ambient, so a cubemap's reflection is renormalised to the local baked brightness [M].
  **On staging_mu1 that lump is all zeros, which would zero every reflection.** Either the export lost it or the engine
  fills it at load [unknown]. Fallback: `1 / lum(cubemap's lowest-mip average)` [L].
- `cubeIndex`: world = vertex `CUBEMAP_ID` (§8); props = `c_modelInst.cubemapID` (from 0x68 `cube_index`) [H].

## 6. Static props (`rgdp`, and `rgdc` uses the same PS): probe lighting [H shader / M-L decode]

`CBufModelInstance.lighting` (cb2 offsets) replaces the lightmap:
```hlsl
float4 ambientSH[3];   // @144: per channel R,G,B: xyz = L1 vector, w = L0
float3 skyDir;         // @192: same meaning as lm1.rgb (sky-scaled bent normal)
uint   packedSunData;  // @204: hi16/32768 = baked sun vis (gate + fallback), lo16/32768 = extra multiplier after CSM
uint4  packedLightData;// @208: x = 4 x u8 weights; y = idx0|idx1<<16; z = idx2|idx3<<16; w = dynamic list (4 x u8, &127)

float3 sh = max(float3(dot(N,SH[0].xyz)+SH[0].w, dot(N,SH[1].xyz)+SH[1].w, dot(N,SH[2].xyz)+SH[2].w), 0);  // L67-73
float3 indirectDiffuse = sh * saturate(dot(N, Nvtx)) + skyTerm(N, skyDir) * c_skyColor;               // L77-86
// static lights (L615-910): for i in 0..3: w = byte_i(x)/255; if w==0 skip;
//   L = s_globalLights[idx_i]; full section-4 evaluation; lightAtten *= w   (shadow-mapped lights: atten*=sh, still *w)
//   light whose index == cascade3LogicData-2 also takes min() with the cascade-3 perspective shadow (cascade3LightDir/X/Y, CotFov, ZNearFar)
```
Evaluation is plain `dot + w`, a pre-convolved L1 irradiance in the same π-folded units as the lightmap. There is no
SH basis constant in the shader [H].

### Lump 0x65 `LightProbe` (44 B) → instance constants
| field | decode | conf |
|---|---|---|
| `ambient_sh[12]` i16 | groups of 4 = R,G,B; each `(L1x, L1y, L1z, L0)`. Data: the three groups are near-proportional and L0 is never negative | layout H |
| | scale **1/8192** → L0 range [0, 4.0], which equals the lightmap RGBM ceiling (3.984375+1/64). 1% of probes saturate at 32767 | L, calibrate vs lightmap |
| `sky_dir_sun_vis[3]` i16 | `skyDir = v/8192` (max length = 8192.7, median 5825 ⇒ visibility-scaled, same as lm1.rgb) | M |
| `sky_dir_sun_vis[3]` | `sunVis = v/8192` ∈ [0,1] (59% exactly 8192). Pack `packedSunData = (u16)(sunVis*32768) << 16` and, lacking better info, **low16 = 0x8000 (1.0)**, so baked vis is not squared | M / lo16 unknown |
| `static_light_weights[4]` u8 | → `packedLightData.x` bytes (weight/255 = baked visibility of that light at the probe) | H (shader) |
| `static_light_indexes[4]` i16 | index into the same 0x8-flag list as 0x7A (max 403, -1 = none) → global index `+32` (the world RTL path adds 32 in-shader; the prop path does not, so the CPU must) | M |

The shader receives one probe per instance and no interpolation [H].

### Which probe a static prop gets (2026-11-17) [H data]
**Lump 0x66 `STATIC_PROP_LIGHTPROBE_INDEX`: one u32 per static prop record, an index into 0x65.** On staging_mu1 it has
11198 entries (= props), values 0..10786, and lump 0x65 splits in two: probes **0..10786 are per-prop probes** baked
for the props themselves, the remaining 41670 are what the k-d tree (0x67 nodes, 0x68 refs) points at (only 2 indices
overlap). Per-prop probes are less directional (median |L1|/L0 0.62 vs 1.25 for tree probes; foliage probes often have
L1 = 0) and their sun visibility agrees with the tree probe at the prop's origin 82% of the time. The env cubemap is the
record's halfword 3 (`StaticProp::unknown_3`, values 0..15 = the 16 cubemaps of 0x2A; it is the nearest cubemap only
34% of the time, so it is assigned, not computed). Flag 0x02 (USE_LIGHTING_ORIGIN) only moves where the bake sampled
that probe; at runtime the index is all that matters.

The k-d tree is for everything that isn't a static prop (dynamic props, characters, weapons). r5rust first walked the
tree at each merged prop component's centre: on the firing-range tower and town kit (`firing_range_kit_mat_*`,
`live_fire_*`) that landed on probes inside/behind the walls and they rendered black (indirect ~0, no sun).
`export-map` now writes each prop's `0x66` probe | cubemap << 24 on every vertex (glb attribute `_LIGHTING`, u32);
`lighting::assign_probes` uses it and keeps the tree walk only as a fallback for older exports.

Neither the world (wldc) nor the prop (rgdp) PS reads `SV_IsFrontFace`: a double-sided surface is lit with its vertex
normal on both sides [H]. Foliage needs this (grass normals point straight up); flipping the normal on back faces
turned the back of every grass blade black under its dim per-prop probe.

Difference to world: no AO texture or vertex colour in this PS; everything else (BRDF, sun/CSM, env, fog, outputs) is
byte-identical code [H]. `sknp` (characters/weapons) was not analysed.

## 7. Final composite, fog, exposure [H] (world L1029-1086)
```hlsl
float3 indirectD = albedo * indirectDiffuse * ao;            // ao: world only
float3 diffuse   = albedo * (diffAcc + albedo * multiAcc);
float3 color     = diffuse + specAcc + envSpec + (c_separateIndirectDiffuse ? 0 : indirectD);
// height fog, k0..k4 = c_fogParams (cb3[11..15], offset 176..240)
float  dist = max(length(camRelPos) + k1.w, 1e-5);
float  zLo = min(worldPos.z, c_cameraOrigin.z), dz = max(max(worldPos.z, c_cameraOrigin.z) - zLo, 1e-5);
float2 f   = saturate((k4.xy - zLo) / dz);
float  a   = f.x*dist,  b = dist*f.y - a,  c = dist - dist*f.y;
float  dens0 = saturate((zLo - k4.x)*k4.z)*k0.z + k0.x;
float  grad  = saturate(dz*k4.z)*k0.z / dist;
float  od  = (b*grad*0.5 + dens0)*b + (c*k0.y + k0.x*a);
float  fog = (1 - exp(-od)) * k0.w;
float  lobe = sq(saturate((dot(k3.xyz, viewRay) - k2.w) * k3.w));
color = lerp(color, k1.rgb + k2.rgb*lobe, fog);
o0 = float4(c_desaturate ? lum(color).xxx : color, 1);       // linear HDR
o4 = (1 - fog) * (c_separateIndirectDiffuse ? indirectD : 0); // indirect-diffuse target for SSAO/GI composite
```
- **No exposure and no tonemap in the forward shader** [H]. Both happen in post. The post shader is not identified, so
  it is unknown. `c_exposureInteractionRate` and `c_lightingGlobalMultiplier` exist in the cbuffers but are unused here.
- Fog params are set per level/script: `c_fogParams` k0..k4. Which `.ent` keys feed them is unknown.
- The other targets are TAA/SSR/streaming data and are not needed for a match: o1/o2 velocity and DoF, o3 texture-streaming
  feedback (`ddx(uv)*4096`), o5 = `N, max(envSpec)`, o6 = packed `Fe`, envSpec, mip and cubeID for SSR.

## 8. Vertex decode (world VS `0x834C6E…`, prop VS `0x8815…` share the normal code) [H]
```hlsl
// NORMAL (u32): bits 0-9 tangent angle, 10-18 a, 19-27 b, 28 sign, 29-30 major axis, 31 binormal sign
float A = (bits10_18) - 256, Bc = (bits19_27) - 256, M = sign ? -255 : 255;
float3 v = float3(A, M, Bc) * rsqrt(A*A + Bc*Bc + 65025);
n = axis==0 ? v.yzx : axis==1 ? v.xyz : v.zxy;          // object space
// tangent: branchless ONB around n, rotated by angle = (bits0_9) * 2π/1023
s = 1/max(n.z+1, (n.x²+n.y²)/2);  b1 = (1 - n.x²*s, -n.x*n.y*s, -n.x);  b2 = (-n.x*n.y*s, 1 - n.y²*s, -n.y);
if (n.z+1 < 0) { b1 = (0,-1,0); b2 = (-1,0,0); }
T = b2*sin(angle) + b1*cos(angle);     B = cross(N, T) * (bit31 ? -1 : 1);     // after object->world
// world TEXCOORD0 (uint2): uv = float(int((u >> 3) - 0x10000000)) / 16384 ; CUBEMAP_ID = ((v & 7) << 3) | (u & 7)
// world TEXCOORD1 (uint2): lightmap texel = t / 64 ;  COLOR: rgb = pow(|c|, 2.2)
// prop POSITION (uint2): x = (lo & 0x1FFFFF), y = (lo>>21) | ((hi & 0x3FF)<<11), z = hi>>10;  pos = xyz/1024 - (1024,1024,2048)
```

## 9. Decals and overlays: depth bias, not draw order alone (2026-11-17) [H]

Decals, puddles, stencils, bullet holes, the road-track / sand `_transparent` overlays and prop decal sheets
(`cargo_container_decals_01`) are separate meshes lying exactly on the surface under them. Drawn with the plain
`GreaterEqual` test and no bias they z-fight with it: whether a decal pixel passes depends on the rounding of two
interpolated depths, so it breaks into flickering holes as the camera moves (Kainz's spots k1 to k5 in
`tools/zscan.py`).

**What the material says.** Every RSX material json carries `rasterizerFlags` and `depthStencilFlags`. On this map:

| rasterizerFlags | depthStencilFlags | materials |
|---|---|---|
| `0x6` / `0x2` | `0x17` | opaque world + props (`0x2` = no culling: tarps) |
| `0x16` | `0x7` | all 38 decal-class materials: `world/decal/*`, `*_decal*`, `s2s_bolt_decals`, `road_tracks_dry_03_blend_2_trans`, `sand_desert_02_bm_transparent`, `sand_desert_02_wld_transparent_scroll`, `stain_02` ... |
| `0x6` | `0x7` | glass, energy, other blended non-decals |

**How the engine reads them** (`r5apex_dx12.exe`, S21):
- `FUN_1405177d0(rasterizerFlags, desc)` builds the rasterizer state: `desc[0] = !(f & 1)`, cull = `f & 4 ? (f & 2 ? 2 : 1) : 0`,
  `desc[2] = (f >> 3) & 1`, and the **depth bias class `f & 0x70`**: `0x00` normal, `0x10` decal, `0x20` shadow map,
  `0x40` tight shadow map, `0x50` UI, anything else z-fill. For every class but the two shadow ones it writes
  `DepthBias = -mat_depthbias_X`, `SlopeScaledDepthBias = -mat_slopescaledepthbias_X`, `DepthBiasClamp = -mat_depthbiasclamp_X`
  (the `* -1.0` at `DAT_1417f8e04`): the convars are written for a normal depth buffer and the main view is reverse-Z.
- Convar defaults (static ConVar inits at `0x1400be2a6..`): decal `-16 / -4 / -0.001`, UI `-50 / -1.7 / -0.001`,
  normal `0 / 0 / 0`, z-fill `16 / 2 / 0`, shadow map `0 / 2 / 0`, tight shadow `10000`.
- `FUN_140517990(depthStencilFlags, desc)`: depth enable = `f & 1 | write`, write = `(f >> 4) & 1`, compare =
  `lut[(f >> 1) & 7]` (`ALWAYS` if depth is off), stencil = `(f >> 7) & 1`. Decals (`0x7`) use the same compare as
  opaque (`0x17`), only without depth write.

So a decal is drawn with the world's own compare, no depth write and **DepthBias 16, SlopeScaledDepthBias 4,
DepthBiasClamp 0.001** towards the camera. On a `D32_FLOAT` target that is 16 units in the last place of the
primitive's largest depth plus 4 x its depth slope, clamped to 0.001 in depth.

**Ours.** `export-map/build_materials.py` copies both flags into `materials.json` (`raster_flags`, `depth_flags`). The loader
gives every draw whose material has `raster_flags & 0x70 == 0x10` a copy of its pipeline with
`DepthBiasState { constant: 16, slope_scale: 4.0, clamp: 0.001 }` (Vulkan applies it the same way on `Depth32Float`),
and sorts those draws after the rest of their opaque / alpha-tested / blended class. `R5_NODECALBIAS=1` turns it off.

**Evidence.** `python tools/zscan.py OUT --spots` renders each pose twice, the second with the camera 0.1 unit off, in two
debug views (`--debug 11` flat colour per material, `--debug 12` view distance) and counts pixels where the material
flips while the distance does not (dense patches only, so silhouettes and creases do not count). Over the 27 poses of
`r5f-probe/poses.json` plus k1..k5 at 8 yaws: **25139 pixels before, 938 after**, and every pair left is an edge
of a trim seen from a few units away, not a decal. Before, 100 % of the hits were a decal-class material against
the surface under it (`water_puddle_ground_decal_01` / rubber floor 7500, `cement_stripes_decals_vert` / wall trim 4280,
`grunge_decal_drips`, `firingrange_town_decal_sheet_01a`, `bullet_hole_metal_01`, `cargo_container_decals_01`, ...).
The map00 / map08 / map18 renders change by a mean 0.85 / 0.20 / 0.03 levels (decals that were missing pixels).

## Implementation notes

1. Work in **linear HDR**, keep the π-folded units, and never divide by π. Apply exposure and tonemap in a separate post
   pass. Apex's curve is not in these shaders, so pick one and calibrate.
2. World pixel = `albedo*(Σdiff + albedo*Σmulti) + Σspec + envSpec + ao*albedo*(lmRGBM*N·Nvtx + skyTerm*skyColor)`, then fog.
   That is the whole equation. The dominant visual terms are the RGBM lightmap, the sky term, and the sun with baked/CSM visibility.
3. The sun needs `lm1.a > 0`. Without CSM, `sunShadow = lm1.a`, which is exactly what the game does beyond the cascades.
   A single CSM can replace it near the camera: `sun = lerp(lm1.a, csm, fadeWeight)`.
4. RTL page: upload IDs as `R32_UINT`, visibility as `BC5` of size w×2h. Build `s_globalLights[32+i]` from the flag-0x8
   worldlights in lump order, using the §4.4 mapping. Inverse-square (lump 0x20) lights have intensities around 1e4, so
   getting the flag wrong is obvious on screen.
5. Props: one probe per instance, the one lump 0x66 names (not a tree lookup); decode with ×1/8192 (calibrate L0 against the lightmap under the same light, the only
   unconfirmed scale). Pack the static-light indices +32.
6. The cubemap reflection is multiplied by `CUBEMAPS_AMBIENT_RCP`. If the lump is zero, compute the reciprocal from the
   cubemap, or every reflection disappears.
7. Normal maps: `xy = (byte-128)/88.92`, `N = normalize(T*x + B*y + Nvtx)`. Do **not** reconstruct z or renormalise xy
   to the unit disk.
8. To verify any item: `python -I scratchpad\shader-re\dxdis.py <prog>_N.fxc out.txt`, set PS = `u64 @5` in the `.msw`.
