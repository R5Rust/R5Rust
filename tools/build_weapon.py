"""Bakes one retail Apex weapon's first-person data for the client (crates/client/src/viewmodel.rs, weapon.rs).

Inputs (RSX exports, see HANDOFF.md "R-99"): the viewmodel Cast (gun), the arms Cast, the anim rig's sequences (Cast),
the weapon settings txt (RSX `wepn` export) + viewkick_patterns.txt, the materials' textures (PNG, slot order
0 albedo 1 normal 2 gloss 3 spec 4 emissive 5 ao 6 cavity).
Output dir: view.bin (skeleton, skinned meshes, animations sampled per frame), weapon.json (settings + recoil pattern),
textures/*.png.

view.bin, little-endian:
  "R5VM" u32 version(1)
  u32 bones;  per bone: str name, i32 parent, f32x3 local pos, f32x4 local rot (xyzw)          (bind pose)
  u32 meshes; per mesh: str material, u32 verts, u32 indices,
              verts x (f32x3 pos, f32x3 normal, f32x2 uv, u8x4 bone, u8x4 weight), indices x u32
  u32 anims;  per anim: str name, f32 fps, u32 frames, u8 additive, frames x bones x (f32x3 pos, f32x4 rot)
str = u16 length + utf8.
"""
import json, math, os, re, shutil, struct, sys
sys.path.insert(0, os.path.dirname(__file__))
import castlib

W = r"D:\R5Rust\_work_retail\weapon"
OUT = r"D:\R5Rust\assets-retail\weapons\mp_weapon_r97"
GUN = W + r"\mdl3\mdl\techart\mshop\weapons\class\smg\r99\r99_base_v_LOD0.cast"
ARMS = W + r"\arms\mdl\Weapons\arms\pov_pilot_light_wraith_cyber_ninja_LOD0.cast"
SEQ = W + r"\seq"
# the same sequences exported as SMD + QC by the kit's patched RSX (--exportformat aseq=2): the QCs carry the events
SEQ_QC = r"D:\R5Rust\_work_retail\seqqc\out"
SETTINGS = W + r"\out\weapon\mp_weapon_r97.txt"
PATTERNS = W + r"\out\weapon\viewkick_patterns.txt"
SPRINGS = W + r"\out\weapon\springs.txt"
TEXMAP = W + r"\mat_textures.json"
# the default bodygroups: body, iron sights, magazine (attachments come with the attachment system)
GUN_MESHES = ("body_0_", "sight_front_1_", "sight_rear_1_", "clip_0_")
# sequence file -> runtime name (variant _0 unless named)
ANIMS = {
    "draw_0": "draw", "drawfirst_0": "drawfirst", "holster_0": "holster", "idle_0": "idle",
    "fire_0": "fire", "fire_1": "fire_1", "fire_2": "fire_2", "fire_3": "fire_3",
    "ads_in_0": "ads_in", "ads_in_1": "ads_in_1", "ads_out_0": "ads_out", "ads_out_1": "ads_out_1",
    "reload_seq_0": "reload", "reload_empty_seq_0": "reload_empty", "sprint_0": "sprint",
    "sprintraise_0": "sprintraise", "jump_0": "jump", "land_0": "land", "crouch_0": "crouch",
    "inspect_new_0": "inspect", "inspect_new_empty_0": "inspect_empty",
}
SLOTS = ["albedo", "normal", "gloss", "spec", "emissive", "ao", "cavity"]


def wstr(f, s):
    b = s.encode()
    f.write(struct.pack("<H", len(b)) + b)


def qmul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx, aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz)


def slerp(a, b, t):
    d = sum(x * y for x, y in zip(a, b))
    if d < 0:
        b, d = tuple(-x for x in b), -d
    if d > 0.9995:
        r = tuple(x + (y - x) * t for x, y in zip(a, b))
    else:
        th = math.acos(d)
        s = math.sin(th)
        wa, wb = math.sin((1 - t) * th) / s, math.sin(t * th) / s
        r = tuple(wa * x + wb * y for x, y in zip(a, b))
    n = math.sqrt(sum(x * x for x in r)) or 1.0
    return tuple(x / n for x in r)


