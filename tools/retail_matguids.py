"""Material asset guids a world.glb uses, kept to the ones a pak catalog has (retail paks name nothing, so the material
export selects by guid: StringToGuid("material/<glb material name>_<shader>.rpak") for every shader suffix).

    python tools/retail_matguids.py <world.glb> <catalog.csv> <out guids.txt>
"""
import csv, json, struct, sys

M32, M64 = 0xFFFFFFFF, 0xFFFFFFFFFFFFFFFF


def guid(name: str) -> int:
    """Respawn's rpak StringToGuid (port of r5ed-compile pak.rs string_to_guid)."""
    b = name.encode("latin1") + b"\0" * 8
    word = lambda at: int.from_bytes(b[at:at + 4], "little")
    acc, at = 0, 0
    while True:
        w = word(at)
        v4 = (~w & M32) & ((w - 0x01010101) & M32) & 0x80808080
        v5 = v4 ^ ((v4 - 1) & M32)
        v6 = (v5 & w) ^ 0x5C5C5C5C
        v7 = (~v6 & M32) & ((v6 - 0x01010101) & M32) & 0x80808080
        v8 = v7 & ((-v7) & M32)
        if v7 != v8:
            v9 = 0xFF000000
            while True:
                prev = v9
                if v9 & v6 == 0:
                    v8 |= v9 & 0x80808080
                v9 >>= 8
                if prev < 0x100:
                    break
        v11 = (0x633D5F1 * acc) & M64
        inner = (((v5 & w) - ((45 * (v8 >> 7)) & M32)) & M32) & 0xDFDFDFDF
        v12 = ((0xFB8C4D96501 * inner) & M64) >> 24
        if v4 != 0:
            break
        at += 4
        s = (v11 + v12) & M64
        acc = (s >> 61) ^ s
    high = 0 if v5 == 0 else v5.bit_length() - 1
    index = (at + high // 8) & M32
    return (v12 + v11 - 0xAE502812AA7333 * index) & M64


assert guid("material/world/concrete/dam_concrete_02_stained_bm_wldc.rpak") == 0xE779D0F273761152

glb, cat, out = sys.argv[1:4]
f = open(glb, "rb")
f.read(12)
n, _ = struct.unpack("<II", f.read(8))
mats = json.loads(f.read(n))["materials"]
have = {int(r["guid"], 16) for r in csv.DictReader(open(cat, encoding="utf-8")) if r["type"] == "matl"}
SH = ["wldc", "wldu", "rgdp", "rgdu", "rgdc", "sknp", "sknc", "skn", "fix"]
found, miss = set(), []
for m in mats:
    name = (m.get("extras") or {}).get("material") or m.get("name", "")
    hits = {guid(f"material/{name}_{sh}.rpak") for sh in SH} & have
    found |= hits
    if not hits:
        miss.append(name)
open(out, "w").write("".join(f"{g:016x}\n" for g in sorted(found)))
print(f"{len(mats)} glb materials -> {len(found)} material assets; {len(miss)} not in the catalog: {miss[:6]}")
