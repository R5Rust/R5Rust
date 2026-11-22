# R5Rust handoff

Read this first in a fresh session. It is the state as of **2026-11-18** plus the next task, written so a new session
does not need to re-read the code or re-derive anything. Deeper detail lives in `docs/` (linked below).

**Next task (2026-11-20): 1:1 ziplines** (cable, E-to-use prompt, mount/ride/detach, hands, sounds). Everything found
so far and the plan: [`docs/ziplines-handoff.md`](docs/ziplines-handoff.md).

## What it is

A from-scratch Rust/wgpu reimplementation of Apex Legends' **Firing Range**, matching the **S21 build that R5Flowstate
runs** (`r5apex_dx12.exe`). Goal: literally 1:1 with Apex (movement, rendering, collision), based on Apex itself, not
stock Source. Owner: Kainz. Prototype for Kainz and friends.

- Workspace: `crates/shared` (map load, collision, deterministic movement sim, content registry), `crates/client`
  (wgpu 30 / Vulkan, winit 0.30, egui menu), `crates/server` (planned dedicated server), `tools/` (exporter, probe).
- Run: `play.bat` (runs `play\current\r5rust.exe`, swapping in a staged `play\next` build first). Build + stage:
  `tools\publish.ps1`. Dev: `cargo run --release -p client` (exe name `r5rust`).
- **Launcher:** `C:\Code\apps\r5rust-launcher` (separate repo, egui). Play / Patch Notes (git log) / Console (live game
  output) / Settings (game settings, launch args, game files check). Build latest runs `tools\publish.ps1`. Its README
  has everything. It edits the game's `r5rust_settings.toml` with `toml_edit` (keys kept, binds untouched), so **any new
  game setting should also get a control there** (`src\game.rs` `GameSettings` + `settings_page` in `src\main.rs`).

