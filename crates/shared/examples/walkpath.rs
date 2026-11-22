//! A smooth first-person walk through the Firing Range for `r5rust --poses`: A* over a walkable grid built from the
//! game's collision (floor, head room, step height, no wall between cells), visiting the waypoints in order, then a
//! smoothed 60 fps camera path with a gentle look-around. Made for apex-splat (Gaussian splats from renders).
//!
//! cargo run --release -p shared --example walkpath -- <waypoints.txt> <out_poses.txt> [speed u/s = 190]
//! waypoints.txt: "x y" per line (feet position, any floor height; snapped to the nearest walkable cell).
//! out: "fNNNNN x y z_eye pitch yaw" per line, one per 1/60 s.
use glam::{Vec2, Vec3};
use shared::{assets::Config, collision::Collision, glb};
use std::collections::BinaryHeap;

const CELL: f32 = 50.0;
const X0: f32 = -13000.0;
const Y0: f32 = -4000.0;
const NX: usize = 300; // -13000 .. 2000
const NY: usize = 220; // -4000 .. 7000
const EYE: f32 = 60.0;
const STEP: f32 = 22.0; // max floor change between neighbouring cells (stairs pass, ledges don't)

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (wp_path, out_path) = (&a[1], &a[2]);
    let speed: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(190.0);

    let w = glb::load_world(&Config::load().world_glb(), glb::Mode::Geometry, true).unwrap();
    let idx: Vec<u32> = w.draws.iter().filter(|d| w.materials[d.material as usize].solid()).flat_map(|d| w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].iter().copied()).collect();
    let col = Collision::build(w.positions.clone(), &idx);

    // floor per cell: highest floor between z -400 and 400 with 80 units of head room (the range's ground level;
    // roofs and the huge buildings are above that band)
    let mut floor = vec![f32::NAN; NX * NY];
    for j in 0..NY {
        for i in 0..NX {
            let (x, y) = (X0 + i as f32 * CELL, Y0 + j as f32 * CELL);
            let mut top = 450.0;
            // walk down through stacked surfaces until one in the band has head room
            for _ in 0..6 {
                let Some(t) = col.ray(Vec3::new(x, y, top), -Vec3::Z, top + 450.0) else { break };
                let z = top - t;
                if z < -400.0 { break; }
                if z <= 400.0 && col.ray(Vec3::new(x, y, z + 2.0), Vec3::Z, 80.0).is_none() {
                    floor[j * NX + i] = z;
                    break;
                }
                top = z - 1.0;
            }
        }
    }
    // clearance: cells next to walls cost more, so the walk stays in the middle of corridors
    let blocked = |x: f32, y: f32, z: f32, dx: f32, dy: f32| col.ray(Vec3::new(x, y, z + 35.0), Vec3::new(dx, dy, 0.0).normalize(), Vec2::new(dx, dy).length()).is_some();
    let mut clear = vec![0.0f32; NX * NY];
    for j in 0..NY {
        for i in 0..NX {
            let z = floor[j * NX + i];
            if z.is_nan() { continue; }
            let (x, y) = (X0 + i as f32 * CELL, Y0 + j as f32 * CELL);
            let mut c = 0.0;
            for k in 0..8 {
                let ang = k as f32 * std::f32::consts::FRAC_PI_4;
                if blocked(x, y, z, ang.cos() * 120.0, ang.sin() * 120.0) { c += 1.0; }
            }
            clear[j * NX + i] = c;
        }
    }
    let walkable = floor.iter().filter(|z| !z.is_nan()).count();
    println!("grid {NX}x{NY} @ {CELL}: {walkable} walkable cells");
    // WALK_GRID=file: dump floor (f32, NaN = not walkable) + clearance, row-major from (X0, Y0), for plotting
    if let Ok(p) = std::env::var("WALK_GRID") {
        let mut b: Vec<u8> = floor.iter().flat_map(|v| v.to_le_bytes()).collect();
        b.extend(clear.iter().flat_map(|v| v.to_le_bytes()));
        std::fs::write(&p, b).unwrap();
        println!("grid -> {p}");
    }

    // nearest walkable, open cell (optionally only among `ok` cells)
    let cell_of = |p: Vec2, ok: Option<&[bool]>| -> Option<usize> {
        let (ci, cj) = (((p.x - X0) / CELL).round() as i64, ((p.y - Y0) / CELL).round() as i64);
        let mut best: Option<(i64, usize)> = None;
        for dj in -30..=30i64 {
            for di in -30..=30i64 {
                let (i, j) = (ci + di, cj + dj);
                if i < 0 || j < 0 || i >= NX as i64 || j >= NY as i64 { continue; }
                let k = j as usize * NX + i as usize;
                if floor[k].is_nan() || clear[k] > 2.0 || ok.is_some_and(|o| !o[k]) { continue; }
                let d = di * di + dj * dj;
                if best.is_none_or(|b| d < b.0) { best = Some((d, k)); }
            }
        }
        best.map(|b| b.1)
    };

    let wps: Vec<Vec2> = std::fs::read_to_string(wp_path).unwrap().lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| { let v: Vec<f32> = l.split_whitespace().filter_map(|s| s.parse().ok()).collect(); Vec2::new(v[0], v[1]) }).collect();

    // everything reachable on foot from the first waypoint; the others snap into that region
    let start = cell_of(wps[0], None).expect("no walkable cell near the first waypoint");
    let mut reach = vec![false; NX * NY];
    let mut stack = vec![start];
    reach[start] = true;
    while let Some(k) = stack.pop() {
        for n in neighbours(k) {
            if !reach[n] && can_step(k, n, &floor, &col) {
                reach[n] = true;
                stack.push(n);
            }
        }
    }
    println!("{} cells reachable from the start", reach.iter().filter(|r| **r).count());
    if let Ok(p) = std::env::var("WALK_GRID") {
        std::fs::write(format!("{p}.reach"), reach.iter().map(|r| *r as u8).collect::<Vec<u8>>()).unwrap();
    }

    // A* leg by leg
    let mut cells: Vec<usize> = vec![];
    for leg in wps.windows(2) {
        let (s, g) = (cell_of(leg[0], Some(&reach)).unwrap(), cell_of(leg[1], Some(&reach)).unwrap_or_else(|| {
            eprintln!("waypoint {:?}: nothing reachable within 1500 u", leg[1]);
            std::process::exit(1);
        }));
        let Some(p) = astar(s, g, &floor, &clear, &col) else {
            eprintln!("no walkable route {:?} -> {:?}", leg[0], leg[1]);
            std::process::exit(1);
        };
        if !cells.is_empty() { cells.pop(); }
        cells.extend(p);
    }
    let pts: Vec<Vec3> = cells.iter().map(|&k| Vec3::new(X0 + (k % NX) as f32 * CELL, Y0 + (k / NX) as f32 * CELL, floor[k])).collect();

    // smooth: a few Laplacian passes (keeps the A* staircase out of the camera), then resample at walk speed
    let mut sm = pts.clone();
    for _ in 0..40 {
        let prev = sm.clone();
        for i in 1..sm.len() - 1 {
            sm[i] = prev[i] * 0.5 + (prev[i - 1] + prev[i + 1]) * 0.25;
        }
    }
    let step = speed / 60.0;
    let mut res = vec![sm[0]];
    let (mut i, mut carry) = (0usize, 0.0f32);
    while i + 1 < sm.len() {
        let seg = sm[i + 1] - sm[i];
        let len = seg.truncate().length().max(1e-3);
        let mut t = step - carry;
        while t <= len {
            res.push(sm[i] + seg * (t / len));
            t += step;
        }
        carry = len - (t - step);
        i += 1;
    }
    // eye height: floor smoothed over ~0.5 s so stairs read as a glide, not a jitter
    let zf: Vec<f32> = (0..res.len()).map(|k| {
        let (a, b) = (k.saturating_sub(15), (k + 15).min(res.len() - 1));
        res[a..=b].iter().map(|p| p.z).sum::<f32>() / (b - a + 1) as f32
    }).collect();
    // heading: direction of travel over the next ~1 s, smoothed; plus a slow look-around so the splat sees the sides
    let mut yaws = vec![0.0f32; res.len()];
    for k in 0..res.len() {
        let (a, b) = (k.saturating_sub(30), (k + 30).min(res.len() - 1));
        let d = res[b] - res[a];
        yaws[k] = d.y.atan2(d.x).to_degrees();
    }
    unwrap_deg(&mut yaws);
    let yaws = box_smooth(&yaws, 45);
    let mut out = String::new();
    let mut walls = 0;
    for k in 0..res.len() {
        let t = k as f32 / 60.0;
        let yaw = yaws[k] + 28.0 * (t * 0.55).sin() + 8.0 * (t * 1.7 + 1.0).sin();
        let pitch = 2.0 + 4.0 * (t * 0.37 + 0.5).sin();
        let p = Vec3::new(res[k].x, res[k].y, zf[k] + EYE);
        if k > 0 {
            let q = Vec3::new(res[k - 1].x, res[k - 1].y, zf[k - 1] + EYE);
            if col.ray(q, (p - q).normalize_or(Vec3::X), (p - q).length()).is_some() { walls += 1; }
        }
        out.push_str(&format!("f{k:05} {:.2} {:.2} {:.2} {:.3} {:.3}\n", p.x, p.y, p.z, pitch, yaw.rem_euclid(360.0)));
    }
    std::fs::write(out_path, out).unwrap();
    let len: f32 = res.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    println!("{} frames ({:.1} s at {speed} u/s, {:.0} u), {walls} eye steps through geometry -> {out_path}", res.len(), res.len() as f32 / 60.0, len);
}

