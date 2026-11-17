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

## Lighting / post status (2026-11-16, session 3)
Ported from the game's own shaders (docs/apex-lighting.md, docs/apex-post.md): RGBM lightmap + sky vector, RTL lights,
probe SH + static lights for props, Respawn diffuse + GGX, baked BC6H cubemap reflections, sun gated by baked vis with
cached cascades, material textures + albedo/emissive tints (materials.json via tools/build_materials.py), vertex
colours, the Firing Range's sky-shaded 'fake sky' dome (HDR skycube.dds), Apex auto exposure (centre-spot mean, dead
band 0.18 calibrated against a real R5Flowstate shot), bloom before exposure, height fog from env_fog_controller.
world.glb now comes from **tools/export-map** (our copy of r5-web-client's exporter: keeps vertex colours + skies/).
Old exports kept next to it: world.novc.glb, world.nosky.glb.
Still off vs the real game: 2-layer blend materials (_bm, e.g. range floor concrete), tonemap curve (unknown which
of Apex's 6; using ACES fit), HDR colour-grading LUT (mp_rr_desertlands_hu_hdr.raw_hdr, in the game VPK), outdoor
exposure slightly dark, mat_* convar values unknown (couldn't type into the client console while the user was active).

## Movement status (2026-11-16, session 4)
Apex S21 movement ported into `crates/shared/src/sim/` from the R5Regenis bit-exact specs (+ a few functions
disassembled from r5apex_ds.exe), on a new AABB hull sweep (`Collision::trace_hull`). Mantle boost from the R5Regenis
bridge. `cargo run --release -p shared --example movecheck` = 87 checks (all pass), ~50 us/tick on the real map.
Details, approximations and Lab disagreements: `docs/movement.md`.

## Rendering fixes (2026-11-16, session 5)
Prop flag 0x10 = no shadow (evidence: the map's CSM caster tree holds none of them), decal meshes too; prop skins
(record halfword 2) applied in export-map; the dome's cube is `freedm_cubemap_firingrange` via skin 1; 3D skybox pass
from `sky_camera + R^T * eye / skyscale` (decompiled, FUN_14099df90). See docs/skybox.md, apex-lighting.md section 3.
Open: skybox fog, vista brightness, water reflections are far darker than the real game (map03/map11 shots).

## Next (v2)
1. `r5rust_ds`: UDP server running `shared::sim` at 60 Hz; client spawns it locally unless `--connect`.
2. Client prediction + reconciliation (sim is already deterministic and shared).
3. Rendering: shadow map for unlightmapped props, interior light probes, dynamic props from meta.