def model_of(path):
    return castlib.read(path)[0].walk("model")[0]


def skeleton(model):
    out = []
    for b in model.walk("bone"):
        out.append((b.get("n"), castlib.scalar(b.get("p"), -1), tuple(b.get("lp", (0, 0, 0))), tuple(b.get("lr", (0, 0, 0, 1)))))
    # Cast stores the parent as an unsigned index: 0xFFFFFFFF = root
    return [(n, -1 if p in (-1, 0xFFFFFFFF, 0xFFFF) else p, lp, lr) for n, p, lp, lr in out]


def meshes(model, bone_remap, keep):
    mats = {m.hash: m.get("n") for m in model.find("material")}
    out = []
    for me in model.find("mesh"):
        name = me.get("n")
        if keep and not name.startswith(keep):
            continue
        vp, vn, uv = me.get("vp"), me.get("vn"), me.get("u0")
        n = len(vp) // 3
        mi = castlib.scalar(me.get("mi"), 1)
        wb, wv = me.get("wb", ()), me.get("wv", ())
        verts = []
        for i in range(n):
            infl = []
            for k in range(mi):
                j = i * mi + k
                if j < len(wb) and j < len(wv) and wv[j] > 0:
                    infl.append((wv[j], bone_remap[wb[j]]))
            infl.sort(reverse=True)
            infl = infl[:4] or [(1.0, 0)]
            tot = sum(w for w, _ in infl) or 1.0
            ws = [int(round(w / tot * 255)) for w, _ in infl]
            ws[0] += 255 - sum(ws)
            bs = [b for _, b in infl]
            while len(bs) < 4:
                bs.append(0)
                ws.append(0)
            verts.append((vp[i * 3:i * 3 + 3], vn[i * 3:i * 3 + 3], (uv[i * 2], uv[i * 2 + 1]), bs, ws))
        out.append((mats.get(castlib.scalar(me.get("m"))), verts, list(me.get("f")), name or ""))
    return out


def qc_events(stem):
    """(seconds, event, options) from the sequence's QC (`{ event "AE_CL_PLAYSOUND" 51 "Wpn_R97_Reload_InsertMag" }`)."""
    import re
    hits = [os.path.join(dp, x) for dp, _dn, fs in os.walk(SEQ_QC) for x in fs if x == stem + ".qc"]
    if not hits:
        return None
    txt = open(hits[0], encoding="utf-8", errors="replace").read()
    m = re.search(r"\bfps\s+([\d.]+)", txt)
    fps = float(m.group(1)) if m else 30.0
    out = []
    for ev, frame, opt in re.findall(r'\{\s*event\s+"?([\w]+)"?\s+(-?[\d.]+)\s*(?:"([^"]*)")?\s*\}', txt):
        out.append((float(frame) / fps, ev, opt or ""))
    return out


