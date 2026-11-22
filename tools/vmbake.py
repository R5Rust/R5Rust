"""
vmbake.py - the shared first-person bake (build_weapon.py: the R-99, build_melee.py: empty hands + heirlooms), laid out
the way the game's own animation system reads a viewmodel (HANDOFF.md "Viewmodel animation, 1:1"; the rules are in
D:\\R5Rust\\_work_retail\\anim_re\\sequence_playback.md, reversed from the S21 client):

- Bones: the anim rig's skeleton, its bind pose = the reference pose every frame starts from (InitPose), plus any
  model-only bone (muzzle_flash, knife_base...) under its parent.
- Meshes (the item's model + the pov arms): each vertex moved from its own model's bind pose into the rig's reference
  pose through its bone weights, so skinning lines up (the models and the rig disagree on ~30 arm bones by up to 6 u).
- Sequences, ALL of them, from their QC (RSX --exportformat aseq=2) + Cast curves (one Cast per animation): activity,
  weight, modifiers, fadein / fadeout (studiomdl default 0.2), entry / exit node (`node`, `transition`), loop / delta /
  autoplay / snap, the blend grid (`blend "param" lo hi`, `blendwidth`, one animation per cell), `addlayer`s and the
  bone weightlist; events per sequence.

view.bin = build_weapon.py's format ("R5VM", bones, meshes, anims) + "EVTS" (per sequence) + "MNMS" (mesh names) +
"SEQS": u32 n; per sequence: str name, str activity, f32 weight, u8 n + str modifiers, f32 fadein, f32 fadeout,
str entry, str exit, u8 flags (1 loop, 2 delta, 4 autoplay, 8 snap), f32 fps, u8 n + str animations, u8 blendwidth,
u8 n + (str param, f32 lo, f32 hi) blends, u8 n + str layers, u32 n (0 = every bone 1) + n x f32 bone weights.
"""
import os, re, struct

import numpy as np

import castlib
import build_weapon as bw


# ---------------------------------------------------------------- QC

def parse_qc(path):
    """One sequence's QC (RSX's SMD+QC export) -> dict."""
    txt = open(path, encoding="utf-8", errors="replace").read()
    seq = {"name": None, "anims": [], "activity": "", "weight": 1.0, "mods": [], "fadein": 0.2, "fadeout": 0.2,
           "entry": "", "exit": "", "flags": 0, "blends": [], "blendwidth": 1, "layers": [], "weights": {}, "events": [], "fps": 30.0}
    fps = {}
    for m in re.finditer(r'\$animation\s+"([^"]+)"\s+"[^"]*"[^{]*\{(.*?)\n\}', txt, re.S):
        f = re.search(r"\bfps\s+([\d.]+)", m.group(2))
        fps.setdefault(m.group(1), float(f.group(1)) if f else 30.0)
    wl = re.search(r'\$weightlist\s+"[^"]+"\s*\{(.*?)\}', txt, re.S)
    if wl:
        for b, w in re.findall(r'"([^"]+)"\s+([-\d.]+)', wl.group(1)):
            seq["weights"][b] = float(w)
    m = re.search(r'\$sequence\s+"([^"]+)"(.*?)\{(.*)\}', txt, re.S)
    if not m:
        return None
    seq["name"] = m.group(1)
    head, body = m.group(2), m.group(3)
    head_anims = re.findall(r'"([^"]+)"', head)
    head_flags = head
    # animations: in the header (single) or as the body's leading quoted names (blend grids)
    anims = list(head_anims)
    for line in body.splitlines():
        s = line.strip()
        if not s:
            continue
        if s.startswith('"'):
            anims += re.findall(r'"([^"]+)"', s)
            continue
        break
    seq["anims"] = anims
    seq["fps"] = fps.get(anims[0], 30.0) if anims else 30.0
    words = set(re.findall(r"\b(loop|delta|autoplay|snap)\b", head_flags))
    for line in body.splitlines():
        s = line.strip()
        if s in ("loop", "delta", "autoplay", "snap"):
            words.add(s)
    seq["flags"] = (1 if "loop" in words else 0) | (2 if "delta" in words else 0) | (4 if "autoplay" in words else 0) | (8 if "snap" in words else 0)
    a = re.search(r'\bactivity\s+"(\w+)"\s+([-\d.]+)', body)
    if a:
        seq["activity"], seq["weight"] = a.group(1), float(a.group(2))
    seq["mods"] = [x.lower() for x in re.findall(r'activitymodifier\s+"(\w+)"', body)]
    for k in ("fadein", "fadeout"):
        f = re.search(r"\b%s\s+([\d.]+)" % k, body)
        if f:
            seq[k] = float(f.group(1))
    n = re.search(r'\bnode\s+"([^"]+)"', body)
    if n:
        seq["entry"] = seq["exit"] = n.group(1)
    t = re.search(r'\btransition\s+"([^"]+)"\s+"([^"]+)"', body)
    if t:
        seq["entry"], seq["exit"] = t.group(1), t.group(2)
    bwid = re.search(r"\bblendwidth\s+(\d+)", body)
    if bwid:
        seq["blendwidth"] = int(bwid.group(1))
    seq["blends"] = [(p, float(lo), float(hi)) for p, lo, hi in re.findall(r'\bblend\s+"(\w+)"\s+([-\d.]+)\s+([-\d.]+)', body)]
    seq["layers"] = re.findall(r'\baddlayer\s+"(\w+)"', body)
    for ev, frame, opt in re.findall(r'\{\s*event\s+"?(\w+)"?\s+(-?[\d.]+)\s*(?:"([^"]*)")?\s*\}', body):
        seq["events"].append((float(frame) / max(seq["fps"], 1.0), ev, opt or ""))
    return seq


