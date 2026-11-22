"""Retail Apex material constants -> the S21-style .uber_struct text tools/build_materials.py reads.

Retail shaders are DXIL (SM6), which RSX can't reflect, so its .uber_struct comes out empty. RSX can still export
the raw constant bytes (matl export format 1 -> <material>.uber). The layout comes from the material's own pixel
shader: the Windows SDK's `dxc -dumpbin` prints `struct struct.CBufUberStatic { <type> <name>; ; Offset: N ... }`.

    python tools/retail_uber.py [assets_root]      (default D:\\R5Rust\\assets-retail)

For every materials/**/<name>.json with a <name>.uber beside it (or anywhere under the root, matched by file name),
writes <name>.uber_struct next to the json. Shader disassemblies are cached in <root>/_dxc/.
"""
import glob, json, os, re, struct, subprocess, sys

root = sys.argv[1] if len(sys.argv) > 1 else r"D:\R5Rust\assets-retail"
# R5_DXC: any dxc.exe (Windows SDK, or DirectXShaderCompiler's release); tools/install.ps1 sets it
DXC = os.environ.get("R5_DXC") or r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\dxc.exe"
cache = os.path.join(root, "_dxc")
os.makedirs(cache, exist_ok=True)
# ";\s*;": long names end ";; Offset" with no space (c_depthBlendScalarAndL0_emissiveTint, ...AndL0_albedoTint)
FIELD = re.compile(r";\s+(float|float2|float3|float4|uint|int|uint2|uint3|uint4|int2|int3|int4)\s+(\w+)(?:\[(\d+)\])?;\s*; Offset:\s+(\d+)")
COMPONENTS = {"float": 1, "float2": 2, "float3": 3, "float4": 4, "uint": 1, "int": 1, "uint2": 2, "uint3": 3, "uint4": 4, "int2": 2, "int3": 3, "int4": 4}


def layout(ps_guid):
    """[(type, name, array_len, offset)] of CBufUberStatic in a pixel shader, or None."""
    out = os.path.join(cache, f"{ps_guid}.txt")
    if not os.path.exists(out):
        fxc = os.path.join(root, "shaders", "programs", f"{ps_guid}_0.fxc")
        if not os.path.exists(fxc):
            return None
        r = subprocess.run([DXC, "-dumpbin", fxc], capture_output=True, text=True)
        open(out, "w", encoding="utf-8").write(r.stdout)
    text = open(out, encoding="utf-8").read()
    i = text.find("struct struct.CBufUberStatic")
    if i < 0:
        return None
    em = re.compile(r"\n;\s+\}").search(text, i)
    end = em.start() if em else i + 20000
    return [(m.group(1), m.group(2), int(m.group(3) or 1), int(m.group(4))) for m in FIELD.finditer(text[i:end])]


def num(v):
    s = f"{v:.9g}"
    return "0" if s in ("-0", "nan", "inf", "-inf") else s


def main():
    ubers = {os.path.basename(p)[:-5].lower(): p for p in glob.glob(os.path.join(root, "**", "*.uber"), recursive=True)}
    done = missing_ps = missing_raw = 0
    for js in glob.glob(os.path.join(root, "materials", "**", "*.json"), recursive=True):
        stem = os.path.basename(js)[:-5].lower()
        raw = ubers.get(stem)
        if not raw:
            missing_raw += 1
            continue
        d = json.load(open(js, encoding="utf-8"))
        ss = d.get("shaderSet")
        msw = os.path.join(root, "shaders", "sets", f"{ss}.msw") if ss else ""
        if not ss or not os.path.exists(msw):
            missing_ps += 1
            continue
        ps = "0x%X" % struct.unpack("<Q", open(msw, "rb").read()[5:13])[0]
        lay = layout(ps)
        if not lay:
            missing_ps += 1
            continue
        b = open(raw, "rb").read()
        lines = ["struct CBufUberStatic", "{"]
        for ty, name, arr, off in lay:
            n = COMPONENTS[ty] * arr
            if off + 4 * n > len(b):
                continue
            fmt = "f" if ty.startswith("float") else ("I" if ty.startswith("uint") else "i")
            vals = struct.unpack_from(f"<{n}{fmt}", b, off)
            if fmt == "f":
                if n == 1:
                    lines.append(f"\tfloat {name} = {num(vals[0])};")
                else:
                    lines.append(f"\tfloat {name}[{n}] = {{ {', '.join(num(v) for v in vals)} }};")
            else:
                c = "uint32_t" if fmt == "I" else "int"
                lines.append(f"\t{c} {name} = {vals[0]};" if n == 1 else f"\t{c} {name}[{n}] = {{ {', '.join(str(v) for v in vals)} }};")
        lines.append("};")
        open(js[:-5] + ".uber_struct", "w", encoding="utf-8").write("\n".join(lines) + "\n")
        done += 1
    print(f"wrote {done} .uber_struct; {missing_raw} materials without raw .uber, {missing_ps} without a readable pixel shader")


main()
