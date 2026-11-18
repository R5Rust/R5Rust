# mods

Each folder here is a mod. A mod is data in the same format as the base game's `content/`, plus a `mod.toml`:

```text
mods/my-mod/
  mod.toml
  content/
    legends/    abilities/  passives/    weapons/
    ammo/       attachments/ consumables/ maps/
    legend_skins/ weapon_skins/ heirlooms/ charms/ cosmetics/
```

```toml
# mod.toml
id = "my_mod"         # namespace for everything in this mod: my_mod:<file name>. lowercase a-z 0-9 _; "core" is reserved
name = "My Mod"
version = "1.0.0"
enabled = true        # false = not loaded by the game (the content checker can still load it with --all)
priority = 0          # load order; a mod that replaces another mod's entries needs a higher number
```

- **Add** something: drop a file in the right folder, e.g. `content/weapons/railgun.toml` becomes `my_mod:railgun`.
- **Use base content:** reference it by full id, e.g. `ammo = "core:energy"`. Bare names mean your own namespace.
- **Replace** base content: give the file `replaces = "core:r99"`. Everything that pointed at `core:r99` now gets
  your data. Replacing without `replaces` (same id twice) is an error.
- **Check your mod:** `cargo run --release -p shared --example content -- --all`.

`example-mod/` shows every kind of entry. It's disabled, so it never appears in the game.
