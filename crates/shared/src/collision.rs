//! Collision over the world triangles: our own BVH (median split, parallel build) plus a
//! capsule-vs-triangle depenetration solver. Deterministic: no threads, floats only, fixed order,
//! so the client's prediction and the server's authoritative sim agree.
//! (Replaces rapier/parry from the original plan: no version coupling, exact control of the maths.)

use glam::Vec3;
use rayon::prelude::*;
use std::time::Instant;

pub const RADIUS: f32 = 16.0;
/// Capsule axis runs from feet+RADIUS to feet+RADIUS+AXIS_LEN (total height 72).
pub const AXIS_LEN: f32 = 40.0;
pub const WALKABLE_Z: f32 = 0.7;
/// Source/Apex DIST_EPSILON: hull traces stop this far short of a surface (along its normal).
pub const DIST_EPSILON: f32 = 1.0 / 32.0;
/// A hull that starts inside the DIST_EPSILON shell of a triangle (a hit or a slide left it there, give or take
/// float rounding) may move closer, down to this gap, without the move counting as a hit. A move clipped parallel
/// to a plane still approaches it by up to ~0.002 units at |x| ~ 10000 (f32 rounding of `end`), and without this
/// slack every such move is blocked at fraction 0: the hull hangs in the air against steep slopes and in creases.
/// The gap stays positive, so the hull still never touches the triangle.
pub const SHELL_SLACK: f32 = DIST_EPSILON * 0.5;
/// sin of the angle (about 3 degrees) under which a move counts as sliding along an embedded triangle's plane
/// rather than going deeper into it (see `trace_hull`).
pub const EMBED_GRAZE: f32 = 0.05;

/// Result of [`Collision::trace_hull`] (the fields of Source's trace_t that movement reads).
#[derive(Clone, Copy, Debug)]
pub struct Trace {
    /// 0..1 of the move completed
    pub fraction: f32,
    pub endpos: Vec3,
    /// hit plane normal (unit, pointing back at the box); zero when nothing was hit
    pub normal: Vec3,
    pub startsolid: bool,
    pub allsolid: bool,
}

impl Trace {
    pub fn hit(&self) -> bool {
        self.fraction < 1.0
    }
}

#[derive(Clone, Copy)]
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    /// internal: index of the right child (left child = self + 1); leaf: first triangle slot
    a: u32,
    /// 0 = internal, otherwise triangle count
    n: u32,
}

