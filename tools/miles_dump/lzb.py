"""RAD LZB "simple" decoder, ported from rad_lzb_simple.c (rr_lzb_simple_decode).

Pure Python. Miles event blobs are tiny (u16 sizes), so speed doesn't matter.
The C version does speculative 4/8-byte over-copies; a forward byte copy gives
the same result for valid streams (the encoder never emits short overlapping
matches outside the explicit low-offset path).
"""

LRL_ESCAPE = 15
MLCONTROL_ESCAPE = 15
MML = 4
MATCHLEN_ESCAPE = MLCONTROL_ESCAPE + 4


def _add_excess(c, cp, val):
    # LZB_AddExcessBW: 1-5 bytes
    b = c[cp]; cp += 1
    if b < 192:
        return val + b, cp
    val += 192 + (b - 192)
    b = c[cp]; cp += 1
    val += b << 6
    if b >= 128:
        b = c[cp]; cp += 1
        val += b << 13
        if b >= 128:
            b = c[cp]; cp += 1
            val += b << 20
            if b >= 128:
                b = c[cp]; cp += 1
                val += b << 27
    return val, cp


def decode(comp: bytes, raw_len: int) -> bytes:
    """Returns raw bytes. Raises ValueError if the stream doesn't consume exactly len(comp)."""
    if len(comp) == raw_len:
        return bytes(comp)
    if len(comp) > raw_len:
        raise ValueError("compLen > rawLen")
    c = comp
    out = bytearray()
    cp = 0
    while True:
        control = c[cp]; cp += 1
        lrl = control & 0xF
        ml_control = control >> 4

        if lrl >= LRL_ESCAPE:
            lrl, cp = _add_excess(c, cp, lrl)
            if len(out) + lrl >= raw_len:
                out += c[cp:cp + lrl]; cp += lrl
                break
        elif lrl > 8:
            if len(out) + lrl >= raw_len:
                if lrl == 9:  # may be a false 9 (forced last LRL)
                    lrl = raw_len - len(out)
                out += c[cp:cp + lrl]; cp += lrl
                break

        out += c[cp:cp + lrl]; cp += lrl

        ml = ml_control + MML
        off = c[cp] | (c[cp + 1] << 8) if cp + 1 < len(c) else c[cp]

        if ml_control < MLCONTROL_ESCAPE:
            cp += 2
            src = len(out) - off
        else:
            excesslow = off & 127
            cp += 1
            if off & 128:
                ml_control = excesslow >> 3
                ml = ml_control + MML
                if ml_control == 0xF:
                    ml, cp = _add_excess(c, cp, ml)
                src = len(out) - (off & 7)
            else:
                myoff = c[cp] | (c[cp + 1] << 8); cp += 2
                ml += excesslow
                if excesslow == 127:
                    ml, cp = _add_excess(c, cp, ml)
                src = len(out) - myoff
        if src < 0:
            raise ValueError("match offset before start")
        for i in range(ml):
            out.append(out[src + i])

    if len(out) != raw_len:
        raise ValueError(f"decoded {len(out)} != {raw_len}")
    if cp != len(comp):
        raise ValueError(f"consumed {cp} != {len(comp)}")
    return bytes(out)
