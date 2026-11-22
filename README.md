# R5Rust

From-scratch Rust engine that loads the Apex Legends **Firing Range** (`mp_rr_canyonlands_staging_mu1`) from
your own extracted assets, and lets you walk around it. Long-term goal: client `r5rust` + headless dedicated
server `r5rust_ds`, like R5Flowstate's `r5apex.exe` / `r5apex_ds.exe`.

**Just want to play? [Download the R5Rust Launcher](https://github.com/R5Rust/launcher/releases/latest/download/R5Rust-Launcher.exe)**
(Windows x64). It downloads and updates the game, builds the game files from your own Apex install, and launches it.
Game builds are published here as Releases (`build-<date>-<hash>`, made by `tools\package.ps1 -Publish`); the install
toolchain as `tools-<n>` (`tools\bundle_tools.ps1`).

## Play from source (while code keeps changing)

Double-click **`play.bat`** (or `.\play.ps1`). It runs your own copy in `play\current\`, so builds never
fight with an open game. `tools\publish.ps1` builds and stages a new version in `play\next\`; the next time
you launch `play.bat` it swaps in automatically and prints what it updated to. `play\` is gitignored.

## Content and mods

Legends, abilities, passives, weapons, ammo types, attachments, consumables and maps are data, not code: one TOML file
each in `content/<kind>/`, loaded into a registry (`crates/shared/src/registry.rs`) at startup. Mods use the same
layout under `mods/<folder>/content/` with a `mod.toml`, get their own id namespace (`mymod:railgun`) and can override
base entries with `replaces`. Right now only the Firing Range map is defined; `mods/example-mod/` (disabled) shows every
kind. Check everything with `cargo run --release -p shared --example content` (`-- --all` includes disabled mods).
Details: `content/README.md`, `mods/README.md`.

## Run (dev)

```powershell
cargo run --release --bin r5rust
```

Shows the real loading screen, loads the map (~8 s cold, ~1.5 s warm), and drops you at the real spawn.

Controls are rebindable: **Esc opens Settings** (two binds per action, like Apex; click a slot and press a key,
mouse button or wheel; right-click clears). Sensitivity (Apex units), FOV and invert are on the Mouse & View tab.
Everything saves to `r5rust_settings.toml` (gitignored). Defaults:

| Action | Binds |
|---|---|
| Move | W / A / S / D |
| Jump | SPACE, MWHEELDOWN (superglides, bhops) |
| Crouch (hold) / slide | LCTRL, C |
| Sprint | LSHIFT (one tap latches it while you keep moving forward) |
| Fly toggle / fly down | B / X |
| Fire / aim down sights / reload | MOUSE1 / MOUSE2 (hold, or toggle: Gameplay tab / `cl_toggle_ads 1`) / R |
| Inspect weapon (hides the HUD) | N |
| Weapon 1 / holster (melee slot) / melee | 1 / 3 / V |
| Respawn | K |
| Fullscreen | F11 |
| Settings menu | Esc (Resume / Quit in the menu) |
| Developer console | ` (BACKQUOTE) |

### The R-99 (first-person weapon)

Retail's R-99 in your hands (no weapon switching yet): the game's own viewmodel, Wraith's arms, its animation
sequences (draw, idle, fire, ADS in/out, reload, empty reload, sprint, inspect) and its settings: 18 rounds, fire rate 18,
reload 1.8 s / 2.45 s empty, ADS FOV and time, hip/ADS spread by stance, the per-bullet recoil pattern (`r97_2`), and
19000 u/s projectiles with drop that leave impact marks. Crosshair + ammo counter on the HUD. Code:
`crates/client/src/weapon.rs` (gameplay), `viewmodel.rs` (skinned rendering + animation layers). Extract it once with
Apex closed: `pwsh tools\extract_weapon.ps1 -ApexIsClosed` (RSX exports, then `tools\build_weapon.py` bakes
`D:\R5Rust\assets-retail\weapons\mp_weapon_r97`). Console: `weapon_pose <name> [t]`, `weapon_fire [ticks]`.

### Gun feel: sounds and effects

Sounds are retail's: `tools\miles_dump` decodes the Miles bank (every event's layers, picks, dB / pitch ranges, delays,
loop regions, controller curves), `tools\build_sounds.py <item dir>` bakes the events an item uses (its settings' sound
keys + every `AE_CL_PLAYSOUND*` of its sequences) to `<item>\sounds\`. The fire loop runs while you hold fire, the low-ammo
layers fade in as the mag empties (Miles' AmmoClipPct), reload / draw / inspect sounds play on the sequences' own frames,
and the reload hides the magazine (`AE_WPN_CLIPBODYGROUP_*`). Effects (`fx.rs`, `tools\build_fx.py`): muzzle flash,
shells, tracers, impact sparks + dust from retail's particle textures and shell model; their timing is tuned by eye (RSX
can't read retail effect assets). Bullet holes are retail's decals per surface (concrete, metal, wood, glass, default:
the hit triangle's surface from the game's collision, `tools\build_decals.py`). Console: `volume 0..1`,
`weapon_fire [ticks] [settle]`.

### Inventory and loot (the Firing Range's default survival system)

You spawn like the real Firing Range: no weapons, white helmet / body shield / knockdown shield, 2 syringes + 2 shield
cells, a 10-slot backpack. The range's loot lies where retail's map puts it (its `cloot4_*` loot rows, the six weapon
racks, the ammo markers: 234 items) and comes back 0.25 s after you take it. **E** picks up (hold **E** 0.3 s to swap a
weapon when both slots are full), **1 / 2** pick a weapon slot, **3** the melee slot, **TAB** opens the inventory screen
(drop stacks or weapons, take attachments off, the infinite-ammo toggle; keys still move you). The rules are retail's /
Flowstate's (`docs/inventory.md`): stacks + slots by backpack tier, attachments go on the held gun if they're an upgrade,
armor / helmet / backpack swap by tier, reloads take the backpack's ammo, swapped guns drop with their mags and pop
their attachments. The weapon panel is the game's own RUI with both weapon tabs, the mag and the backpack reserve. Data:
retail's loot tables + weapon settings (`toolsuild_loot.py`, install step 10, `<assets>\data\loot\`). Check:
`cargo run --release -p shared --example lootcheck`. Console: `give <loot ref> [count]`, `use [hold]`, `inventory
[open|close]`, `loot_near [radius]`, `infinite_ammo`. Only the R-99 has a first-person model so far: other guns are
carried and shown on the HUD, but nothing is in your hands while one is selected. Loot on the ground is a text label
for now (no world models), and med kits / cells can't be used yet (no health system).

### Holster, melee and heirlooms

1 = the R-99, 3 = the melee slot (retail's weaponSelectPrimary2), V = melee (a quick melee with the gun out). A switch
plays the item's holster sequence for its holster_time, then the other's draw (drawfirst the first time). Run speed
follows retail's move_speed_modifier: 1 for the R-99, 1.15 for fists and heirlooms (the held `mp_weapon_*_primary` /
`mp_weapon_melee_survival` settings, not the swing's `melee_*` ones); ADS slows by the gun's ads_move_speed_scale. The melee slot is empty hands or an heirloom picked in
Settings > **Mods** (also lists installed mods): Wraith's kunai, Octane's butterfly knife, the Grand Slam bat. Each is
retail's viewmodel + every draw / idle / sprint / swing / inspect sequence, picked the way the game does (by activity,
weighted, crouch / air / sprint variants) with its sounds. Their materials use the retail shaders' constants (the kunai's scrolling void glow, the balisong's stim fluid behind
glass); `python tools\build_melee.py --mats` rewrites just those. Bake: `python tools\build_melee.py` (from the recon manifest
`D:\R5Rust\_work_retail\heirlooms\manifest.json`), then `python tools\build_sounds.py D:\R5Rust\assets-retail\melee\<id>`.
Console: `weapon_slot 1|3`, `heirloom <id|none>`, `melee_pose <activity|sequence> [t]`.

### Animation and gun motion, 1:1 with the engine

`anim.rs` plays sequences the way S21's engine does (reversed in Ghidra, notes in
`D:\R5Rust\_work_retailnim_re\sequence_playback.md`): activity -> sequence picking by modifiers, one-hop transition
sequences from the node graph (the kunai's grip flip into a sprint), the old sequence keeps playing while it fades
(fadein/fadeout, smoothstep, up to 6 layers), rates from the item's times, delta sequences added onto the hip <-> ADS
pose, items lowered while mantling / wall climbing / wall running. `sway.rs` is the engine's viewmodel sway + bob
(`viewmodel_motion.md`): move and turn lag, strafe roll and the walk bob from each item's `sway_*` / `bob_*` settings,
switching to the `_zoomed` set in ADS. Heirloom meshes are re-bound onto the shared rig so blades stay in the hand.

### The HUD is the game's own UI code

The weapon panel (`ui/weapon_hud_v2`) and the R-99's crosshair (`ui/crosshair_plus`) are Respawn's compiled RUI, run
live through apex-content-kit's rui-host (`C:\Code\apex\apex-content-kit`, used read-only) by `tools\rui\bridge.mjs`
(Node), and drawn by a port of the kit's renderer / the game's `ui_ps` shader (`crates/client/src/rui.rs`, `rui.wgsl`).
Live ammo, mag size and attachments feed it; every ammo count of the current mag is fetched ahead, so the counter is
instant. The first call warms the host (~9 s), so the HUD shows up a few seconds after the map. Without node / the kit
the simple egui crosshair + ammo counter is used. Attachments from the console: `attach mag 3`, `attach barrel 2`,
`attach optic 1 holo`, `attach stock 4`, `detach all`.

### Developer console

` drops a Quake / Source style console from the top (rebindable in Controls). Type a command or a cvar (alone it
prints the value, with a value it sets it); `;` chains commands, Tab completes, Up/Down is history, the game's log
shows up in it. `help` lists commands, `find <text>` searches, `cvarlist` shows every cvar (`*` = changed).

