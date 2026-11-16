# Apex (S21 / R5) post-processing chain, from the shipped shaders

The source is the compiled DXBC in `D:\R5Rust\assets\shaders\programs`. RDEF names are intact. Engine (non-material)
shaders were found by cataloguing every program whose bindings don't include `CBufModelInstance`/`CBufUberStatic`
(960 programs), then reading the post-related ones. Dumps are in the session scratchpad,
`...\scratchpad\shader-re\post\*.asm` / `*.short`. `EP L###` = line in `ENGINEPOST_0xB93D3531CBE5AD02_0_numbered.asm`.
Tags: **[H]** read from instructions/names · **[M]** inference · **[L]** guess · **unknown**.

Two DXBC traps when reading these dumps: `exp`/`log` are base 2, and a `log` followed by `mul 0.693147` is `ln`.

## 0. The passes and how they connect

| # | Pass | Program | Reads | Writes / evidence |
|---|---|---|---|---|
| 1 | Forward opaque | world/prop PS (see `apex-lighting.md`) | — | HDR scene (linear, pre-exposure, alpha = 1), separated indirect diffuse, velocity |
| 2 | SSAO + indirect-diffuse composite | `0x81A3DCFB86AD90AE`, `0xBDA739F0DA9F48F8` (`CBufSSAODynamic`: `aoTex`, `indirectDiffuseTex`, `sssBuffer`) | forward MRTs | adds AO'd indirect diffuse back [M] |
| 3 | **TSAA** | `0x3C43F20F19A63E6` (`CBufTSAA`) | `CurFrame`, `PrevFrame`, `Mrt2*`, `VelocityBuffer`, `ZBuffer`, **`ExposureTexture` (t61)** | resolved HDR + history [H] |
| 4 | **Luminance meter** | `0x2A6ED216764B910E` (`CBufLogLuminance`) | `srcTexture` = HDR frame | luminance RT [H] |
| 4b | reduce to 1×1 | `0xF8C1EE7E1430C3D3_4` (`CBufDownsample`, 4 bilinear taps × 0.25) | luminance RT | [M] which downsampler is used |
| 5 | **Exposure adaptation** | `0xD6729E1D8B437577_0` (`CBufExposureAdaptation`) | `srcTexture` (1×1 avg lum), `exposureTexture` (previous) | new `exposureTexture` [H] |
| 6 | **Bloom prefilter/downsample** | `0xF8C1EE7E1430C3D3_0.._3` (`CBufDownsample`: `c_bloomCutoff…`) | HDR frame, **`exposureTexture`** | thresholded half/quarter-res [H] |
| 7 | **Bloom blur** | `0xB836245D2ECF8C41` PS / `0x96B3028C9A088D04` CS (`CBufBlurDynamic`) | prefiltered | `BaseTexture`, `WideBloomTexture`, `StreakBloomTexture` [M] |
| 7b | DoF (optional) | `CBufDoFBlurFilter` CS, `0xF8C1…_6/_7` (CoC downsample) | | `DoFBlurSmallTexture`, `CoCTexture` |
| 8 | **Engine post composite** (sharpen, DoF, bloom add, exposure, HDR LUT, tonemap, LDR LUT, desat, colour scale) | `0xB93D3531CBE5AD02` (`CBufEnginePost`, `CBufEnginePostPerDraw`; `_1`/`_3` add LCD screen effect, `_2`/`_3` add debug views) | `FBTexture`, `BaseTexture`, `WideBloomTexture`, `StreakBloomTexture`, `DoFBlurSmallTexture`, `CoCTexture`, `ColorCorrectionVolumeTexture0-2` (3D), `HDRColorCorrectionVolumeTexture0-2` (3D), **`ExposureTexture`** | display-linear LDR [H] |
| 9 | Dynamic-res upscale | `0xB301B13C1B1A8B2A` (`CBufUpscale`: `c_fbMaxUVs`) | `CurFrame` | bilinear stretch [H code; M position] |
| 10 | **Gamma/brightness** | `0xC4B71129BD056B0F` (`CBufGammaControl`) | `srcTex` | swapchain [H] |
| 11 | UI (RUI) | `bink*`/`fontTexture` shaders | | not covered |