fn unwrap_deg(v: &mut [f32]) {
    for k in 1..v.len() {
        while v[k] - v[k - 1] > 180.0 { v[k] -= 360.0; }
        while v[k] - v[k - 1] < -180.0 { v[k] += 360.0; }
    }
}

fn box_smooth(v: &[f32], r: usize) -> Vec<f32> {
    (0..v.len()).map(|k| {
        let (a, b) = (k.saturating_sub(r), (k + r).min(v.len() - 1));
        v[a..=b].iter().sum::<f32>() / (b - a + 1) as f32
    }).collect()
}

fn neighbours(k: usize) -> impl Iterator<Item = usize> {
    let (i, j) = ((k % NX) as i64, (k / NX) as i64);
    [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)].into_iter().filter_map(move |(di, dj)| {
        let (ni, nj) = (i + di, j + dj);
        (ni >= 0 && nj >= 0 && ni < NX as i64 && nj < NY as i64).then(|| nj as usize * NX + ni as usize)
    })
}

/// Can a player walk from cell k to neighbour n: both walkable, floor change within a stair step, and nothing in
/// between at knee or eye height.
fn can_step(k: usize, n: usize, floor: &[f32], col: &Collision) -> bool {
    let (za, zb) = (floor[k], floor[n]);
    let diag = k % NX != n % NX && k / NX != n / NX;
    if za.is_nan() || zb.is_nan() || (zb - za).abs() > STEP * if diag { 1.42 } else { 1.0 } { return false; }
    let at = |c: usize, z: f32| Vec3::new(X0 + (c % NX) as f32 * CELL, Y0 + (c / NX) as f32 * CELL, z);
    let (a3, b3) = (at(k, za), at(n, zb));
    let d = b3 - a3;
    col.ray(a3 + Vec3::Z * 30.0, d.normalize(), d.length()).is_none() && col.ray(a3 + Vec3::Z * EYE, d.normalize(), d.length()).is_none()
}

