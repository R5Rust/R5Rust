# R5Rust handoff

## Goal
Own from-scratch engine: client `r5rust` + headless dedicated server `r5rust_ds` (like R5Flowstate's
r5apex.exe / r5apex_ds.exe). Later: HUD from apexkit (`C:\Code\apex\apex-content-kit`).

## Stack (decided)
Rust workspace: `crates/shared` (map load, BVH capsule collision, deterministic `simulate()`), `crates/server`
(planned: UDP 127.0.0.1:37015, 60 Hz, authoritative), `crates/client` (wgpu, Vulkan first, DX12/GL fallback; WGSL).

## Data
See README. Map meta: `playerSpawns[0]` = (-11740, 512, 1584) yaw 0; lightmap RGB = indirect/4, A = sun visibility.
Never cat/print world.glb. Verify with `--screenshot` and look at the PNG; don't click on the desktop.

## Status (2026-11-16, session 2)
v1 done and verified: load screen, map rendered with lightmaps + sun (rebalanced: bake-only ambient + 0.45 hemi fill
on lightmapped surfaces, sun 3.2, hemi 0.9 for unlightmapped), walk/sprint/jump (with 0.12 s jump buffer)/fly,
collision. 2.9 ms/frame headless at 1600x900, ~600 fps windowed at spawn. Cold load 7.6 s.

## Next (v2)
1. `r5rust_ds`: UDP server running `shared::sim` at 60 Hz; client spawns it locally unless `--connect`.
2. Client prediction + reconciliation (sim is already deterministic and shared).
3. Rendering: shadow map for unlightmapped props, interior light probes, dynamic props from meta.
