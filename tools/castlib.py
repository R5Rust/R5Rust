"""Minimal Cast reader (github.com/dtzxporter/cast) for RSX exports: models (skeleton, skinned meshes, materials) and
animations (curves). Used by tools/build_weapon.py.

Layout: "cast" magic, version u32, root count u32, flags u32; nodes. Node = id u32, size u32, hash u64, property count
u32, child count u32, properties, children. Property = type u16, name length u16, value count u32, name, values.
"""
import struct

IDS = {
    0x746F6F72: "root", 0x6C646F6D: "model", 0x6873656D: "mesh", 0x6C74616D: "material", 0x6E6F6B73: "skeleton",
    0x656E6F62: "bone", 0x6D696E61: "animation", 0x76727563: "curve", 0x56414E4E: "notetrack", 0x656C6966: "file",
    0x74736E69: "instance", 0x6D617274: "notification",
}
FMT = {0x62: ("B", 1), 0x68: ("H", 2), 0x69: ("I", 4), 0x6C: ("Q", 8), 0x66: ("f", 4), 0x64: ("d", 8),
       0x7632: ("f", 8), 0x7633: ("f", 12), 0x7634: ("f", 16)}


class Node:
    __slots__ = ("id", "kind", "hash", "props", "children")

    def __init__(self, id_, hash_):
        self.id, self.kind, self.hash, self.props, self.children = id_, IDS.get(id_, hex(id_)), hash_, {}, []

    def find(self, kind):
        return [c for c in self.children if c.kind == kind]

    def walk(self, kind):
        out = []
        for c in self.children:
            if c.kind == kind:
                out.append(c)
            out.extend(c.walk(kind))
        return out

    def get(self, name, default=None):
        return self.props.get(name, default)


def _node(b, at):
    start = at
    id_, size, hash_, nprop, nchild = struct.unpack_from("<IIQII", b, at)
    at += 24
    n = Node(id_, hash_)
    for _ in range(nprop):
        ty, nlen, count = struct.unpack_from("<HHI", b, at)
        at += 8
        name = b[at:at + nlen].decode("utf-8", "replace")
        at += nlen
        if ty == 0x73:
            end = b.index(b"\0", at)
            n.props[name] = b[at:end].decode("utf-8", "replace")
            at = end + 1
        else:
            f, sz = FMT[ty]
            per = sz // struct.calcsize(f)
            vals = struct.unpack_from("<%d%s" % (count * per, f), b, at)
            at += count * sz
            n.props[name] = vals
    for _ in range(nchild):
        c, at = _node(b, at)
        n.children.append(c)
    return n, start + size


def read(path):
    b = open(path, "rb").read()
    assert b[:4] == b"cast", path
    _, nroot, _ = struct.unpack_from("<III", b, 4)
    at, roots = 16, []
    for _ in range(nroot):
        r, at = _node(b, at)
        roots.append(r)
    return roots


def scalar(v, default=None):
    if v is None:
        return default
    return v[0] if isinstance(v, tuple) else v