def sample_anim(path, bones):
    a = castlib.read(path)[0].walk("animation")[0]
    fps = castlib.scalar(a.get("fr"), 30.0)
    curves = a.walk("curve")
    if not curves:
        return None
    additive = any(c.get("m") == "additive" for c in curves)
    frames = max(max(c.get("kb", (0,))) for c in curves) + 1
    idx = {n: i for i, (n, *_r) in enumerate(bones)}
    # per bone: pos channels + rot keys
    pos = {}
    rot = {}
    for c in curves:
        b = idx.get(c.get("nn"))
        if b is None:
            continue
        kb, kv, kp = list(c.get("kb", ())), list(c.get("kv", ())), c.get("kp")
        if kp == "rq":
            rot[b] = (kb, [tuple(kv[i * 4:i * 4 + 4]) for i in range(len(kb))])
        elif kp in ("tx", "ty", "tz"):
            pos.setdefault(b, {})[kp] = (kb, kv)

    def at(kb, vals, f, lerp):
        if f <= kb[0]:
            return vals[0]
        if f >= kb[-1]:
            return vals[-1]
        for k in range(len(kb) - 1):
            if kb[k] <= f <= kb[k + 1]:
                t = (f - kb[k]) / max(kb[k + 1] - kb[k], 1e-6)
                return lerp(vals[k], vals[k + 1], t)
        return vals[-1]

    data = []
    for f in range(frames):
        for b, (_n, _p, lp, lr) in enumerate(bones):
            p = list((0.0, 0.0, 0.0) if additive else lp)
            if b in pos:
                for ax, ch in enumerate(("tx", "ty", "tz")):
                    if ch in pos[b]:
                        kb, kv = pos[b][ch]
                        p[ax] = at(kb, kv, f, lambda x, y, t: x + (y - x) * t)
            q = (0.0, 0.0, 0.0, 1.0) if additive else lr
            if b in rot:
                q = at(rot[b][0], rot[b][1], f, slerp)
            data.append((p, q))
    return fps, frames, additive, data


def kv_tree(text):
    """RSX's KeyValues text -> nested dicts (repeated keys inside one block: the first wins)."""
    toks = re.findall(r'"((?:[^"\\]|\\.)*)"|([{}])', re.sub(r"//[^\n]*", "", text))
    pos = 0

    def block():
        nonlocal pos
        d = {}
        while pos < len(toks):
            s, br = toks[pos]
            pos += 1
            if br == "}":
                return d
            key = s
            s2, br2 = toks[pos]
            pos += 1
            if br2 == "{":
                v = block()
            else:
                v = s2
            d.setdefault(key, v)
        return d

    return block()


RIG = W + r"\rig\animrig\techart\mshop\weapons\class\smg\r99\r99_base_v_animRig.cast"


def r97_sequences():
    """Every R-99 sequence: its QC (both export trees) + its Casts (one per animation, `<name>.rseq/<name>_<k>.cast`)."""
    import vmbake
    casts = {}
    for dp, _dn, fs in os.walk(SEQ):
        if dp.endswith(".rseq"):
            nm = os.path.basename(dp)[:-5]
            casts[nm] = sorted((os.path.join(dp, f) for f in fs if f.endswith(".cast")), key=lambda x: int(x.rsplit("_", 1)[1].split(".")[0]))
    out, seen = [], set()
    for dp, _dn, fs in os.walk(SEQ_QC):
        if "ptpov_r97" not in dp and "r99_base_v_animRig" not in dp:
            continue
        for f in sorted(fs):
            if not f.endswith(".qc"):
                continue
            q = vmbake.parse_qc(os.path.join(dp, f))
            if not q or q["name"] in seen:
                continue
            seen.add(q["name"])
            out.append((q, casts.get(q["name"], [])))
    return out


def weapon_springs(flat):
    """The viewkick springs the settings name (viewkick_spring + _hot): springs.txt blocks of hipfire_/ADS_
    pitch/yaw/roll springConstant + damping."""
    tree = kv_tree(open(SPRINGS, encoding="utf-8", errors="replace").read())
    top = next(iter(tree.values())) if len(tree) == 1 and isinstance(next(iter(tree.values())), dict) else tree
    out = {}
    for slot, key in (("normal", "viewkick_spring"), ("hot", "viewkick_spring_hot")):
        name = flat.get(key)
        blk = top.get(name) if name else None
        if isinstance(blk, dict):
            out[slot] = {k: v for k, v in blk.items() if isinstance(v, str)}
            out[slot]["name"] = name
    return out


