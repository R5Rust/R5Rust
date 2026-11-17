# Collision: where it comes from

The player collides with **Apex's own collision data**, not with the triangles we draw. `export-map`
decodes it from the map BSP and the static prop models into `<map>/collision.bin` next to `world.glb`
(`D:\R5Rust\assets\maps\mp_rr_canyonlands_staging_mu1\collision.bin`). The client, the examples and `movecheck`
load it through `shared::collmesh`. Without that file they fall back to world.glb's solid render triangles and log
a warning.

```powershell
cd ..\export-map
cargo run --release -- --collision --out D:\R5Rust\assets\maps\mp_rr_canyonlands_staging_mu1   # ~10-20 s
```

`--collision` reuses the BSP the last full export extracted (`%TEMP%\r5-export-map\bsp\<map>`) and the cached
model files. A full export writes `collision.bin` too.

## What's in it

| Source | What it is | Firing Range |
|---|---|---|
| World tree | The BSP's BVH4 for model 0 (lumps 0x12 nodes, 0x13 leaf stream, 0x03 vertices, 0x10 contents masks). Leaf kinds 4/6 are triangle and parallelogram leaves on float vertices, 5/7 are the same on int16 vertices, 3 is a bundle of sub-leaves, 8 is a convex hull (triangulated from its own tri/quad sets). Decoded triangle count matches r5ed's `rcomp::collision::decode_leaves` exactly. | 274,201 tris (269,585 poly + 4,616 hull) |
| Static props | Kind-9 leaves name a static prop (`sprp` game lump). The prop's model carries its own BVH4 (studiohdr v16/v17 `bvhOffset` at 0xD6, the same node/leaf format) in studiohdr axes. It is placed with the prop's origin, angles and scale. | 6,592 props, 9.99 M tris |
| Brush models 1+ | None of them has a collision tree on this map. | 0 |

A model carries one or two trees. The byte after `bvhOffset` is 0xFF on every one-tree model and 1 on every
two-tree model, and exactly the two-tree models ship a vphysics `.phy`. So tree 1 is the physics replica: a
coarse shell, 12 triangles for a crate, 170 for a 27k-triangle cliff rock. Tree 0 is the detailed mesh, the
only one with surface-property data, and it is what traces use. Movement uses tree 0. Tree 1 is only written
with `--prop-physics` (source 3). On rocks the shell sits as much as 115 units above the visible surface, so
using it for movement would make you float. **Not verified against the engine binary:** if Apex's player hull
traces turn out to use the physics tree, flip `split_trees` in `export-map/src/coll.rs`.

2,088 props (decals, wires, conduits, small pebble clusters, `tree_acacia_small_01`, tripod cones, ...) have a
prop leaf but a model with no BVH and no `.phy`. They have nothing to collide with in the game, so you walk
through them here too.

## Contents masks: what blocks players

Every triangle keeps the contents mask of its leaf. A player trace uses R5's `TRACE_MASK_PLAYERSOLID` =
`0x0201400B` (R5Regenis `data/natives/server_consts.tsv`): SOLID 0x1, WINDOW 0x2, GRATE 0x8, MOVEABLE 0x4000,
PLAYERCLIP 0x10000, MONSTER 0x2000000. A triangle blocks the player when `contents & 0x0201400B != 0`.

- World geometry never has the bare SOLID bit. Normal floors and walls are `0xEB1280` (OPAQUE, PHYSICSCLIP,
  OCCLUDESOUND, PLAYERCLIP, MONSTERCLIP, BLOCKLOS, TITANCLIP, OPERATORCLIP). Invisible player clips are
  `0xA30000` and similar. The map's outer box is `0xFB1280` (adds NOCLIMB).
- Not player-solid in the world: `0x200080` (titan clip only), `0x400` (sound trigger), `0x20` (water),
  `0x80000` (line of sight only).
- **Props combine two masks** (`prop_contents`). The BSP prop leaf decides whether player traces reach the prop
  at all. 411 props on this map have `0xEA1280` with no PLAYERCLIP: wall lights, clamps, vents, signs, some
  pallets and pelican cases. The model's own mask (0x1 solid, 0x8 grate, 0x2 window) decides what the triangle
  is. Stored value = the leaf's non-player bits | (the model's player bits if the leaf admits player traces).

Of 10.26 M triangles in the file, 8.90 M block players.

## collision.bin

Little endian: `R5COLL\0\0`, u32 version 1, u32 vertex count V, u32 triangle count T, u32 0, then V x f32[3]
positions (Source inches, Z up, world space), T x u32[3] indices, T x u32 contents, T x u8 source (0 world
poly, 1 world hull, 2 static prop, 3 prop physics tree). 243 MB for the Firing Range. It loads plus builds the
BVH in about 3.4 s on the loader thread, alongside lighting, so cold load is still about 10.5 s.

## How it compares to the old render-triangle collision

`cargo run --release -p shared --example collcompare` drops the player hull along lines and over a grid in both
sets. `collprobe -- x,y [zmin zmax] [radius]` lists the triangles under a point in both, with masks and
materials.

- The ramps by spawn, (-10439,-266) and (-10868,557): world brushes, identical in both. The 6-unit bump at
  (-10868,589..605) is `desert_rock_cluster_small` pebbles. They are solid in render, but the game gives them no
  collision.
- Grid of 3072x3072 around (-11000,0), every 32 units: 3,679 samples agree within 0.5. In 180 the game is
  lower, because render-only blockers are gone: tripod cones by 42 units, small acacia trees by up to 108,
  pebbles by 6. In 119 the game is higher: prop collision meshes differ from the LOD the render export
  picked.
- (-14647,-6266,-1035) is `wraith_tt_water_opaque`, a water plane out of bounds. The game has no floor there
  (only the map box at z -37000), so you fall into it, as in Apex.

## Mid-air hangs: how to check for them

`cargo run --release -p shared --example stuckdbg -- x,y,z` drops the player at a spot, jumps, and holds forward and
sprint at 8 yaws for 2 s, printing height and state every 12 ticks. A flat run of "air" heights is a hang.
`TRACE=1` (`TRACE_YAW`, `TRACE_T`, `TICKS`) prints every tick, and on ticks with no movement every triangle touching
the trace along the velocity, with its fraction and normal. A start inside geometry is moved to the nearest free spot
first and says so, because reported spots are rounded to whole units and can land a hull inside a wall.

`cargo run --release -p shared --example hangscan [-- x,y,z ...]` drops the player on a 9x9 grid, 24 units apart, around
each spot (default: the three reported ones) and counts hangs: 30+ ticks with no z change while not on the ground.
`VERBOSE=1` lists them. On 2026-11-17: 300 of 1944 runs hung before the shell slack / crease / Q3 nudge fixes
(docs/movement.md "Collision"), 1 after. That one is a real wedge (a 54 degree slope running into the underside edge
of a ledge, narrower than the hull) that frees itself after 30 ticks.