- movement: `noclip`, `fly`, `setpos x y z`, `setang p y`, `getpos` (prints + copies a setpos line), `respawn`,
  `host_timescale 0.25`, plus every Apex movement cvar the sim uses (`sv_gravity`, `jump_graceperiodmax`, ...;
  `reset <name>` restores it, they last until the game closes)
- static props: `r_lod -1|0|1..` (force a LOD), `r_lod_switch_scale`, `fadedistscale`, `r_drawstaticprops 0`
- overlays: `cl_showfps 1|2` (2 = frame time graph), `cl_showpos 1`; settings: `sensitivity`, `fov`, `fps_max`,
  `mat_vsync`, `mat_antialias`, `r_renderscale`, ...; debug views `mat_debug 1|2|3|11|12`
- `bind F5 "noclip"` (saved), `alias`, `toggle`, `exec <cfg>`, `condump`, `status`, `quit`

`cfg\autoexec.cfg` (yours, gitignored) runs after the map loads; `cfg\example.cfg` shows the syntax. Launch
options like Source: `play.bat -console +cl_showfps 2 +noclip`. With `--screenshot` they render the console into
the shot. Code: `crates\client\src\console.rs` (UI, log capture) and `commands.rs` (commands + cvars).

Movement is Apex's own (ported from R5Regenis' disassembly specs + the game's pilot settings): walk 173.5, sprint 260,
crouch 80, slide + boost, slide jump, bhop rules, tap strafe, step 22, mantle + superglide + **mantle boost**,
**wall climb** + jump off the wall.
See `docs/movement.md`; `cargo run --release -p shared --example movecheck` runs its 130 checks.