#[derive(PartialEq)]
struct Node(f32, usize);
impl Eq for Node {}
impl PartialOrd for Node { fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) } }
impl Ord for Node { fn cmp(&self, o: &Self) -> std::cmp::Ordering { o.0.partial_cmp(&self.0).unwrap() } }

fn astar(s: usize, g: usize, floor: &[f32], clear: &[f32], col: &Collision) -> Option<Vec<usize>> {
    let pos = |k: usize| Vec2::new((k % NX) as f32, (k / NX) as f32);
    let mut dist = vec![f32::MAX; floor.len()];
    let mut from = vec![usize::MAX; floor.len()];
    let mut heap = BinaryHeap::new();
    dist[s] = 0.0;
    heap.push(Node(0.0, s));
    while let Some(Node(_, k)) = heap.pop() {
        if k == g {
            let mut p = vec![g];
            let mut c = g;
            while c != s { c = from[c]; p.push(c); }
            p.reverse();
            return Some(p);
        }
        for n in neighbours(k) {
            if !can_step(k, n, floor, col) { continue; }
            let step = (pos(n) - pos(k)).length() * (1.0 + clear[n] * 0.6);
            let nd = dist[k] + step;
            if nd < dist[n] {
                dist[n] = nd;
                from[n] = k;
                heap.push(Node(nd + (pos(n) - pos(g)).length(), n));
            }
        }
    }
    None
}
