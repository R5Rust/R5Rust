"""miles_dump: dump Apex Legends' Miles bank (general.mbnk, v49/v50) event table + source markers to JSON.

Read-only: opens the .mbnk and the .mstr stream files (for channel counts) as plain files.
Field layout and names mirror RSX (src/game/audio/{miles.h,source.h,event.h,event.cpp,
event_action.cpp,actions/action_play.cpp}). Event action blobs are RAD LZB compressed (lzb.py).

Usage:
  python miles_dump.py [--bank PATH] [--out PATH] [--hex-all] [--no-channels] [--print NAME ...]
"""
import argparse
import json
import math
import os
import re
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import lzb  # noqa: E402

DEFAULT_BANK = r"C:\Program Files\EA Games\Apex\audio\ship\general.mbnk"
DEFAULT_OUT = r"D:\R5Rust\_work_retail\audio\events.json"
LANGS = ["english", "french", "german", "spanish", "italian",
         "japanese", "polish", "russian", "mandarin", "korean"]
DEFAULT_SLIM_OUT = r"D:\R5Rust\_work_retail\audio\events_r97_r99.json"
HEX_RE = re.compile(r"r97|r99", re.I)
# extra events kept in the slim file (case-insensitive)
SLIM_EXTRA_EVENTS = ["assault_rifle_dryfire"]

ACTION_NAMES = {0: "play", 8: "runEvents", 9: "setControllerValue", 11: "referencedControllers"}

ACT_GRAPHFLAG_VOLUME = 1 << 0
ACT_GRAPHFLAG_PITCH = 1 << 2
ACT_GRAPHFLAG_4C = 1 << 9
ACT_GRAPHFLAG_50 = 1 << 10


def u8(b, o): return b[o]
def u16(b, o): return struct.unpack_from("<H", b, o)[0]
def s16(b, o): return struct.unpack_from("<h", b, o)[0]
def u32(b, o): return struct.unpack_from("<I", b, o)[0]
def s32(b, o): return struct.unpack_from("<i", b, o)[0]
def u64(b, o): return struct.unpack_from("<Q", b, o)[0]
def f32(b, o): return round(struct.unpack_from("<f", b, o)[0], 6)


