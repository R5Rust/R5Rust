"""build_decals.py - retail bullet-hole decals for the client's impact marks (crates/client/src/decals.rs).

Source: retail `material/decals/bullets/<family>/shotN_wldu` materials' textures, exported from common.rpak (the
export-map pak mirror, no install reads) with the kit's RSX into SRC (see HANDOFF "Bullet impacts"):
  concrete, wood: material export (`--exportformat matl=2,txtr=0 -matltextures`, name filter `shot1,shot2,shot3`);
  default, metal, glass: their materials crash RSX, so their textures by StringToGuid
  (`texture/decals/bullets/<family>/shotN_<col|nml|msk>.rpak`, `export-map --guid`).

Which family a hit uses: the impact table (`impact_effect_table`, R-99 = default_far: gamematerial letter ->
impact_<surface> effect, scripts/impacts/default_far.txt in the S21 SDK) and each effect's decal material. Retail
effects can't be read (RSX), so the effect -> decal family link is by name (impact_concrete -> concrete, ...); the
world size of each family is by eye.

Retail decal colour is mod2x: 128 grey = leaves the surface alone, darker / lighter modulate it; coverage = the
colour's alpha (wood: its _msk). Output (all layers 256x256, stacked vertically):
  D:\\R5Rust\\assets-retail\\decals\\bullets\\col.png  (rgb colour, a coverage)
  D:\\R5Rust\\assets-retail\\decals\\bullets\\nml.png  (rg normal, flat 128,128 where a family has none)
  D:\\R5Rust\\assets-retail\\decals\\bullets\\decals.json

    python tools/build_decals.py
"""
import json
import os

from PIL import Image

SRC = r"D:\R5Rust\_work_retail\decals\src"
OUT = r"D:\R5Rust\assets-retail\decals\bullets"
SIZE = 256

# family -> (variants, world diameter in units (by eye; retail's effect sets it), coverage source)
FAMILIES = {
    "default": (["shot1"], 4.0, "mod2x"),
    "concrete": (["shot1"], 4.5, "alpha"),
    "metal": (["shot1", "shot2"], 3.5, "alpha"),
    "wood": (["shot1", "shot2", "shot3"], 5.5, "msk"),
    "glass": (["shot1", "shot2", "shot3"], 8.0, "alpha"),
}
# gamematerial letter (surfaceproperties.rson) -> family, after default_far's FX table: C concrete/rock/asphalt,
# E/G/M/P/V metal, W wood, U/Y glass; dirt, sand, plastic, tile, foliage... use the default decal
GAMEMATERIAL = {"C": "concrete", "E": "metal", "G": "metal", "M": "metal", "P": "metal", "V": "metal", "W": "wood",
                "U": "glass", "Y": "glass"}


def load(name):
    p = os.path.join(SRC, name)
    return Image.open(p).convert("RGBA").resize((SIZE, SIZE), Image.LANCZOS) if os.path.exists(p) else None


def main():
    os.makedirs(OUT, exist_ok=True)
    cols, nmls, fams = [], [], {}
    for fam, (variants, size, cover) in FAMILIES.items():
        layers = []
        for v in variants:
            col = load(f"{fam}_{v}_col.png")
            if col is None:
                print(f"  {fam} {v}: no colour texture in {SRC}")
                continue
            if cover == "msk":
                msk = load(f"{fam}_{v}_msk.png")
                col.putalpha(msk.split()[0] if msk else Image.new("L", (SIZE, SIZE), 255))
            elif cover == "mod2x":
                col.putalpha(Image.new("L", (SIZE, SIZE), 255))
            nml = load(f"{fam}_{v}_nml.png") or Image.new("RGBA", (SIZE, SIZE), (128, 128, 255, 255))
            layers.append(len(cols))
            cols.append(col)
            nmls.append(nml)
        if layers:
            fams[fam] = {"layers": layers, "size": size}
    for name, ims in (("col.png", cols), ("nml.png", nmls)):
        strip = Image.new("RGBA", (SIZE, SIZE * len(ims)))
        for i, im in enumerate(ims):
            strip.paste(im, (0, i * SIZE))
        strip.save(os.path.join(OUT, name))
    json.dump({"layer_size": SIZE, "layers": len(cols), "families": fams, "gamematerial": GAMEMATERIAL, "fallback": "default"},
              open(os.path.join(OUT, "decals.json"), "w"), indent=1)
    print(f"{len(cols)} layers: " + ", ".join(f"{k} {len(v['layers'])}" for k, v in fams.items()) + f" -> {OUT}")


main()