The order comes from the dataflow:
- `ExposureTexture` is read by TSAA, the bloom prefilter and the composite, and written only by the adaptation pass.
- The bloom names (`BaseTexture`, `WideBloomTexture`, `StreakBloomTexture`) are inputs of the composite only.
- The composite and the upscale both carry `c_fbMaxUVs` (dynamic-resolution viewport).

TSAA running *before* the luminance meter is [M]. TSAA uses the exposure texture at its start, which at that point holds
the previous frame's value.

Not present in any post shader [H]: **vignette, film grain, dithering, lens chromatic aberration**. The only aberration
is the cloak/Wraith effect (`c_cloakAberrationScale`, flag bit 3). There is no FXAA; AA is TSAA only.

## 1. Auto exposure [H]

**Meter** (`CBufLogLuminance`; the "Log" in the name is misleading, because the code averages *linear* luminance):
```hlsl
float2 uv = ((texUV - 0.5) * c_spotMeteringScreenRatio + 0.5) * c_framebufferViewportScale;  // centre spot only
out = min(lum709(src.Sample(uv).rgb), c_maxExposureLuminance);  // lum709 = dot(c, (0.2126, 0.7152, 0.0722))
```
- No histogram and no log average. It is an arithmetic mean of per-pixel clamped luminance over a centred window that
  covers `c_spotMeteringScreenRatio` of the screen [H].
- Reduction to one value is done with repeated 2×2-bilinear box downsamples, `0xF8C1…_4`:
  `0.25 * Σ src(uv ± 1 texel)`, i.e. 4×4 per pass [M]. The adaptation pass then samples the 1×1 result at (0.5, 0.5).

**Adaptation** (`CBufExposureAdaptation`, run once per frame, writes a tiny RT) [H]:
```hlsl
float avg  = lumTex.Sample(float2(0.5, 0.5)).x;
float e    = (avg < c_targetLumMin) ? c_targetLumMin / avg
           : (avg > c_targetLumMax) ? c_targetLumMax / avg : 1.0;   // dead band: no correction inside [min,max]
float target = e * exp2(c_exposureCompensation);                    // compensation in EV stops
float prev   = exposureTex.Load(int3(0,0,0)).x;
float rate   = saturate(c_adaptationRate / max(prev, target));      // same formula up and down
float newE   = prev + (target - prev) * rate;   if (isnan(newE)) newE = 0;
newE = clamp(newE, c_exposureMin, c_exposureMax);
if (c_snapExposure) newE = target;                                  // snap skips the clamp
// texel (0,0) = newE (linear multiplier); (1,0) = ln(newE); (0,1) = target; other texels = ln(target)
```
- The target is *not* "key / average". It is a band: exposure is only pushed when average luminance leaves
  `[c_targetLumMin, c_targetLumMax]`, and then just enough to put it back on the nearest edge.
- `c_adaptationRate` has no frame-time term in the shader, so the CPU must pre-scale it by dt [M].
- Because the rate is divided by `max(prev, target)`, the step is roughly a constant *relative* change per frame.
  Adaptation up and down are symmetric [H].
- `c_forcedExposure` (variant `_1`) writes a fixed exposure into every texel [H].
- Exposure is applied **once, in the composite**: `color *= ExposureTexture[0,0].x` (EP L153-154). The forward shaders
  don't apply it.
- `c_exposureInteractionRate`, `c_overrideExposureInteractionRate` and `c_exposureCounteractionType` live in
  material/particle/UI cbuffers. They let emissive and UI materials counteract exposure; they play no part in scene
  exposure.
- `c_lightingGlobalMultiplier` is declared in `CBufCommonPerCamera` but unused in every forward/post shader read.

## 2. Bloom

