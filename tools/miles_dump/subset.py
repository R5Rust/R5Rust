"""Cuts a working subset out of the full Miles dump (events.json, ~550 MB): every event matching one of the regexes
(case-insensitive), the events they run (runEvents), and every source those reference.

  python subset.py [--json events.json] [--out events_subset.json] REGEX...
"""
import argparse
import json
import re

FULL = r"D:\R5Rust\_work_retail\audio\events.json"
OUT = r"D:\R5Rust\_work_retail\audio\events_subset.json"


def sources_of(sel, out):
    if not isinstance(sel, dict):
        return
    if "source" in sel:
        out.add(sel["source"])
    for k in sel.get("children", []):
        sources_of(k, out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", default=FULL)
    ap.add_argument("--out", default=OUT)
    ap.add_argument("regex", nargs="+")
    a = ap.parse_args()
    print("loading", a.json)
    d = json.load(open(a.json, encoding="utf-8"))
    ev, src = d["events"], d["sources"]
    lower = {k.lower(): k for k in ev}
    rx = [re.compile(r, re.I) for r in a.regex]
    todo = [k for k in ev if any(r.search(k) for r in rx)]
    keep, srcs = {}, set()
    while todo:
        k = todo.pop()
        if k in keep:
            continue
        keep[k] = ev[k]
        for act in ev[k].get("actions", []):
            for st in act.get("states", []):
                for t in st.get("tracks", []):
                    sources_of(t.get("selector"), srcs)
            for e in act.get("events", []) if act.get("type") == "runEvents" else []:
                n = lower.get(e.rsplit("/", 1)[-1].lower())
                if n and n not in keep:
                    todo.append(n)
    out = {"bank": d.get("bank"), "notes": d.get("notes"), "events": keep, "sources": {s: src[s] for s in srcs if s in src}}
    json.dump(out, open(a.out, "w", encoding="utf-8"))
    print(f"{len(keep)} events, {len(out['sources'])} sources -> {a.out}")


if __name__ == "__main__":
    main()
