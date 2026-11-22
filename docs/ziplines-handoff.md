# Ziplines: handoff for a fresh session (written 2026-11-20)

**Task (Kainz):** port the Firing Range's ziplines **1:1 with retail Apex**: the cable, the "E to use" prompt,
mounting, riding, the first-person view and hands, sounds and detaching. Use Ghidra, REA and any other reversing
needed. No approximations where the game's own code or data can be read.

Read `HANDOFF.md` (rules, data paths, how to verify) before this. Short version of the rules that matter here:
- Retail is the reference. Read retail from files only, with Apex closed; never touch a running Apex process.
  Flowstate S21 (the binary in Ghidra, its scripts, its SDK) is allowed as reference: Kainz says "Flowstate is the
  same as Apex" for mechanics.
- No game assets in the repo, never push. Verify headlessly (`--screenshot`, `+console commands`) and look at the PNG.
  No UI driving on Kainz's desktop.
- Kainz's display: game 1300x1070 stretched (Video > Stretched Res), desktop 1550x1070, monitor 1920x1080. His
  retail screenshots are 1549x1069 desktop captures.

## What already exists to build on

| Piece | Where | Reuse for ziplines |
|---|---|---|
| Movement sim, S21 order | `crates/shared/src/sim/mod.rs` (line ~565 lists ziplines as not done), `sim/wall.rs:224` notes the zipline state at S21 `0x140bf5480` | add a zipline movement state the same way traversal/mantle was added |
| Mantle = template for "first-person proxy" | `main.rs` `proxy_frame` / `proxy_pose` / `update_proxy`, `viewmodel.rs` `bone_world` / `upload_world`, HANDOFF "Mantle view + hands" | zipline hands use the same `pov_mp_core` arms proxy |
| Arms rig + sequences | `tools/build_arms.py` -> `D:\R5Rust\_work_retail\arms_core` (rig, cast, qc). Already exported: `qc\...\pov_mp_core\zipline_mount.qc`, `zipline_mount_vertical.qc`, `ptpov_zipline_start_dmx`, `ptpov_zipline_start_vertical_dmx` (SMD) | add the zipline sequences to the baked arms (build_arms.py currently picks mantle_*) |
| Sounds | `tools/miles_dump` + `build_sounds.py` (general.mbnk events), `audio.rs` | bake the zipline events (names below) |
| HUD / RUI | `tools/rui/bridge.mjs` + `ruihost.js` run Respawn's compiled RUI; `rui.rs` draws them | the use prompt is RUI too; import its asset like crosshair_plus (HANDOFF "HUD" section) |
| Map entities | `D:\R5Rust\_work_retail\export-map\bsp\mp_rr_canyonlands_staging_mu1\wrap\maps\*_script.ent` | ziplines are NOT exported yet (meta.json has only the `zipline_balloon` prop models) |
| Ghidra | S21 `r5apex_dx12.exe` at `C:\Code\reverse-engineering\reverse-engineering-projects\targets\r5apex\` (MCP `ghidra`, GUI must be open with the plugin) | all code-side behaviour |

## Data found so far

**Firing Range zipline entities** (retail `_script.ent`): 7 × `"classname" "zipline"`, plus their `zipline_end`
partners linked by `link_guid`. Example (a vertical one):
```
"origin" "-4040.24 512 1403.05"  "angles" "0 -180 0"  "ZiplineVertical" "1"
"_zipline_rest_point_0..5"  (the cable's sag points, precomputed)
"Material" "cable/zipline.vmt"  "Width" "2"  "scale" "1"
"ZiplineSpeedScale" "1"  "ZiplineLengthScale" "1" (0.96 on another)  "ZiplineAutoDetachDistance" "150" (32 on another)
"ZiplineDropToBottom" "1"  "ZiplinePreserveVelocity" "0"  "ZiplinePushOffInDirectionX" "0"
"ziplineMountReverseDistance" "0"  "ziprailMountReverseDistance" "200"  "isZiprailStart" "0"
"DetachEndOnUse" "0"  "DetachEndOnSpawn" "0"  "useAutoDetachSpeed" "0"  "ZiplineFadeDistance" "-1"
```
Every key matters for 1:1. Some ziplines are vertical (`ZiplineVertical 1`: straight up/down the pole, its own mount
anim `zipline_mount_vertical`).

**Scripts (Flowstate S21, reference):** `C:\Code\apex\r5-modkit\s21-platform\scripts\vscripts\`
- `pilot/_zipline.gnut` (960 lines): `CodeCallback_ZiplineMount / Start / Move / Stop` (what script does on each code
  event: sounds, jet effects `CreateZiplineJetEffects`, callbacks), `Zipline_Init`.
- `client/cl_zipline.gnut` (small), `weapons/mp_weapon_zipline.nut` (Pathfinder's, but shares the zipline code path),
  `mp/levels/mp_rr_canyonlands_common.nut` (map zipline setup), `_utility_shared.nut`, `_sh_init.gnut`.
- Sound / anim names seen in scripts: `Zipline_Stop`, `zipline_attach`, `zipline_loop`, `zipline_land`,
  `zipline_detach`, `zipline_strafe` / `Zipline_StrafeSounds`. Confirm exact Miles event names in general.mbnk.
- Activity modifiers: `scripts\activity_modifier_types.txt` has zipline entries (the held weapon's animation while
  riding: the gun is lowered/holstered on the line).

**SDK (C++ server/shared, reference):** `C:\Code\apex\r5-modkit\s21-r5sdk\src\`: `engine/server/zipline_validation.cpp`,
`game/shared/dt_extend.cpp` (networked zipline fields = the state the client needs), `game/server/glide.cpp`,
`jetpack.cpp`. Read these first: they name the player-side zipline fields and convars.

**Binary (Ghidra, S21):** strings to start from: `zipline_path_mount_debug_crosshair_dist` (0x1416c93b0) plus every
`zipline_*` convar (list them with `list_strings filter=zipline`). Code-side, find and reverse:
1. **Mount check / use prompt:** how the client decides a zipline is usable (distance/angle to the cable from the
   crosshair, `zipline_path_mount_*` convars), the "E" prompt's RUI and text (`#..._ZIPLINE` style localisation key;
   find the hint RUI asset name next to it).