**Prefilter downsample** (`CBufDownsample`; variants differ only in tap pattern: `_0` 1 tap, `_1` 2 taps ±1 px
horizontal, `_2` 4 point loads (2×2), `_3` 4 bilinear taps at ±1 px = 4×4 box) [H]:
```hlsl
float E = exposureTex.Load(0).x;
float cutoff = lerp(c_bloomCutoff, c_bloomCutoff * E, c_bloomCutoffExposureAdapt);
float3 tap(float3 c) {
    c = pow(max(c, 0), c_bloomExp);                       // (variant _0 multiplies by c_preScaleFactor before the pow)
    float L = lum709(c) * E;                              // exposed luminance
    float w = min(min(L, c_bloomClamp), max(2*(L - cutoff), 0)) / L;   // soft knee: slope 2 above cutoff, capped
    return (L > 0) ? c * w : 0;                           // NOTE: result is in UNEXPOSED units
}
out.rgb = average(taps) * c_preScaleFactor;   out.a = c_alpha;
```
- The threshold is applied in exposed space, but the stored bloom is pre-exposure. That is consistent with step 3,
  where bloom is added before the exposure multiply [H].
- **Blur**: a generic separable kernel [H]: `center*c_centerWeight + Σ_i (s(+o_i)+s(-o_i))*w_i`, with up to 18
  symmetric tap pairs packed in `c_sideTapPacks[9]` (`.x`=offset, `.y`=weight, `.z`/`.w` the second pair), along
  `c_offsetToNeighborPixel`. **Weights, sigma, tap count and the number of bloom mips are CPU-side: unknown.**
- Three bloom images reach the composite [H]. `BaseTexture` (normal bloom), `WideBloomTexture` (sampled from a "tiny"
  RT with 4 taps at (-4.5,-4.5), (3.5,3.5), (-4.5,3.5), (3.5,-4.5) full-res texels, or 1 tap if flag bit 11 is set) and
  `StreakBloomTexture` (presumably the same blur, 1-D horizontal [M]).

**Composite add** (EP L141-154) [H]:
```hlsl
float3 bloom = c_bloomAmount * Base + c_wideBloomAmount * Wide
             + c_streakBloomAmount * pow(abs(Streak), c_streakBloomExponentPost);
color = (sceneHDR + bloom) * exposure;          // bloom added BEFORE exposure
```

## 3. Engine post composite: exact order (`0xB93D3531CBE5AD02_0`)

```hlsl
// a) Sharpen (always on, EP L11-57): 4 diagonal bilinear taps at ±c_sharpenWidth texels around the full-res pixel
float L0 = lum(c), Ld[4] = lum(taps);  float ratio = min5(L) / (max5(L) + 1e-5);
if (ratio > c_sharpenThreshold)        // only in LOW-contrast areas (avoids halos on hard edges)
    c *= max(L0 - c_sharpenAmount * (avg(Ld) - L0), 0) / (L0 + 1e-5);   // luminance-only unsharp mask
// b) bit3: cloak (aberration via FB.a codes)       c) bit5: DoF lerp toward DoFBlurSmall by 4*max(|blur.a/2|, CoC)
// d) + bloom, * exposure  (section 2)
// e) HDR colour correction (if c_HDRColorCorrectionInfo & 3), EP L155-214:
float3 pq = PQ_encode(c * 0.01 * min(1, 100/max3(c)));   // 1.0 scene = 100 nits; hue-preserving clamp at 10k nits
float3 uvw = pq * (32/33.) + 0.5/33.;                     // 33^3 LUT
float3 g = Σ_k w_k * HDRLUT_k.Sample(uvw);                // w = c_HDRColorCorrectionVolumeWeights.xyz, k < (info & 3)
// unless bit3: g *= saturate(lerp(max3(pq), max3(g), alpha) / max3(g))   (alpha = sat(FB.a + max3(streak)) = 1 for opaque)
c = c * saturate(1 - Σw) + PQ_decode(g) * 100;
// f) Tonemap (only if c_enginePostBranchFlags bit 8), see section 4
// g) c = c * c_viewFadeScale + c_viewFadeBias                                         (EP L1303)
// h) LDR colour correction (if c_colorCorrectionVolumeWeights.x > 0), EP L1304-1344:
float3 s = sRGB_encode(c) * (31/32.) + 0.5/32.;          // exact piecewise sRGB, 32^3 LUT
float3 l = Σ_k w_k * LUT_k.Sample(s);                     // up to 3 volumes, weights c_colorCorrectionVolumeWeights
// same alpha-based max-channel renormalisation as (e) unless bit3
c = c * saturate(1 - Σw) + l;                            // => LUT outputs LINEAR colour (blended with linear c)
// i) bit6: c = lerp(c, lum(c), c_desaturationLerp); bit7 keeps hues whose normalised CbCr
//    dot c_desaturationExcludeNormalizedCBCR > c_desaturationExcludeDotRangeBegin (ramp *DeltaInverse)
// j) c *= c_postColorScale;   k) bit4: Wraith-phase overlay (c_phase_*)   l) out = c * c_finalLuminanceScale, a = FB.a
```
There is **no sRGB encode at the end** of the composite [H]. The output is display-linear and the swapchain or
intermediate RT does the encoding, which fits the gamma pass in §5 [M].

