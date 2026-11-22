"""
build_melee.py - bakes a melee-slot item (empty hands, or an heirloom) for the client (crates/client/src/melee.rs).

  python tools/build_melee.py [item ...]        items: emptyhand wraith_kunai octane_knife melee_bat (default: all)

Input: the recon manifest D:\\R5Rust\\_work_retail\\heirlooms\\manifest.json (viewmodel Cast, every sequence of its anim
rig as Cast + QC with activity / weight / modifiers / events, materials with texture slots, weapon settings), and the
pov arms the R-99 uses (Wraith cyber ninja: the only first-person arms in common.rpak).
Output: D:\\R5Rust\\assets-retail\\melee\\<id>\\ view.bin (build_weapon.py's format + an "ACTS" section), weapon.json
(settings, materials, anim_events, activities), textures/.

ACTS: u32 activities; per activity: str name, u32 seqs; per seq: str anim, f32 weight, u8 n, n x str modifier.
The client picks a sequence per activity the way the game does: by activity, weighted, preferring the variants whose
modifiers (crouch, air, sprinting, sliding, airsprint) match the player.
"""
import json, os, shutil, struct, sys

sys.path.insert(0, os.path.dirname(__file__))
import castlib
import build_weapon as bw
import heirloom_mats

MANIFEST = r"D:\R5Rust\_work_retail\heirlooms\manifest.json"
OUT_ROOT = r"D:\R5Rust\assets-retail\melee"
# manifest key -> client id (main.rs HEIRLOOMS) and the weapon settings file
ITEMS = {
    "emptyhand": ("emptyhand", "melee_pilot_emptyhanded"),
    "wraith_kunai": ("wraith_kunai", "melee_wraith_kunai"),
    "octane_knife": ("octane_knife", "melee_octane_knife"),
    "melee_bat": ("bat", "melee_agnostic_rivnation"),
}
# the weapon you HOLD in the melee slot is a separate "_primary" weapon (draw/holster/lower times, move speed, sway,
# bob); the melee_* one above is the swing. Their settings merge, the primary's winning for every non-melee_ key.
PRIMARY = {
    "emptyhand": "mp_weapon_melee_survival",
    "wraith_kunai": "mp_weapon_wraith_kunai_primary",
    "octane_knife": "mp_weapon_octane_knife_primary",
    "melee_bat": "mp_weapon_agnostic_rivnation_primary",
}
# the activities the client plays (one-handed, zipline, door kicks, double jump and the bat's charge come later)
ACTS = ["ACT_VM_DRAW", "ACT_VM_DRAWFIRST", "ACT_VM_DRAW_TO_SPRINT", "ACT_VM_HOLSTER", "ACT_VM_HOLSTER_FROM_SPRINT", "ACT_VM_IDLE",
        "ACT_VM_SPRINT", "ACT_VM_LOWER", "ACT_VM_RAISE", "ACT_VM_RAISE_FROM_SPRINT", "ACT_VM_RAISE_FROM_MELEE", "ACT_VM_MELEE_ATTACK2",
        "ACT_VM_WEAPON_INSPECT", "ACT_VM_JUMP", "ACT_VM_LAND", "ACT_VM_LAND_LOW", "ACT_VM_LAND_MEDIUM", "ACT_VM_LAND_HIGH"]
# modifiers the client can set; a variant needing any other (a legend class, a game mode, a charge level) is left out
KNOWN = {"crouch", "air", "sprinting", "sliding", "airsprint"}
R97 = r"D:\R5Rust\assets-retail\weapons\mp_weapon_r97\weapon.json"


def merged_settings(key, melee):
    """The held item's settings: the primary weapon's (draw/holster/sprint/lower times, move speed, sway) with only
    the swing's melee_* keys from the melee weapon. A key the primary leaves out stays out: the engine then plays
    that sequence at its natural length (the melee file's sprintcycle_time 0.55 squeezed the balisong's 5.5 s sprint
    loop 10x and made its left hand flicker)."""
    prim = primary_settings(key)
    if not prim:
        return melee
    out = dict(prim)
    out.update({k: v for k, v in melee.items() if k.startswith("melee_") or k in ("printname", "shortprintname")})
    return out


def primary_settings(key):
    """The held weapon's top-level settings (PRIMARY), minus the melee_ keys the swing owns."""
    path = os.path.join(bw.W, "out", "weapon", PRIMARY[key] + ".txt")
    if not os.path.exists(path):
        print(f"  {key}: no {path}, held-item settings fall back to the melee weapon's")
        return {}
    st = bw.kv_tree(open(path, encoding="utf-8", errors="replace").read())
    top = next(iter(st.values())) if len(st) == 1 and isinstance(next(iter(st.values())), dict) else st
    return {k: v for k, v in top.items() if isinstance(v, str) and not k.startswith("melee_")}


