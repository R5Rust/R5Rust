# R5Rust handoff (2026-11-16)

Start a fresh session here instead of continuing the old one (it got huge and ate the usage limit).

## Goal (v1)
Own from-scratch engine: **client `r5rust` + headless dedicated server `r5rust_ds`** (like R5Flowstate's
r5apex.exe / r5apex_ds.exe). Loading screen with the real Firing Range image, spawn at the real spawn, walk /
sprint / jump / fly (V) with collision. No UI, no weapons yet. Later: HUD from apexkit (`C:\Code\apex\apex-content-kit`).

## Stack (decided)
Rust workspace: `crates/shared` (map load, rapier3d collision, deterministic `simulate()`, net types),
`crates/server` (UDP 127.0.0.1:37015, 60 Hz, authoritative), `crates/client` (wgpu: Vulkan first, DX12/GL
fallback; WGSL; client prediction + reconciliation; spawns its own local server unless `--connect`).
Logo: `assets/logo.png` (window + exe icon).

## Data (owner's own extraction, never commit it)
- Map: `D:\R5Rust\assets\maps\mp_rr_canyonlands_staging_mu1\world.glb` (236 MB) + `meta.json`
  (copied from `C:\Code\apex\r5-web-client\mapcache`, made by its `tools\export-map`). Source units, Z-up.
  meta: `playerSpawns[0]` = (-11740, 512, 1584) yaw 0, `sun`, `lightmap` (TEXCOORD_1; RGB = indirect light /4,
  A = sun visibility), 6.3M tris, 549 draws, 828 embedded images, 11k static props baked in.
  Port the lighting from r5-web-client's shaders.
- Load screen: `D:\R5Rust\assets\images\ui\maps_mp_rr_canyonlands_staging_mu1_loadscreen\0x1EF443938E948449.png`
- All other extracted assets: `D:\R5Rust\assets` (+ `index.json`), extractor + README in `D:\R5Rust`.

## Rules for agents
- Never cat/print world.glb or other big files; inspect with small scripts.
- Verify with a `--screenshot <path> --frames N` render-to-texture flag, look at the PNG. Don't click on the desktop.

## Status (2026-11-16, after 3 usage-limit hits)
`crates/shared` is partly written (assets.rs, glb.rs, collision.rs, ~900 lines, uncommitted after the scaffold);
client/server `main.rs` are stubs. Builders burned the limit on research + a too-big brief.

## Plan for the next session (keep usage low)
- **v1 = one program only**: `r5rust` client loads the map + load screen, renders with lightmaps, walk/jump/fly
  with collision. **No server, netcode or prediction yet** (v2). Keep `crates/shared` so the split stays easy.
- ONE Sonnet builder (not Haiku for engine code), SHORT brief, continue from the existing shared crate, write
  code first and only look things up when a compile error needs it. Don't re-read r5-web-client unless the
  lighting looks wrong. Commit the shared crate first.
- Lead: don't poll; one check when the builder reports.
