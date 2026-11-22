# Inventory and loot (default survival, as in the Firing Range)

Spec for r5rust's inventory, read from R5Flowstate S21's scripts + datatables and checked against retail's datatables
(2026-11-20). Only the default survival system the Firing Range uses (playlist `survival_firingrange`); TDM, Arenas,
1v1, gun game etc. swap in their own systems and are out of scope.

Paths: `vs\` = `C:\Code\apex\r5-modkit\s21-platform\scripts\vscripts\`, `dt\` = `...\s21-platform\datatable\`,
`wp\` = `...\s21-platform\scripts\weapons\`, `ret\` = `D:\R5Rust\_work_retail\dtbl\out\datatable\` (retail, unnamed by
guid), retail weapon settings = `D:\R5Rust\_work_retail\weapon\out\weapon\*.txt` (all 324 wepn assets, RSX).

## 0. What the Firing Range changes on top of default survival

- Playlists `survival_firingrange` / `_classic` inherit `survival_firingrange_base` (`playlists_r5_patch.txt:1020-1093`):
  `has_infinite_clips 1`, default armor `helmet_pickup_lv1 armor_pickup_lv1 incapshield_pickup_lv1`, default
  consumables `health_pickup_health_small:2 health_pickup_combo_small:2` (lines 1064, 1076-1077).
- Spawn loadout: no armor + no helmet -> the armor loadout + full shields; empty backpack -> the consumables
  (`vs\mp\firing_range.gnut:3840-3874`). No backpack: capacity is the base 10. No weapons.
- Infinite ammo is a per-player toggle, default OFF (`firing_range.gnut:112, 2361-2373`); on = every main weapon
  `SetInfiniteAmmoState(INFINITEAMMO_CLIPS)` (`vs\weapons\_weapon_utility.nut:7363`), HUD `passiveInfiniteClips`.
- No death box (`vs\gamemodes\_gamemode_survival.nut:989-991`); respawn in place (`firing_range.gnut:4003-4061`).
- Range loot respawns at its spot `firing_range_item_respawn_time` (0.25 s) after pickup (`vs\mp\_staging_area.gnut:170,
  242, 3671, 3714, 3772-3791`): non-weapons at `countPerDrop`, weapons clip -1 (full). Partly taken ammo refills to its
  original count after the same delay (`sh_survival_loot_actions.gnut:707-711`).

### Range layout (`vs\mp\_staging_area.gnut`, retail map markers in `<map>\bsp\*_script.ent`)

- Loot rows (CLootV4, `:1022-1270`, spawner `:1382-1490`): each `cloot4_<cat>_start` info_target links to its end
  (`link_to_guid_0`); the row's refs are spread evenly from start to end (`distBtwnItems = width / (n - 1)`, one item
  = the middle), angles = the start node's forward, count 1 each unless listed:
  | row | refs |
  |---|---|
  | scopes_basic / _advanced / _sniper | spawnable `optic_cq` / `optic_ranged` / `optic_sniper` refs sorted by tier; the last cq one (digital threat) moves to advanced |
  | mags_light / heavy / energy / sniper | `bullets_mag` / `highcal_mag` / `energy_mag` / `sniper_mag` refs by tier |
  | stocks_basic / _sniper | `stock_tactical` / `stock_sniper` |
  | barrels / lasers / shotgunbolts | `barrel_stabilizer` / `laser_sight` / `shotgun_bolt` |
  | hopups | loot groups `attachments_hopups_only` + `corrupted_attachments`, minus disabled, deduped |
  | bodyarmor | `armor_pickup_lv1, lv2, lv3, lv4_all_fast` (no armor cores) |
  | evoarmor | `armor_pickup_lv1_evolving, lv2_evolving, lv3_evolving, lv4_all_fast, lv5_evolving` |
  | helmets | (empty) |
  | incaps | every spawnable incap shield, `incapshield_pickup_lv1` first |
  | ordinances | loot group `ordnance_drops` |
  | batts | phoenix, battery, cell, cell (1 each) |
  | health | medkit 1, syringe 2, syringe 2 |
  | specialgear | `health_pickup_ultimate`, `mp_ability_redeploy_balloon` |
  | backpacks | every spawnable backpack, `backpack_pickup_lv1` first |
- Weapon racks (`:1893-2098`): loot group `firing_range_all_weapons`, grouped by `Staging_RackAmmoType` (crate / tier 5
  -> `_none`, else the weapon's `ammo_pool_type`, no pool -> `_none`), one rack node per group, `_none` last. Per group:
  racks start at node + right * 32 * -3, one rack every 32 u to the right (`STAGING_GUNRACK_SPACING`), the weapon at
  rack + forward * 2 + up * 47 with clip 500, angles node + <-85, 180, 0>. Then (not for `special`) 6 ammo boxes of
  `countPerDrop`: base = racksStart + right * (32 * rackCount - 5), box i at + forward * (i % 3) * 16 + right * (i / 3)
  * 25. Firing Range also puts every ordnance (2 each) after the rack line (`CreateOrdnanceForRealmNode`, `:2378`).
  Retail's map has 7 `info_survival_weapon_location` script_refs (light, heavy, energy, shotgun, sniper, arrows,
  `_none` = 7 groups) where S21 had `weapon_racks` info_targets. Rack model `mdl/industrial/gun_rack_arm_down.rmdl`.
- **Retail map (and R5F 2.0, `CLootV2_Spawn_WeaponRowAndAmmo_ForRealm`, `_staging_area.gnut:1285-1350`):** one
  `guns_<ammoType>_start` -> `_end` row per ammo type (`guns__none` for crate / tier 5 / no pool) on top of the
  `firing_range_weapon_rack` props, spread like a CLootV4 row (`distBtwnItems = width / (n - 1)`), angles
  `<0, 0, -90> + startNode.GetAngles()` (node pitch -35: on its side on the slope), bow / bow_crate / throwing knife
  `AnglesCompose( angles, <85, 0, 0> )`. There's no arrows row: R5F leaves the bow out, we put it on `_none`.
- `ammo_<type>` info_targets (retail: 9 each for bullet, highcal, special, shotgun, sniper): `countPerDrop` of that ammo
  (`:1336-1345`).

## 1. Data model

- Backpack = array of `{slot, type = loot index, count}` (`vs\sh_survival_loot.gnut:2732`); natively 32 slots of
  `{u8 type, u8 count}` (`s21-r5sdk\src\game\server\consumable_inv.cpp:17-19`). Every stack = one slot.
- Equipment slots (`vs\sh_survival_equipment_slot.gnut:200-546`): backpack, helmet, armor, incapshield, gadgetslot,
  main_weapon0 / main_weapon1 (sub slots `_sight _mag _grip _barrel _hopup _hopupMulti_a _hopupMulti_b`).
- Ammo: engine `AmmoPool[type]` (capacity 2000, `sh_survival_loot.gnut:557`), mirrored as backpack stacks
  (`CodeCallback_OnPlayerAmmoPoolCountChanged`, `vs\survival_loot.gnut:8336-8360`). Types: 0 bullet (light), 1 highcal
  (heavy), 2 special (energy), 3 shotgun, 4 sniper, 5 arrows.
- Weapons carry their clip; `uses_ammo_pool 0` weapons (energy LMG family) carry their reserve.
- Grenades are backpack stacks (stack 1); one kind is equipped as the offhand, clip = backpack count
  (`sh_survival_loot_actions.gnut:731-805`).
- Backpack sort: category (attachments, consumables, special, equipment, weapons, ammo), loot type, tier desc, index,
  count (`sh_survival_loot.gnut:7010-7140`).

## 2. Item catalogue (`dt\survival_loot.csv`; retail `ret\0xA1843BD4C56C1A7E.csv`, same columns)

Columns: `isDisabled, ref, type, baseWeapon, baseMods, supportedAttachments, lootTags, startingLevel, tier,
pingCommsAction, fakeAmmoIcon, icon, pickupModel, skinOverride, attachSound, pickupSound_1p/3p, carrySound,
pickupString, desc, countPerDrop, inventorySlotCount, netInt, passive, ...`. `countPerDrop` = units per ground item,
`inventorySlotCount` = max per stack. Tiers 0 none, 1 white, 2 blue, 3 purple, 4 gold, 5 red/mythic.

| ref | type | tier | per drop | stack |
|---|---|---|---|---|
| bullet | ammo | 1 | 20 | 60 |
| highcal | ammo | 1 | 20 | 60 |
| shotgun | ammo | 1 | 10 | 20 |
| sniper | ammo | 1 | 14 | 28 |
| special (disabled) | ammo | 1 | 18 | 54 |
| arrows (disabled) | ammo | 1 | 16 | 32 |
| health_pickup_health_small (syringe) | health | 1 | 2 | 6 |
| health_pickup_health_large (medkit) | health | 2 | 1 | 2 |
| health_pickup_combo_small (cell) | health | 1 | 2 | 6 |
| health_pickup_combo_large (battery) | health | 2 | 1 | 2 |
| health_pickup_combo_full (phoenix) | health | 3 | 1 | 1 |
| health_pickup_ultimate | health | 3 | 1 | 1 |
| frag / thermite / grenade_emp | ordnance | 1 | 1 | 1 |
| armor / helmet / backpack / incapshield pickups | equipment | 0-5 | 1 | 1 |

Attachments are stack 1. Point + ammo lock: `dt\survival_weapon_mods.csv` (retail `ret\0x62CE41C5376EA985.csv`);
which weapon takes which: `dt\weapon_attachment_matrix.csv` (retail `ret\0xE861B7C77949555A.csv`). Loot groups:
retail `ret\0xA51E2D8E35E395C9.csv` (`groupRef, entryRef, entryDistribution, groupTier, priority`).
Weapon ammo / mag / pool: the weapon settings file (`ammo_pool_type`, `ammo_clip_size`, `uses_ammo_pool`,
`ammo_stockpile_max`).
Armor shield by tier: 0 25, 1 50, 2 75, 3/4 100, 5 125 (+ base shield 0) (`sh_survival_loot.gnut:6366-6450`).
Retail vs S21 (66 rows): retail disables `backpack_pickup_lv4_revive_boost`, `incapshield_pickup_lv4_selfrevive`,
`optic_cq_holosight`, `optic_cq_threat`, every `*_mag_l4`, `laser_sight_l4`, `shotgun_bolt_l4`, `stock_sniper_l4`,
`hopup_kinetic_loader`, `hopup_smart_reload`; other supported-hopup / baseMods differences on a few weapons.

## 3. Backpack capacity

None 10, lv1 12, lv2 14, lv3/lv4 16 slots (`sh_consts.gnut:2504-2507`, `sh_survival_loot.gnut:2528-2570`).

## 4. Pickup

- Stacking (`sh_survival_loot.gnut:2786-2835`): top up partial stacks of the same item first, then new stacks while
  slots are free; the rest stays on the ground (partial pickup, `sh_survival_loot_actions.gnut:885-940`). Ammo also
  capped by the pool (2000 - current).
- Auto pickup (touch) never opens a new stack; own drops not auto-picked for 3 s; ordnance auto pickup only with
  `auto_pickup_ordnance` (default off).
- Equipment (`sh_survival_loot_types.gnut:367-560`): empty slot / higher tier / same tier other perk -> equip; armor of
  the same or lower tier still equips if the ground piece holds more shield; else "you have a better X". The old one
  drops at the throw origin (forward * 100); armor keeps its shield value (`sh_survival_loot_actions.gnut:1131-1240`).
- Weapons (`sh_survival_loot_types.gnut:582-646`): <= 1 primary -> slot 0 if empty else slot 1, switch to it; 2
  primaries -> swap with the active one (hold use 0.3 s, `sh_survival_loot_actions.gnut:539-575`); active not a primary
  -> the last-cycled slot (`survival_loot.gnut:3445-3470`).
- Attachments (`sh_survival_loot.gnut:4218-4360`): onto the active weapon if compatible and an upgrade (sights / hopups
  always count), else the stowed weapon, else the backpack; fits but not better -> ignored ("you have a better X"),
  alt-use -> backpack. The replaced one goes to the other weapon if better / empty there, else onto the ground.
- A new weapon takes compatible backpack attachments, best tier first (`survival_loot.gnut:4255-4280`).
- Prompt targeting (`sh_survival_loot.gnut:6091-6175`): crosshair within 14 u of the vertical line item -6 .. item +46;
  weapons also by the ground point within 32 u. Engine use range: UNSURE (code side).

## 5. Drop

- Backpack items (`survival_loot.gnut:6444-6655`): the chosen count as one ground item, thrown view dir pitched -10
  deg at 150-250 u/s + 100 up.
- Weapons (`:4941-5105`): the ground weapon keeps its mag; 0.5 s later its attachments pop off as separate items and mag
  rounds above the base clip are thrown as ammo. Last gun gone -> melee.

## 6. Reload

Pool weapons fill the mag from `AmmoPool[type]` (= backpack ammo); empty pool -> no reload. A picked-up weapon keeps its
ground clip (-1 = full) and tops up from the pool. HUD reserve = `AmmoPool[type]` (energy LMGs: their own reserve).

## 7. Consumables (`vs\weapons\mp_ability_consumable.nut`, times `wp\mp_ability_consumable.txt:106-284`)

| item | heal | shield | time |
|---|---|---|---|
| syringe | 25 | 0 | 3 s |
| medkit | 100 | 0 | 7 s |
| cell | 0 | 25 | 1.5 s |
| battery | 0 | full | 4 s |
| phoenix | 100 | full | 9 s |

Retail `mp_ability_consumable` charge_time (2026-11-20 export) confirms every time above (ult accel 6 s, fast 3 s; `quick_heal` x0.85, `fast_heal` x0.5).

Consumed and applied only at 100 %; refused at full health / full shields / no armor (shield items).

## 8. Use input

Tap = main action; alt-use = the alternative (attachment -> backpack); `USE_INPUT_LONG` (hold 0.3 s) = swaps. A pickup
is refused within 0.1 s of the item spawning.

## 9. HUD (`ui/weapon_hud_v2`)

Per tab: weapon name, ammo colour; active: `maxMagAmmo`, `clipAmmoFrac`, `maxAmmo` / `remainingAmmoFrac`,
`ammoPoolVisible = uses_ammo_pool`, attachment slots + tiers (`cl_weapon_status.gnut:1060-1095, 1981-2027`). Reserve =
the player's backpack ammo of that type.

## 10. What loot looks like (models + icons)

- Ground model: the loot row's `pickupModel`, or for a main weapon its settings' `playermodel` (`w_*` world model).
  `tools/build_loot.py` lists them in `<assets>\data\loot\models.txt` (95); export-map (`R5_EXTRA_MODELS`, install
  step 1, run after step 10) adds them to the map's prop library as variants without instances, so their materials go
  through steps 4/5 like the map's. The client draws ground loot as runtime prop instances (`Props::dynamic`, up to
  `DYNAMIC_CAP` 4096): same LODs, shadows and probe lighting as the map's props. Rack weapons lie at pitch -85.
- Icons: the row's `icon` / weapon `hud_icon` (rui paths), copied from apex-content-kit's extracted ui icons to
  `<assets>\images\loot\<rui path>.png` (110 of 125; ui paks aren't in our pak mirror). The inventory screen and the
  use prompt show them.
- Retail draws no floating labels; the test UI labels only items without an exported model.

## 11. Rarity outline

Ground loot gets the `survival_item_<tier>` highlight (SURVIVAL_GetHighlightForTier; radius 1.0 for every tier). Colours
= retail `colorpalette_table` LOOT_TIER1..5 (192,192,192 / 30,144,255 / 125,0,255 / 255,205,60 / 255,78,29). The
client tags each loot instance with its tier (`InstGpu.pad[0]`), draws the highlighted ones again into an R8 mask
against the world's depth (only the visible part is outlined), shrinks it to a quarter-res max mask, and post.wgsl
draws a ~4 px (at 1080p) soft band outside the silhouette. Width matched by eye to R5F 2.0 (rack, 1600x900). Cost:
within noise of off with the range's loot in view (was +0.9 ms before the quarter-res early-out).
Weapon world models keep only bodygroup model 0 (export-map drops `<part>_<n>` meshes with n != 0): every attachment
is its own bodypart with an empty default, so all sights and suppressors showed at once before.
