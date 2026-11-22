# Object highlight (loot rarity outline): retail, 1:1

Code: `crates/client/src/highlight.rs`, `highlight.wgsl`, `world.wgsl` (`vs_loot_hl` / `fs_loot_hl`), slots in
`loot.rs` (`ground_instances`, `highlight_setting`). Everything below comes from the game's own data and code;
nothing was matched by eye.

## Sources

- **Contexts:** retail `hcxt` asset 0xF4784F882F0199CB (common_early.rpak), 90 contexts. Dumped with the raw
  dumper added to the RSX fork (`rsx-fs/src/game/rtech/assets/highlight_system_raw.cpp`, types `hsys` / `hcxt`).
  The record is 48 B: param0[3], param1[3], radius, f1c, far, bytes [colourId, inside, outline, focusedInside],
  focusedOutline, flags. Names are 16 B: char*, u64 flags.
  - Loot `survival_item_<tier>`: colourId 14..18, inside/outline 135, focused 136/136, radius 1.0, param1 0,
    far 3000.
- **Shaders:** `hsys` 0x2B76C5602DC7F5D1 lists the 12 highlight geometry materials and 6 shaders. Translated from
  the DXIL into `D:\R5Rust\_work_retail\hsys\highlight_shaders.md`:
  - `49D0_N`: horizontal gather.
  - `B323_N`: vertical gather.
  - `2B0C`: composite.
  - `98A0`: apply.
- **Engine (S21 r5apex_dx12.exe, Ghidra):**
  - `FUN_140ac2c20` hcxt record → runtime context (0x34 B):
    - packed = inside | outline << 8 | radiusByte << 16 | flags << 24.
    - radiusByte = int(r · 255 · 0.125 + 0.5).
    - flags: 0x80 = record 0x2B (after post-process), 0x40 = record 0x2A (entity visible).
    - colour: colourId 0xFF → param0, else palette(colourId) · (1/255), raw sRGB values, not linearized.
      `FUN_14052ceb0` reads colorpalette_table column = colourblind mode + 1.
  - `FUN_14078a620`: a focused entity takes the focused functions and radius byte 0x80
    (128 · 8/255 = 4.0157 px).
  - `FUN_14078a320`: the per-entity GPU record.
    - opacities = fade-in/out · `FUN_14078a140` (1.0 for loot).
    - radius = byte · 8/255, no resolution scaling.
  - `FUN_1404d2b50`: highlight buffers at full render resolution, 32-px tiles, 256-entry × 48-B settings buffer.

## What draws

1. ID pass: each highlighted item writes its slot (1..255) to an R8 ID buffer, plus its own reversed-Z depth.
2. Horizontal and vertical gather (radius-limited nearest-owner search): outline pixels within the owner's radius
   whose owner is nearer.
3. Composite:
   - outline 135: `P0 · saturate((1 - saturate((z - 128)/142)) · visible)`, full up to 128 u, gone at 270 u.
   - outline 136 (focused): `P0` where visible, no distance fade.
   - inside 135: `P1.x`-scaled, 0 for loot.
   - inside 136: `(P0 + K(1 + sin(0.15 z + 5 t))) · v`, alpha 0.95 v.
4. The result is packed into retail's RGBA8 encoding, decoded, divided by exposure, and blended
   ONE / ONE_MINUS_SRC_ALPHA into the HDR scene before bloom (that's what makes the soft halo).

Unfocused loot: a 1-px line in the tier colour that fades out between 128 and 270 units. Looked-at loot: a 4-px
band plus the pulsing fill.

## Still assumed

- Whether retail's `primaryDepthBuffer` includes the item. It only changes the focused fill.
- The colorBuffer sampler mode (assumed point).
