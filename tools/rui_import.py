"""Imports patched-RSX RUI exports into D:\R5Rust\rui\assets\<name>\ (asset.bin, meta.json, args.json), the layout
apex-content-kit's server reads; reuses the kit's import + arg naming (tools/rui-host/import-asset.py, args.py) read-only.
    python -I tools/rui_import.py <export dir> <name>[,<name>...] [owner script ...]
"""
import os, sys, importlib.util
KIT = r"C:\Code\apex\apex-content-kit\tools\rui-host"
sys.path.insert(0, KIT)
spec = importlib.util.spec_from_file_location("kit_import", os.path.join(KIT, "import-asset.py"))
kit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kit)
kit.here = r"D:\R5Rust\rui"   # assets go to D:\R5Rust\rui\assets\<name>, not into the kit
exp, names, owners = sys.argv[1].rstrip("/\\"), sys.argv[2].split(","), sys.argv[3:]
for n in names:
    meta, args = kit.import_one(exp, n, owners)
    print(n, meta, len(args), "args:", [a["name"] for a in args if a["name"]])

# Args set from weapon settings (RUI_CrosshairData: adjustedSpread, isSprinting, playerFov, ...) aren't in any script:
# name the rest by hashing every identifier in the retail weapon settings (tools/extract_weapon.ps1 exports them).
import glob, json, re, struct
WEPN = r"D:\R5Rust\_work_retail\weapon\out\weapon"
cands = set()
for f in glob.glob(os.path.join(WEPN, "*.txt")):
    cands |= set(re.findall(r'"([A-Za-z_][A-Za-z0-9_]{2,40})"', open(f, encoding="utf-8", errors="replace").read()))
for n in names:
    pre = os.path.join(exp, n + ".")
    cl = open(pre + "argClusters.bin", "rb").read()[:18]
    argIndex, argCount, mul, add = struct.unpack("<HHBB", cl[:6])
    def h(name):
        x = 0
        for c in name.encode():
            x = ((x >> 20) ^ (add + x * mul + c)) & 0xffffffff
        return x
    table = {}
    for c in cands:
        x = h(c)
        table.setdefault((x & (argCount - 1), (x >> 4) & 0xffff), []).append(c)
    out = os.path.join(kit.here, "assets", n, "args.json")
    args = json.load(open(out))
    for a in args:
        if not a.get("name"):
            hits = sorted(table.get((a["slot"], a["short"]), []), key=len)
            if hits:
                a["name"] = hits[0]
    json.dump(args, open(out, "w"), indent=0)
    print(n, "args now:", [(a["name"], a["type"], a.get("default")) for a in args])