def main():
    import vmbake
    os.makedirs(OUT + r"\textures", exist_ok=True)
    gun, arms = model_of(GUN), model_of(ARMS)
    gun_bones, arms_bones = skeleton(gun), skeleton(arms)
    rig_bones = skeleton(model_of(RIG))
    # the rig's reference pose is what every frame starts from; gun-only bones (muzzle_flash, shell...) hang under it
    bones, names = vmbake.merge_skeleton(rig_bones, [gun_bones])
    missing = [n for n, *_r in arms_bones if n not in names]
    if missing:
        print("arms bones not in the rig (bound to the root):", missing[:10])
    gun_ms = meshes(gun, [names[n] for n, *_r in gun_bones], GUN_MESHES)
    arms_ms = meshes(arms, [names.get(n, 0) for n, *_r in arms_bones], None)
    ms = vmbake.rebind(gun_ms, gun_bones, bones, names) + vmbake.rebind(arms_ms, arms_bones, bones, names)

    seqs = r97_sequences()
    anims, recs, events = vmbake.bake_sequences(seqs, bones)
    ref = vmbake.reference_pose(bones, W + r"\mdl3\mdl\techart\mshop\weapons\class\smg\r99\anims_r99_base_v\ref\ref_0.cast", anims, recs)
    vmbake.write_view(OUT + r"\view.bin", bones, ms, anims, recs, events, ref)
    acts = sorted({q["activity"] for q, _ in recs if q["activity"]})
    print(f"sequences: {len(recs)} ({len(anims)} animations), activities {len(acts)}, transitions {sum(1 for q, _ in recs if q['entry'] != q['exit'])}")

    # materials: textures by slot, copied with readable names
    texmap = json.load(open(TEXMAP))
    mats = {}
    for mpath, guids in texmap.items():
        short = os.path.basename(mpath).replace("_sknp.rpak", "")
        entry = {}
        for slot, g in zip(SLOTS, guids):
            src = os.path.join(W, "tex", "texture", "0x%s.png" % g[2:].lstrip("0").upper())
            if not os.path.exists(src):
                src = os.path.join(W, "tex", "texture", g + ".png")
            if os.path.exists(src):
                dst = f"textures/{short}_{slot}.png"
                shutil.copyfile(src, os.path.join(OUT, dst.replace("/", "\\")))
                entry[slot] = f"weapons/mp_weapon_r97/{dst}"
        mats[short] = entry

    st = kv_tree(open(SETTINGS, encoding="utf-8", errors="replace").read())
    top = next(iter(st.values())) if len(st) == 1 and isinstance(next(iter(st.values())), dict) else st
    flat = {k: v for k, v in top.items() if isinstance(v, str)}
    pats = kv_tree(open(PATTERNS, encoding="utf-8", errors="replace").read())

    def find(d, key):
        if isinstance(d, dict):
            if key in d and isinstance(d[key], dict):
                return d[key]
            for v in d.values():
                r = find(v, key)
                if r:
                    return r
        return None

    pat = find(pats, flat.get("viewkick_pattern", "")) or {}
    bullets = []
    for i in range(200):
        v = pat.get(f"bullet_{i}")
        if v is None:
            break
        bullets.append([float(x) for x in v.split()])
    springs = weapon_springs(flat)
    json.dump({"settings": flat, "springs": springs, "viewkick": {"name": flat.get("viewkick_pattern"), "loop_offset": int(pat.get("loop_offset", "0") or 0), "bullets": bullets},
               "materials": mats, "anim_events": events}, open(OUT + r"\weapon.json", "w"), indent=1)
    tris = sum(len(i) for _m, _v, i, _n in ms) // 3
    print("events:", {k: len(v) for k, v in events.items()})
    print(f"view.bin: {len(bones)} bones, {len(ms)} meshes ({tris} tris), {len(anims)} anims: {[a[0] for a in anims]}")
    print(f"weapon.json: {len(flat)} settings, {len(bullets)} recoil bullets, materials {list(mats)}")


if __name__ == "__main__":
    main()