## 4. Tonemap: `c_tonemapFunction` selects at runtime (EP L215-1300) [H]

Input is exposed scene-linear Rec.709 after the HDR LUT. Parameters: `c_tonemapShoulderOrMaxValue` = **S**,
`c_tonemapToe` = **T**, `c_tonemapMid1` = **M1**, `c_tonemapMid2` = **M2**.

| value | curve |
|---|---|
| **0 / default** | `c*(10c + 0.3) / (c*(10c + 0.5) + 1.5)` per channel. No parameters; the asymptote is 1 |
| **1** | rational: `c*(M1*c + T) / (c*(M1*c + S) + M2)` per channel (same form as the Narkowicz/Hill ACES fit, parameterised) |
| **2** | **ACES 1.0.3 reference RRT + ODT "RGB monitor 100 nits dim" (sRGB)**, the full thing: sRGB→XYZ→Bradford D65→D60→AP0, glow module (gain 0.05, mid 0.08), red modifier (hue window 135°, scale 0.82, pivot 0.03), RRT sat 0.96, segmented spline c5 (coefs in the ICB), ODT spline c9 48-nit, `(Y-0.02)/(48-0.02)`, dim-surround gamma **0.9811**, ODT sat **0.93**, D60→D65, XYZ→sRGB, `saturate` |
| **3** | same as 2 but surround gamma **0.835** and ODT saturation **0.835** (non-standard Respawn variant) |
| **4** | hue-preserving max-channel exponential shoulder: `x = c/S; m = max3(x); m' = m < M2 ? m : M2 + (1-M2)*(1 - exp(-(m-M2)/(1-M2))); c = x*(m'/m)*S` |
| **5** | Frostbite-style ICtCp tonemap (Fry 2017). `c/S` → Rec.709→LMS → PQ (1.0 = 100 nits) → ICtCp, with Ct/Cp scaled by `pow(smoothstep(saturate((1-I)/0.7)), 1.3)`. Back to linear, clamp [0,100]. Shoulder: per-channel and max-channel versions of the type-4 curve (knee M2), mixed 0.6 toward max-channel. Back to ICtCp: I from the tonemapped value, CtCp = `lerp(tm, orig*(I_tm/max(I_orig, 0.001)), 0.3*smoothstep(saturate(2*(1-I))))`. Then linear, clamp [0,100], `*S` |

The ICB constants for function 2/3 are the stock ACES `coefsLow/High` (RRT c5: −4…1.8477 / −0.7185…4.0; ODT 48-nit
c9: −1.699…0.9114 / 0.5154…1.6879). They are in `0xB93D3531CBE5AD02_0.short` [H].

**Which function and parameters S21 actually uses is not in the shaders.** It is a runtime constant
(convar/script/level) [unknown]. If you have to pick one: 2 (stock ACES sRGB), because it is the only one with fully
baked constants and it matches Apex's saturated-highlight look [L]. Verify against a reference screenshot.

## 5. After the composite

- **Upscale** (`CBufUpscale`): `CurFrame.Sample(min(uv, c_fbMaxUVs))`, bilinear, no sharpening (sharpening happened
  in the composite) [H].
- **Gamma control** (`CBufGammaControl`) [H]:
  - `_0`: `out = src * c_finalOutputScale`.
  - `_1`: user brightness/gamma.
    `g = (x <= xstart) ? x*ystart/xstart : (1+offset)*pow(x, c_scaledGamma) - offset`, using the
    `c_linearToScaledGammaTransFunc_*` parameters. Then the exact sRGB EOTF `((g+0.055)/1.055)^2.4` (linear below
    0.04045 /12.92), then `* c_finalOutputScale`.
  - With the default sRGB parameters (gamma 1/2.4, xstart 0.0031308, ystart 0.04045, offset 0.055) this is the
    identity, so the source is display-linear and the target is an sRGB-format swapchain [M].
