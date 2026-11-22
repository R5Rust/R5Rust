"""Compact, human-readable view of events/sources from a miles_dump JSON.

  python summarize.py [--json PATH] NAME_OR_REGEX ...

A name matches case-insensitively; anything with regex chars (.*[]|^$) is treated as a regex.
One line per play-action layer/track: source pick list (weights), per-track params, and the
action's volume/pitch/unk_10 gain. Graphs print as controller:(x,y)... and random ranges as rand[a,b].
"""
import argparse
import json
import re

DEFAULT_JSON = r"D:\R5Rust\_work_retail\audio\events_r97_r99.json"


def fmt_graph(g):
    if "random" in g:
        return f"rand[{g['random'][0]:g},{g['random'][1]:g}]"
    pts = " ".join(f"({p['x']:g},{p['start']:g})" for p in g["points"])
    return f"{g['controller'] or '?'}:{pts}"


def fmt_val(v):
    if isinstance(v, dict) and "graph" in v:
        return fmt_graph(v["graph"])
    if isinstance(v, float):
        return f"{v:g}"
    return str(v)


def fmt_sel(s):
    if "children" in s:
        kids = s["children"]
        inner = ", ".join(fmt_sel(k) for k in kids)
        return f"pick1of{len(kids)}[{inner}]" if len(kids) > 1 else inner
    if "source" in s:
        return s["source"] + (f"(w{s['weight']})" if s.get("weight") not in (None, 100) else "")
    return str(s)


def fmt_params(ps):
    out = []
    for p in ps:
        v = fmt_graph(p["graph"]) if "graph" in p else f"{p['value']:g}"
        out.append(f"{p['name']}={v}")
    return " ".join(out)


def show_event(name, ev):
    print(f"EVENT {name}")
    for i, a in enumerate(ev.get("actions", [])):
        t = a["type"]
        if t == "play":
            hdr = (f"  [{i}] play  volume={fmt_val(a['volume'])}dB pitch={fmt_val(a['pitch'])}st "
                   f"unk_10={a['unk_10']:g}({a['unk_10_asDb']:+g}dB) unk_14={a['unk_14']:g}"
                   f"({a['unk_14_asSemitones']:+g}st) unk_44={a['unk_44']:g} unk_48={a['unk_48']:g} "
                   f"unk_58={a['unk_58']} graphFlags={a['graphFlags']}")
            print(hdr)
            for st in a["states"]:
                for t_ in st["tracks"]:
                    sel = fmt_sel(t_["selector"]) if "selector" in t_ else "(none)"
                    ps = fmt_params(t_["params"])
                    print(f"      {st['name']}: {sel}" + (f"  {{{ps}}}" if ps else ""))
                if st.get("stop"):
                    print(f"      (stopped: {st['stop']})")
        elif t == "runEvents":
            print(f"  [{i}] runEvents {a['events']}")
        elif t == "setControllerValue":
            print(f"  [{i}] setControllerValue {a['controller']}={a['value']:g} "
                  f"duration={a['duration']:g} delay={a['delay']:g}")
        elif t == "referencedControllers":
            print(f"  [{i}] referencedControllers {a['controllers']}")
        else:
            rest = {k: v for k, v in a.items() if k not in ("type", "actionType", "sizeBytes")}
            print(f"  [{i}] {t} {json.dumps(rest)}")


def show_source(name, s):
    print(f"SOURCE {name}: {s['sampleRate']} Hz, {s.get('channels')} ch, {s['sampleCount']} frames "
          f"({s['sampleCount'] / s['sampleRate']:.3f}s), loop {s['loopStart']}..{s['loopEnd']}, "
          f"markers {[(m['name'], m['frame']) for m in s['markers']]}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", default=DEFAULT_JSON)
    ap.add_argument("names", nargs="+")
    args = ap.parse_args()
    d = json.load(open(args.json, encoding="utf-8"))
    ev, src = d["events"], d["sources"]
    for n in args.names:
        if re.search(r"[.*\[\]|^$]", n):
            rx = re.compile(n, re.I)
            hits = [k for k in ev if rx.search(k)] + [k for k in src if rx.search(k)]
        else:
            hits = [k for k in ev if k.lower() == n.lower()] + [k for k in src if k.lower() == n.lower()]
        if not hits:
            print(f"NOT FOUND {n}")
        for k in hits:
            show_event(k, ev[k]) if k in ev else show_source(k, src[k])


if __name__ == "__main__":
    main()