The window title shows fps, position and ground/air/FLY.

## Game data (not in the repo, never commit it)

**R5Rust targets retail Apex** (the current Firing Range), built from your own retail install. The easy way: the
launcher's **INSTALL** button (Play page when files are missing, or Settings > Game files > Install / reinstall). It
finds Apex (EA app or Steam), asks you to confirm Apex is closed, builds everything (map, props, collision,
lightmaps, materials, reflections, sky, colour grade, loading screen) and points the game at it. Same thing by hand:

```powershell
pwsh tools\install.ps1 -ApexIsClosed [-Assets D:\R5Rust\assets-retail]   # resumes; -Only map,sky; -Force redoes all
pwsh tools\install.ps1 -Check        # installed / partial / stale (Apex patched, installer changed) / missing
pwsh tools\retail_setup.ps1 -ApexIsClosed -Only '2,4,5'                  # the underlying steps, one or a few
```

It only reads the install's files, never the running game, and writes `<assets>\install.json` (what it made, from
which Apex files) so it can tell when Apex patches. Tools it needs (checked first, with a plain message if one is
missing): R5Flowstate's RSX fork built in `D:\R5Rust\tools\rsx-fs` (github.com/R5Flowstate/rsx, rsx.sln x64
Release), the extractor in `D:\R5Rust\extractor`, `dxc.exe` (Windows SDK), Python 3.11, export-map (built by cargo).
Not part of the install yet: the R-99, arms, heirlooms, sounds, decals and effects (one-off exports, HANDOFF.md);
the game runs without them. What each step does and the retail format notes (BSP v54, mdl v21, BC7 lightmaps, cube
assets, DXIL shaders): `HANDOFF.md` and the comments in `tools\retail_*`.