- **No dithering anywhere in the chain** [H].

## 6. What depends on values we can't see

| Field (cbuffer) | Source | Status |
|---|---|---|
| `c_spotMeteringScreenRatio`, `c_maxExposureLuminance` (CBufLogLuminance) | engine/convar | unknown |
| `c_exposureMin/Max`, `c_exposureCompensation`, `c_adaptationRate`, `c_snapExposure`, `c_targetLumMin/Max`, `c_forcedExposure` | engine/convar/script | unknown |
| `c_bloomCutoff`, `c_bloomClamp`, `c_bloomCutoffExposureAdapt`, `c_bloomExp`, `c_preScaleFactor`; blur `c_sideTapPacks`/`c_centerWeight`; mip count | engine | unknown |
| `c_bloomAmount`, `c_wideBloomAmount`, `c_streakBloomAmount`, `c_streakBloomExponentPost` | engine/script | unknown |
| `c_sharpenAmount/Width/Threshold` | user setting (likely "sharpening" option) | unknown |
| `c_tonemapFunction`, `c_tonemapShoulderOrMaxValue/Toe/Mid1/Mid2` | engine/convar | unknown |
| **HDR LUT** (`HDRColorCorrectionVolumeTexture*`, 33³ PQ-encoded) | map entity `color_correction`: staging_mu1 has `"filename" "materials/correction/mp_rr_desertlands_hu_hdr.raw_hdr"`, `maxweight 1`, falloff −1 (global) | the file is in the game VPK, **not in our extraction** |
| LDR LUTs (32³, sRGB-encoded input) | script / ability effects | unknown, probably unused in normal play |
| `c_viewFadeScale/Bias`, `c_postColorScale`, `c_finalLuminanceScale`, desaturation, cloak/phase | gameplay effects (damage flash, abilities) | default identity: scale 1, bias 0 |

## Implementation notes

1. Order to implement: HDR scene → TAA (optional) → centre-spot mean luminance → adaptation → bloom prefilter
   (exposure-aware knee) → separable blurs (base + wide + streak) → composite:
   `(scene + bloom) * exposure → [HDR LUT] → tonemap → [LDR LUT] → postColorScale` → sRGB swapchain.
2. Store exposure as a linear multiplier in a 2×2 R32F texture. Adaptation is a dead-band controller, not "key/avg".
   Starting guesses [L]: `targetLumMin ≈ targetLumMax ≈ 0.18` (degenerates to classic key=0.18), rate ≈ 1-2/s × dt
   relative, spot ratio ≈ 0.5-0.7.
3. Bloom knee: `w = min(min(L·E, clamp), 2(L·E − cutoff))/(L·E)`, applied to the *unexposed* colour.
4. To get the HDR LUT, extract `materials/correction/mp_rr_desertlands_hu_hdr.raw_hdr` from the R5 VPK. Expect 33³
   texels in PQ space (input = PQ(c·0.01), output decoded ×100). Its byte layout (likely RGB f16/f32) is unknown.
   Without the file, skip stage 3e; the HDR LUT weights default to 0.
5. Tonemap: implement ACES function 2 exactly, as the ACES 1.0.3 CTL reference `RRT` + `ODT.Academy.RGBmonitor_100nits_dim`
   (the constants above are those files' constants). Add switches for 0/4 to compare looks.
7. TSAA (if you do TAA): it works in YCoCg with exposure-aware inverse-luma weighting, `ycocg / (1 + Y·exposure)`. It
   uses a min/max neighbourhood clamp built from quad derivatives plus cross taps, and closest-depth velocity dilation
   from 4 taps at ±2 px. Blend uses `c_curFrameBlendAmount`, raised on disocclusion and high velocity
   (`c_blendFactorIncreaseWhenUnoccluded`, `…AtMaxVelocity`, `…MaxesOutAtVelocityInv`) [H names, M detail].
6. Sharpen is cheap and always on: luminance unsharp mask gated by a min/max contrast ratio.
