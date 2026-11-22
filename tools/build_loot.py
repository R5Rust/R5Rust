"""build_loot.py - the loot / inventory data the client reads (docs/inventory.md), from retail Apex's own tables.

    python tools/build_loot.py <datatable csv dir> <weapon settings dir> <assets root> [icon source dir]

Inputs (retail_setup.ps1 step 10 exports them with R5Flowstate's RSX fork from the pak mirror):
  - every datatable as CSV (`--exporttypes dtbl`). Retail names none of them; the four we need are recognised by their
    columns: survival_loot (isDisabled, ref, type, ...), the loot groups (groupRef, entryRef, ...),
    survival_weapon_mods (mod, type, ..., attachmentPoint, ammoType) and weapon_attachment_matrix (attachmentName,
    attachmentPoint, <weapon refs>).
  - every weapon settings file (`--exporttypes wepn`, KeyValues text `"WeaponData" { ... "Mods" { "<mod>" { ... } } }`).
Output: <assets>\\data\\loot\\
  survival_loot.csv, loot_groups.csv, weapon_mods.csv, attachment_matrix.csv   copies of the four tables
  weapons.json   per main weapon ref: clip, ammo pool type, uses_ammo_pool, stockpile max, reload times, printname,
                 playermodel (the world model on the ground), hud_icon, and per mod the keys an attachment changes
                 (ammo_clip_size / ammo_stockpile_max: extended mags)
  models.txt     every model a spawnable loot item shows on the ground (pickupModel, weapons: playermodel): export-map
                 puts them in the map's prop library (R5_EXTRA_MODELS) so loot draws with the world's materials
<assets>\images\loot\<rui path>.png   the items' icons (loot `icon`, weapons `hud_icon`), copied from the icon source
                 (apex-content-kit's public\game\icons, which holds them extracted from retail's ui paks) when given
"""
import csv, glob, json, os, re, shutil, sys

dt_dir, wepn_dir, out_root = sys.argv[1:4]
out = os.path.join(out_root, "data", "loot")
os.makedirs(out, exist_ok=True)

WANT = {
    "survival_loot.csv": ("isDisabled", "ref", "type"),
    "loot_groups.csv": ("groupRef", "entryRef"),
    "weapon_mods.csv": ("mod", "type", "name"),
    "attachment_matrix.csv": ("attachmentName", "attachmentPoint"),
}
found = {}
for f in glob.glob(os.path.join(dt_dir, "**", "*.csv"), recursive=True):
    with open(f, encoding="utf-8", errors="replace", newline="") as fh:
        head = next(csv.reader(fh), [])
    for name, cols in WANT.items():
        if tuple(head[: len(cols)]) == cols:
            if name in found:
                # two candidates (e.g. a mode overlay of the loot table): keep the bigger one
                if os.path.getsize(f) <= os.path.getsize(found[name]):
                    continue
            found[name] = f
missing = [n for n in WANT if n not in found]
if missing:
    sys.exit(f"datatables not found in {dt_dir}: {missing}")
for name, f in found.items():
    shutil.copyfile(f, os.path.join(out, name))
    print(f"  {name:24s} <- {os.path.basename(f)}")


def keyvalues(path):
    """Top-level keys of a weapon settings file + {mod: {key: value}} from its Mods block."""
    top, mods = {}, {}
    stack = []  # names of open blocks
    pending = None
    for raw in open(path, encoding="utf-8", errors="replace"):
        line = raw.strip()
        if not line or line.startswith("//"):
            continue
        if line == "{":
            stack.append(pending)
            pending = None
            continue
        if line == "}":
            if stack:
                stack.pop()
            continue
        m = re.match(r'^"([^"]*)"\s+"([^"]*)"', line)
        if m:
            k, v = m.groups()
            if len(stack) == 1:
                top[k] = v
            elif len(stack) == 3 and stack[1] == "Mods":
                mods.setdefault(stack[2], {})[k] = v
            continue
        m = re.match(r'^"([^"]*)"$', line)
        if m:
            pending = m.group(1)
    return top, mods


loot = list(csv.DictReader(open(os.path.join(out, "survival_loot.csv"), encoding="utf-8", errors="replace")))
refs = sorted({(r["baseWeapon"] or r["ref"]) for r in loot if r["type"] == "main_weapon"})
weapons, nofile = {}, []
KEEP_MOD = ("ammo_clip_size", "ammo_stockpile_max", "ammo_default_total")
for ref in refs:
    path = os.path.join(wepn_dir, ref + ".txt")
    if not os.path.exists(path):
        nofile.append(ref)
        continue
    top, mods = keyvalues(path)
    num = lambda k, d=0.0: float(top.get(k, d)) if re.match(r"^-?[0-9.]+$", top.get(k, "") or "") else d
    weapons[ref] = {
        "printname": top.get("printname", ""),
        "clip": int(num("ammo_clip_size")),
        "pool": top.get("ammo_pool_type", ""),
        "uses_pool": top.get("uses_ammo_pool", "1") == "1",
        "stockpile_max": int(num("ammo_stockpile_max")),
        "reload": num("reload_time"),
        "reload_empty": num("reloadempty_time"),
        "playermodel": top.get("playermodel", ""),
        "hud_icon": top.get("hud_icon", ""),
        "mods": {m: {k: v for k, v in kv.items() if k in KEEP_MOD} for m, kv in mods.items() if any(k in KEEP_MOD for k in kv)},
    }
json.dump(weapons, open(os.path.join(out, "weapons.json"), "w"), indent=1, sort_keys=True)
print(f"  weapons.json: {len(weapons)} weapons" + (f"; no settings file for {nofile}" if nofile else ""))

# what spawnable loot looks like on the ground, and its icon
models, icons = set(), set()
for r in loot:
    if r["isDisabled"] == "true" or r["type"] in ("", "string", "blank"):
        continue
    w = weapons.get(r["baseWeapon"] or r["ref"]) if r["type"] == "main_weapon" else None
    m = r["pickupModel"] or (w or {}).get("playermodel", "")
    if m.startswith("mdl/"):
        models.add(m.replace("\\", "/").lower())
    i = r["icon"] or (w or {}).get("hud_icon", "")
    if i.startswith("rui/"):
        icons.add(i)
open(os.path.join(out, "models.txt"), "w").write("".join(m + "\n" for m in sorted(models)))
print(f"  models.txt: {len(models)} ground models")
if len(sys.argv) > 4 and os.path.isdir(sys.argv[4]):
    got = 0
    for i in sorted(icons):
        src = os.path.join(sys.argv[4], *i[4:].split("/")) + ".png"
        if os.path.exists(src):
            dst = os.path.join(out_root, "images", "loot", *i.split("/")) + ".png"
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copyfile(src, dst)
            got += 1
    print(f"  icons: {got} of {len(icons)} from {sys.argv[4]}")
