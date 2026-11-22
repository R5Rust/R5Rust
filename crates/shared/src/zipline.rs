//! Map ziplines, read from the BSP's `<map>_script.ent` (retail S30 Firing Range: ~20 lines).
//!
//! Each line is a `zipline` entity pair: the start carries `link_to_guid_0` = the end's `link_guid` and the cable's
//! precomputed sag as `_zipline_rest_point_0..N` (world space, endpoints included); the end only marks the far anchor.
//! Every per-line key the game reads is kept (`ZiplineVertical`, `ZiplineSpeedScale`, `ZiplineLengthScale`,
//! `ZiplineAutoDetachDistance`, `ZiplineDropToBottom`, `ZiplinePreserveVelocity`, `ZiplinePushOffInDirectionX`,
//! `ziplineMountReverseDistance`, `Width`).

use glam::Vec3;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Zipline {
    /// the cable path, start to end (rest points; a straight start-end segment when the start has none)
    pub points: Vec<Vec3>,
    /// running length at each point (same count as `points`)
    pub dist: Vec<f32>,
    pub vertical: bool,
    pub speed_scale: f32,
    pub length_scale: f32,
    /// the start entity's auto-detach distance (used riding towards the start)
    pub auto_detach_distance: f32,
    /// the end entity's own (zipline_end +0x970: used riding towards the end)
    pub end_auto_detach_distance: f32,
    pub drop_to_bottom: bool,
    pub preserve_velocity: bool,
    pub push_off_x: f32,
    pub mount_reverse_distance: f32,
    /// cable width (the `Width` key, Source units)
    pub width: f32,
}

impl Zipline {
    pub fn length(&self) -> f32 {
        *self.dist.last().unwrap_or(&0.0)
    }

