# content

Everything the game knows about: one TOML file per entry, in the folder for its kind. The file name is the entry's
name, so `content/weapons/r99.toml` is the weapon `core:r99`. Mods use the same layout under
`mods/<folder>/content/` (see `mods/README.md`).

| Folder | Kind | Main fields |
|---|---|---|
| `legends/` | playable characters | `name`, `class`, `settings` (the game's player settings file, e.g. `pilot_survival_stim`), `tactical`, `ultimate`, `passives`, `model` |
| `abilities/` | tacticals + ultimates | `name`, `slot` (tactical / ultimate), `cooldown`, `charges` |
| `passives/` | always-on perks | `name`, `description` |
| `weapons/` | guns + melee | `name`, `class`, `game_class` (e.g. `mp_weapon_r97`), `ammo`, `fire_modes`, `damage`, `headshot_multiplier`, `rpm`, `magazine`, `attachment_slots`, `move_speed_scale` |
| `ammo/` | ammo types | `name`, `stack`, `color` |
| `attachments/` | weapon attachments | `name`, `slot`, `tier`, `weapons` (empty = every weapon with the slot) |
| `consumables/` | heals, batteries, grenades | `name`, `category`, `use_time`, `health`, `shield`, `stack` |
| `maps/` | playable maps | `name`, `bsp` (the game's map name, also its asset folder), `modes` |

- **References** are ids: `ammo = "light"` means `core:light` (same namespace as the file); `ammo = "core:light"` works
  from a mod.
- **Unknown fields are kept** in the entry's `extra` table, so new data can be added before the code that reads it.
- **Check everything:** `cargo run --release -p shared --example content` lists every entry and reports parse
  errors, duplicate ids and broken references (exit code 1 if any). `-- --all` also loads disabled mods.

The code is `crates/shared/src/registry.rs`. Right now only the Firing Range map is defined; legends, weapons and the
rest get added here as they're implemented.
