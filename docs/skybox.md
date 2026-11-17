# The sky: 3D skybox, the Firing Range dome, sky cubes

How the S21 client draws everything past the playable space, and how r5rust copies it. Evidence levels as in
`apex-lighting.md`: [H] read from the binary / the map data, [M] strong inference, [L] guess.

## What the player actually sees in the Firing Range

Almost nothing of the 3D skybox. The range is enclosed by a hex-patterned dome of props (`mdl/colony/fs_modular_*_skulltown`
and friends) that use the sky material `skies/freedm_cubemap_*`: an unlit cube lookup by view direction plus the
blue hex emissive with a fresnel edge (world.wgsl `fs_skymat`). The snowy mountains, sea and red spires in the
reference shots are that **cube texture**, not geometry. Only the open ring at the top of the dome shows the 3D skybox.

**The dome props are drawn with skin 1** [H]: the static prop record's second halfword (`unknown_2`) is the skin,
and the dome pieces (and 1084 props in all: walls, rocks, grass, fixtures) use a non-default one. Skin 1 of the
dome models swaps `freedm_cubemap_skulltown` (Kings Canyon desert, the walker, a medkit sign) for
`freedm_cubemap_firingrange` (sea + mountains). r5rust used to ignore skins, so the dome showed Skull Town.

Skins live in the studiohdr (rmdl v16/v17, `studio_r5_v16.h`): `numskinref` u16 @148, `numskinfamilies` u16 @150,
`skinindex` u16 @152 (a header offset: `(o & 0xFFFE) << (4 * (o & 1))`), then `families x skinrefs` i16 texture
indices. `export-map` `skin_remap()` applies them per prop.

The cube textures (HDR BC6H, 2048^2) sit in `<map>/skies/<material leaf>.dds`; the client loads the one the sky
material names and falls back to `skycube.dds`. Which extracted file is which was matched by eye against the real
shots (`sky_a.dds` = firingrange, `sky_b.dds` = skulltown); the map pak holds both materials
(`material/skies/freedm_cubemap_{firingrange,skulltown}_rgdp.rpak`), the textures only as GUIDs.

## The 3D skybox [H]

Entities (staging_mu1, `_env.ent`): `sky_camera` "skybox_cam_level" at (-29090, 20528, 5044), `"skyscale" "1000"`,
`"scale" "1"`, `"use_angles" "0"`, its own fog keys. The level script then moves it
(`mp_rr_canyonlands_staging_mu1.nut`: `+<0,0,32>`, angles `<0, 60, 0>`), so the export writes origin z 5076, yaw 60
(`sky_script_override()` in export-map; the BSP can't know).

The skybox contents are static props with record flag **0x40** (r5ed `rcomp/src/props`: "set on no other prop";
here exactly one: `mdl/vistas/eden_se.rmdl`, flags 0x54, origin 152 units under the camera, yaw 90). It is a
miniature: the vista is modelled at 1/skyscale.

The S21 client builds the skybox view in `FUN_14099df90` (r5apex_dx12.exe), gated on `r_drawsky` and the local
player's sky camera handle (`m_hSkyCamera` +0x1e20, networked as `DT_SkyCamera`):

```c
s = 1.0 / max((float)player->m_skybox3d.scale /* +0x1e24, int */, 1.0);
o = skyCam->GetAbsOrigin(); a = skyCam->GetAbsAngles();
R = r_sky_ignoreAngles ? I : AngleMatrix(a);             // rows m[i][j], Source layout
out[0..11]  = R;                                         // 3x4, no translation
out[12..23] = [ s*R^T | o ];                             // world -> sky: sky = o + s * R^T * world
out[24..26] = o; out[27] = s;
```

So `sky_eye = origin + R^T * eye / skyscale`, view directions turn by `R^T`. The dev script `ToggleSkyboxView`
(`_utility.gnut`, scale 0.001) does the same translation by hand. r5rust (`gfx.rs` `sky_uniform`): a second frame
uniform with that eye, the rotated view, near plane `4 / skyscale`, cascades disabled (shadow lookups land outside
them and fall back to the baked term) and world height fog off; the skybox draws first, then a fullscreen triangle
resets depth to the far plane and the world draws over it (Source's `CSkyboxView` clears depth the same way).
Skybox geometry is kept out of the world pass, the shadow cascades and the render-triangle collision fallback.
Cost: within noise (5.0-5.2 ms/frame either way at 1600x900).

The vista is lit like any static prop (`rgdc` shares the prop PS), with its own probe from LUMP 0x66
(`STATIC_PROP_LIGHTPROBE_INDEX`, prop 0 -> probe 0: no SH, sky fully visible, sun visibility 1) and env cubemap
`unknown_3` = 12; both go into meta.json `skyboxProps`.

## Not done / unverified

- **Vista brightness.** Looking up through the dome's open top our vista sky is a deep, dark blue (the dome
  normals face inward, so the sun and sky terms barely light it). No reference shot looks up, so this is unchecked.
- **Skybox fog.** The sky camera's own fog keys (`fogenable 1`, colour 116 118 124, start 800, end 25000,
  max density .6, `useworldfog 0`, `m_skybox3d.fog.*` on the client) are not ported; the skybox draws unfogged.
- `R5_SKY_ROT_R=1` swaps `R^T` for `R` and `R5_NOSKYBOX=1` / `R5_SKYBOX_ONLY=1` hide / isolate the pass (debugging).