    /// The point at `d` along the cable (clamped to its ends) and the unit direction there (start -> end).
    pub fn at(&self, d: f32) -> (Vec3, Vec3) {
        let n = self.points.len();
        if n < 2 {
            return (self.points.first().copied().unwrap_or(Vec3::ZERO), Vec3::X);
        }
        let d = d.clamp(0.0, self.length());
        let i = self.dist.partition_point(|&x| x <= d).clamp(1, n - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let seg = (self.dist[i] - self.dist[i - 1]).max(1e-6);
        let t = (d - self.dist[i - 1]) / seg;
        (a.lerp(b, t), (b - a).normalize_or(Vec3::X))
    }

    /// Closest point on the cable to `p`: (distance along the cable, the point, its distance from `p`).
    pub fn closest(&self, p: Vec3) -> (f32, Vec3, f32) {
        let mut best = (0.0, self.points.first().copied().unwrap_or(Vec3::ZERO), f32::MAX);
        for i in 1..self.points.len() {
            let (a, b) = (self.points[i - 1], self.points[i]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            let q = a + ab * t;
            let dd = q.distance(p);
            if dd < best.2 {
                best = (self.dist[i - 1] + (self.dist[i] - self.dist[i - 1]) * t, q, dd);
            }
        }
        best
    }

    /// Closest approach of the ray `o + dir * t` (t in 0..max_t) to the cable: (distance along the cable, cable
    /// point, ray-cable distance, t).
    pub fn ray_closest(&self, o: Vec3, dir: Vec3, max_t: f32) -> (f32, Vec3, f32, f32) {
        let mut best = (0.0, Vec3::ZERO, f32::MAX, 0.0);
        for i in 1..self.points.len() {
            let (a, b) = (self.points[i - 1], self.points[i]);
            let (s, t, pa, pr) = segment_ray(a, b, o, dir, max_t);
            let dd = pa.distance(pr);
            if dd < best.2 {
                best = (self.dist[i - 1] + (self.dist[i] - self.dist[i - 1]) * s, pa, dd, t);
            }
        }
        best
    }
}

/// Closest points between segment a..b (param s in 0..1) and ray o + d*t (t in 0..max_t).
fn segment_ray(a: Vec3, b: Vec3, o: Vec3, d: Vec3, max_t: f32) -> (f32, f32, Vec3, Vec3) {
    let u = b - a;
    let w = a - o;
    let (aa, bb, cc, dd, ee) = (u.dot(u), u.dot(d), d.dot(d), u.dot(w), d.dot(w));
    let den = aa * cc - bb * bb;
    let mut s = if den.abs() > 1e-9 { ((bb * ee - cc * dd) / den).clamp(0.0, 1.0) } else { 0.0 };
    let mut t = ((bb * s + ee) / cc.max(1e-9)).clamp(0.0, max_t);
    // re-project s for the clamped t
    s = ((t * bb - dd) / aa.max(1e-9)).clamp(0.0, 1.0);
    t = ((bb * s + ee) / cc.max(1e-9)).clamp(0.0, max_t);
    (s, t, a + u * s, o + d * t)
}

fn vec3(s: &str) -> Option<Vec3> {
    let v: Vec<f32> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    (v.len() >= 3).then(|| Vec3::new(v[0], v[1], v[2]))
}

/// All ziplines in an entity file's text (`<map>_script.ent`).
pub fn parse(text: &str) -> Vec<Zipline> {
    let mut ents: Vec<HashMap<&str, &str>> = Vec::new();
    let mut at = 0;
    while let Some(o) = text[at..].find('{') {
        let s = at + o;
        let Some(e) = text[s..].find('}').map(|e| s + e) else { break };
        let kv: HashMap<&str, &str> = text[s..e].split('"').collect::<Vec<_>>().chunks(4).filter_map(|c| (c.len() >= 4).then(|| (c[1], c[3]))).collect();
        at = e + 1;
        if kv.get("classname").is_some_and(|c| *c == "zipline") {
            ents.push(kv);
        }
    }
    let by_guid: HashMap<&str, &HashMap<&str, &str>> = ents.iter().filter_map(|e| e.get("link_guid").map(|g| (*g, e))).collect();
    let mut out = Vec::new();
    for e in &ents {
        let Some(end) = e.get("link_to_guid_0").and_then(|g| by_guid.get(g)) else { continue };
        let (Some(a), Some(b)) = (e.get("origin").and_then(|s| vec3(s)), end.get("origin").and_then(|s| vec3(s))) else { continue };
        let mut rest: Vec<(usize, Vec3)> = e.iter().filter_map(|(k, v)| Some((k.strip_prefix("_zipline_rest_point_")?.parse().ok()?, vec3(v)?))).collect();
        rest.sort_by_key(|r| r.0);
        let points: Vec<Vec3> = if rest.len() >= 2 { rest.into_iter().map(|r| r.1).collect() } else { vec![a, b] };
        let mut dist = vec![0.0];
        for i in 1..points.len() {
            dist.push(dist[i - 1] + points[i].distance(points[i - 1]));
        }
        let f = |k: &str, d: f32| e.get(k).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(d);
        out.push(Zipline {
            points,
            dist,
            vertical: f("ZiplineVertical", 0.0) != 0.0,
            speed_scale: f("ZiplineSpeedScale", 1.0),
            length_scale: f("ZiplineLengthScale", 1.0),
            auto_detach_distance: f("ZiplineAutoDetachDistance", 150.0),
            end_auto_detach_distance: end.get("ZiplineAutoDetachDistance").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(150.0),
            drop_to_bottom: f("ZiplineDropToBottom", 1.0) != 0.0,
            preserve_velocity: f("ZiplinePreserveVelocity", 0.0) != 0.0,
            push_off_x: f("ZiplinePushOffInDirectionX", 0.0),
            mount_reverse_distance: f("ziplineMountReverseDistance", 0.0),
            width: f("Width", 2.0),
        });
    }
    out
}

/// The map's ziplines: `<bsp dir>/<map>_script.ent`.
pub fn load(bsp_dir: &std::path::Path, map: &str) -> Vec<Zipline> {
    match std::fs::read(bsp_dir.join(format!("{map}_script.ent"))) {
        Ok(b) => parse(&String::from_utf8_lossy(&b)),
        Err(_) => Vec::new(),
    }
}