# ---------------------------------------------------------------- skeleton math

def qmat(q):
    x, y, z, w = q
    return np.array([[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
                     [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
                     [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]])


def world_mats(bones):
    """[(name, parent, lp, lr)] -> 4x4 world matrices (parents first)."""
    out = []
    for n, p, lp, lr in bones:
        m = np.eye(4)
        m[:3, :3] = qmat(lr)
        m[:3, 3] = lp
        out.append(out[p] @ m if 0 <= p < len(out) else m)
    return out


def merge_skeleton(rig, extra_models):
    """The rig's bones, then any bone of the item models it lacks (parented by name, the model's local transform)."""
    bones = list(rig)
    names = {n: i for i, (n, *_r) in enumerate(bones)}
    for model in extra_models:
        for n, p, lp, lr in model:
            if n in names:
                continue
            pn = model[p][0] if p >= 0 else None
            bones.append((n, names.get(pn, -1), lp, lr))
            names[n] = len(bones) - 1
    return bones, names


def rebind(meshes, src_bones, dst_bones, dst_names):
    """Move mesh vertices from the source model's bind pose into the destination skeleton's bind pose.
    Each mesh's bone indices are already remapped to dst; the source bone for a dst index is found by name."""
    src_w = world_mats(src_bones)
    dst_w = world_mats(dst_bones)
    src_idx = {n: i for i, (n, *_r) in enumerate(src_bones)}
    corr = {}
    for n, di in dst_names.items():
        si = src_idx.get(n)
        corr[di] = dst_w[di] @ np.linalg.inv(src_w[si]) if si is not None else np.eye(4)
    out = []
    for mat, verts, idx, name in meshes:
        nv = []
        for p, nrm, uv, bs, ws in verts:
            m = np.zeros((4, 4))
            tot = 0.0
            for b, w in zip(bs, ws):
                if w:
                    m += corr.get(b, np.eye(4)) * (w / 255.0)
                    tot += w / 255.0
            if tot <= 0:
                m = np.eye(4)
            else:
                m /= tot
            pp = m @ np.array([p[0], p[1], p[2], 1.0])
            nn = m[:3, :3] @ np.array(nrm)
            nn /= max(np.linalg.norm(nn), 1e-9)
            nv.append((tuple(pp[:3]), tuple(nn), uv, bs, ws))
        out.append((mat, nv, idx, name))
    return out


# ---------------------------------------------------------------- sequences

def bake_sequences(seqs, bones):
    """seqs: [(qc dict, [cast paths in animation order])] -> (anims, seq records, events)."""
    anims, recs, events = [], [], {}
    zero = None
    for q, casts in seqs:
        names = []
        for k, aname in enumerate(q["anims"] or ["zeroanim"]):
            key = f"{q['name']}#{k}"
            smp = bw.sample_anim(casts[k], bones) if k < len(casts) and casts[k] and os.path.exists(casts[k]) else None
            if smp is None:
                # `noanimation` / missing: a one-frame no-op (zero delta, or the reference pose)
                if zero is None:
                    zero = [((0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0)) for _ in bones]
                delta = bool(q["flags"] & 2)
                data = zero if delta else [(lp, lr) for _n, _p, lp, lr in bones]
                smp = (q["fps"], 1, delta, data)
            anims.append((key,) + smp)
            names.append(key)
        recs.append((q, names))
        events[q["name"]] = q["events"]
    return anims, recs, events


def reference_pose(bones, ref_cast=None, anims=None, recs=None):
    """The pose every frame starts from (the model's default bone pose, InitPose): its `ref` sequence when exported,
    else the skin bind with the root bones taken from the idle's first frame (they carry the anim frame's root
    offset + turn, jx_c_delta 63 u down and turned 90 degrees, which the bind lacks)."""
    if ref_cast and os.path.exists(ref_cast):
        smp = bw.sample_anim(ref_cast, bones)
        if smp and not smp[2]:
            return [tuple(x) for x in smp[3][: len(bones)]]
    ref = [(lp, lr) for _n, _p, lp, lr in bones]
    if anims and recs:
        by = {a[0]: a for a in anims}
        for q, names in recs:
            if q["activity"] == "ACT_VM_IDLE" and not q["flags"] & 2 and names and names[0] in by:
                data = by[names[0]][4]
                for i, (_n, p, _lp, _lr) in enumerate(bones):
                    if p < 0:
                        ref[i] = data[i]
                break
    return ref


def write_view(path, bones, meshes, anims, recs, events, ref=None):
    bone_index = {n: i for i, (n, *_r) in enumerate(bones)}
    with open(path, "wb") as o:
        o.write(b"R5VM" + struct.pack("<I", 1))
        o.write(struct.pack("<I", len(bones)))
        for n, p, lp, lr in bones:
            bw.wstr(o, n)
            o.write(struct.pack("<i3f4f", p, *lp, *lr))
        o.write(struct.pack("<I", len(meshes)))
        for mat, verts, idx, _name in meshes:
            bw.wstr(o, mat or "")
            o.write(struct.pack("<II", len(verts), len(idx)))
            for p, nrm, uv, bs, ws in verts:
                o.write(struct.pack("<3f3f2f4B4B", *p, *nrm, *uv, *bs, *ws))
            o.write(struct.pack("<%dI" % len(idx), *idx))
        o.write(struct.pack("<I", len(anims)))
        for name, fps, frames, additive, data in anims:
            bw.wstr(o, name)
            o.write(struct.pack("<fIB", fps, frames, int(additive)))
            for p, q in data:
                o.write(struct.pack("<3f4f", *p, *q))
        o.write(b"EVTS" + struct.pack("<I", len(events)))
        for name, evs in events.items():
            bw.wstr(o, name)
            o.write(struct.pack("<I", len(evs)))
            for t, ev, opt in evs:
                o.write(struct.pack("<f", t))
                bw.wstr(o, ev)
                bw.wstr(o, opt)
        o.write(b"MNMS" + struct.pack("<I", len(meshes)))
        for _m, _v, _i, name in meshes:
            bw.wstr(o, name)
        if ref:
            o.write(b"REFP" + struct.pack("<I", len(ref)))
            for p, q in ref:
                o.write(struct.pack("<3f4f", *p, *q))
        o.write(b"SEQS" + struct.pack("<I", len(recs)))
        for q, names in recs:
            bw.wstr(o, q["name"])
            bw.wstr(o, q["activity"])
            o.write(struct.pack("<fB", q["weight"], len(q["mods"])))
            for x in q["mods"]:
                bw.wstr(o, x)
            o.write(struct.pack("<ff", q["fadein"], q["fadeout"]))
            bw.wstr(o, q["entry"])
            bw.wstr(o, q["exit"])
            o.write(struct.pack("<Bf", q["flags"], q["fps"]))
            o.write(struct.pack("<B", len(names)))
            for x in names:
                bw.wstr(o, x)
            o.write(struct.pack("<BB", max(q["blendwidth"], 1), len(q["blends"])))
            for prm, lo, hi in q["blends"]:
                bw.wstr(o, prm)
                o.write(struct.pack("<ff", lo, hi))
            o.write(struct.pack("<B", len(q["layers"])))
            for x in q["layers"]:
                bw.wstr(o, x)
            # bone weights: an empty $weightlist = every bone 1; else listed bones, the rest 0
            if q["weights"]:
                o.write(struct.pack("<I", len(bones)))
                w = [0.0] * len(bones)
                for b, v in q["weights"].items():
                    if b in bone_index:
                        w[bone_index[b]] = v
                o.write(struct.pack("<%df" % len(bones), *w))
            else:
                o.write(struct.pack("<I", 0))
