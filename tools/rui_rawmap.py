"""Rebuilds `<prefix>rawmap.txt` for a RUI exported by the patched RSX (D:\\tmp-apex-kit\\rsx-src) when the export died
after writing header.bin + defaultValues.bin + the snapshot but before the rawmap/TXT dump (seen on ui/crosshair_plus,
2026-11-18). Offsets come from the S21 rpak UI header (apex-content-kit tools/rui-host/RUI_ABI.md section 8):
  0x08 dataStructInitData (= base, defaultValues.bin starts there)  0x10 transformData  0x18 element w h wRcp hRcp
  0x30 argClusters  0x38 arguments  0x40 argumentCount  0x44 dataStructSize  0x46 dataStructInitSize
  0x48 styleDescriptorCount  0x4C renderJobCount  0x4E argClusterCount  0x50 styleDescriptors  0x58 renderJobData
Every region pointer is written relative to base, like the RSX patch writes it.
    python -I tools/rui_rawmap.py D:/R5Rust/rui/crosshair/ui/crosshair_plus.   (check: ... weapon_hud_v2. --check)
"""
import struct, sys


def rawmap(prefix):
    h = open(prefix + "header.bin", "rb").read(0x70)
    q = lambda o: struct.unpack_from("<Q", h, o)[0]
    w = lambda o: struct.unpack_from("<H", h, o)[0]
    base = q(0x08)
    rel = lambda o: q(o) - base if q(o) else None
    ew, eh, er, ehr = struct.unpack_from("<4f", h, 0x18)
    lines = [
        f"base {base:x}",
        f"defaultValues 0 size {w(0x46)}",
        f"transformData {rel(0x10):x}" if rel(0x10) is not None else "transformData 0",
        f"argClusters {rel(0x30):x} count {w(0x4E)}" if rel(0x30) is not None else "argClusters 0 count 0",
        f"args {rel(0x38):x} count {w(0x40)}" if rel(0x38) is not None else "args 0 count 0",
        f"styleDescriptors {rel(0x50):x} count {w(0x48)}",
        f"renderJobs {rel(0x58):x} count {w(0x4C)}",
        f"ruiDataStructSize {w(0x44)}",
        f"element {ew:g} {eh:g} {er:g} {ehr:g}",
    ]
    return lines


if __name__ == "__main__":
    pre = sys.argv[1]
    lines = rawmap(pre)
    print("\n".join(lines))
    if "--check" not in sys.argv:
        open(pre + "rawmap.txt", "w").write("\n".join(lines) + "\n")
