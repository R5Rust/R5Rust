"""
build_sounds.py - an item's sounds from retail Apex's Miles bank (HANDOFF.md "Gun feel").

  python tools/build_sounds.py <item dir with weapon.json>      e.g. D:\\R5Rust\\assets-retail\\weapons\\mp_weapon_r97

Which events: the item's weapon settings (fire / ADS / low ammo / dry fire sound keys) plus every sound its sequences
play (weapon.json "anim_events": AE_CL_PLAYSOUND, and AE_CL_PLAYSOUND_FOR_ARCHETYPE as the Human_ archetype).
How they play: tools/miles_dump's decode of general.mbnk (events_subset.json, cut by miles_dump/subset.py): every
play action's tracks become layers with their pick list, volume (dB, constant / random range / controller curve),
pitch (semitones) and delay; runEvents are followed. The event-level gain (miles_dump's unk_10, +17 dB on the fire
loop, -9 dB on ADS foley) is applied relative to GAIN_REF.

Output: <item>/sounds/sounds.json + the sources as 16-bit stereo WAVs (5.1 folded down), exported with RSX from the
install's general.mbnk (read-only; Apex closed).
"""
import json, os, re, struct, subprocess, sys, wave

import numpy as np

# R5_SOUND_SUBSET: another miles_dump/subset.py cut (the player bank uses events_zipline.json)
SUBSET = os.environ.get("R5_SOUND_SUBSET", r"D:\R5Rust\_work_retail\audio\events_subset.json")
RSX = r"D:\R5Rust\tools\rsx-2.4.1\bin\rsx.exe"
BANK = r"C:\Program Files\EA Games\Apex\audio\ship\general.mbnk"
EXPORT = r"D:\R5Rust\_work_retail\audio\out"
# event gain (dB) that maps to 0 dB in the game: the loudest 1p event (the fire loop, +17) lands at +3
GAIN_REF = 14.0

SOUND_KEYS = re.compile(r"(fire_sound|looping_fire_sound|^sound_|low_ammo_sound_name)", re.I)


def rng(v):
    """A Miles value (constant, {graph: random}, {graph: controller curve}) -> ([lo, hi], curve or None)."""
    if isinstance(v, (int, float)):
        return [float(v), float(v)], None
    g = v.get("graph", v) if isinstance(v, dict) else None
    if g and "random" in g:
        return [float(g["random"][0]), float(g["random"][1])], None
    if g and g.get("points"):
        pts = [[float(p["x"]), float(p["start"])] for p in g["points"]]
        return [0.0, 0.0], {"ctrl": g.get("controller") or "", "points": pts}
    return [0.0, 0.0], None


def picks(sel, out):
    """selector -> [(source, weight)] (a list picks one child by weight; nested lists flatten)."""
    if not isinstance(sel, dict):
        return
    if "source" in sel:
        out.append((sel["source"], float(sel.get("weight") or 100)))
    for k in sel.get("children", []):
        picks(k, out)


def layers_of(name, ev_by_lower, events, depth=0):
    e = events.get(ev_by_lower.get(name.lower(), ""))
    if e is None or depth > 4:
        return None
    out = []
    for a in e.get("actions", []):
        if a.get("type") == "runEvents":
            for sub in a.get("events", []):
                out += layers_of(sub.rsplit("/", 1)[-1], ev_by_lower, events, depth + 1) or []
            continue
        if a.get("type") != "play":
            continue
        gain = float(a.get("unk_10_asDb", 0.0)) - GAIN_REF
        avol, actrl = rng(a.get("volume", 0.0))
        apitch, _ = rng(a.get("pitch", 0.0))
        states = a.get("states") or []
        if not states:
            continue
        # several states = a switch on some game state; the first is the default
        for t in states[0].get("tracks", []):
            files = []
            picks(t.get("selector"), files)
            if not files:
                continue
            vol, pitch, delay, ctrl = list(avol), list(apitch), [0.0, 0.0], actrl
            for p in t.get("params", []):
                val = p.get("graph") and {"graph": p["graph"]} or p.get("value", 0.0)
                r, c = rng(val)
                if p["name"] == "volume_dB":
                    vol = [vol[0] + r[0], vol[1] + r[1]]
                    ctrl = c or ctrl
                elif p["name"] == "pitch_st":
                    pitch = [pitch[0] + r[0], pitch[1] + r[1]]
                elif p["name"] == "delay_ms":
                    delay = [r[0] / 1000.0, r[1] / 1000.0]
            out.append({"files": [f for f, _w in files], "weights": [w for _f, w in files], "vol_db": [vol[0] + gain, vol[1] + gain],
                        "pitch_st": pitch, "delay": delay, "ctrl": ctrl})
    return out


