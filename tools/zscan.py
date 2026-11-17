"""Z-fighting scan: renders every pose twice with the camera nudged 0.1 unit, in two debug views of the client
(--debug 11 = a flat colour per material, --debug 12 = view distance), and counts the pixels where the visible
material flips between the two renders while the distance stays the same (two coplanar surfaces trading places).
Silhouette edges move with the nudge too, but there the distance jumps; creases where two surfaces meet are thin
lines, removed by requiring a dense patch of flips (z-fighting is noise over an area).

    python tools/zscan.py OUT_DIR [--exe target/release/r5rust.exe] [--poses tools/r5f-probe/poses.json]
                                  [--extra name,x,y,z_eye,pitch,yaw ...] [--spots] [--pitches 0,25,50]
                                  [--nudge 1.0] [--only a,b] [--reuse]

Prints the count per pose, the total, and the material pairs that fight; writes OUT_DIR/<pose>_zf.png
(flipping pixels in red over the material view) for every pose with hits.
"""
import argparse, json, os, struct, subprocess, sys
import numpy as np
from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NUDGE = (0.06, 0.06, 0.05)  # small, so trims a unit off a wall can not slide over it
# Kainz's reported spots (eye positions), looked at from 8 directions
SPOTS = [("k1", -5065, -521, 180), ("k2", -5988, 70, -19), ("k3", -4453, 3936, 112), ("k4", -4039, 4769, 112),
         ("k5", -7999, -1338, 492)]


def hash_u32(x):
    x = (((x >> 16) ^ x) * 0x45d9f3b) & 0xFFFFFFFF
    x = (((x >> 16) ^ x) * 0x45d9f3b) & 0xFFFFFFFF
    return (x >> 16) ^ x


def material_names(glb):
    with open(glb, "rb") as f:
        f.read(12)
        n, _ = struct.unpack("<II", f.read(8))
        j = json.loads(f.read(n))
    return [m.get("name", f"#{i}") for i, m in enumerate(j.get("materials", []))]


def render(exe, poses, out, debug):
    lst = out + ".txt"
    with open(lst, "w") as f:
        for p in poses:
            f.write(" ".join(str(v) for v in p) + "\n")
    r = subprocess.run([exe, "--screenshot", out, "--poses", lst, "--fov", "100", "--debug", str(debug)], capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"render failed: {r.stderr[-2000:]}")


def load(path):
    a = np.asarray(Image.open(path).convert("RGB")).astype(np.uint32)
    return a[..., 0] | (a[..., 1] << 8) | (a[..., 2] << 16)


def box(m, r):
    """Mean of a boolean mask over a (2r+1)^2 window."""
    c = np.pad(m.astype(np.float32), r).cumsum(0).cumsum(1)
    c = np.pad(c, ((1, 0), (1, 0)))
    k = 2 * r + 1
    return (c[k:, k:] - c[:-k, k:] - c[k:, :-k] + c[:-k, :-k]) / (k * k)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--exe", default=os.path.join(ROOT, "target", "release", "r5rust.exe"))
    ap.add_argument("--poses", default=os.path.join(ROOT, "tools", "r5f-probe", "poses.json"))
    ap.add_argument("--glb", default=r"D:\R5Rust\assets\maps\mp_rr_canyonlands_staging_mu1\world.glb")
    ap.add_argument("--spots", action="store_true", help="add Kainz's z-fighting spots at 8 yaws")
    ap.add_argument("--pitches", default="25", help="pitches for --spots")
    ap.add_argument("--nudge", type=float, default=1.0, help="scale of the camera nudge (1 = ~0.1 unit)")
    ap.add_argument("--reuse", action="store_true", help="analyse the renders already in OUT_DIR")
    ap.add_argument("--extra", nargs="*", default=[], help="name,x,y,z_eye,pitch,yaw")
    ap.add_argument("--only", default="", help="comma list of pose name prefixes")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)

    poses = []
    if a.poses:
        for name, x, y, z, pitch, yaw in json.load(open(a.poses)):
            poses.append((name, x, y, z + 60, pitch, yaw))
    if a.spots:
        for name, x, y, z in SPOTS:
            for pitch in map(float, a.pitches.split(",")):
                for yaw in range(0, 360, 45):
                    poses.append((f"{name}_y{yaw}" + (f"_p{pitch:g}" if a.pitches != "25" else ""), x, y, z, pitch, yaw))
    for e in a.extra:
        v = e.split(",")
        poses.append((v[0], *map(float, v[1:])))
    if a.only:
        pre = a.only.split(",")
        poses = [p for p in poses if any(p[0].startswith(q) for q in pre)]
    k = a.nudge
    nudged = [(n, x + NUDGE[0] * k, y + NUDGE[1] * k, z + NUDGE[2] * k, p, w) for n, x, y, z, p, w in poses]

    dirs = {k: os.path.join(a.out, k) for k in ("idA", "idB", "dA", "dB")}
    if not a.reuse:
        render(a.exe, poses, dirs["idA"], 11)
        render(a.exe, nudged, dirs["idB"], 11)
        render(a.exe, poses, dirs["dA"], 12)
        render(a.exe, nudged, dirs["dB"], 12)

    names = material_names(a.glb)
    ids = {hash_u32(i + 1) & 0xFFFFFF: i for i in range(len(names))}
    lut_keys = np.array(sorted(ids))
    total, pairs = 0, {}
    for n, *_ in poses:
        ia, ib = load(os.path.join(dirs["idA"], n + ".png")), load(os.path.join(dirs["idB"], n + ".png"))
        da, db = load(os.path.join(dirs["dA"], n + ".png")) / 64.0, load(os.path.join(dirs["dB"], n + ".png")) / 64.0
        pure = np.isin(ia, lut_keys) & np.isin(ib, lut_keys)
        flip = pure & (ia != ib)
        same = np.abs(da - db) < 0.15 * k + 2e-4 * da
        cand = flip & same
        zf = cand & (box(cand, 3) >= 0.25)
        c = int(zf.sum())
        total += c
        top = {}
        for x, y in zip(ia[zf], ib[zf]):
            pk = tuple(sorted((names[ids[int(x)]], names[ids[int(y)]])))
            pairs[pk] = pairs.get(pk, 0) + 1
            top[pk] = top.get(pk, 0) + 1
        best = max(top, key=top.get) if top else None
        print(f"{n:14} {c:7d}" + (f"   mostly {best[0]} <-> {best[1]}" if best else ""))
        if c:
            img = np.stack([(ia >> s) & 255 for s in (0, 8, 16)], -1).astype(np.uint8) // 3
            img[zf] = (255, 0, 0)
            Image.fromarray(img).save(os.path.join(a.out, n + "_zf.png"))
    print(f"TOTAL {total} z-fighting pixels over {len(poses)} poses")
    for k, v in sorted(pairs.items(), key=lambda kv: -kv[1])[:40]:
        print(f"{v:7d}  {k[0]}  <->  {k[1]}")


if __name__ == "__main__":
    main()