**R5F 2.0 exists (found 2026-11-20):** the Flowstate launcher (0.1.64-0.1.66, 19 Nov 2026) adds "R5F 2.0", a newer
client installed *next to* the Season 21 folder (Season 21 / R5F 2.0 switch in its top bar, 120 Hz servers; game notes
already mention `sdk_s30` paks and S30 mantle anims; client 34.7 GB, HD pack 25 GB which Kainz doesn't use). Kainz is
installing it on **D:** (C: is full). Once it's there, add its exe as a second Ghidra target and diff the functions we
ported (movement, mantle, ziplines) against it.

**The stock S21 install is gone (Kainz removed it 2026-11-20, on purpose).** `%USERPROFILE%\Games\R5Flowstate` is
empty. Its data lives on in r5-modkit's side installs (`R5Flowstate-modkit*`, hardlinks: stock exes/paks/maps, but
r5-modkit's own client/server.dll). What r5rust used from it moved: `ui.dll` (RUI HUD) and `surfaceproperties.rson`
are stock copies in `D:\R5Rust\s21\`; export-map's S21 default root and `r5f-probe\run.ps1` default to
`R5Flowstate-modkit-test` (so probe runs use modkit SDK hooks, not stock ones). The Ghidra copy of the S21 exe is
separate and unaffected.

## Hard rules (do not break)

- **Retail Apex is the visual/asset reference, not R5 Flowstate** (Kainz, 2026-11-18; `C:\Code\CLAUDE.md`).
  Assets and comparisons come from the retail install (files only, game closed). Flowstate only for its
  dedicated server, SDK, the S21 binary in Ghidra and r5f-probe movement recordings. `D:\R5Rust\assets` is the
  old S21/Flowstate extraction and is being replaced by a retail one.

- **No game assets in any repo.** Map data comes from the player's own install, extracted to `D:\R5Rust\assets`
  (`R5RUST_ASSETS` env or `assets_root` in `r5rust.toml` override it).
- **Never push this local repo** or add the GitHub org as its remote: old commits contain extracted models
  (`mapcache/*.glb`). Publishing = copy changed files into the filtered staging clones (see GitHub below).
- **Kainz plays retail Apex on this PC (anti-cheat).** Never query, screenshot, inject into or kill retail Apex
  processes. Recording R5Flowstate (a separate install; S21 now only as `%USERPROFILE%\Games\R5Flowstate-modkit-test`,
  see above) is fine.
- Verify visually: `r5rust.exe --screenshot out.png --pos x,y,z --yaw Y --pitch P --fov 100 --fly` renders headless;
  look at the PNG. Never print `world.glb`.

## Data

- Map: `D:\R5Rust\assets\maps\mp_rr_canyonlands_staging_mu1\` = `world.glb`, `collision.bin` (the game's own BVH),
  `materials.json`, `meta.json`, `bsp\` lumps, `skies\`, cubemaps. `*.prev.*`, `*.v1.*`, `*.pre-*` are backups.
  `collision.detailed.bin` (current default) and `collision.physics.bin` are the two prop-tree variants (see Collision).
- Textures: PNGs exported by RSX into `D:\R5Rust\assets\images\...` (top mip only, capped by export-map `world_cap`
  4096 and the client's `max_texture_size`). Loading screen: `images\ui\maps_<map>_loadscreen\*.png`.
- Spawn: `playerSpawns[0]` = (-11740, 512, 1584) yaw 0.
- Exporter: `export-map` (Rust, uses R5Flowstate's r5ed rpak code) + `export-map/build_materials.py` (needs
  `d3dcompiler_47`; disassembles each material's pixel shader to route UVs).

## Status by area

**Movement: done (S21 port).** Every movement function mapped to its S21 dx12 address, decompiled in Ghidra and diffed
against the port; address map + per-run table in `docs/movement.md`. `cargo run --release -p shared --example
movecheck` = 132/132. Against 87 recorded real-R5Flowstate runs (`probecmp`), 31-33 match within 2 u/s and 10 u
(23 before the port). Player = Bangalore's settings file (all legends move the same except `closer` sprintAccel 102,
`nova` air 80/800, `revenant_rework` climbHeight 1250). Holstered speed scale 1.15, auto-sprint on, KeyState half
first tick, mantle boost SDK-only (sim default off, menu toggle). Out of scope for now: ziplines, zip towers, grapple,
jump pads, abilities.

**Collision: one open problem.** Remaining movement misses mostly trace to static props: the game picks a prop's
collision tree **per prop** by a rule not yet found. Using every prop's physics tree (`R5_PROPCOLL=physics` export)
fixes some runs and breaks others (33 vs 31 of 87). Next step: reverse the S21 static-prop trace path in Ghidra to
find the rule, re-export, re-run probecmp. Details: `docs/collision.md` "Which prop tree players collide with (open)".

**Rendering: close to the real game.** Ported from the game's shaders: RGBM lightmaps, sun + cascades, probe SH,
cubemaps, GGX, tonemap function 4 (S=1, knee 0.5), auto exposure (per-frame speed 0.15, min 1, max 4.5, spot 0.8),
bloom (cutoff 1.5, clamp 10, baseline 0.1), 3D skybox, decal depth bias (map-wide z-fighting fixed), per-material
UV routing (white terrain patches fixed), missing perf_spec -> F0 0.04, cutout opacity masks (pines), per-prop baked
probes (black props fixed). Docs: `docs/apex-lighting.md`, `docs/apex-post.md`, `docs/skybox.md`.
Still off: water too dark, skybox fog, olive kit walls ~35% darker than real, `pl+0x350` unresolved.

**Content registry: done (empty by design).** `crates/shared/src/registry.rs`, TOML per entry in `content/<kind>/` and
`mods/<mod>/content/<kind>/`; kinds: legends, abilities, passives, weapons, ammo, attachments, consumables, maps,
legend_skins, weapon_skins, heirlooms, charms, cosmetics (typed). Check: `cargo run --release -p shared --example
content -- --all`. `content/README.md` + `mods/README.md` explain the format.

## Tools and ground truth

- **r5f-probe** (`r5f-probe`): `python plan.py` + `run.ps1` plays scripted runs in real R5Flowstate (Octane,
  port 37121) through the mod `Kainz.R5RustProbe` (installed/removed by the script) and logs per-frame prediction;
  `--shots`/`-Shots` take screenshots at poses. Compare: `cargo run --release -p shared --example probecmp`.
- **Ghidra**: project with `r5apex_dx12.exe`, functions renamed `S21_*`. GhidraMCP tools work when the GUI is open
  with the MCP plugin on (headless analyzeHeadless is locked while the GUI runs).
- Debug examples in `crates/shared/examples/` (movecheck, probecmp, hangscan, stuckdbg, collvsrender, ...).
- F8 in game appends the current spot to `r5rust_marks.txt`.

## Releases (players)

Players use the **R5Rust Launcher** (`C:\Code\apps\r5rust-launcher`, player mode): it installs itself, downloads game
builds + the install toolchain from this repo's GitHub Releases and updates itself from R5Rust/launcher's.
- Game build: Kainz decides when. **Build latest** (tools\publish.ps1) only builds for him (player-style: static CRT,
  paths remapped); **Push build** in his launcher (tools\push.ps1 -NotesFile) releases the exact exe he's playing
  (play\current) with that commit's files -> release `build-<yyyy.MM.dd.HHmm>-<hash>` + `release.txt`. Never publish
  on build (Kainz, 2026-11-22: no bugged builds for players). By hand: `& .\tools\package.ps1 -Publish -Notes ...`
  in-process (`-File` mangles the list).
- Install toolchain: `pwsh tools\bundle_tools.ps1 -Version n` -> publish `dist\r5rust-tools-n.zip` as release
  `tools-n`; package builds with `-Tools n`. Clean-PC test: `D:\R5Rust\_standalone_test` (mirror lacks the grade file
  and loadscreen rpak; real installs have both).
- Launcher: `pwsh tools\release.ps1 -Publish -Notes ...` in the launcher repo.
- Discord (discord.gg/kcmDzhJfk8): R5Rust Bot (`C:\Code\apps\r5rust-bot`, task "R5Rust Bot") posts new builds and
  launcher versions to #patch-notes by itself; big news: `node tools/announce.mjs <folder> --post` there.
- Shipped 2026-11-22: tools-1, build 2026.11.22-99922c5 (first public build), launcher 0.4.1. Players get the map,
  lighting, movement, ziplines; not yet the first-person items (one-off exports), the HUD (RUI host needs node + S21
  ui.dll) or zipline braid textures (cables draw dark). `$PipelineVersion` is 4: older installs show "update needed".

## GitHub

**Public** org **github.com/R5Rust** (public since 2026-11-22, Kainz's call), split like R5Flowstate: `R5Rust`
(game), `export-map`, `r5f-probe`, `docs` (movement docs for now), `.github` (public org profile with screenshots),
`launcher` (`C:\Code\apps\r5rust-launcher`). Before going public every repo's history was rewritten (git-filter-repo:
author email -> the gorpnostic noreply address, `C:\Users\<name>\` -> `%USERPROFILE%\`) and pushed to fresh repos;
the pre-rewrite repos are `*-old`, private, safe to delete. R5Rust was recreated once more the same day (`R5Rust-old2`, private) to drop an email domain from a script; its releases were re-uploaded byte-identical, same tags and order. Anything pushed is public: no personal email (commit
as the noreply address), no user-folder paths, no secrets, no extracted game files. Old staging clones have the
pre-rewrite SHAs: re-clone.
Publishing: staging clones live in the session scratchpad (`...\scratchpad\gh\<repo>`); if they're gone, re-clone the
org repos. Copy the committed files in (`git archive HEAD`, not the working tree); for R5Rust `.md` files rewrite
`export-map` -> `export-map`, `r5f-probe` -> `r5f-probe`, `export-map/build_materials.py` ->
`export-map/build_materials.py`; keep the org-only files (export-map's `Cargo.toml`/`Cargo.lock` use git deps on
R5Flowstate/r5ed instead of local paths, both split repos' `README.md`, R5Rust `docs/img/`); commit; `git push
https://github.com/R5Rust/<repo>.git HEAD:main`. Map pictures (`docs/img`, `.github/profile/img`): headless
`r5rust.exe --screenshot <dir> --poses <list>` (free camera, no HUD/weapon), poses `range -7400 -800 380 6 45`,
`town -4400 2200 700 12 200`, `overview -6000 -2000 900 12 110`, JPG q90. The launcher's history was pushed with
`assets/firing-range.jpg` (a game loadscreen, early commits) filtered out: push from a filtered clone, never the
local repo directly. Last synced 2026-11-22 (post-rewrite SHAs): game 77fca14 + this note (= local a11cb1b),
export-map d5d93b8, r5f-probe 6e6231d, docs c807ddc, profile d03493a, launcher 69ef198 (= local b2445cd).

---

## NOW: switching to retail Apex assets (2026-11-18, in progress)

Kainz's reference is **retail Apex** (current Firing Range: red-painted walls, covered roof pits); everything so far
was built from R5 Flowstate S21 data, which is an older version of the map. Rule in `C:\Code\CLAUDE.md`.

Done:
- Retail install: `C:\Program Files\EA Games\Apex` (Firing Range paks dated 2026-11-03). Read only with Apex closed;
  the exporter works on mirrored copies in `D:\R5Rust\_work_retail\export-map\mirror`.
- Retail export command (writes `D:\R5Rust\assets-retail\maps\mp_rr_canyonlands_staging_mu1`):
  `$env:R5_GAME_ROOT='C:\Program Files\EA Games\Apex'; $env:R5_EXPORT_WORK='D:\R5Rust\_work_retail\export-map';
  $env:R5_RSX_MODELS='D:\R5Rust\tools\rsx-src\bin\Release\rsx.exe'; export-map\target\release\export-map.exe
  mp_rr_canyonlands_staging_mu1 --out D:\R5Rust\assets-retail\maps\mp_rr_canyonlands_staging_mu1` (log:
  `D:\R5Rust\_work_retail\export-map.log`).
- Retail format gaps handled in export-map: BSP **v54** read with the v52 layouts (same record sizes; new lumps
  0x19 0x1A 0x1B 0x4A ignored); listings' `v23.2` rsxver; retail paks carry **no asset names**, so names are looked up
  by `string_to_guid` hash (`assets.rs`, unit-tested against the catalog).
- World mesh, materials (v23), textures and world collision export fine (275k tris, run 3).
- Models are **mdl v21** (= v20 layout). RSX 1.2.1 (r5ed bundle) and 2.4.0 can't read them. `D:\R5Rust\tools\rsx-src`
  now has upstream's v21 commits cherry-picked (branch r5rust, rebuilt 2026-11-17) and exports them raw
  (.rmdl/.vg/.phy) with `--exportformat mdl_=3`; export-map uses it for models via `R5_RSX_MODELS`. It ignores
  `--exportguids`, so it exports every model in each pak (slow on common.rpak).

**Props done (b701081):** retail mdl v21 geometry comes from RSX's Cast export (`export-map/src/cast.rs`,
written to `<work>\models_cast\` beside the raw export); verified identical to r5ed's raw reading on S21 models. All
10957 props placed. **Lightmaps (solved 2026-11-18): retail page type 11 = the same two lightmaps BC7-compressed in lump 0x61** (w*h bytes each, lm0 then lm1); decoded in export-map + `shared/src/bsplight.rs`. Retail world PS (DXIL: disassemble with the Windows SDK's `dxc.exe -dumpbin`) decodes them like S21 (RGBM behind a material flag bit 0x8000, sky vec *2.007874-1). Older notes: Lumps 0x61/0x62/0x69 still
measure w*h*{2,8,6} bytes (one 832x768 page), but decoding 0x62 as S21's two RGBA8 planes (planar or interleaved) gives
block noise ("cotton candy"); RSX doesn't know it either. Both export-map and the client skip type 11 (probe lighting
only) until it's reversed: find the client's lightmap upload/decode for type 11 in the retail-era shader/engine code
(not the protected retail exe: read the map's world PS for how lightmapTexture0/1 are sampled + the texture format).
The retail map renders with `$env:R5RUST_ASSETS='D:\R5Rust\assets-retail'` (red walls, Bodden bands, yellow bridge
all match Kainz's retail shots). Its `bsp\` folder = the export's `wrap\maps` lumps renamed `<id>.bin` (do that after
every export). Still missing: prop collision (v21 BVH; collision.bin has 0 static props), materials.json, cubemaps,
sky faces, grading LUT.

**Retail materials done (2026-11-18):** the full extractor dump of core materials (30k skins) times out, so the map's
own 574 material assets were exported by guid (`scratchpad matguids.py`: `material/<glb material name>_<shader>.rpak`
-> StringToGuid, kept if in `D:\R5Rust\_work_retail\catalog.csv`) with `--exportguids`, a flag added to
`D:\R5Rust\tools\rsx-src` (befa8c1), into `_raw_retail\materials\maps\...`; then `r5rust.py sort` (R5RUST_CONFIG =
config.retail.json) and `python export-map\build_materials.py D:\R5Rust\assets-retail`: 449 materials, 0 missing, 2302
textures. Renders now match Kainz's retail shots closely (red wall panels, sand colour). Gaps: build_materials read
no tints / uv_route (retail v23 uber struct + shader sets differ: check `.uber_struct` + shaderSet parsing), no
cubemaps.dds / sky faces / grading LUT for retail, no lightmaps (type 11), the dome
panels draw as a yellow/blue placeholder (sky material), the roof pit's blue covers aren't there (find what draws them).

**Retail prop collision done:** v19.2 header keeps `bvhOffset` at 0xE6 / physics index 0xE8 (`coll.rs`); all 6530 props,
8.6M tris (S21: same scale). Walk tests match S21 (stops at the spawn-hall crate). **Retail is the default now:**
`r5rust.toml` (gitignored) sets `assets_root = 'D:\R5Rust\assets-retail'`; a fresh build is staged for play.bat.

**Retail state 2026-11-18 (bb7a173):** material constants done (`tools/retail_uber.py`: RSX raw `matl=1` export +
each DXIL PS's CBufUberStatic layout from `dxc -dumpbin`); skins done (dome draws `skies/freedm_cubemap_firingrange`,
cube = map-pak txtr 0xA309D30F0572218F copied to `<map>/skies/freedm_cubemap_firingrange.dds`, 0xD2AFC2386013AC7B =
skulltown, matched by eye). Remaining visible gaps vs Kainz's retail shots, biggest first:
1. **Reflections: DONE (2026-11-18).** `tools/retail_cubemaps.py` builds `cubemaps.dds` from RSX's cube dump (the data is the asset's starpak stream, Oodle-compressed when header +0x20 = 3; +0x22 mips; 6 faces x 87408 B BC6H). Client reads 24-byte lump 0x2A records on retail maps. Old notes: Chrome/gold trims look dark with one thin highlight because there's no env cubemap.
   Retail cubemaps are the new `cube` asset (12 in the map pak; lump 0x2A is 12 x 24-byte records: origin i32x3 +
   u32 + u64). RSX can't export them; `D:\R5Rust\tools\rsx-src` (8433681+) now dumps each as `cubemap\<guid>.cubehdr`
   (u32 version 3, u32 size 56, header: +0x10 u16 w,h = 256; +0x14 u32 cubemap index 0..11; +0x18 u32 data size;
   +0x23 u8 mips 9; +0x24 f32; +0x30 data ptr) and `.cubedata` (the data the pointer names). The data starts with
   (offset, rowpitch, size, 0) mip records for 256^2 BC6H (65536, 16384, ... 16 bytes) repeated a varying number of times
   (1/10/9/4 x 9 per cube), and sizes vary per cube (~500 KB): BC6H decode of the obvious offsets is noise. Next: work
   out the layout (compare against how RSX's texture code handles streamed/compressed mips; or Ghidra the S21 binary's
   cubemap upload, which may share the code), then write `cubemaps.dds` (256^2 BC6H, 9 mips, cube array) in the lump's
   order, as the S21 one is.
2. **Colour grading: DONE** (retail ships grades loose in `<install>\materials\correction\`; the map's `color_correction` entity names `master_grade_hdr.raw_hdr`; copy it next to the map). Still open: the UV routing table (build_materials
   gets `uv_route` from the PS via d3dcompiler, which can't read DXIL: port its parser to the `dxc -dumpbin` text).
3. Blue roof-pit covers (Kainz's shot at -5375,1923,541): NOT in the map data. Ruled out: static props + skins, entity props (no prop_dynamic/door/glass entity near), world glass (no glass verts within 700u). Our render shows the room floor (mcor_floor_panel_02) through the opening. Most likely spawned by the retail level script at runtime: needs the retail script/rson data extracted and read.
4. UV routing: DONE (5dc6922, DXIL tracer in build_materials.py).

**One-command retail setup:** `pwsh tools\retail_setup.ps1 -ApexIsClosed` (steps 1-9, `-Only '2,4,5'` for a subset: quote the list). Tested end to end on steps 2,4-7 (226 s) and 9. Steps 1, 3, 8, 9 read the install; the rest use export-map's pak mirror.
**Windows Defender** flagged our locally built `D:\R5Rust\tools\rsx-src\bin\Release\rsx.exe` as `Trojan:Win32/Bearfoos.A!ml` (ML heuristic, 2026-11-18 08:50) and deleted it. **Solved 2026-11-20: the pipeline no longer needs it.** It runs on **R5Flowstate's RSX fork** (github.com/R5Flowstate/rsx, cloned + built in `D:\R5Rust\tools\rsx-fs`, rsx.sln x64 Release, not flagged): reads mdl v20/v21, has `--exportguids`, exports retail `cube` assets as DDS (+ raw json with the index), formats via `--exportsetting <type>=<n>` (txtr 0 PNG / 4 DDS mipmapped, matl 1 raw / 2 struct, mdl_ 0 Cast / 2 raw RMDL, cube 0 DDS / 1 raw, uiia 0 PNG). r5ed's bundled rsx.exe is an older build of the same fork (export-map detects the fork by the `--exportpak` string). Upstream RSX releases (2.4.1) have no `--exportformat`/`--exportsetting` for these and can't do steps 4-6. With the fork: export-map output matches the last good export (10957 props, 1030268 library triangles), `cubemaps.dds` is byte-identical. The extractor (`D:\R5Rust\extractor`) defaults to the fork too. If Defender ever flags the fork build, ask Kainz before changing anything. **Two local patches to the fork (2026-11-20), redo them after a re-clone:** `texture.cpp` `HighestAvailableMip` (PNG/DDS "highest mip" took `mipArray.back()` even when its starpak wasn't there: PNG crashed with a null read, step 4 died with exit 1) and `cpakfile.cpp` `RSX_STARPAK_DIRS` (';'-separated dirs searched for a starpak not next to the pak). Retail's top texture mips live in `pc_all.opt.starpak` (20.8 GB), which the pak mirror skips, so step 4 sets `RSX_STARPAK_DIRS=<Apex>\paks\Win64` and now reads the install (needs -ApexIsClosed). Without it textures cap at the mandatory mip (2048 -> 512); with it they're pixel-identical to the old export. **Third patch (2026-11-20):** `material.cpp` `ParseMaterialTextureExportInfo` names a slot `<name>_<slot>` when an earlier slot has the same semantic name but a different texture (Nemesis emissive t4 + emissive-multiply t7 were both `_ilm`, the last write won: the whole gun glowed; 302 textures, 165 of them map blend layers). `build_materials.py` picks `<name>_<slot>` for the repeat. **Trap:** the extractor's sort keeps the *first* copy of a path, so a re-export never replaces a stale texture; move the stale file out first (the 302 old ones are in `D:\R5Rust\_work_retail\stale-ilm-backup-2026-11-20`).

**Install button (2026-11-20):** `tools\install.ps1` = the launcher's INSTALL (also by hand; header documents it): finds Apex (EA registry, default folder, Steam libraries; a folder counts when it has the map pak), checks tools (RSX fork, extractor, dxc via `R5_DXC` / newest Windows Kit, export-map or cargo, python; `tools\bin\` first for a future bundle), writes the extractor config into the work dir, runs `retail_setup.ps1 -Only n` per step with `@@` progress lines, stamps `<assets>\install.json` (pipeline version + Apex file sizes/dates + finished steps; resumes, an Apex patch or `$PipelineVersion` bump = stale), then sets `assets_root` in `r5rust.toml`. `-Check` / `-Detect` / `-Demo` for the launcher. Reflections and loading screen are optional steps. Not in it yet: first-person items (R-99, arms, heirlooms, sounds, decals, fx: their inputs are one-off exports) and bundling the tools for a friend's PC (portable Python, prebuilt RSX fork + export-map + dxc in `tools\bin\`, then shipping our own builds (never Apex's files) like R5Flowstate's launcher does: a channel manifest pins a content manifest of every file's sha256, each fetched from `cas/<2 hex>/<sha256>`, see `C:\Code\apex\r5-modkit\launcher\src\R5Flowstate.Content\`). Kainz picked the order: launcher Install first, then stale detection, then the tools bundle, then updates.

Open / next (older):
1. (superseded by the Cast reader above) **Confirmed (run 5):** the new RSX exports all 475 retail models raw and the GUID lookup finds every one, but
   r5ed's `rpak::rmdl::parse_model` (v16/v17 layout only) fails on each: "no vertex group carries LOD0". So write a
   v20/v21 reader in
   export-map (layouts in `D:\R5Rust\tools\rsx-src\src\core\mdl\`), or read RSX's Cast export instead: Cast meshes
   carry the material **asset guid** as the material hash, one file per LOD (scratch `castdump.py` showed the layout).
2. Lightmaps: retail uses lightmap page type 11 (not decoded; skipped). Find its encoding (RSX source / Ghidra).
3. Retail materials/textures/shaders for `build_materials.py`: `D:\R5Rust\extractor` now takes `R5RUST_CONFIG`
   (`config.retail.json`: game = retail, out = `assets-retail`, groups = core + the Firing Range). Run `scan`, then
   `export shaders materials`, then `sort`. Use the patched rsx-src build (it has v21/v23 cases).
4. Point the game at it (`assets_root` in `r5rust.toml`), compare against Kainz's retail screenshots (poses in his
   screenshot overlays: pos + ang), then the launcher Graphics card.

Fixed on S21 data this session (still valid for retail): 8 materials took a variant texture via the sibling fallback
(`build_materials.py` now uses the GUID's own export); vertex colour tint on `_vc` blend materials (world VS
`pow(rgb, 2.2)`); video settings + Video tab (no shading toggles, by Kainz's call).

## Game HUD through the game's own UI code (done, 2026-11-18)

`tools/rui/bridge.mjs` (stdin/stdout JSON) + `tools/rui/ruihost.js` (a copy of apex-content-kit `server/rui.js`, asset
dirs extended to `D:\R5Rust\rui\assets`) run `weapon_hud_v2` and `crosshair_plus` in the kit's `host.exe --serve`
(Flowstate S21 `ui.dll` + the RE copy of `r5apex_dx12.exe` + memory snapshots on D:, see the kit README). The client
(`rui.rs`) caches draw lists per request, prefetches all ammo counts per loadout, draws with `rui.wgsl` (the kit's
`ui_ps` port) into the sRGB swapchain; the plate's code texture -4 is `backdrop.rs` (post pass at 1/4 res, 10 px
gaussian at 1080p).
- crosshair_plus export: the kit's patched RSX (`D:\tmp-apex-kit\rsx-src\bin\Release\rsx.exe`) on a copy of
  Flowstate's ui.rpak (`D:\R5Rust\rui\pak`), `--exporttypes ui --exportfilter "crosshair_plus,-crosshair_plus_"`
  (it writes one asset then dies: export one at a time). It dies before rawmap.txt: `tools/rui_rawmap.py` rebuilds it
  from header.bin (checked against weapon_hud_v2 + a badge), `tools/rui_import.py` imports into
  `D:\R5Rust\rui\assets` and names the RUI_CrosshairData args from the retail weapon settings.
- Retail fixes in bridge.mjs (2026-11-18): the S21 RUI draws `weapon_toggle/automatic` (the infinity loop) both as
  the fire-mode glyph and as the reserve infinity; retail's auto glyph is four bullets (live image 0x93CB6874EBC1F756),
  swapped in `retailize()`. The RUIs run on a 16:9 plane fitted into the window (uniform scale, panel anchored bottom
  right, crosshair centred): on other aspects the host stretched some widgets (the infinity hung off the plate at
  1548x1003). Crosshair `adjustedSpread` (engine `weapon_spread`) is the cone in radians: crosshair_plus places the
  ticks at 30 + s * 540 * 90 / fov px; degrees threw them to the screen edges. Checked against Kainz's retail shot.
- Stretched res (`graphics.stretch`, gfx.rs `game_screen()` / `view_aspect()`): render targets, projection aspect and
  the RUI layout use the stretch size, the post pass stretches to the window. Kainz's retail setup: game 1300x1070,
  desktop 1550x1070, monitor 1920x1080; his screenshots are desktop captures (1549x1069). Rendered at 1549x1069 with
  stretch 1300x1070, the weapon panel matches his retail shot pixel for pixel; crosshair gap ~20 px wide in both.
- Open: `%keybind%` tokens aren't resolved (no prompts in this HUD); other weapons' crosshairs need their own export;
  spread changes are instant (retail may lerp between stances).

## Holster + heirlooms + retail sounds (v1, 2026-11-18)

- Sounds: `tools/miles_dump` (pure Python decode of general.mbnk v50: events -> play actions -> tracks with pick lists,
  volume dB / pitch st / delay ms ranges, controller graphs; source loop regions; LZB port in lzb.py). `subset.py` cuts
  events_subset.json; `build_sounds.py` bakes an item's events (event gain = miles_dump's unk_10 relative to +14 dB:
  a guess that fits fire +17 / ADS -9, check by ear). `_FOR_ARCHETYPE` events = `Human_` + name, `_FOR_TYPE` = the
  legend's name (`Wraith_`, the arms we use). audio.rs: cpal mixer (lock-free command channel), Sfx (fire loop,
  controller updates, sequence sounds cut when the sequence is).
- Sequence events: the kit's patched RSX `--exportformat aseq=2` writes SMD + QC; the QC has `activity`, modifiers,
  weights and `event`s. build_weapon/build_melee bake them into view.bin "EVTS" (+ "MNMS" mesh names, "ACTS" activity
  table). viewmodel.rs EventClock fires them as the pose advances.
- Melee items: build_melee.py (manifest from the recon pass: viewmodel + rig + every sequence of 4 items). melee.rs
  picks sequences by activity like the game (variants scored by active modifiers, weighted random). loadout.rs: slots,
  holster/draw switching, quick melee.
- Heirloom materials (tools/heirloom_mats.py, `build_melee.py --mats`): raw constants come from the kit's RSX
  `--exportformat matl=1` filtered by NAME (a GUID filter exports nothing) into `D:\R5Rust\_work_retail\heirlooms\uber`;
  textures are named by each material's pixel-shader resource table. Shaders reversed (world.wgsl `vmfx`): emissive =
  ilm(uv0) * L0 tint * emissiveMultiplyTexture(uv1 + time scroll + uvDistortionTexture(uv2) push). These shaders pack
  c_uvN as (angle, scaleU)(pivot, scaleV), not the 2x2 the map shaders use. The balisong glass is premultiplied alpha
  (albedo.a * opacityMultiplyTexture.r * c_opacity; diffuse fades, reflections stay). Kunai tint (12.75, 17.05, 50).
- tools/retail_uber.py missed every CBufUberStatic field written `name;; Offset` (no space), i.e.
  c_depthBlendScalarAndL0_emissiveTint and c_zUpBlendingVertexAlphaAndL0_albedoTint: the map ran with emissive and
  albedo tints at 1. Fixed 2026-11-18 (light strips 12x, coloured rack lights, ~20 albedo tints);
  `materials.pre-tintfix.json` is the old file.
- Open: heirloom particle effects + props (smears, the bat's drone / ball / cards: sequences `AE_CL_CREATE_PROP` /
  `AE_CL_CREATE_PARTICLE_EFFECT`; attachments fx_gauntlet / L_HAND / FX_Blade aren't in any Cast); the balisong's
  anisotropic spec (`_asa`); exposure counteraction on emissive (c_exposureCounteractionType); melee hits (no targets yet), lunge, door kicks, the bat's charge;
  legend-specific arms (all use Wraith's).
- Fixed the same day: crosshair stuck wide (rui.rs: one request in flight per HUD slot), viewmodel stepping at 60 Hz
  (render-time `lead`), breachable_glass brush models exported at the map origin (export-map places entity brush
  models by origin + angles).

## Engine-accurate animation + sway (done, 2026-11-18)

- Reversed from S21 r5apex_dx12 (Ghidra): `D:\R5Rust\_work_retailnim_re\sequence_playback.md` (SendWeaponAnim,
  SelectWeightedSequence scoring, FindTransitionSequence, rate = duration / desired time capped at 12, transitioner:
  fade = min(old fadeout, new fadein), smoothstep, <= 6 old layers, addlayer / delta / weightlist) and
  `viewmodel_motion.md` (CalcViewModelView, sway + bob simulation FUN_1406afc40, matrix FUN_1406b0e60).
- `anim.rs` Animator (both items), `sway.rs` (settings parse incl. u64-wrapped negatives, per-frame sim, M applied in
  `viewmodel.rs upload/attachments` as view * M * model). `tools/vmbake.py` = shared bake (every sequence's QC fields,
  REFP reference pose, mesh rebind onto the merged skeleton). The R-99's idle/fire deltas sit on ads_in/ads_out
  sampled by the zoom fraction.
- Held heirlooms/fists use their `mp_weapon_*_primary` / `mp_weapon_melee_survival` settings (build_melee.py
  PRIMARY): holster/lower 0.3, move_speed 1.15 (retail cl_gamemode_survival.nut treats > 1 as a faster sprint), sway.
- Open: the SWAY_ROTATE / SWAY_ROTATE_ZOOMED pivot attachments (official RSX 2.4.1 crashes in its QC writer on these
  v21 models, the kit build exports nothing), so sway rotates about the viewmodel origin (the engine's fallback); the
  hip/ADS viewmodel_offset (all zero on our items); tickInterval for turn sway assumed 1/20. Not yet checked live.
- 1:1 pass (2026-11-21/22, compared against Kainz's retail shots, `--fov 119` = his cl_fovScale 1.7):
  zUp triplanar rock PS (7 materials), set-dress cull (`mat_set_dress_level`, model header @0xF0, default 1 =
  what retail shows), cubemap rcp from the nearest probe, HDR sky dome (BC6H DDS: re-export with RSX
  `txtr=4` via the material guid, Apex closed), opacityMultiplyTexture alpha (43 materials), foliage vertex AO,
  coverage-kept alpha mips, perf gloss/spec constants when the PS binds no gloss/spec texture (water = mirror).
  Open: grass/tree albedo a touch lighter than retail, the yellow sand strip at grass edges, water uv
  distortion scroll, foliage scatter (scatterThicknessTexture), fog.
- Prop fade (2026-11-21, S30-exact): `fadedistscale` defaults to 1 like retail (0 = never fade, the old default).
  Reversed from r5apex_dx12_s30: setup FUN_1403f7620, gather FUN_1403f9e60 (+ job params FUN_14040c800), band as in
  FUN_1408bcef0. Fade start F = record fade (+0x24) if > 0, else model header fadeDistance @204 * scale, else
  max(0.5 * hull diag * scale * model_defaultFadeDistScale 40, model_defaultFadeDistMin 400); never with
  STUDIOHDR_FLAGS_NO_FORCED_FADE (0x800, F = 227023); floor 100. Alpha in squared distance from the origin,
  1 inside F*fds -> 0 at F*fds*1.1 (model_fadeRangeFraction 0.1), dropped below 1/255. Size cull: dropped once
  dist^2 >= 5 * 2500 * worldAABBdiag^2 (staticProp_gather_size_weight * staticProp_max_scaled_dist; x 1/0.7 for
  never-fade props). That's why small rocks / bullet casings (model fade 1380) vanish past ~38 m in retail.
  props.json variants carry `hull` (header @0x44/@0x50) + `noForcedFade`; export-map writes them, the current
  file was patched in place (backup props.pre-hull.json). Not modelled: the ADS zoom factor (retail draws farther
  zoomed in), prop solid type 4 (never fades; none in this map).

## Inventory + loot, default survival as in the Firing Range (2026-11-20)

Spec with sources: `docs/inventory.md` (Flowstate S21 scripts + retail datatables; TDM / Arenas / other modes' systems
are out of scope by Kainz's call). Code: `crates/shared/src/loot/` (db = retail survival_loot / loot groups / weapon mods /
attachment matrix / weapons.json; inventory = stacks, slots, pickup / swap / drop / reload rules; ground = world items,
range respawn 0.25 s, throws, use targeting; range = the Firing Range layout), `crates/client/src/loot.rs` (use key,
HUD request, names), console.rs (labels, prompt, TAB screen), main.rs (`start_loot`, `sync_r99`, `loot_hands`,
`loot_overlay`, `loot_hud_lists`, `App::loot_actions`), tools/rui/bridge.mjs (two-slot `hud` with loot refs).
- Data: install step 10 / `retail_setup.ps1 -Only 10` exports every datatable + weapon settings (wepn=1 text) from the
  mirror's common paks with the RSX fork; `tools/build_loot.py` picks the four tables by their columns (retail names
  only survival_loot, weapon_mods and attachment_matrix by StringToGuid; the groups table is `survival_loot_groups_new`).
- Retail racks are map props (6 x `firing_range_weapon_rack.rmdl` at x -7228, from props.json); each rack's ammo type =
  its nearest `ammo_<type>` marker (the bow + no-pool weapons share the markerless one at y 22). Weapons hang at the S21
  offsets (32 u apart, +2 forward, +47 up): by formula, not checked against retail's rack model (no loot models drawn).
  The 7 `info_survival_weapon_location` script_refs are on the dropship, not racks.
- Verified headless: `lootcheck` 23/23; `+use` picks the R-99 off the light rack (prompt, HUD tabs, viewmodel in hand),
  `+give ... "+inventory open"` shows the TAB screen (screens in the session's scratchpad, not kept).
- Open: loot world models (pickupModel from the loot table) instead of labels; other weapons' viewmodels; consumables
  (needs health + shields; retail charge times confirmed: 3 / 7 / 1.5 / 4 / 9 s); the engine's use range (`USE_RANGE`
  128 is a guess: read it from the binary); attachments popping off 0.5 s after a drop (instant here); ordnance as an
  offhand; alt-use for attachments is a hold here (retail: a separate alt-use input); auto-pickup of ammo on touch.

## Recoil, ADS, jump/land, mantle hands (2026-11-18)

- Recoil = retail's view punch spring (`viewkick.rs`, reversed from S21: per-shot kick FUN_140711ce0, exact spring
  FUN_140521af0, application FUN_140af1ed0 / FUN_140b09870, mouse absorption FUN_140a33ff0). Pattern row c0 yaw /
  c1 pitch / c2 c3 random scales; hard = instant punch, soft x 20 = punch velocity; springs.txt constants lerped
  normal -> hot by heat (held cooldown_holdtime, faded cooldown_fadetime) and hip -> ADS by zoom. Camera shows
  (1 - weaponFraction), the gun adds w x vmScale, bullets use the whole punch; eye angles never move (perm_* = 0).
  build_weapon.py bakes "springs" into weapon.json. Verified headless (`weapon_probe spray`): -12 deg peak, back
  to 0 in ~2 s. Not done: roll on the camera (no camera roll yet), viewmodel jolt, crosshair following the punch.
- ADS: the zoom fraction advances per rendered frame (`ads_view`; the 60 Hz sim value made the gun and the zoom
  FOV step). Viewmodel FOV zooms by the world's tan ratio (SetUpView FUN_140838830). `viewmodel_offset_ads_by_
  centerpoint` is off for the R-99 (its sight lines up through the ADS animation alone).
- Jump / land bounce: ACT_VM_JUMP / ACT_VM_LAND (delta, ads/crouch blends, retail fades) on the ground change.
  Not done: LAND_LOW / MEDIUM / HIGH thresholds (unknown), heirloom jump/land.
- Held heirlooms: settings are the primary weapon's only (+ the swing's melee_* keys); a missing time = natural
  length (DesiredDuration <= 0). The melee file's sprintcycle_time 0.55 had played the balisong's 5.5 s sprint
  loop 10x (left hand flicker).
- Mantle view + hands (done): `tools/build_arms.py` bakes the bare arms on `pov_mp_core` (rig + mantle_* sequences,
  exported from the mirror into `D:\R5Rust\_work_retailrms_core`). Retail draws these on C_FirstPersonProxy (a
  world entity, so the WORLD FOV) and takes the view from it: GetTraversalViewPosition S21 0x1406668a0 (found by the
  R5Regenis SDK pattern), attachments looked up by name in FUN_140666190: `ref` / `camera` / `camera_base`
  (= jx_c_start / jx_c_camera / jx_c_pov: the only fit for the clips' data). Proxy model = ledge frame (ledge_point
  pl+0x2b70, forward pl+0x2b64, up Z) * inv(ref); view origin = the `camera` attachment, lerped in from the eye over
  traversal_viewLerpInDuration 0.15 s (minus the gap's forward part on a mantle), clamped to (corner - start) . fwd +
  half hull - 10 past the start column; view angles = eye angles (camera vs camera_base is level in every clip);
  after the traversal the offset decays over 0.2 s. main.rs `proxy_frame` / `update_proxy`. Sequence = traversal
  state (3 below, 4 level, 5 above, 6 high, +4 crouch, 11 hang, 12 jump-off; _NEW_linear weight 999) at the sim's
  cycle. Verified headless at Kainz's retail crate (-2197, 2101): hands grip the lip.
  Not done: the client's automantle view pitch correction (automantle_view_* convars), mantle sounds.
- Probes (console): `weapon_probe ads|spray|spray_ads|jump [bone]`, `anim_probe sprint|deploy [thr] [from]`,
  `sim_hold <keys> <ticks> [stop N ticks into a traversal]` (screenshot tests of real movement),
  `mantle_pose <state> [cycle]` / `mantle_pose off`.

## Bullet impacts: retail decals per surface (done, 2026-11-18)

- collision.bin v2 (export-map coll.rs) adds a u8 per triangle: the surfaceproperties id. World leaves: poly leaf
  header `h & 0xFFF` -> LUMP_SURFACE_PROPERTIES (0x11) record byte 2. Retail prop collision records don't decode
  (all default, or ids like "paintbomb"), so props take the model's `$surfaceprop` (studiohdr v19.2 u16 at 0xA6).
  `surfaces.json` next to the map = id -> [name, gamematerial] from the S21 SDK's `surfaceproperties.rson`
  (script reference; `R5_SURFACEPROPS` overrides the path). `Trace::surface` reports the hit triangle's id.
- Collision-only re-export without touching the install (Apex may be running): `R5_GAME_ROOT` = the mirror
  (`D:\R5Rust\_work_retail\export-map\mirror`), `R5_EXPORT_WORK = D:\R5Rust\_work_retail\export-map`, then
  `export-map mp_rr_canyonlands_staging_mu1 --collision --out <map dir>`. `collision.pre-surface.bin` = the v1 file.
- Decals: `material/decals/bullets/<family>/shotN_wldu` textures (concrete + wood via the kit RSX material export
  by name; default/metal/glass materials crash RSX, so their textures by StringToGuid, list in
  `D:\R5Rust\_work_retail\decals\decal_guids.txt`; sources in `_work_retail\decals\src`). `tools/build_decals.py` ->
  `assets-retail\decals\bullets`. gamematerial -> family follows the R-99's `default_far` impact table (C concrete,
  M/E/G/V/P metal, W wood, Y/U glass, rest default); retail decal colour is mod2x (128 = no change), so decals.wgsl
  multiplies the lit surface by colour / mid-grey with the decal's normal map shaded against the sun.
- By eye, not read: decal world sizes (3.5-8 u; the impact effects that set them are efct, which RSX can't read).
  Open: bullets still trace the player-solid set (retail uses MASK_SHOT: bullet-only clutter is missed), the impact
  particle effect per surface, impact sounds, the far-impact variant, decals on glass don't crack the glass.

## Weapon inspect (done, 2026-11-18)

N (Apex's default) plays retail `inspect_new_0` (`inspect_new_empty_0` with an empty mag) as the base pose; fire, ADS,
reload and sprint cut it, it ends when the sequence does (`weapon.rs pose`). The whole game HUD (RUI panel +
crosshair, egui fallback) is hidden while it plays; dev overlays (fps/pos/console) stay. Unused: `inspect_basic_0`
(older inspect), `inspect_new_1` / `inspect_new_empty_1`. Open: the empty variant still shows the mag's top round (no
ammo bodygroups yet); retail fades the HUD, we hard-cut it.

## R-99 first-person weapon (v1 done, 2026-11-18)

Retail data, extracted with the **official** RSX 2.4.1 (`tools\extract_weapon.ps1`, then `tools\build_weapon.py`):
- settings: `weapon\mp_weapon_r97` (wepn asset, RSX writes KeyValues txt; top level = base, mods nested) +
  `weapon\viewkick_patterns` (`r97_2`: bullet_N = yaw pitch a b degrees, negative pitch = up).
- viewmodel `mdl/techart/mshop/weapons/class/smg/r99/r99_base_v.rmdl` (0x8415FDF21FC5DE64): base bodygroups body,
  sight_front, sight_rear, clip_0; 115-bone rig incl. arms.
- arms `mdl/Weapons/arms/pov_pilot_light_wraith_cyber_ninja.rmdl` (0xF17804FB849712D4): the only first-person arms in
  common.rpak. Legends' default arms load on demand (`odl_asset` from the character_skin settings): **open**.
- anim rig `animrig\techart\mshop\weapons\class\smg\r99\r99_base_v_animRig.rrig` lists 99 sequences (RSX
  `--depfilepath` adjacency list), named `animseq\weapons\r97\ptpov_r97\*` and `animseq_derived\...\*`. draw/reload/
  ads/sprint are absolute, idle/fire/jump/land additive. Anim space != bind space: camera bone at the origin, Z up,
  forward -Y; `viewmodel.rs RIG_TO_SOURCE`. The additive root curve (jx_c_delta) is a constant -90 deg turn, not a
  delta: additive layers skip roots (it rolled the gun onto its side).
- textures: material deps give 7 textures in slot order (albedo, normal, gloss, spec, emissive, ao, cavity); the
  official RSX only exports the top mip, which lives in `pc_all.opt.starpak` (install only, not in the mirror).
- `export-map --guid <asset path>` prints StringToGuid for finding unnamed retail assets.

Open: muzzle flash / tracers / shell ejects (efct particle systems), sounds, targets + damage, viewkick springs
(only the pattern is applied), attachments/optics, jump/land/crouch layers, the legend's real arms. Viewmodel FOV 58 (console`viewmodel_fov`), picked
by Kainz against his retail hipfire screenshot. Binds: Fire MOUSE1, ADS MOUSE2, Reload R; Respawn moved R -> K.

## Graphics quality settings (done, 2026-11-18)

Runtime static-prop LOD is in (props.rs; exporter writes props.json + model-space LOD library). Rule reversed from the
S21 client: r5apex_dx12 FUN_1403d5e20 / FUN_1403d6e80 ("Gather static props second pass"): LOD i while
dist^2 * k < (r_lod_switch_scale * view * 13.8 * switch[i+1])^2; r_lod_switch_scale floor 0.6 (DAT_1417f82d4), 13.8 =
DAT_1417f8980. Retail model header v21: lodOffset u16 @200, lodCount @202, fadeDistance f32 @204 (export-map
`lod_info`); static prop record unknown_4 = the prop's own fade distance (-1 = the model's). Kainz's retail
videoconfig: r_lod_switch_scale 0.6, fadeDistScale 1.

Not pinned down: `k` (per-instance float at record +0xC, used as 1/scale^2), `view` (job float 9, used as 1; probably
the zoom/FOV factor for ADS), the fade band width (0.85..1.0 of the fade distance here). Some far props look flat
white on their lowest LOD (firing-range solar frame LOD2, gun platform LOD3): their own meshes, worth a look against
retail from the same spot. Model Detail Low vs High is now only ~5% apart: props stopped being the bottleneck.

Kainz wants players to tune quality for FPS, Apex-style (Kainz plays Apex on lowest). Build it end to end: exporter ->
renderer -> in-game menu Video tab -> launcher Settings card. One system, presets on top, every knob also custom.

### What exists today (verified 2026-11-18)

- **Meshes:** every static prop is baked at **LOD0** into `world.glb` (`export-map/src/main.rs` ~1294: "r5rust has
  no runtime LOD switching"). The rmdl files carry the game's own lower LODs: `lod_count()` / `parse_lod(rmdl, vg,
  lod)` (~205-300) already read any LOD. Two export-time knobs exist: `--far-lods` (distance rings outside the play
  area: <3000 u LOD1, <10000 LOD2, else LOD3) and `--prop-tris <n>` (triangle budget, default 16M; the model costing
  the most drops a LOD at a time).
- **Textures:** top mip only, as PNG, capped at export (`world_cap` 4096) and at load (`max_texture_size` in
  `r5rust.toml`, `crates/shared/src/assets.rs` Config). No mip chain from the game, no streaming.
- **Post:** `Config` already has `bloom_amount/cutoff/clamp`, `exposure`, `auto_exposure`, `tonemap`, `vsync`.

### Recommended design (one system, two independent axes + shading)

Model detail and texture detail are **separate**, like Apex's "Model Detail" vs "Texture Streaming Budget". So "low
textures + high models" and the reverse both work.

1. **Model detail (mesh LOD), runtime.**
   - Export: write **every LOD** of each prop model once (not baked per instance), plus instance transforms and the
     rmdl's own LOD switch distances (find them in the rmdl header: verify the field with r5ed/RSX source or Ghidra).
     World geometry (BSP meshes) has no LODs; it stays as is.
   - Runtime: per instance, pick `lod = f(distance / lod_scale)` using the game's switch distances, plus a
     **minimum LOD** clamp. Draw instanced per (model, lod) bucket; rebuild buckets when the camera moves a cell.
   - Settings: `model_detail` = High / Medium / Low (Apex names) -> LOD distance scale (e.g. 1.0 / 0.6 / 0.35; check
     what Apex's Model Detail actually maps to: likely `r_lod_switch_scale` or similar, read it from the R5Flowstate
     install's `videoconfig.txt` / cvar dump via r5f-probe's DUMP step) and `max_model_lod` (0 = full detail allowed).
   - Keep the export-time `--prop-tris` budget as the ceiling for the High preset.
2. **Texture detail (mips), load time first, streaming later.**
   - Export: keep full mip chains. Best: write **BC-compressed DDS/KTX2 with mips** instead of top-mip PNGs (smaller,
     faster to load, and the GPU format the game itself uses). RSX can export DDS; otherwise generate mips + BC7 in
     the exporter.
   - Runtime v1: `texture_quality` = Ultra/High/Medium/Low/Lowest -> skip the top 0/1/2/3/4 mips at load ("picmip")
     and `max_texture_size` follows. Needs a restart or a texture reload.
   - v2 (optional): Apex-style **texture budget in MB** (None / 2 GB / 3 GB / 4 GB / 6 GB / 8 GB): stream mips by
     screen size until the budget is used.
   - `anisotropic` 1x/2x/4x/8x/16x (sampler setting, free to change live).
3. **Shading / effects quality** (each a uniform flag or a pipeline variant):
   - `bloom` on/off, `auto_exposure` on/off (fixed exposure when off), `sun_shadows` off/low/high (cascade count +
     resolution), `specular` on/off (drops cubemap + GGX = the "flat lighting" Kainz asked about), `fog` on/off,
     `decals` on/off, `render_scale` 50-100%, `vsync`, maybe `fps_cap`.
   - "Flat lighting" preset = lightmap/probe diffuse only, no specular, no bloom, no shadows: the cheapest look that
     still reads as the map.
4. **Presets:** Lowest / Low / Medium / High / Ultra + Custom. Picking a preset fills every knob; touching a knob
   switches to Custom. Show an FPS hint per preset if measured.

### Where it goes

- Settings: a `[graphics]` table in `r5rust_settings.toml` (game's `UserSettings` in `crates/client/src/binds.rs`;
  add a `GraphicsSettings` struct with `#[serde(default)]`). Mark which keys need a restart.
- In-game menu: new **Video** tab in `crates/client/src/menu.rs`.
- Launcher: a **Graphics** card in Settings (`C:\Code\apps\r5rust-launcher\src\main.rs` `settings_page`, read/write
  in `src\game.rs` `GameSettings` via `toml_edit`, same pattern as FOV/sensitivity). Disabled while the game runs.

### Order of work (each step verifiable)

1. Shading toggles + presets (no re-export needed): bloom, auto exposure, specular/flat, shadows, fog, render scale,
   anisotropic. Measure FPS per preset at 3 fixed poses (spawn, the range, the outside vista) with `--screenshot` timing.
2. Texture quality via mip skipping (needs the exporter to keep mips; re-export textures once).
3. Mesh LOD at runtime (exporter writes per-model LODs + instances; renderer picks LODs). Biggest change; verify no
   popping at Medium, and that High looks identical to today.
4. Menu Video tab + launcher Graphics card for all of it.
Report FPS deltas per step ("Lowest: 410 -> 900 fps at spawn") and screenshots of Lowest vs Ultra.

### Open questions to settle by looking, not asking

- The rmdl LOD switch-distance field and Apex's Model Detail -> LOD scale mapping (Ghidra / R5Flowstate cvar dump).
- Whether RSX can export DDS with mips for every material texture (else build mips in export-map).
- Apex's exact video settings list in S21: read `videoconfig.txt` from the R5Flowstate profile folder (not retail's).

## Performance pass (2026-11-21)

Tools: `r5rust --bench 20` (headless game loop along bench.rs's path; `R5_PROF=1` adds per-pass GPU timestamps) and
`r5rust --bench 20 --window` (real windowed loop, inactive window, no cursor grab). Kainz's PC is in use while
benchmarking: compare builds interleaved, several runs each, best run (scratchpad `abbench.py` pattern).
- Done: loot labels skip ray tests for items with a model (0.22 -> 0.01 ms), RUI caches texture bind groups, bindless
  materials (mat.wgsl), world-pass encoder `finish` on a worker thread (main thread busy 3.0 -> 2.1 ms/frame).
  Renders pixel-identical. Windowed: ~230-290 fps before, CPU no longer the limit.
- Found: in motion the GPU is the limit (75-94% busy). World pass is pixel-bound (half render scale: 1.9 -> 0.6 ms) with
  5-6 fragment-shader runs per pixel (Vulkan pipeline statistics). Tried and dropped: prop culling grid (no gain),
  `CAN_DISCARD` override (no gain), depth prepass (image ok, no clear win in noisy runs). Next: find which draws overdraw
  (3D skybox under the world? big blended overlays?), measured on an idle PC.

## Experimental: ray-traced sun shadows (2026-11-21)

Console `r_rtshadows 1` (off by default, not in the Video menu: not retail's look). `crates/client/src/rt.rs` builds
BLAS/TLAS from the world vb/ib (one world BLAS, one per prop variant's LOD0 opaque draws, TLAS = world + static
props: 338 BLAS / 4588 instances, 45 ms); `rt_on.wgsl` casts one sun ray per pixel instead of the cascades (CSM
renders are skipped while on). wgpu 30 `EXPERIMENTAL_RAY_QUERY` (requested when the adapter has it). Foliage /
alpha-tested and loot aren't in it. Cost +1.5 ms GPU (bench `R5_RT=1`), partly overdraw (rays for hidden pixels).
DLSS: possible via `dlss_wgpu` 6.0.0 (wgpu 30, Vulkan only) but needs NVIDIA's DLSS SDK 310.9.1 + Vulkan SDK + clang to
build, and motion vectors + camera jitter that r5rust doesn't produce yet. Not started.