2. **Mount:** snap onto the cable (closest point, direction from view vs cable, `ziplineMountReverseDistance`), the
   mount animation and the view transition.
3. **Ride:** speed (convar + `ZiplineSpeedScale`), acceleration, sag/rest points as the path, strafe sway, slope,
   looking around while riding (view limits?), what the held weapon does (activity modifier).
4. **Detach:** jump off (velocity inherited? `ZiplinePreserveVelocity`, `ZiplinePushOffInDirectionX`), auto detach at
   the end (`ZiplineAutoDetachDistance`, `ZiplineDropToBottom`), crouch-to-drop, re-mount cooldown.
5. **Vertical ziplines:** the separate up/down behaviour and anim.
6. **Cable rendering:** `cable/zipline.vmt` with `Width 2` along the rest points (Source-style rope/cable render;
   find the retail material + its shader, and whether rest points are used directly or re-sagged).

## Suggested order

1. `export-map`: write the zipline + zipline_end pairs (all keys, rest points) into meta.json (or a `ziplines.json`
   next to it). Loader reads them.
2. Render the cables (retail material) and check against a retail screenshot (ask Kainz for one at a known
   zipline; the probe overlay `ang/pos` lets you match the pose).
3. Reverse the code path (Ghidra + SDK + scripts), then add the zipline state to the shared sim (deterministic, 60 Hz
   like everything else) and the use prompt (RUI).
4. First-person: arms proxy sequences (mount, loop, detach) + view, sounds.
5. Headless tests: a console command to mount the nearest zipline and `sim_hold` ticks along it, screenshots at
   mount / mid-ride / detach. Then a HANDOFF section and README line.

## Open items left from the last session (not ziplines)

- The launcher (`C:\Code\apps\r5rust-launcher`) has no control for the new **Stretched Res** setting
  (`[graphics] stretch = [1300, 1070]`); HANDOFF says every game setting should get one there (ask before editing
  that sibling project).
- Crosshair spread is instant between stances; retail may lerp. The weapon HUD's `%keybind%` tokens aren't resolved.
- Gloves look very white in sun: probably Wraith's voidwalker arms really are white; not compared to retail yet.
- Window guard (0f44ff5): the game window once opened at (-32000, -32000) from the launcher; now moved back on-screen
  at startup. Cause not found.