pub struct Collision {
    pos: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
    nodes: Vec<Node>,
    pub build_ms: f64,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Contact {
    pub ground: bool,
    pub ceiling: bool,
    pub wall: bool,
}

impl Contact {
    fn or(&mut self, o: Contact) {
        self.ground |= o.ground;
        self.ceiling |= o.ceiling;
        self.wall |= o.wall;
    }
}

#[inline]
fn v3(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

fn union(a: &Node, b: &Node) -> ([f32; 3], [f32; 3]) {
    let mut mn = a.min;
    let mut mx = a.max;
    for i in 0..3 {
        mn[i] = mn[i].min(b.min[i]);
        mx[i] = mx[i].max(b.max[i]);
    }
    (mn, mx)
}

const LEAF: usize = 4;

fn build_rec(ids: &mut [u32], base: u32, tris: &[[u32; 3]], pos: &[[f32; 3]], cent: &[[f32; 3]]) -> Vec<Node> {
    if ids.len() <= LEAF {
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for &t in ids.iter() {
            for &vi in &tris[t as usize] {
                let p = pos[vi as usize];
                for k in 0..3 {
                    mn[k] = mn[k].min(p[k]);
                    mx[k] = mx[k].max(p[k]);
                }
            }
        }
        return vec![Node { min: mn, max: mx, a: base, n: ids.len() as u32 }];
    }
    // longest centroid axis
    let mut cmn = [f32::MAX; 3];
    let mut cmx = [f32::MIN; 3];
    for &t in ids.iter() {
        let c = cent[t as usize];
        for k in 0..3 {
            cmn[k] = cmn[k].min(c[k]);
            cmx[k] = cmx[k].max(c[k]);
        }
    }
    let ext = [cmx[0] - cmn[0], cmx[1] - cmn[1], cmx[2] - cmn[2]];
    let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 } else if ext[1] >= ext[2] { 1 } else { 2 };
    let mid = ids.len() / 2;
    ids.select_nth_unstable_by(mid, |&x, &y| cent[x as usize][axis].partial_cmp(&cent[y as usize][axis]).unwrap_or(std::cmp::Ordering::Equal));
    let (l, r) = ids.split_at_mut(mid);
    let (mut ln, mut rn) = if l.len() + r.len() > 40_000 {
        rayon::join(|| build_rec(l, base, tris, pos, cent), || build_rec(r, base + mid as u32, tris, pos, cent))
    } else {
        (build_rec(l, base, tris, pos, cent), build_rec(r, base + mid as u32, tris, pos, cent))
    };
    let right_idx = 1 + ln.len() as u32;
    for n in ln.iter_mut() {
        if n.n == 0 {
            n.a += 1;
        }
    }
    for n in rn.iter_mut() {
        if n.n == 0 {
            n.a += right_idx;
        }
    }
    let (mn, mx) = union(&ln[0], &rn[0]);
    let mut out = Vec::with_capacity(1 + ln.len() + rn.len());
    out.push(Node { min: mn, max: mx, a: right_idx, n: 0 });
    out.append(&mut ln);
    out.append(&mut rn);
    out
}

impl Collision {
    /// Builds the BVH over `indices` (triples into `positions`). Takes ownership of both.
    pub fn build(positions: Vec<[f32; 3]>, indices: &[u32]) -> Collision {
        let t0 = Instant::now();
        let tris_in: Vec<[u32; 3]> = indices.par_chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        // drop degenerate triangles (zero area): they only cost time
        let tris_in: Vec<[u32; 3]> = tris_in
            .into_par_iter()
            .filter(|t| {
                let a = v3(positions[t[0] as usize]);
                let b = v3(positions[t[1] as usize]);
                let c = v3(positions[t[2] as usize]);
                (b - a).cross(c - a).length_squared() > 1e-8
            })
            .collect();
        let cent: Vec<[f32; 3]> = tris_in
            .par_iter()
            .map(|t| {
                let a = positions[t[0] as usize];
                let b = positions[t[1] as usize];
                let c = positions[t[2] as usize];
                [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0]
            })
            .collect();
        let mut ids: Vec<u32> = (0..tris_in.len() as u32).collect();
        let nodes = if ids.is_empty() { vec![Node { min: [0.0; 3], max: [0.0; 3], a: 0, n: 0 }] } else { build_rec(&mut ids, 0, &tris_in, &positions, &cent) };
        let tris: Vec<[u32; 3]> = ids.par_iter().map(|&i| tris_in[i as usize]).collect();
        Collision { pos: positions, tris, nodes, build_ms: t0.elapsed().as_secs_f64() * 1000.0 }
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn tri(&self, slot: u32) -> [Vec3; 3] {
        let t = self.tris[slot as usize];
        [v3(self.pos[t[0] as usize]), v3(self.pos[t[1] as usize]), v3(self.pos[t[2] as usize])]
    }

    pub fn query_aabb(&self, mn: Vec3, mx: Vec3, out: &mut Vec<[Vec3; 3]>) {
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        stack[0] = 0;
        while sp > 0 {
            sp -= 1;
            let idx = stack[sp];
            let node = &self.nodes[idx as usize];
            if node.min[0] > mx.x || node.max[0] < mn.x || node.min[1] > mx.y || node.max[1] < mn.y || node.min[2] > mx.z || node.max[2] < mn.z {
                continue;
            }
            if node.n == 0 {
                stack[sp] = node.a;
                stack[sp + 1] = idx + 1;
                sp += 2;
            } else {
                for s in node.a..node.a + node.n {
                    out.push(self.tri(s));
                }
            }
        }
    }

    /// Nearest hit distance along a ray (any triangle, double sided).
    pub fn ray(&self, o: Vec3, d: Vec3, max: f32) -> Option<f32> {
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best = max;
        let mut hit = false;
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        while sp > 0 {
            sp -= 1;
            let idx = stack[sp];
            let node = &self.nodes[idx as usize];
            let (mut t0, mut t1) = (0.0f32, best);
            let mut miss = false;
            for k in 0..3 {
                let ta = (node.min[k] - o[k]) * inv[k];
                let tb = (node.max[k] - o[k]) * inv[k];
                let (lo, hi) = if ta < tb { (ta, tb) } else { (tb, ta) };
                t0 = t0.max(lo);
                t1 = t1.min(hi);
                if t0 > t1 {
                    miss = true;
                    break;
                }
            }
            if miss {
                continue;
            }
            if node.n == 0 {
                stack[sp] = node.a;
                stack[sp + 1] = idx + 1;
                sp += 2;
            } else {
                for s in node.a..node.a + node.n {
                    let [a, b, c] = self.tri(s);
                    let e1 = b - a;
                    let e2 = c - a;
                    let p = d.cross(e2);
                    let det = e1.dot(p);
                    if det.abs() < 1e-9 {
                        continue;
                    }
                    let id = 1.0 / det;
                    let tv = o - a;
                    let u = tv.dot(p) * id;
                    if !(0.0..=1.0).contains(&u) {
                        continue;
                    }
                    let q = tv.cross(e1);
                    let v = d.dot(q) * id;
                    if v < 0.0 || u + v > 1.0 {
                        continue;
                    }
                    let t = e2.dot(q) * id;
                    if t > 0.0 && t < best {
                        best = t;
                        hit = true;
                    }
                }
            }
        }
        if hit { Some(best) } else { None }
    }

    /// Pushes the capsule (feet at `pos`) out of all triangles. Removes velocity into contacts.
    pub fn resolve(&self, pos: &mut Vec3, vel: &mut Vec3) -> Contact {
        let mut contact = Contact::default();
        let lo = Vec3::new(pos.x - RADIUS - 6.0, pos.y - RADIUS - 6.0, pos.z - 6.0);
        let hi = Vec3::new(pos.x + RADIUS + 6.0, pos.y + RADIUS + 6.0, pos.z + RADIUS * 2.0 + AXIS_LEN + 6.0);
        let mut cand = Vec::with_capacity(32);
        self.query_aabb(lo, hi, &mut cand);
        for _ in 0..8 {
            let mut moved = false;
            let a = *pos + Vec3::new(0.0, 0.0, RADIUS);
            let b = a + Vec3::new(0.0, 0.0, AXIS_LEN);
            let (mut a, mut b) = (a, b);
            for tri in &cand {
                if let Some(push) = capsule_tri(a, b, RADIUS, tri) {
                    let l = push.length();
                    if l < 1e-5 {
                        continue;
                    }
                    let n = push / l;
                    *pos += push;
                    a += push;
                    b += push;
                    moved = true;
                    if n.z > WALKABLE_Z {
                        contact.ground = true;
                    } else if n.z < -0.5 {
                        contact.ceiling = true;
                    } else {
                        contact.wall = true;
                    }
                    let vn = vel.dot(n);
                    if vn < 0.0 {
                        *vel -= n * vn;
                    }
                }
            }
            if !moved {
                break;
            }
        }
        contact
    }

    /// Moves in sub-steps of at most 8 units so nothing tunnels through thin geometry.
    pub fn move_capsule(&self, pos: &mut Vec3, delta: Vec3, vel: &mut Vec3) -> Contact {
        let len = delta.length();
        let n = ((len / 8.0).ceil() as usize).max(1);
        let step = delta / n as f32;
        let mut c = Contact::default();
        for _ in 0..n {
            *pos += step;
            c.or(self.resolve(pos, vel));
        }
        c
    }

    /// Sweeps the capsule (feet at `pos`) straight down by up to `dist`. Returns the distance travelled
    /// before touching geometry (can be slightly negative = already resting a hair inside) and the contact normal.
    /// Only the bottom sphere can lead a vertical sweep, so that's all that is tested.
    pub fn sweep_down(&self, pos: Vec3, dist: f32) -> Option<(f32, Vec3)> {
        const TOL: f32 = 0.5;
        let c = pos + Vec3::new(0.0, 0.0, RADIUS);
        let r = RADIUS;
        let mut cand = Vec::with_capacity(32);
        self.query_aabb(Vec3::new(c.x - r, c.y - r, c.z - r - dist - 1.0), Vec3::new(c.x + r, c.y + r, c.z + 1.0), &mut cand);
        let mut best: Option<(f32, Vec3)> = None;
        let mut take = |t: f32, n: Vec3| {
            if t >= -TOL && t <= dist && best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, n));
            }
        };
        for [p0, p1, p2] in cand {
            let nn = (p1 - p0).cross(p2 - p0);
            let nl = nn.length();
            if nl < 1e-6 {
                continue;
            }
            let n_wind = nn / nl;
            let n = if n_wind.z < 0.0 { -n_wind } else { n_wind };
            // face
            if n.z > 1e-4 {
                let t = ((c - p0).dot(n) - r) / n.z;
                let hit = c - Vec3::Z * t - n * r;
                if in_tri(hit, p0, p1, p2, n_wind) {
                    take(t, n);
                }
            }
            // edges
            for (e0, e1) in [(p0, p1), (p1, p2), (p2, p0)] {
                let el = (e1 - e0).length();
                if el < 1e-6 {
                    continue;
                }
                let u = (e1 - e0) / el;
                let w0 = c - e0;
                let a = w0 - u * u.dot(w0);
                let b = Vec3::Z - u * u.z;
                let bb = b.dot(b);
                if bb < 1e-8 {
                    continue;
                }
                let ab = a.dot(b);
                let disc = ab * ab - bb * (a.dot(a) - r * r);
                if disc < 0.0 {
                    continue;
                }
                let t = (ab - disc.sqrt()) / bb;
                let pt = c - Vec3::Z * t;
                let s = u.dot(pt - e0);
                if s >= 0.0 && s <= el {
                    let perp = (pt - e0) - u * s;
                    take(t, perp / r);
                }
            }
            // vertices
            for v in [p0, p1, p2] {
                let h2 = r * r - (c.x - v.x).powi(2) - (c.y - v.y).powi(2);
                if h2 >= 0.0 {
                    let t = (c.z - v.z) - h2.sqrt();
                    take(t, (c - Vec3::Z * t - v) / r);
                }
            }
        }
        best
    }

    /// Swept axis-aligned box (Apex's TraceHull): moves the box `mins..maxs` (relative to the origin) from `start`
    /// to `end` against every solid triangle and returns where it stops, Source-style:
    /// - the box stops `DIST_EPSILON` (1/32) short of the surface along the hit normal, so a box resting on a
    ///   floor sits 1/32 above it and sliding along that floor is not a hit;
    /// - a triangle the box already overlaps at `start` sets `startsolid` and does not block, unless the move
    ///   goes deeper into its plane (then it blocks at fraction 0, so an embedded floor can't be fallen through);
    ///   `allsolid` is set when such a triangle blocks the whole move;
    /// - exact separating-axis test (3 box axes, the triangle normal, 9 edge crosses), two-sided.
    ///   A zero-length trace is a pure overlap test.
    pub fn trace_hull(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Trace {
        let half = (maxs - mins) * 0.5;
        let c0 = start + (mins + maxs) * 0.5;
        let d = end - start;
        let mut out = Trace { fraction: 1.0, endpos: end, normal: Vec3::ZERO, startsolid: false, allsolid: false };
        let pad = Vec3::splat(DIST_EPSILON + 0.01);
        let lo = c0.min(c0 + d) - half - pad;
        let hi = c0.max(c0 + d) + half + pad;
        let mut best = 1.0f32;
        let mut best_n = Vec3::ZERO;
        let mut hit = false;
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        while sp > 0 {
            sp -= 1;
            let idx = stack[sp];
            let node = &self.nodes[idx as usize];
            if node.min[0] > hi.x || node.max[0] < lo.x || node.min[1] > hi.y || node.max[1] < lo.y || node.min[2] > hi.z || node.max[2] < lo.z {
                continue;
            }
            if node.n == 0 {
                stack[sp] = node.a;
                stack[sp + 1] = idx + 1;
                sp += 2;
                continue;
            }
            for s in node.a..node.a + node.n {
                let [a, b, c] = self.tri(s);
                // cheap per-triangle AABB reject
                let tmn = a.min(b).min(c);
                let tmx = a.max(b).max(c);
                if tmn.x > hi.x || tmx.x < lo.x || tmn.y > hi.y || tmx.y < lo.y || tmn.z > hi.z || tmx.z < lo.z {
                    continue;
                }
                match sweep_box_tri(a - c0, b - c0, c - c0, half, d) {
                    Sweep::Miss => {}
                    Sweep::Start { blocks } => {
                        out.startsolid = true;
                        if let Some(n) = blocks {
                            // moving deeper into an embedded triangle: no motion at all
                            if !hit || best > 0.0 || n.z > best_n.z {
                                best = 0.0;
                                best_n = n;
                                hit = true;
                                out.allsolid = true;
                            }
                        }
                    }
                    Sweep::Hit(f, n) => {
                        // Ties: prefer the plane the move goes into hardest (the one actually blocking it), then the
                        // most upward normal (flat floors made of many triangles). In a crease (a steep slope against
                        // a wall a fraction of a degree off-axis) both planes hit at fraction 0; reporting only the
                        // slope left TryPlayerMove clipping against it forever and never learning the wall.
                        let tie = f <= best + 1e-6 && f >= best - 1e-6;
                        let (into, best_into) = (d.dot(n), d.dot(best_n));
                        let better = into < best_into - 1e-6 || (into <= best_into + 1e-6 && n.z > best_n.z);
                        if !hit || f < best - 1e-6 || (tie && better) {
                            best = if f < best { f } else { best };
                            best_n = n;
                            hit = true;
                        }
                    }
                }
            }
        }
        if hit {
            out.fraction = best;
            out.normal = best_n;
            out.endpos = start + d * best;
        }
        out
    }

    /// Debug: what each nearby triangle does to a hull trace: (triangle, kind, fraction, normal), kind 0 = overlaps
    /// at the start without blocking, 1 = overlaps and blocks (allsolid), 2 = swept hit.
    pub fn trace_hull_detail(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Vec<([Vec3; 3], u8, f32, Vec3)> {
        let half = (maxs - mins) * 0.5;
        let c0 = start + (mins + maxs) * 0.5;
        let d = end - start;
        let pad = Vec3::splat(DIST_EPSILON + 0.01);
        let mut cand = Vec::new();
        self.query_aabb(c0.min(c0 + d) - half - pad, c0.max(c0 + d) + half + pad, &mut cand);
        let mut out = Vec::new();
        for [a, b, c] in cand {
            match sweep_box_tri(a - c0, b - c0, c - c0, half, d) {
                Sweep::Miss => {}
                Sweep::Start { blocks: None } => out.push(([a, b, c], 0, 0.0, Vec3::ZERO)),
                Sweep::Start { blocks: Some(n) } => out.push(([a, b, c], 1, 0.0, n)),
                Sweep::Hit(f, n) => out.push(([a, b, c], 2, f, n)),
            }
        }
        out
    }

    /// Debug: every triangle the capsule at `pos` overlaps, with its push-out vector.
    pub fn contacts(&self, pos: Vec3) -> Vec<([Vec3; 3], Vec3)> {
        let lo = Vec3::new(pos.x - RADIUS - 6.0, pos.y - RADIUS - 6.0, pos.z - 6.0);
        let hi = Vec3::new(pos.x + RADIUS + 6.0, pos.y + RADIUS + 6.0, pos.z + RADIUS * 2.0 + AXIS_LEN + 6.0);
        let mut cand = Vec::new();
        self.query_aabb(lo, hi, &mut cand);
        let a = pos + Vec3::new(0.0, 0.0, RADIUS);
        let b = a + Vec3::new(0.0, 0.0, AXIS_LEN);
        cand.into_iter().filter_map(|t| capsule_tri(a, b, RADIUS, &t).map(|p| (t, p))).collect()
    }

    /// True if the capsule at `pos` is not overlapping any triangle.
    pub fn is_free(&self, pos: Vec3) -> bool {
        let mut p = pos;
        let mut v = Vec3::ZERO;
        self.resolve(&mut p, &mut v);
        (p - pos).length_squared() < 1e-6
    }
}

fn closest_on_tri(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    // Ericson, Real-Time Collision Detection 5.1.5
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Closest points between segments p1-q1 and p2-q2: (point on first, point on second).
fn seg_seg(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let (s, t);
    if a <= 1e-9 && e <= 1e-9 {
        return (p1, p2);
    }
    if a <= 1e-9 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-9 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 1e-9 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

fn in_tri(x: Vec3, a: Vec3, b: Vec3, c: Vec3, n: Vec3) -> bool {
    let e0 = (b - a).cross(x - a).dot(n);
    let e1 = (c - b).cross(x - b).dot(n);
    let e2 = (a - c).cross(x - c).dot(n);
    e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0
}

/// Push vector moving the capsule (segment a-b, radius r) out of the triangle, if overlapping.
fn capsule_tri(a: Vec3, b: Vec3, r: f32, tri: &[Vec3; 3]) -> Option<Vec3> {
    let [p0, p1, p2] = *tri;
    let nn = (p1 - p0).cross(p2 - p0);
    let nl = nn.length();
    if nl < 1e-6 {
        return None;
    }
    let n = nn / nl;
    let da = (a - p0).dot(n);
    let db = (b - p0).dot(n);
    // segment pierces the triangle: both sides are "inside"; pick the cheaper way out
    if da * db < 0.0 {
        let t = da / (da - db);
        let x = a + (b - a) * t;
        if in_tri(x, p0, p1, p2, n) {
            let up = r + (-da).max(-db).max(0.0); // shift to the + side
            let down = r + da.max(db).max(0.0); // shift to the - side
            return Some(if up <= down { n * up } else { -n * down });
        }
    }
    let mut best = f32::MAX;
    let mut bs = Vec3::ZERO;
    let mut bq = Vec3::ZERO;
    for s in [a, b] {
        let q = closest_on_tri(s, p0, p1, p2);
        let d = (s - q).length_squared();
        if d < best {
            best = d;
            bs = s;
            bq = q;
        }
    }
    for (e0, e1) in [(p0, p1), (p1, p2), (p2, p0)] {
        let (s, q) = seg_seg(a, b, e0, e1);
        let d = (s - q).length_squared();
        if d < best {
            best = d;
            bs = s;
            bq = q;
        }
    }
    if best >= r * r {
        return None;
    }
    let d = best.sqrt();
    let dir = if d > 1e-5 {
        (bs - bq) / d
    } else if da >= 0.0 {
        n
    } else {
        -n
    };
    Some(dir * (r - d))
}

enum Sweep {
    Miss,
    /// overlapping at the start; `blocks` = the plane normal when the move goes deeper into the triangle
    Start { blocks: Option<Vec3> },
    Hit(f32, Vec3),
}

/// Swept separating-axis test of an axis-aligned box (half extents `h`, centre at the origin, moving by `d`)
/// against the triangle p0 p1 p2 (relative to the box centre). Entry happens when the box comes within
/// DIST_EPSILON along the separating axis, so the returned fraction already includes Source's back-off.
fn sweep_box_tri(p0: Vec3, p1: Vec3, p2: Vec3, h: Vec3, d: Vec3) -> Sweep {
    let e = [p1 - p0, p2 - p1, p0 - p2];
    let tn = e[0].cross(p2 - p0);
    let mut axes = [Vec3::ZERO; 13];
    axes[0] = Vec3::X;
    axes[1] = Vec3::Y;
    axes[2] = Vec3::Z;
    axes[3] = tn;
    let mut k = 4;
    for ei in e {
        axes[k] = ei.cross(Vec3::X);
        axes[k + 1] = ei.cross(Vec3::Y);
        axes[k + 2] = ei.cross(Vec3::Z);
        k += 3;
    }
    let mut t_enter = f32::NEG_INFINITY;
    let mut t_exit = f32::INFINITY;
    let mut enter_n = Vec3::ZERO;
    let mut solid_now = true;
    for a in axes.iter() {
        let l2 = a.length_squared();
        if l2 < 1e-12 {
            continue;
        }
        let a = *a / l2.sqrt();
        let r = h.x * a.x.abs() + h.y * a.y.abs() + h.z * a.z.abs();
        let (q0, q1, q2) = (p0.dot(a), p1.dot(a), p2.dot(a));
        let lo = q0.min(q1).min(q2) - r;
        let hi = q0.max(q1).max(q2) + r;
        let da = d.dot(a);
        if lo >= 0.0 {
            // triangle lies ahead along +a: only reachable by moving +a
            solid_now = false;
            if da <= 0.0 {
                return Sweep::Miss;
            }
            // already inside the DIST_EPSILON shell and never getting closer than half of it: no contact
            if lo < DIST_EPSILON && lo - da >= SHELL_SLACK {
                return Sweep::Miss;
            }
            let te = (lo - DIST_EPSILON) / da;
            if te > t_enter {
                t_enter = te;
                enter_n = -a;
            }
            t_exit = t_exit.min((hi + DIST_EPSILON) / da);
        } else if hi <= 0.0 {
            solid_now = false;
            if da >= 0.0 {
                return Sweep::Miss;
            }
            if -hi < DIST_EPSILON && -hi + da >= SHELL_SLACK {
                return Sweep::Miss;
            }
            let te = (hi + DIST_EPSILON) / da;
            if te > t_enter {
                t_enter = te;
                enter_n = a;
            }
            t_exit = t_exit.min((lo - DIST_EPSILON) / da);
        } else if da > 0.0 {
            t_exit = t_exit.min((hi + DIST_EPSILON) / da);
        } else if da < 0.0 {
            t_exit = t_exit.min((lo - DIST_EPSILON) / da);
        }
    }
    if solid_now {
        // embedded: block only a move that goes deeper into the triangle's plane (towards it from the side the
        // box centre is on), so an embedded box can still walk out but can't fall through a floor. A move within
        // EMBED_GRAZE of the plane only grazes along it: falling down a wall that leans back by a fraction of a
        // degree is not "deeper", and blocking it pinned the hull in mid-air
        let l2 = tn.length_squared();
        if l2 > 1e-12 {
            let n = tn / l2.sqrt();
            let side = if p0.dot(n) > 0.0 { -1.0 } else { 1.0 }; // centre is at the origin
            let n = n * side;
            if d.dot(n) < -(1e-4f32).max(EMBED_GRAZE * d.length()) {
                return Sweep::Start { blocks: Some(n) };
            }
        }
        return Sweep::Start { blocks: None };
    }
    if t_enter >= t_exit || t_enter > 1.0 || t_exit <= 0.0 {
        return Sweep::Miss;
    }
    Sweep::Hit(t_enter.max(0.0), enter_n)
}