def export_sources(names, dst):
    """RSX asrc export of exactly these sources (the filter is a substring match: names are long enough)."""
    os.makedirs(EXPORT, exist_ok=True)
    have = {os.path.splitext(f)[0].lower(): os.path.join(dp, f) for dp, _d, fs in os.walk(EXPORT) for f in fs if f.endswith(".wav")}
    need = [n for n in names if n.lower() not in have]
    for i in range(0, len(need), 40):
        chunk = need[i:i + 40]
        subprocess.run([RSX, "-nogui", "-export", "--exporttypes", "asrc", "--exportfilter", ",".join(chunk), "--exportdir", EXPORT, BANK],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=900)
    have = {os.path.splitext(f)[0].lower(): os.path.join(dp, f) for dp, _d, fs in os.walk(EXPORT) for f in fs if f.endswith(".wav")}
    os.makedirs(dst, exist_ok=True)
    done = {}
    for n in names:
        src = have.get(n.lower())
        if not src:
            print("  missing source", n)
            continue
        out = os.path.join(dst, n + ".wav")
        if not os.path.exists(out):
            to_stereo16(src, out)
        done[n] = n + ".wav"
    return done


def to_stereo16(src, out):
    b = open(src, "rb").read()
    o, fmt, data = 12, None, None
    while o + 8 <= len(b):
        cid, n = b[o:o + 4], struct.unpack_from("<I", b, o + 4)[0]
        if cid == b"fmt ":
            fmt = b[o + 8:o + 8 + n]
        elif cid == b"data":
            data = b[o + 8:o + 8 + n]
        o += 8 + n + (n & 1)
    tag, ch, rate = struct.unpack_from("<HHI", fmt, 0)
    bits = struct.unpack_from("<H", fmt, 14)[0]
    x = np.frombuffer(data, dtype=np.float32 if bits == 32 else np.int16).astype(np.float32)
    if bits == 16:
        x /= 32768.0
    x = x[: len(x) // ch * ch].reshape(-1, ch)
    if ch == 1:
        st = np.stack([x[:, 0], x[:, 0]], 1)
    elif ch == 2:
        st = x
    elif ch == 6:  # FL FR FC LFE SL SR
        c = x[:, 2] * 0.707
        st = np.stack([x[:, 0] + c + x[:, 4] * 0.707 + x[:, 3] * 0.5, x[:, 1] + c + x[:, 5] * 0.707 + x[:, 3] * 0.5], 1)
    else:
        st = x[:, :2]
    pcm = (np.clip(st, -1.0, 1.0) * 32767.0).astype("<i2")
    with wave.open(out, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm.tobytes())


def main():
    item = sys.argv[1]
    wj = json.load(open(os.path.join(item, "weapon.json")))
    names = set()
    for k, v in wj.get("settings", {}).items():
        if SOUND_KEYS.search(k) and v and isinstance(v, str) and not v.isdigit():
            names.add(v)
    for evs in wj.get("anim_events", {}).values():
        for _t, kind, opt in evs:
            if kind in ("AE_CL_PLAYSOUND", "AE_SV_PLAYSOUND", "AE_CL_PLAYSOUND_FOR_TYPE") and opt:
                names.add(opt)
            elif kind == "AE_CL_PLAYSOUND_FOR_ARCHETYPE" and opt:
                names.add("Human_" + opt)
    d = json.load(open(SUBSET, encoding="utf-8"))
    events, sources = d["events"], d["sources"]
    ev_by_lower = {k.lower(): k for k in events}
    src_by_lower = {k.lower(): k for k in sources}
    out_events, used = {}, set()
    for n in sorted(names):
        # _FOR_TYPE events are per legend (Wraith_Mvmt_Melee_UpperCut_1P, wraith_effort_melee_1p): the arms are Wraith's
        ls = layers_of(n, ev_by_lower, events) or layers_of("Wraith_" + n, ev_by_lower, events)
        if ls is None:
            print("  no Miles event", n)
            continue
        out_events[n.lower()] = ls
        for l in ls:
            used.update(l["files"])
    dst = os.path.join(item, "sounds")
    files = export_sources(sorted(used), dst)
    src_info = {}
    for n, f in files.items():
        s = sources.get(src_by_lower.get(n.lower(), ""), {})
        lp = [s.get("loopStart", 0), s.get("loopEnd", s.get("sampleCount", 0))]
        src_info[f] = {"loop": lp, "rate": s.get("sampleRate")}
    for ls in out_events.values():
        for l in ls:
            keep = [(files[f], w) for f, w in zip(l["files"], l["weights"]) if f in files]
            l["files"] = [f for f, _w in keep]
            l["weights"] = [w for _f, w in keep]
    json.dump({"events": out_events, "sources": src_info}, open(os.path.join(dst, "sounds.json"), "w"), indent=1)
    mb = sum(os.path.getsize(os.path.join(dst, f)) for f in os.listdir(dst)) / 1e6
    print(f"sounds: {len(out_events)} events, {len(files)} sources ({mb:.1f} MB) -> {dst}")


if __name__ == "__main__":
    main()