def main():
    if sys.argv[1:2] == ["--mats"]:
        # materials only: rewrite weapon.json "materials" without re-baking the viewmodel
        man = json.load(open(MANIFEST))
        r97 = json.load(open(R97))
        for key in sys.argv[2:] or list(ITEMS):
            cid = ITEMS[key][0]
            out = os.path.join(OUT_ROOT, cid)
            wj = json.load(open(os.path.join(out, "weapon.json")))
            wj["materials"] = heirloom_mats.item_materials(man[key], out, cid, r97, bw.SLOTS)
            st = bw.kv_tree(open(os.path.join(bw.W, "out", "weapon", ITEMS[key][1] + ".txt"), encoding="utf-8", errors="replace").read())
            top = next(iter(st.values())) if len(st) == 1 and isinstance(next(iter(st.values())), dict) else st
            wj["settings"] = merged_settings(key, {k: v for k, v in top.items() if isinstance(v, str)})
            json.dump(wj, open(os.path.join(out, "weapon.json"), "w"), indent=1)
            print(cid, {k: (v.get("emissive_tint"), v.get("translucent"), sorted(x for x in v if x in heirloom_mats.TEX_KEYS.values())) for k, v in wj["materials"].items() if "wraith_lgnd" not in k})
        return
    man = json.load(open(MANIFEST))
    want = sys.argv[1:] or list(ITEMS)
    r97 = json.load(open(R97))
    for key in want:
        bake(key, man[key], r97)


def bake(key, m, r97):
    cid, settings_name = ITEMS[key]
    out = os.path.join(OUT_ROOT, cid)
    os.makedirs(os.path.join(out, "textures"), exist_ok=True)
    import vmbake
    vm = bw.model_of(m["viewmodel"]["cast_lod0"])
    vm_bones = bw.skeleton(vm)
    rig_bones = bw.skeleton(bw.model_of(m["anim_rig"]["cast"]))
    # the rig's reference pose is where every frame starts; model-only bones (muzzle_flash, knife_base) hang under it
    bones, names = vmbake.merge_skeleton(rig_bones, [vm_bones])
    arms = bw.model_of(bw.ARMS)
    arms_bones = bw.skeleton(arms)
    missing = [n for n, *_r in arms_bones if n not in names]
    if missing:
        print(f"  {cid}: arms bones not in the rig (bound to the root): {missing[:8]}")
    # empty hands carries the data knife (a bodygroup the game keeps off)
    vm_ms = [x for x in bw.meshes(vm, [names[n] for n, *_r in vm_bones], None) if not (x[0] or "").startswith("data_knife")]
    arms_ms = bw.meshes(arms, [names.get(n, 0) for n, *_r in arms_bones], None)
    ms = vmbake.rebind(vm_ms, vm_bones, bones, names) + vmbake.rebind(arms_ms, arms_bones, bones, names)

    # every sequence of the rig (the game picks by activity + modifiers and links nodes through the transitions)
    seqs = []
    for sq in m["sequences"]:
        q = vmbake.parse_qc(sq["qc"]) if sq.get("qc") and os.path.exists(sq["qc"]) else None
        if q:
            seqs.append((q, sq.get("cast") or []))
    anims, recs, events = vmbake.bake_sequences(seqs, bones)
    ref = vmbake.reference_pose(bones, None, anims, recs)
    vmbake.write_view(os.path.join(out, "view.bin"), bones, ms, anims, recs, events, ref)
    acts = {}
    for q, _n in recs:
        if q["activity"]:
            acts.setdefault(q["activity"], []).append([q["name"], q["weight"], q["mods"]])

    # materials: textures named by each material's pixel shader + its constants (heirloom_mats.py)
    mats = heirloom_mats.item_materials(m, out, cid, r97, bw.SLOTS)
    cast_mats = {mat for mat, *_r in ms}
    for cm in cast_mats:
        if cm and cm not in mats:
            print(f"  {cid}: Cast material {cm} has no textures")

    st = bw.kv_tree(open(os.path.join(bw.W, "out", "weapon", settings_name + ".txt"), encoding="utf-8", errors="replace").read())
    top = next(iter(st.values())) if len(st) == 1 and isinstance(next(iter(st.values())), dict) else st
    flat = merged_settings(key, {k: v for k, v in top.items() if isinstance(v, str)})
    json.dump({"settings": flat, "display_name": m.get("name", cid), "materials": mats, "anim_events": events,
               "activities": acts},
              open(os.path.join(out, "weapon.json"), "w"), indent=1)
    tris = sum(len(i) for _m, _v, i, _n in ms) // 3
    mb = os.path.getsize(os.path.join(out, "view.bin")) / 1e6
    print(f"{cid}: {len(bones)} bones, {len(ms)} meshes ({tris} tris), {len(recs)} sequences ({len(anims)} animations) in {len(acts)} activities, {mb:.0f} MB; "
          f"materials {sorted(cast_mats)}")


if __name__ == "__main__":
    main()