class Bank:
    def __init__(self, path):
        self.path = path
        with open(path, "rb") as f:
            self.b = f.read()
        b = self.b
        if b[:4] != b"KNBC":
            raise SystemExit("not a Miles bank (magic != CBNK)")
        self.version = u32(b, 4)
        if self.version not in (49, 50):
            raise SystemExit(f"unsupported bank version {self.version} (expected 49/50)")
        # MilesBankHeader_v49_t
        self.buildTag = u32(b, 0x10)
        self.bankHash = u32(b, 0x14)
        self.eventCount = u32(b, 0x1C)
        self.eventDataOffset = u32(b, 0x2C)
        self.sourcesOffset = u32(b, 0x34)
        self.sourceCount = u32(b, 0x38)            # unlocalised
        self.localisedSourceCount = u32(b, 0x3C)   # per language
        self.markersOffset = u64(b, 0x58)
        self.eventNamesOffset = u64(b, 0x68)
        self.stringsOffset = u64(b, 0x70)
        self.graphDataOffset = u64(b, 0x78)
        self.totalSources = self.sourceCount + self.localisedSourceCount * len(LANGS)
        end = self.sourcesOffset + self.totalSources * 80
        if end != u32(b, 8):  # header fileSize (the file has a few trailing bytes past it)
            print(f"warning: source table ends at {end}, header fileSize is {u32(b, 8)}", file=sys.stderr)
        self._srcname_cache = {}

    def string(self, off):
        if off == 0xFFFFFFFF:
            return None
        o = self.stringsOffset + off
        e = self.b.index(b"\0", o)
        return self.b[o:e].decode("utf-8", "replace")

    # ---- sources (MilesSource_v49_t, 80 bytes) ----
    def source_raw(self, idx):
        o = self.sourcesOffset + idx * 80
        return self.b[o:o + 80]

    def source_name(self, idx):
        if idx in self._srcname_cache:
            return self._srcname_cache[idx]
        if not (0 <= idx < self.totalSources):
            return f"(bad source index {idx})"
        n = self.string(u64(self.source_raw(idx), 0))
        self._srcname_cache[idx] = n
        return n

    def source(self, idx):
        e = self.source_raw(idx)
        markerCount = e[21]
        markerOffset = u32(e, 64)
        markers = []
        for j in range(markerCount):
            mo = self.markersOffset + (markerOffset // 16 + j) * 16
            markers.append({"name": self.string(u32(self.b, mo)), "frame": u32(self.b, mo + 4)})
        lang = u16(e, 8)
        return {
            "index": idx,
            "name": self.string(u64(e, 0)),
            "language": None if lang == 0xFFFF else LANGS[lang] if lang < len(LANGS) else lang,
            "patchIdx": u16(e, 10),
            "sampleRate": u16(e, 16),
            "bitRate": u16(e, 18),
            "sampleCount": u64(e, 40),
            "bpm": u16(e, 30),
            "streamHeaderSize": u32(e, 36),
            "streamHeaderOffset": u64(e, 48),
            "streamDataOffset": u64(e, 56),
            "markers": markers,
            # RSX's gap8 at +0x48. Inferred: loop region in frames. Default is 0..sampleCount
            # (whole file); "_LP" ambiences end early (tail after the loop end).
            "loopStart": u32(e, 72),
            "loopEnd": u32(e, 76),
            # bytes RSX doesn't name: +0x14 'unk', +0x16..+0x1E gap (float at +0x18 is usually 1.0)
            "unk_14": e[20],
            "unk_16": e[22],
            "unk_float_18": f32(e, 24),
            "unk_20": u16(e, 32),
            "unk_22": u16(e, 34),
        }

    # ---- events ----
    def event_names(self):
        for i in range(self.eventCount):
            no, do = struct.unpack_from("<II", self.b, self.eventNamesOffset + i * 8)
            yield self.string(no), do

    def event_blob(self, dataOffset):
        o = self.eventDataOffset + dataOffset
        decomp, comp = u16(self.b, o), u16(self.b, o + 2)
        return lzb.decode(self.b[o + 4:o + 4 + comp], decomp), decomp, comp

    # ---- graphs (MilesValueGraph_s) ----
    def graph(self, off):
        o = self.graphDataOffset + off
        b = self.b
        n = b[o]
        ctrl = u32(b, o + 4)
        xs = [f32(b, o + 8 + 4 * i) for i in range(n)]
        so = o + 8 + 4 * n
        segs = []
        for i in range(n):
            s = so + 20 * i
            segs.append({"x": xs[i], "start": f32(b, s), "end": f32(b, s + 4),
                         "paramA": f32(b, s + 8), "paramB": f32(b, s + 12), "type": b[s + 16]})
        g = {"graphOffset": off, "controller": self.string(ctrl), "unk_1": b[o + 1],
             "unk_2": u16(b, o + 2), "points": segs}
        if n == 0:
            # Not handled by RSX. Observed: unk_2 == 1, no controller, followed by two floats
            # = a uniform random range [min, max] (e.g. volume -4..-3 dB, pitch 3..5 st).
            g["random"] = [f32(b, o + 8), f32(b, o + 12)]
        return g


# ---------------------------------------------------------------- actions

def graph_value(bank, raw, off, is_graph):
    if is_graph:
        return {"graph": bank.graph(u32(raw, off))}
    return f32(raw, off)


def parse_selector(bank, raw, sel_off, base_off, seen=()):
    """ParsedSourceSelector::Construct. sel_off/base_off are byte offsets into the action.
    Child offsets are dwords relative to the root selector (RSX passes the root as 'base')."""
    if sel_off in seen or len(seen) > 16 or not (0 <= sel_off and sel_off + 8 <= len(raw)):
        return {"error": f"selector at +0x{sel_off:x} out of range or cyclic"}
    b0 = raw[sel_off]
    typ = b0 & 7
    out = {"weight": raw[sel_off + 1]}
    if typ == 0:
        idx = s32(raw, sel_off + 4)
        out["source"] = bank.source_name(idx) if idx != -1 else "(no source)"
        out["sourceIndex"] = idx
        if idx != -1:
            out["_used"] = idx
    else:
        out["list"] = typ  # RSX: "List (type N)"; picks a child by weight
        n = u16(raw, sel_off + 2)
        out["unk_0"] = (b0 >> 3) & 0xF
        out["unk_4"] = s32(raw, sel_off + 4)
        out["unk_8"] = s32(raw, sel_off + 8)
        kids = []
        for i in range(n):
            co = u16(raw, sel_off + 0xC + 2 * i)
            kids.append(parse_selector(bank, raw, base_off + 4 * co, base_off, seen + (sel_off,)))
        out["children"] = kids
    return out


# Track entry (RSX's struct_v1, which RSX treats as a fixed 36 bytes; on disk it's variable-length):
#   +0  s32 selectorOffset (dwords, relative to sourceSelectorOffset; -1 = none)
#   +4  u32 unk_4
#   +8  f32 unk_8 (almost always FLT_MAX)
#   +12 u8  paramCount, u8 paramId[3]       (paramId: low nibble = id, 0x10 = value is a graph offset)
#   [+16 u32 more paramIds if paramCount > 3, one per byte]
#   then paramCount dwords: f32 constant, or graph offset when id & 0x10
# Param ids (inferred from value ranges across the whole bank, not from RSX):
PARAM_NAMES = {0: "volume_dB", 1: "pitch_st", 2: "delay_ms", 3: "param3_dB"}


def parse_track(bank, raw, o, sel_base):
    n = raw[o + 12]
    ids = list(raw[o + 13:o + 16])
    vo = o + 16
    if n > 3:
        extra = u32(raw, vo); vo += 4
        ids += [(extra >> (8 * i)) & 0xFF for i in range(n - 3)]
    params = []
    for i in range(n):
        pid = ids[i]
        p = {"id": pid & 0xF, "name": PARAM_NAMES.get(pid & 0xF, f"param{pid & 0xF}")}
        if pid & 0x10:
            p["graph"] = bank.graph(u32(raw, vo + 4 * i))
        else:
            p["value"] = f32(raw, vo + 4 * i)
        if pid & 0xE0:
            p["idFlags"] = pid & 0xE0
        params.append(p)
    so = s32(raw, o)
    t = {"selectorOffset": so, "unk_4": u32(raw, o + 4), "unk_8": f32(raw, o + 8), "params": params}
    if so != -1:
        root = 4 * (sel_base + so)
        t["selector"] = parse_selector(bank, raw, root, root)
    return t, vo + 4 * n


def parse_states(bank, raw):
    """EventAction_0_s::GetSourceStates, mirrored (same stop conditions), but walking the
    variable-length track entries instead of RSX's fixed 36-byte stride."""
    count = raw[1]
    st_off = u16(raw, 0x76) * 4
    v78 = u16(raw, 0x78)
    sel_base = u16(raw, 0x7A)
    states = []
    for _ in range(count):
        if st_off + 0x50 > len(raw):
            states.append({"stop": "state out of range"}); break
        wordE = u16(raw, st_off + 0xE)
        word12 = u16(raw, st_off + 0x12)
        n4 = raw[st_off + 4]
        state = {"name": None, "trackCount": n4, "unsigned___int86": raw[st_off + 6],
                 "unsigned___int88": raw[st_off + 8], "unsigned___int89": raw[st_off + 9],
                 "wordE": wordE, "word12": word12,
                 "dword40": s32(raw, st_off + 0x40), "dword44": s32(raw, st_off + 0x44),
                 "tracks": []}
        if wordE & 1:
            states.append(dict(state, stop="wordE & 1")); break
        if (word12 & 0x20) or not state["dword44"]:
            states.append(dict(state, stop="word12 & 0x20 or dword44 == 0")); break
        if n4:
            state["name"] = bank.string(u32(raw, st_off))
            if word12 & 0x10:
                states.append(dict(state, stop="word12 & 0x10")); break
            if not state["dword40"]:
                states.append(dict(state, stop="dword40 == 0")); break
            o = 4 * (v78 + raw[st_off + 6])
            for _j in range(n4):
                t, o = parse_track(bank, raw, o, sel_base)
                state["tracks"].append(t)
        states.append(state)
        base = 104 if (wordE & 0x100) else 80
        st_off += base + 36 * (raw[st_off + 8] + raw[st_off + 9])
    return states


def num(raw, off):
    """f32, unless the bits are clearly a small integer (denormal float)."""
    v = u32(raw, off)
    if v and v < 0x00800000:
        return v
    return f32(raw, off)


def lin_to_db(v):
    return round(20 * math.log10(v), 3) if v > 0 else -96.0


def parse_play(bank, raw):
    gf = u32(raw, 0x80)
    g10, r14 = f32(raw, 0x10), f32(raw, 0x14)
    a = {
        "states": parse_states(bank, raw),
        "graphFlags": gf,
        "pitch": graph_value(bank, raw, 0x3C, gf & ACT_GRAPHFLAG_PITCH),    # semitones (RSX plots "Pitch (st)")
        "volume": graph_value(bank, raw, 0x40, gf & ACT_GRAPHFLAG_VOLUME),  # dB (RSX plots "Volume (dB)")
        "unk_44": f32(raw, 0x44),
        "unk_48": f32(raw, 0x48),
        "unkGraphVal_4C": graph_value(bank, raw, 0x4C, gf & ACT_GRAPHFLAG_4C),
        "unkGraphVal_50": {"graph": bank.graph(u32(raw, 0x50))} if gf & ACT_GRAPHFLAG_50 else num(raw, 0x50),
        # Inside RSX's unk_0[28]; same value on every play action of an event. Inferred:
        # +0x10 linear gain (3.1623 = +10 dB), +0x14 pitch ratio (1.1225 = +2 st).
        "unk_10": g10, "unk_10_asDb": lin_to_db(g10),
        "unk_14": r14, "unk_14_asSemitones": round(12 * math.log2(r14), 3) if r14 > 0 else None,
        "unk_18": f32(raw, 0x18),
        "unk_1C": s32(raw, 0x1C),
        "unkBankOffset_20": u32(raw, 0x20),
        "unkCount_34": raw[0x34],
        "unk_36": u16(raw, 0x36),
        "unk_38": u32(raw, 0x38),
        "unk_58": s32(raw, 0x58),
        "unk_60": s32(raw, 0x60),
        "unk_68": u32(raw, 0x68),
        "unkGraphOffset_6C": s32(raw, 0x6C),
        "dwordOffsets": {k: u16(raw, o) for k, o in (
            ("unk_72", 0x72), ("unk_74", 0x74), ("sourceStates", 0x76), ("unk_78", 0x78),
            ("sourceSelectors", 0x7A), ("unk_7C", 0x7C), ("unk_7E", 0x7E))},
    }
    return a


def parse_action(bank, raw):
    t = raw[0] & 0xF
    a = {"type": ACTION_NAMES.get(t, f"unnamed_{t}"), "actionType": t, "unk_1": raw[1],
         "sizeBytes": len(raw)}
    try:
        if t == 0 and len(raw) >= 0x84:
            a.update(parse_play(bank, raw))
        elif t == 2:
            n = u32(raw, 4)
            a.update({"event": "(none)" if n == 0xFFFFFFFF else bank.string(n), "flags": raw[8],
                      "unk_9": raw[9], "unk_A": s16(raw, 0xA), "unk_C": u32(raw, 0xC),
                      "unk_10": f32(raw, 0x10), "unk_14": u32(raw, 0x14)})
        elif t == 8:
            n = u32(raw, 4)
            a.update({"events": [bank.string(u32(raw, 0xC + 4 * i)) for i in range(n)], "b": u32(raw, 8)})
        elif t == 9:
            a.update({"controller": bank.string(u32(raw, 0x10)), "duration": f32(raw, 4),
                      "delay": f32(raw, 8), "value": f32(raw, 0xC), "unk_14": s32(raw, 0x14)})
        elif t == 11:
            a.update({"controllers": [bank.string(u32(raw, 4 + 4 * i)) for i in range(raw[1])]})
        elif t == 13:
            a.update({"unk_4": raw[4], "controllerCount": raw[5]})
    except Exception as ex:  # keep going; the hex is still there
        a["parseError"] = repr(ex)
    return a


def split_actions(blob):
    out, c = [], 0
    while c < len(blob):
        size = u16(blob, c + 2) * 4
        if size == 0:
            raise ValueError("zero-size action")
        out.append(blob[c:c + size])
        last = (blob[c] >> 4) & 1
        c += size
        if last:
            break
    return out


def collect_sources(node, acc):
    if isinstance(node, dict):
        if isinstance(node.get("source"), str):
            acc.add(node["source"])
        for v in node.values():
            collect_sources(v, acc)
    elif isinstance(node, list):
        for v in node:
            collect_sources(v, acc)


def collect_used(node, acc):
    if isinstance(node, dict):
        if "_used" in node:
            acc.add(node.pop("_used"))
        for v in node.values():
            collect_used(v, acc)
    elif isinstance(node, list):
        for v in node:
            collect_used(v, acc)


# ---------------------------------------------------------------- channels from .mstr

def read_channels(bank, sources):
    """RADA/BinkA stream header lives in <stem>_{stream|lang}[_patch_N].mstr at streamHeaderOffset."""
    d = os.path.dirname(bank.path)
    stem = bank.string(0)
    handles = {}
    missing = set()
    for s in sources:
        lang = s["language"] or "stream"
        fn = f"{stem}_{lang}" + (f"_patch_{s['patchIdx']}" if s["patchIdx"] else "") + ".mstr"
        s["streamFile"] = fn
        if fn not in handles:
            p = os.path.join(d, fn)
            handles[fn] = open(p, "rb") if os.path.exists(p) else None
            if handles[fn] is None:
                missing.add(fn)
        f = handles[fn]
        s["channels"] = None
        if f is None:
            continue
        f.seek(s["streamHeaderOffset"])
        h = f.read(24)
        # tags are multi-char constants ('RADA', 'BCF1') stored little-endian
        if h[:4] == b"ADAR":
            s["codec"] = "rada"
            s["channels"] = h[9]  # RadAFileHeader.channels
            s["streamFrameCount"] = u64(h, 16)
        elif h[:4] == b"1FCB":
            s["codec"] = "binka"
            s["channels"] = h[5]  # BinkFileHeader_t.channelCount
        else:
            s["codec"] = h[:4].hex()
    for f in handles.values():
        if f:
            f.close()
    if missing:
        print("missing stream files:", sorted(missing), file=sys.stderr)


# ---------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bank", default=DEFAULT_BANK)
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--hex-all", action="store_true", help="raw action hex for every event, not just r97/r99")
    ap.add_argument("--no-channels", action="store_true", help="skip reading .mstr headers for channel counts")
    ap.add_argument("--all-languages", action="store_true", help="also emit non-English localised sources")
    ap.add_argument("--slim-out", default=DEFAULT_SLIM_OUT,
                    help="pretty r97/r99 subset ('' to skip)")
    ap.add_argument("--print", nargs="*", default=[], metavar="NAME", help="print these events/sources after dumping")
    args = ap.parse_args()

    if os.path.normcase(os.path.abspath(args.out)).startswith(os.path.normcase(os.path.dirname(os.path.abspath(args.bank)))):
        raise SystemExit("refusing to write next to the bank")

    t0 = time.time()
    bank = Bank(args.bank)
    print(f"bank v{bank.version} buildTag={bank.buildTag} events={bank.eventCount} sources={bank.sourceCount}"
          f"+{bank.localisedSourceCount}x{len(LANGS)} (total {bank.totalSources})")

    events, used = {}, set()
    fails = 0
    dupes = 0
    for name, do in bank.event_names():
        try:
            blob, dsz, csz = bank.event_blob(do)
            actions_raw = split_actions(blob)
        except Exception as ex:
            fails += 1
            events[name] = {"error": repr(ex)}
            continue
        acts = [parse_action(bank, r) for r in actions_raw]
        collect_used(acts, used)
        ev = {"actions": acts, "decompressedSize": dsz, "compressedSize": csz}
        if args.hex_all or HEX_RE.search(name):
            ev["actionsHex"] = [r.hex() for r in actions_raw]
        if name in events:
            dupes += 1
        events[name] = ev
    print(f"events: {len(events)} parsed ({fails} failed, {dupes} duplicate names) in {time.time() - t0:.1f}s")

    # sources: all unlocalised + English (or all languages), plus anything referenced
    sources = {}
    src_list = []
    eng0 = bank.sourceCount
    engN = bank.sourceCount + bank.localisedSourceCount
    for i in range(bank.totalSources):
        if not (i < engN or args.all_languages or i in used):
            continue
        s = bank.source(i)
        src_list.append(s)
    if not args.no_channels:
        t1 = time.time()
        read_channels(bank, src_list)
        print(f"channels read in {time.time() - t1:.1f}s")
    for s in src_list:
        key = s["name"] if s["language"] in (None, "english") else f"{s['name']}@{s['language']}"
        sources[key] = s
    bad_used = [i for i in used if not (0 <= i < bank.totalSources)]
    used_loc = sum(1 for i in used if i >= eng0)
    print(f"sources: {len(sources)} emitted; {len(used)} referenced by events "
          f"({used_loc} localised, {len(bad_used)} out of range: {bad_used[:5]})")
    if bad_used:
        bad_ev = [k for k, v in events.items() if "(bad source index" in json.dumps(v)]
        print(f"  events with bad source indices: {bad_ev[:10]}")

    out = {
        "bank": {"path": args.bank, "version": bank.version, "buildTag": bank.buildTag, "bankHash": bank.bankHash,
                 "eventCount": bank.eventCount, "sourceCount": bank.sourceCount,
                 "localisedSourceCount": bank.localisedSourceCount, "languages": LANGS},
        "notes": {
            "volume": "dB when graphFlags&1 == 0, else a graph over a controller (y in dB)",
            "pitch": "semitones when graphFlags&4 == 0, else a graph (y in semitones)",
            "states/selectors": "selector 'list' = selector type (non-zero = list of children); weight = pick weight",
            "markers.frame": "sample frame within the source",
            "unk_*": "fields RSX reads but doesn't name; see actionsHex for r97/r99 events",
        },
        "sources": sources,
        "events": events,
    }
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(out, f, separators=(",", ":"))
    print(f"wrote {args.out} ({os.path.getsize(args.out) / 1e6:.1f} MB) in {time.time() - t0:.1f}s")

    # slim, pretty file: r97/r99 events + extras, the sources they reference, and r97/r99 sources
    if args.slim_out:
        extra = {n.lower() for n in SLIM_EXTRA_EVENTS}
        slim_ev = {k: v for k, v in events.items() if HEX_RE.search(k) or k.lower() in extra}
        refs = set()
        collect_sources(slim_ev, refs)
        slim_src = {k: v for k, v in sources.items() if HEX_RE.search(k) or k in refs}
        slim = dict(out, events=slim_ev, sources=slim_src)
        with open(args.slim_out, "w", encoding="utf-8") as f:
            json.dump(slim, f, indent=1)
        print(f"wrote {args.slim_out} ({len(slim_ev)} events, {len(slim_src)} sources, "
              f"{os.path.getsize(args.slim_out) / 1e6:.1f} MB)")

    # Miles event/source lookups are case-insensitive in practice (bank names are mixed-case,
    # e.g. "weapon_r97_fire_loop_1p"), so --print matches case-insensitively.
    ev_l = {k.lower(): k for k in events}
    src_l = {k.lower(): k for k in sources}
    for n in args.print:
        if n.lower() in ev_l:
            k = ev_l[n.lower()]
            print(f"--- event {k}")
            print(json.dumps(events[k], indent=1))
        elif n.lower() in src_l:
            k = src_l[n.lower()]
            print(f"--- source {k}")
            print(json.dumps(sources[k], indent=1))
        else:
            print(f"--- {n}: not found")



if __name__ == "__main__":
    main()
