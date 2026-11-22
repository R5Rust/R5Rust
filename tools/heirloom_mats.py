"""Heirloom / melee item materials for build_melee.py: textures named by the material's own pixel shader, plus the
constants the viewmodel shader needs.

Inputs:
- raw material constants: the kit's patched RSX `--exportformat matl=1` on common.rpak, filtered by NAME (a GUID filter
  exports nothing): <material>.json (shaderSet, blendStates, $textureTypes) + <material>.uber (CBufUberStatic bytes),
  copied to UBER_DIR.
- the shader set -> pixel shader (assets-retail/shaders/sets/<set>.msw) and its `dxc -dumpbin` text (assets-retail/_dxc,
  written by tools/retail_uber.py): the resource table names each texture register, the CBufUberStatic table the
  constants.
"""
import json
import os
import re
import shutil
import struct
import subprocess

UBER_DIR = r"D:\R5Rust\_work_retail\heirlooms\uber"
ASSETS = r"D:\R5Rust\assets-retail"
DXC = r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\dxc.exe"

# shader resource name -> weapon.json texture key
TEX_KEYS = {"albedoTexture": "albedo", "normalTexture": "normal", "glossTexture": "gloss", "specTexture": "spec",
            "emissiveTexture": "emissive", "aoTexture": "ao", "cavityTexture": "cavity",
            "emissiveMultiplyTexture": "emissive_mul", "uvDistortionTexture": "distortion",
            "opacityMultiplyTexture": "opacity_mul", "anisoSpecDirTexture": "aniso_dir"}
FIELD = re.compile(r";\s+(float|float2|float3|float4|uint|int|uint2|uint3|uint4|int2|int3|int4)\s+(\w+)(?:\[(\d+)\])?;\s*; Offset:\s+(\d+)")
COMPONENTS = {"float": 1, "float2": 2, "float3": 3, "float4": 4, "uint": 1, "int": 1, "uint2": 2, "uint3": 3, "uint4": 4, "int2": 2, "int3": 3, "int4": 4}


def pixel_shader(shader_set):
    msw = os.path.join(ASSETS, "shaders", "sets", f"{shader_set}.msw")
    if not shader_set or not os.path.exists(msw):
        return None
    return "0x%X" % struct.unpack("<Q", open(msw, "rb").read()[5:13])[0]


def disasm(ps):
    txt = os.path.join(ASSETS, "_dxc", f"{ps}.txt")
    if not os.path.exists(txt):
        fxc = os.path.join(ASSETS, "shaders", "programs", f"{ps}_0.fxc")
        if not os.path.exists(fxc):
            return ""
        r = subprocess.run([DXC, "-dumpbin", fxc], capture_output=True, text=True)
        open(txt, "w", encoding="utf-8").write(r.stdout)
    return open(txt, encoding="utf-8").read()


def shader_textures(text):
    """{t register: resource name} of 2D textures in a pixel shader's resource table."""
    return {int(m.group(2)): m.group(1) for m in re.finditer(r"^;\s+(\w+)\s+texture\s+f32\s+2d\s+T\d+\s+t(\d+)\s+1\s*$", text, re.M)}


def uber_layout(text):
    i = text.find("struct struct.CBufUberStatic")
    if i < 0:
        return []
    em = re.compile(r"\n;\s+\}").search(text, i)
    end = em.start() if em else i + 20000
    return [(m.group(1), m.group(2), int(m.group(3) or 1), int(m.group(4))) for m in FIELD.finditer(text[i:end])]


def constants(stem, text):
    raw = os.path.join(UBER_DIR, stem + ".uber")
    if not os.path.exists(raw) or not text:
        return {}
    b = open(raw, "rb").read()
    out = {}
    for ty, name, arr, off in uber_layout(text):
        n = COMPONENTS[ty] * arr
        if off + 4 * n <= len(b):
            fmt = "f" if ty.startswith("float") else ("I" if ty.startswith("uint") else "i")
            out[name] = list(struct.unpack_from(f"<{n}{fmt}", b, off))
    return out


def item_materials(m, out, cid, r97, fallback_slots):
    """weapon.json "materials" for one item: its own materials + the arms' (from the R-99 bake)."""
    mats = {}
    for mat in m["materials"]:
        stem = os.path.basename(mat["path"]).replace(".rpak", "")
        short = stem.rsplit("_", 1)[0]
        meta_p = os.path.join(UBER_DIR, stem + ".json")
        meta = json.load(open(meta_p)) if os.path.exists(meta_p) else {}
        ps = pixel_shader(meta.get("shaderSet") or mat.get("shaderset"))
        text = disasm(ps) if ps else ""
        names = shader_textures(text)
        entry = {}
        for t in mat.get("textures", []):
            slot = t.get("slot")
            if names:
                key = TEX_KEYS.get(names.get(slot, ""))
            else:
                key = fallback_slots[slot] if slot is not None and slot < len(fallback_slots) else None
            if key is None or not t.get("png") or not os.path.exists(t["png"][0]):
                continue
            dst = f"textures/{short}_{key}.png"
            shutil.copyfile(t["png"][0], os.path.join(out, "textures", f"{short}_{key}.png"))
            entry[key] = f"melee/{cid}/{dst}"
        c = constants(stem, text)
        if c:
            entry["emissive_tint"] = [round(x, 4) for x in c.get("c_depthBlendScalarAndL0_emissiveTint", [0, 1, 1, 1])[1:4]]
            entry["uv"] = [[round(x, 5) for x in c.get(f"c_uv{i}RotScaleX", [1, 0]) + c.get(f"c_uv{i}RotScaleY", [0, 1]) + c.get(f"c_uv{i}Translate", [0, 0])] for i in range(1, 6)]
            entry["uv_time_flags"] = int(c.get("c_uvUseModelAnimUVTimeFlags", [0])[0])
            entry["distortion"] = [round(x, 4) for x in c.get("c_uvDistortionIntensity", [0, 0]) + c.get("c_uvDistortion2Intensity", [0, 0])]
            entry["opacity"] = c.get("c_opacity", [1.0])[0]
            entry["alpha_modulate_emissive"] = c.get("c_useAlphaModulateEmissive", [0.0])[0]
            entry["emissive_edge"] = [c.get(k, [0.0])[0] for k in ("c_emissiveEdgeFadeExponent", "c_emissiveEdgeFadeInner", "c_emissiveEdgeFadeOuter")]
            entry["perf_spec"] = [c.get(f"c_L0_perfSpecColor{a}", [0.04])[0] for a in "XYZ"]
            entry["perf_gloss"] = c.get("c_L0_perfGlossAndL1_albedoTint", [0.25])[0]
        else:
            # no constants: retail tints the emissive mask; off rather than full white
            entry["emissive_tint"] = [0.0, 0.0, 0.0]
        # alpha blended (blend state other than the opaque 0xF0000000): drawn after the opaque meshes
        entry["translucent"] = (meta.get("blendStates") or ["0xF0000000"])[0].upper() != "0XF0000000"
        entry["pixel_shader"] = ps
        mats[short] = entry
    for name, entry in r97["materials"].items():
        if "wraith" in name:
            mats[name] = entry
    return mats
