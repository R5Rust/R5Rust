# R5Rust

From-scratch Rust engine that loads the Apex Legends **Firing Range** (`mp_rr_canyonlands_staging_mu1`) from
your own extracted assets, and lets you walk around it. Long-term goal: client `r5rust` + headless dedicated
server `r5rust_ds`, like R5Flowstate's `r5apex.exe` / `r5apex_ds.exe`.

## Run

```powershell
cargo run --release --bin r5rust
```

Shows the real loading screen, loads the map (~8 s cold, ~1.5 s warm), and drops you at the real spawn.

| Key | Does |
|---|---|
| WASD / mouse | move / look (mouse is grabbed; click the window to re-grab) |
| Shift | sprint (fly: 3x speed) |
| Space | jump (fly: up) |
| Ctrl / C | fly: down |
| V | toggle fly (noclip-ish: still collides) |
| R | respawn |
| F11 | borderless fullscreen |
| Esc | release mouse; again to quit |

The window title shows fps, position and ground/air/FLY.

## Game data (not in the repo, never commit it)

Default root `D:\R5Rust\assets` (override with `R5RUST_ASSETS` or `assets_root` in `r5rust.toml`):

- `maps\mp_rr_canyonlands_staging_mu1\world.glb` + `meta.json`, made by `C:\Code\apex\r5-web-client\tools\export-map`
- `images\ui\maps_mp_rr_canyonlands_staging_mu1_loadscreen\*.png`

## Config: `r5rust.toml` (optional, gitignored)

Put it in the repo root or next to the exe. All keys are optional:

```toml
width = 1600
height = 900
fullscreen = false
vsync = false
sensitivity = 1.6     # Apex/Source scale (m_yaw 0.022)
fov = 100             # horizontal at 4:3, like Apex
fly_speed = 1200
backend = "vulkan"    # vulkan | dx12 | gl (falls back through the others)
max_texture_size = 1024
```

## Screenshot / test mode (no window)

```powershell
.\target\release\r5rust.exe --screenshot out.png [--loadshot load.png] [--frames 120]
    [--pos x,y,z] [--yaw deg] [--pitch deg] [--hold w,shift,space] [--fly] [--debug 1|2|3]
```

Settles for 30 ticks, holds `--hold` keys for `--frames` ticks (60/s), renders, and prints load stats,
GPU ms/frame, where the player ended up and the jump peak. `--debug`: 1 = sun visibility (lightmap A),
2 = baked indirect light, 3 = albedo only.

## Layout

- `crates/shared`: GLB loader, BVH capsule collision, deterministic `sim::simulate()` (movement), config
- `crates/client`: `r5rust`: wgpu renderer (4x MSAA HDR, reverse-Z, lightmaps + sun, ACES), loader thread, input
- `crates/server`: `r5rust_ds`, a stub for now

## Status (2026-11-16)

v1 works: load screen, Firing Range with baked lighting at ~340-600 fps on a 2080 Ti, walk / sprint / jump / fly
with collision. See `HANDOFF.md` for what's next.

Not done: dedicated server + netcode + prediction (v2), shadows on unlightmapped props (big rocks, some props),
interior lights (the bake has none, so interiors use a flat fill), dynamic props, HUD, weapons, audio.
