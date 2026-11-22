# miles_dump

Dumps retail Apex's Miles audio bank (`general.mbnk`, bank version 49/50; S30 retail is v50) to JSON:
every sound event's decoded actions (which sources it plays, volume/pitch, random ranges, delays,
controllers) and every source's sample rate, channels, length, loop region and markers.

Pure Python 3.11, no deps. Reads the bank and the `.mstr` stream headers as plain files (read-only;
Apex doesn't need to be running and must not be touched). Refuses to write next to the bank.

## Run

```powershell
python C:\Code\games\r5rust\tools\miles_dump\miles_dump.py      # ~5 min
#  -> D:\R5Rust\_work_retail\audio\events.json          everything, compact (~550 MB)
#  -> D:\R5Rust\_work_retail\audio\events_r97_r99.json  r97/r99 events + assault_rifle_dryfire, the
#                                                       sources they use + r97/r99 sources (pretty, ~3.5 MB)

# readable one-line-per-layer view (from the slim file by default; names case-insensitive, regex ok):
python C:\Code\games\r5rust\tools\miles_dump\summarize.py weapon_r97_fire_loop_1p "wpn_r97_reload_.*"
```

`miles_dump.py` flags: `--bank`, `--out`, `--slim-out` (`''` to skip), `--hex-all` (raw action hex for
every event; default only r97/r99), `--no-channels`, `--all-languages` (non-English VO sources too),
`--print NAME...` (dump full JSON entries after the run).

## Files

- `miles_dump.py` - bank/source/event/graph parsing, JSON writer.
- `lzb.py` - RAD LZB "simple" decoder, Python port of RSX's `rad_lzb_simple.c` (event blobs are LZB).
- `summarize.py` - compact text view of events/sources from either JSON.

## JSON shape

- `sources[name]`: `sampleRate`, `channels` (RADA stream header), `sampleCount` (frames), `markers`
  `[{name, frame}]` (e.g. `LoopStart`/`LoopEnd`), `loopStart`/`loopEnd` (frames), stream file + offsets.
- `events[name].actions[]`, `type` = RSX names: `play` / `runEvents` / `setControllerValue` /
  `referencedControllers` / `unnamed_N`. Field names follow RSX (event.h, event_action.cpp).
- `play`: one per layer. `volume` (dB) / `pitch` (semitones): a constant, or `{graph}` when
  `graphFlags` bit 0 / bit 2 is set. `states[].tracks[]` = sub-layers that all play:
  - `selector`: a `source`, or a `list` whose `children` are picked by `weight` (random pick).
  - `params`: per-track `volume_dB`, `pitch_st`, `delay_ms`, `param3_dB`, each a `value` or a `graph`.
- Graphs: `points` over a named `controller` (e.g. `AmmoClipPct`, `ManualDistance`), or
  `random: [min, max]` = uniform random range.
- r97/r99 events also carry `actionsHex` (one hex string per decompressed action).

## Status / caveats

- Event names are mixed-case in the bank (`weapon_r97_fire_loop_1p`); look them up case-insensitively.
- Goes beyond RSX in four places, all inferred from data across the whole bank (not from the Miles
  runtime): (1) track entries are variable-length (RSX assumes 36 bytes, which breaks any multi-layer
  state); (2) param ids 0-3 = volume dB / pitch st / delay ms / a dB-like curve; (3) zero-point graphs
  are random ranges; (4) source `+0x48/+0x4C` = loop start/end frames. Play action `+0x10` looks like an
  event-level linear gain and `+0x14` a pitch ratio (dumped as `unk_10`/`unk_14` with dB/st conversions).
- No per-event loop flag found. Loop sources loop the region `loopStart..loopEnd` (the whole file for
  the R97 fire loops); the game starts/stops the loop event. `unk_58` = 900 only on the R97 loop core
  layer, maybe a release/fade time in ms (unconfirmed).
- Action types 2 and 13 are dumped raw (`unnamed_2`, `unnamed_13`). R99 fire events don't have "r99" in
  their names (only VO pings do), so they're in the full dump but not the slim file.