The older Season 21 (R5 Flowstate) extraction still sits in `D:\R5Rust\assets` (the default root when no
`assets_root` is set); it's the movement-probe reference only, not a visual target. Layout either way:

- `maps\mp_rr_canyonlands_staging_mu1\world.glb` + `meta.json`, made by `export-map`
- `maps\mp_rr_canyonlands_staging_mu1\collision.bin`: the game's own collision, made by `export-map --collision`
  (see `docs/collision.md`). Without it the player collides with the render triangles and the log warns.
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
max_texture_size = 4096  # never caps: the extracted textures stop at 4096 (most are 128-512)
```

## Screenshot / test mode (no window)

```powershell
.\target\release\r5rust.exe --screenshot out.png [--loadshot load.png] [--frames 120]
    [--pos x,y,z] [--yaw deg] [--pitch deg] [--hold w,shift,space] [--fly] [--debug 1|2|3]
    [--expo-curve x,y,z,pitch,yaw]
```

Settles for 30 ticks, holds `--hold` keys for `--frames` ticks (60/s), renders, and prints load stats,
GPU ms/frame, where the player ended up and the jump peak. `--debug`: 1 = sun visibility (lightmap A),
2 = baked indirect light, 3 = albedo only, 11 = flat colour per material, 12 = view distance (both raw, for scripts).
`--poses list.txt` renders one PNG per `name x y z_eye pitch yaw` line into the `--screenshot` directory with one map
load. `python tools\zscan.py OUT --spots` uses it to count z-fighting pixels (docs/apex-lighting.md section 9).
`--expo-curve` then cuts the camera between `--pos` and that second pose (feet position) both ways and prints the
auto exposure after every frame, the frames to 50/90/99 %, and the gap to the engine formula run on the CPU.
Dark rock crevice to bright road: `--pos 1800,-5200,500.8 --pitch 5 --yaw 202.5 --expo-curve -3000,-200,-82.9,5,67.5`.

## Re-exporting the map

`export-map` (Rust, standalone) rebuilds `world.glb` + `meta.json` from the game's paks; `export-map/build_materials.py`
maps materials to the extracted textures + constants (`materials.json`). Lighting lumps live in `<map>/bsp/`,
cubemaps in `cubemaps.dds`, the dome's sky cubes in `skies\<material>.dds` (fallback `skycube.dds`). Compressed texture
cache: `D:\R5Rust\_cache\tex`. Run `python export-map\build_materials.py` after every full export (new skins bring new materials).

Full export (~2 min; the exporter's `../../../../apex` path dependencies only resolve from the main checkout, not a worktree):
`cd export-map; cargo run --release -- --out D:\R5Rust\assets\maps\mp_rr_canyonlands_staging_mu1`. It applies each static
prop's skin, and tags geometry in the glb material extras: `noShadow` (prop flag 0x10, decal meshes: never in the shadow
cascades, `docs/apex-lighting.md`) and `skybox` (prop flag 0x40: drawn only from the sky camera, `docs/skybox.md`).
It also writes each static prop's own light probe (lump 0x66) + cubemap on its vertices (`_LIGHTING`), which is how
the game lights props (`docs/apex-lighting.md` section 6). Older world.glb files without those extras still load.
Each export first renames the previous `world.glb` / `meta.json` / `collision.bin` to `world.prev.glb` / `meta.prev.json` /
`collision.prev.bin` (one generation back). The export before per-prop probes + LOD0 is kept as `*.pre-probes.*`.

**Static props use runtime LOD** (since 2026-11-18): the exporter writes each prop model once per LOD in model space
(world.glb materials with `propLibrary`) plus `props.json` (10,956 instances, the model's own LOD switch points and fade
distance from the retail header, the prop's own fade distance). The client picks a LOD per prop every frame with the
S21 client's own rule (`crates/client/src/props.rs`: distance vs switch x 13.8 x `r_lod_switch_scale`; Model Detail
High/Medium/Low = 1.0/0.8/0.6), dithers props out at their fade distance and draws them instanced (also into the sun
shadows). world.glb went 488 -> 92 MB, triangles per frame 7-11 M -> 1-1.7 M. `--bake-props` exports the old way
(every prop baked at one LOD; `--prop-tris` / `--far-lods` only apply then). One texture cap for world and props
(`--tex`, default 4096; `--prop-tex` only to override). The cap rarely matters: R5Flowstate ships no `.opt.starpak`, so every texture stops at its permanent +
mandatory-stream mips (most 128-512 px, a handful 1024-4096), the same mips the real game shows on this install.

Collision only (about 20 s, reuses the last export's BSP + models):
`cd export-map; cargo run --release -- --collision --out D:\R5Rust\assets\maps\mp_rr_canyonlands_staging_mu1`.
Compare it with the render triangles: `cargo run --release -p shared --example collcompare`; inspect one spot:
`--example collprobe -- x,y [zmin zmax] [radius]`.

## Layout

- `crates/shared`: GLB loader, the game's collision (`collmesh.rs` loads `collision.bin`), BVH collision with an Apex-style hull sweep (`trace_hull`), deterministic
  `sim::simulate()` = Apex S21 movement ported from the R5Regenis specs (`docs/movement.md`), player settings + cvars
  (`settings.rs`), config. Checker: `cargo run --release -p shared --example movecheck`
- `crates/client`: `r5rust`: wgpu renderer (4x MSAA HDR, reverse-Z, lightmaps + sun, ACES), loader thread, input
- `crates/server`: `r5rust_ds`, a stub for now

## Status (2026-11-18)

v1 works: load screen, Firing Range with baked lighting at ~340-600 fps on a 2080 Ti, fly with collision, and Apex's
real movement (walk/sprint/crouch/slide/jump/air strafe/tap-strafe/step/mantle/mantle boost/wall climb; ~50 us per tick, see
`docs/movement.md`). See `HANDOFF.md` for what's next.

Not done: dedicated server + netcode + prediction (v2), shadows on unlightmapped props (big rocks, some props),
interior lights (the bake has none, so interiors use a flat fill), dynamic props, HUD, weapons, audio, the 3D skybox's
own fog (and its vista brightness is unverified; `docs/skybox.md`).

2026-11-18: switched to **retail Apex** data (map, props, skins, collision, BC7 lightmaps, material constants + UV
routing from the DXIL shaders, sky cubes, reflection cubemaps, colour grade, loading screen) via `tools\retail_setup.ps1`.
Graphics settings (presets, textures, model detail, aniso, shadows, AA, render scale, stretched res, vsync, fps cap) in
Esc > Video and in the launcher. **Stretched Res** works like Apex's: the world, gun and HUD render at that size (e.g.
1300x1070) and get stretched to the window; console `r_stretch 1300x1070` / `r_stretch 0`. Kainz plays retail at
1300x1070 on a 1550x1070 desktop (1920x1080 panel), so his retail screenshots are 1549x1069 with the game stretched 1.19x wide. Runtime prop LOD (above) roughly doubled the frame rate (Ultra, 1600x900, 2080 Ti: spawn 340 -> 627 fps,
vista 218 -> 403). Still open: the blue roof-pit covers (spawned by retail level scripts, not in the map data). The `export-map` commands below still show the old `D:\R5Rust\assets` path; for retail use the setup script.

2026-11-16: no-shadow props/decals stay out of the cascades (the range road under the giant gaskets is sunlit again),
static prop skins are applied (the dome shows the Firing Range sea and mountains instead of Skull Town), and the 3D
skybox draws from the sky camera like the S21 client (`docs/skybox.md`).
