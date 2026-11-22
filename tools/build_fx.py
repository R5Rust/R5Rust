"""
build_fx.py - bakes the R-99's first-person effects from retail Apex textures (see HANDOFF.md "Gun feel").

Inputs (RSX exports into D:\\R5Rust\\_work_retail\\fx, see tools/extract_fx.ps1):
  tex/texture/0x*.png   the particle materials' textures (official RSX 2.4.1, from the install's common.rpak)
  mdl/mdl/Weapons/shellejects/shelleject_pistol_LOD0.cast   the shell eject model

Retail draws these through its effect assets (wpn_muzzleflash_smg_FP, P_tracer_projectile_smg,
wpn_shelleject_pistol_FP), whose parameters RSX can't read. The textures and their material ramps are retail's;
how they're spawned and animated is tuned by eye in crates/client/src/fx.rs.

Output: D:\\R5Rust\\assets-retail\\weapons\\mp_weapon_r97\\fx\\
  layers.bin  u32 size, u32 count, then count x size x size RGBA8, premultiplied: rgb = light added, a = coverage.
              Additive layers have a = 0 (pure glow); the smoke has a = its density.
  layers.json name -> first layer (+ frames for flipbooks)
  shell.bin   u32 nverts, nverts x (pos3, nrm3, uv2) f32, u32 nidx, nidx x u32 (model units, Source space)
"""
import json, os, struct, sys

from PIL import Image

sys.path.insert(0, os.path.dirname(__file__))
import castlib

SRC = r"D:\R5Rust\_work_retail\fx"
TEX = SRC + r"\tex\texture"
OUT = r"D:\R5Rust\assets-retail\weapons\mp_weapon_r97\fx"
SIZE = 256


def tex(guid, i=None):
    p = os.path.join(TEX, f"{guid}.png" if i is None else f"{guid}_{i:03d}.png")
    return Image.open(p).convert("RGBA").resize((SIZE, SIZE), Image.LANCZOS)


def ramp(guid):
    """A material's 256x16 colour ramp -> 256 RGB tuples (left = dim, right = hot)."""
    im = Image.open(os.path.join(TEX, f"{guid}.png")).convert("RGB")
    return [im.getpixel((min(int(x * im.width / 256), im.width - 1), im.height // 2)) for x in range(256)]


def additive(im, src="r", colour=None, ramp_=None):
    """Intensity from a channel ('r', 'a' or 'ra' = r*a), coloured by a fixed colour (rgb) or the material ramp."""
    out = Image.new("RGBA", im.size)
    px, po = im.load(), out.load()
    for y in range(im.height):
        for x in range(im.width):
            r, g, b, a = px[x, y]
            if src == "rgb":
                po[x, y] = (r, g, b, 0)
                continue
            v = {"r": r, "a": a, "ra": r * a // 255}[src]
            if ramp_ is not None:
                c = ramp_[v]
                po[x, y] = (c[0] * v // 255, c[1] * v // 255, c[2] * v // 255, 0)
            else:
                c = colour or (255, 255, 255)
                po[x, y] = (c[0] * v // 255, c[1] * v // 255, c[2] * v // 255, 0)
    return out


def blended(im, colour):
    """Smoke: density from r, premultiplied with the material's grey."""
    out = Image.new("RGBA", im.size)
    px, po = im.load(), out.load()
    for y in range(im.height):
        for x in range(im.width):
            d = px[x, y][0]
            po[x, y] = (colour[0] * d // 255, colour[1] * d // 255, colour[2] * d // 255, d)
    return out


def main():
    os.makedirs(OUT, exist_ok=True)
    layers, names = [], {}

    def add(name, ims):
        names[name] = {"layer": len(layers), "frames": len(ims)}
        layers.extend(ims)

    flash_ramp = ramp("0xC4C00B9C06361618")   # flash_core / flash_cloud materials' ramp: dark ember -> white
    smoke_grey = ramp("0x615F785F066D7436")[160]
    add("flash_core", [additive(tex("0x2DF045A5F08A9231", i), "ra", ramp_=flash_ramp) for i in range(4)])
    add("flash_cloud", [additive(tex("0xD5E0756E6E0A3661"), "r", ramp_=flash_ramp)])
    add("flash_round", [additive(tex("0x42C135A68F015295"), "rgb")])
    add("spark", [additive(tex("0xDB09E23099057557"), "rgb")])
    add("glow", [additive(tex("0x33C951F132E74A9"), "r")])
    add("trail", [additive(tex("0x8A42316D7404489"), "a")])
    add("beam", [additive(tex("0xBF8FC85872FAEF59"), "a")])
    add("smoke", [blended(tex("0xE16DB8CFB975BC77", i), smoke_grey) for i in range(0, 32, 2)])
    # the shell's albedo, opaque (sampled by the shell mesh, not as a particle)
    add("shell", [tex("0xB099CF7CF005943F")])

    with open(os.path.join(OUT, "layers.bin"), "wb") as f:
        f.write(struct.pack("<II", SIZE, len(layers)))
        for im in layers:
            f.write(im.tobytes())
    with open(os.path.join(OUT, "layers.json"), "w") as f:
        json.dump(names, f, indent=1)

    m = castlib.read(SRC + r"\mdl\mdl\Weapons\shellejects\shelleject_pistol_LOD0.cast")[0].walk("model")[0]
    verts, idx = [], []
    for me in m.find("mesh"):
        vp, vn, uv, fs = me.get("vp"), me.get("vn"), me.get("u0"), list(me.get("f"))
        base = len(verts)
        for i in range(len(vp) // 3):
            verts.append(tuple(vp[i * 3:i * 3 + 3]) + tuple(vn[i * 3:i * 3 + 3]) + (uv[i * 2], uv[i * 2 + 1]))
        idx += [base + i for i in fs]
    with open(os.path.join(OUT, "shell.bin"), "wb") as f:
        f.write(struct.pack("<I", len(verts)))
        for v in verts:
            f.write(struct.pack("<8f", *v))
        f.write(struct.pack("<I", len(idx)))
        f.write(struct.pack(f"<{len(idx)}I", *idx))
    lo = [min(v[k] for v in verts) for k in range(3)]
    hi = [max(v[k] for v in verts) for k in range(3)]
    print(f"layers.bin: {len(layers)} layers {SIZE}px {list(names)}; shell: {len(verts)} verts {len(idx) // 3} tris, bounds {lo} {hi}")


if __name__ == "__main__":
    main()
