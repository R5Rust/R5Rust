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
cached cascades, material textures + albedo/emissive tints (materials.json via export-map/build_materials.py), vertex
colours, the Firing Range's sky-shaded 'fake sky' dome (HDR skycube.dds), Apex auto exposure (centre-spot mean, dead
band 0.18 calibrated against a real R5Flowstate shot), bloom before exposure, height fog from env_fog_controller.
world.glb now comes from **export-map** (our copy of r5-web-client's exporter: keeps vertex colours + skies/).
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

## Session 6 (2026-11-16) - state when Kainz restarted the Claude session

Done + committed: S21 lurch (96dc48d), auto sprint + holstered 1.15 speed, real collision.bin, no-shadow/skins/3D skybox,
cut-out opacity masks (pine branches), F8 = append spot to r5rust_marks.txt, auto exposure per-frame speed (c692f5a).
Ground truth: `r5f-probe` (README in plan.py/run.ps1 headers): `python plan.py` + `run.ps1` records movement in real
R5Flowstate (Octane; Lifeline can't lurch); `python plan.py --shots` + `run.ps1 -Shots` screenshots poses.json; compare with
`cargo run --release -p shared --example probecmp` and the headless `r5rust.exe --screenshot ... --pos --yaw --pitch --fov 100 --fly`.
Not yet staged to play\next (last staged build = cca4bb5).

**Interrupted mid-work (uncommitted in the tree, unverified - review the diff before committing):**
- DONE (mid-air hang fix, see docs/movement.md "Collision" and docs/collision.md "Mid-air hangs"): f32 rounding made
  moves parallel to a steep slope hit it at fraction 0; fixed with trace shell slack, the crease tie-break and a Q3
  nudge in TryPlayerMove. hangscan 300 -> 1 of 1944 runs, movecheck 130/130.
- `crates/client/src/loader.rs`: fix for the near-white hard-edged terrain patches. They are blended overlay meshes
  (`road_tracks_dry_03_blend_2_trans`, `sand_desert_02_bm_transparent`, ...; vanish with R5_NOBLEND=1). Albedo, lightmap,
  env spec are fine; the overlay ends up lit far brighter than the floor under it. The opacity rules in world.wgsl
  (overlay `1-bl`, old-style `bl`) were inferred, not decompiled. Poses (eye): (-2980,1340,189) p55.9 y324.6;
  (-3397,2771,1153) p71.2 y9.9; (-3927,-4069,81) p79.1 y57.7.

**New report, not started:** some surfaces render black / untextured across the map. Pair: real R5F at feet
(-5428,915,45) yaw -24.96 pitch 0 vs ours at eye (-5290,826,45) yaw 341.8 pitch -7.5: the firing-range tower's lower
olive-green kit walls (`firing_range_kit_*` / `live_fire_*` materials) and building fronts are black in ours, and the
sky above the dome reads dark navy. Check those materials in materials.json (missing texture files? wrong role mapping
for `albedoTexture`-style keys vs `col`?) with `--debug 3` at that pose.
Also open: water much darker than the real game (map03/map11), skybox fog, D-lurch 256 vs real 263 (jump_grace_max?).

## GitHub (2026-11-16)
Private org **github.com/R5Rust**, split like R5Flowstate: `R5Rust` (game), `export-map` (export-map +
build_materials.py at its root; r5ed via pinned git dep on R5Flowstate/r5ed), `r5f-probe` (r5f-probe at its
root), `.github-private` (member-only org profile with screenshots). CafeFPS has read access to the three code repos.
**Never push this local repo as-is:** its history contains extracted Apex models (`mapcache/*.glb`, now removed and
gitignored but still in old commits). The published repos were built from filtered clones (git filter-branch removing
mapcache + the split-out tools); to publish new work, re-run that split or copy the changes into clones of the org
repos. Unfinished agent work lives on branch `wip/paused-agents` (unverified).

## Next (v2)
1. `r5rust_ds`: UDP server running `shared::sim` at 60 Hz; client spawns it locally unless `--connect`.
2. Client prediction + reconciliation (sim is already deterministic and shared).
3. Rendering: shadow map for unlightmapped props, interior light probes, dynamic props from meta.
