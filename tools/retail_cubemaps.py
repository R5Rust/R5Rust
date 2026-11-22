"""Retail reflection cubemaps -> <map>/cubemaps.dds (the layout the client reads, same as the S21 file).

Retail keeps env cubemaps in `cube` assets (new since S21). R5Flowstate's RSX fork (D:\\R5Rust\\tools\\rsx-fs) exports
each one twice into the same folder (retail_setup.ps1 step 7):
  --exportsetting cube=0  cubemap/<guid>.dds   DX10 BC6H_UF16 cube, Oodle-decompressed, face-major (6 faces x full mip
                                               chain: 87408 bytes per face at 256^2 / 9 mips)
  --exportsetting cube=1  cubemap/<guid>.json  {"index": n, ...}: the cubemap's slot = the BSP cubemap lump order
Output: one DX10 BC6H array of n*6 slices (one slice per face, cube i = slices 6i..6i+5), which is how the S21 file was
laid out and what crates/client/src/lighting.rs load_cubes reads.

    python tools/retail_cubemaps.py <export dir> <out cubemaps.dds>
"""
import glob, json, os, struct, sys

dump, out = sys.argv[1], sys.argv[2]
cubes = {}
for j in glob.glob(os.path.join(dump, "**", "*.json"), recursive=True):
    dds = j[:-5] + ".dds"
    if not os.path.exists(dds):
        continue
    idx = json.load(open(j))["index"]
    b = open(dds, "rb").read()
    if b[:4] != b"DDS " or b[84:88] != b"DX10":
        sys.exit(f"{dds}: not a DX10 dds")
    h, w, mips = struct.unpack_from("<III", b, 12)[0], struct.unpack_from("<I", b, 16)[0], struct.unpack_from("<I", b, 28)[0]
    if struct.unpack_from("<I", b, 128)[0] != 95:
        sys.exit(f"{dds}: not BC6H_UF16")
    if idx in cubes:
        sys.exit(f"two cubemaps claim index {idx}")
    cubes[idx] = (w, h, mips, b[:148], b[148:])
if not cubes:
    sys.exit(f"no cubemap .dds + .json pairs under {dump}")
n = max(cubes) + 1
missing = [i for i in range(n) if i not in cubes]
if missing:
    sys.exit(f"cubemap indices missing: {missing}")
w, h, mips, hdr, _ = cubes[0]
if any((c[0], c[1], c[2]) != (w, h, mips) or len(c[4]) != len(cubes[0][4]) for c in cubes.values()):
    sys.exit("cubemaps differ in size / mips")
t = bytearray(hdr)
struct.pack_into("<I", t, 24, 1)                          # depth 1 (as the S21 file)
struct.pack_into("<I", t, 112, 0)                         # caps2: a plain array, not a cube (caps at 108 stay)
struct.pack_into("<I", t, 136, 0)                         # DX10 miscFlag: no TEXTURECUBE
struct.pack_into("<I", t, 140, n * 6)                     # one array slice per face
with open(out, "wb") as f:
    f.write(t)
    for i in range(n):
        f.write(cubes[i][4])
print(f"{n} cubemaps {w}x{h}, {mips} mips -> {out} ({os.path.getsize(out):,} bytes)")
