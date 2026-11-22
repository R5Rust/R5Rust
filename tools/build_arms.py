"""build_arms.py - the first-person proxy: the bare arms on the `pov_mp_core` rig, shown instead of the held item
during traversal (mantle hands on the ledge), like retail's "first_person_proxy" entity.

Input (exported from common.rpak through the export-map mirror, no install reads; see HANDOFF "Mantle hands"):
  D:\\R5Rust\\_work_retail\\arms_core\\rig\\animrig\\weapons\\arms\\pov_mp_core.cast      the rig skeleton (official RSX, arig)
  D:\\R5Rust\\_work_retail\\arms_core\\qc\\animseq\\weapons\\arms\\pov_mp_core\\*.qc       sequences (kit RSX aseq=2)
  D:\\R5Rust\\_work_retail\\arms_core\\cast\\animseq\\weapons\\arms\\pov_mp_core\\<seq>.rseq\\*.cast  their curves
  the arms model the R-99 uses (build_weapon.ARMS) and its materials (the R-99 bake's weapon.json)
Output: D:\\R5Rust\\assets-retail\\arms\\pov_mp_core\\ view.bin (vmbake format) + weapon.json (materials).

    python tools/build_arms.py
"""
import glob
import json
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
import build_weapon as bw
import vmbake

SRC = r"D:\R5Rust\_work_retail\arms_core"
RIG = SRC + r"\rig\animrig\weapons\arms\pov_mp_core.cast"
OUT = r"D:\R5Rust\assets-retail\arms\pov_mp_core"
R97 = r"D:\R5Rust\assets-retail\weapons\mp_weapon_r97\weapon.json"
# the traversal sequences the client plays (everything else in the rig is ability / emote / hack animation)
WANT = ("mantle_",)


def main():
    os.makedirs(OUT, exist_ok=True)
    rig_bones = bw.skeleton(bw.model_of(RIG))
    arms = bw.model_of(bw.ARMS)
    arms_bones = bw.skeleton(arms)
    bones, names = vmbake.merge_skeleton(rig_bones, [arms_bones])
    missing = [n for n, *_r in arms_bones if n not in names]
    if missing:
        print(f"  arms bones not in the rig: {missing[:8]}")
    ms = vmbake.rebind(bw.meshes(arms, [names.get(n, 0) for n, *_r in arms_bones], None), arms_bones, bones, names)
    seqs = []
    for qc in sorted(glob.glob(SRC + r"\qc\animseq\weapons\arms\pov_mp_core\*.qc")):
        stem = os.path.basename(qc)[:-3]
        if not stem.startswith(WANT):
            continue
        q = vmbake.parse_qc(qc)
        casts = sorted(glob.glob(SRC + rf"\cast\animseq\weapons\arms\pov_mp_core\{stem}.rseq\*.cast"))
        if q and casts:
            seqs.append((q, casts))
    anims, recs, events = vmbake.bake_sequences(seqs, bones)
    ref = vmbake.reference_pose(bones, None, anims, recs)
    vmbake.write_view(os.path.join(OUT, "view.bin"), bones, ms, anims, recs, events, ref)
    r97 = json.load(open(R97))
    mats = {k: v for k, v in r97["materials"].items() if "wraith" in k}
    json.dump({"settings": {}, "display_name": "arms", "materials": mats, "anim_events": events}, open(os.path.join(OUT, "weapon.json"), "w"), indent=1)
    print(f"arms proxy: {len(bones)} bones, {len(ms)} meshes, {len(recs)} sequences: {sorted(q['name'] for q, _n in recs)}")


main()
