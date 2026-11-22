//! Loot lying in the world: ground items, the Firing Range's respawn (0.25 s after a pickup, at the same spot with the
//! original contents), thrown drops settling onto the floor, and which item the crosshair is on (the use prompt).

use glam::Vec3;

use super::db::{ItemId, Kind, LootDb};
use super::inventory::{Drop, GroundView, Weapon};

/// `firing_range_item_respawn_time` (vs\mp\_staging_area.gnut:170).
pub const RANGE_RESPAWN: f64 = 0.25;
/// A pickup is refused this soon after the item appeared (sh_survival_loot.gnut:4421).
pub const PICKUP_DELAY: f64 = 0.1;
/// The use prompt's vertical line: item -6 .. item +46, crosshair within 14 u of it (sh_survival_loot.gnut:6091-6175).
pub const LINE_BOTTOM: f32 = -6.0;
pub const LINE_TOP: f32 = 46.0;
pub const LINE_RADIUS: f32 = 14.0;
/// Weapons also take the crosshair passing within 32 u of their ground point.
pub const WEAPON_RADIUS: f32 = 32.0;
/// How far from the eye loot can be used. UNSURE: the engine's use range is code side (not in scripts / SDK); this is
/// what feels like retail's reach and should be read out of the binary.
pub const USE_RANGE: f32 = 128.0;

#[derive(Clone, Debug)]
pub struct Respawn {
    pub item: ItemId,
    pub count: u32,
    pub clip: Option<u32>,
    pub pos: Vec3,
    pub angles: Vec3,
}

#[derive(Clone, Debug)]
pub struct GroundItem {
    pub id: u32,
    pub item: ItemId,
    pub count: u32,
    pub pos: Vec3,
    /// Source angles (pitch, yaw, roll), degrees
    pub angles: Vec3,
    pub weapon: Option<Weapon>,
    pub shield: Option<u32>,
    /// world time it appeared
    pub spawned: f64,
    /// range loot: comes back like this after a pickup
    pub respawn: Option<Respawn>,
    /// thrown: still falling
    pub vel: Option<Vec3>,
}

impl GroundItem {
    pub fn view(&self) -> GroundView<'_> {
        GroundView { item: self.item, count: self.count, weapon: self.weapon.as_ref(), shield: self.shield }
    }
}

enum Pending {
    /// range loot coming back after a pickup
    Spawn(Respawn),
    /// a partly taken range stack refilling to its original count
    Refill(u32),
}

#[derive(Default)]
pub struct LootWorld {
    pub items: Vec<GroundItem>,
    next_id: u32,
    pending: Vec<(f64, Pending)>,
    pub time: f64,
}

/// Closest approach of a ray (o + t d, t in 0..len) and a segment a..b: (distance, t along the ray).
fn ray_segment(o: Vec3, d: Vec3, len: f32, a: Vec3, b: Vec3) -> (f32, f32) {
    let u = b - a;
    let w = o - a;
    let (aa, bb, cc, dd, ee) = (d.dot(d), d.dot(u), u.dot(u), d.dot(w), u.dot(w));
    let den = aa * cc - bb * bb;
    let (mut t, mut s) = if den.abs() < 1e-6 { (0.0, (ee / cc.max(1e-6)).clamp(0.0, 1.0)) } else { ((bb * ee - cc * dd) / den, (aa * ee - bb * dd) / den) };
    t = t.clamp(0.0, len);
    let _ = s;
    // re-solve each side against the clamped other
    s = ((o + d * t - a).dot(u) / cc.max(1e-6)).clamp(0.0, 1.0);
    t = ((a + u * s - o).dot(d) / aa.max(1e-6)).clamp(0.0, len);
    ((o + d * t - (a + u * s)).length(), t)
}

impl LootWorld {
    pub fn spawn(&mut self, item: ItemId, count: u32, pos: Vec3, angles: Vec3, weapon: Option<Weapon>, shield: Option<u32>, respawn: bool) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let respawn = respawn.then(|| Respawn { item, count, clip: weapon.as_ref().map(|w| w.clip), pos, angles });
        self.items.push(GroundItem { id, item, count, pos, angles, weapon, shield, spawned: self.time, respawn, vel: None });
        id
    }

    /// A drop from the inventory, thrown from `from` along `dir` (pitched down 10 deg, 150-250 u/s, +100 up;
    /// survival_loot.gnut:6444-6655). `n` spreads several drops from one action.
    pub fn throw(&mut self, d: Drop, from: Vec3, dir: Vec3, n: usize) {
        // view dir pitched 10 deg down, a speed in 150..250 (spread by n so several drops don't stack), +100 up
        let flat = Vec3::new(dir.x, dir.y, 0.0).normalize_or(Vec3::X);
        let pitch = (dir.z.clamp(-1.0, 1.0).asin() - 10f32.to_radians()).clamp(-1.4, 1.4);
        let side = Vec3::new(-flat.y, flat.x, 0.0) * (((n as f32) * 1.7).sin() * 0.25);
        let aim = (flat * pitch.cos() + Vec3::new(0.0, 0.0, pitch.sin()) + side).normalize();
        let v = aim * (150.0 + (n as f32 * 37.0) % 100.0) + Vec3::new(0.0, 0.0, 100.0);
        let id = self.spawn(d.item, d.count, from, Vec3::new(0.0, dir.y.atan2(dir.x).to_degrees(), 0.0), d.weapon, d.shield, false);
        if let Some(g) = self.items.iter_mut().find(|g| g.id == id) {
            g.vel = Some(v);
        }
    }

    pub fn get(&self, id: u32) -> Option<&GroundItem> {
        self.items.iter().find(|g| g.id == id)
    }

    /// Takes `taken` units off an item (all of it: the item goes; range loot is queued to come back).
    pub fn take(&mut self, id: u32, taken: u32) {
        let Some(i) = self.items.iter().position(|g| g.id == id) else { return };
        let g = &mut self.items[i];
        if taken >= g.count || g.weapon.is_some() {
            let g = self.items.remove(i);
            if let Some(r) = g.respawn {
                self.pending.push((self.time + RANGE_RESPAWN, Pending::Spawn(r)));
            }
        } else if taken > 0 {
            g.count -= taken;
            // a partly taken range stack refills to its original count after the same delay
            if g.respawn.is_some() {
                let id = g.id;
                self.pending.push((self.time + RANGE_RESPAWN, Pending::Refill(id)));
            }
        }
    }

    /// Advances time: respawns, thrown items falling. `floor(from, to)` traces the world (hit point).
    pub fn tick(&mut self, db: &LootDb, dt: f64, floor: impl Fn(Vec3, Vec3) -> Option<Vec3>) {
        self.time += dt;
        let now = self.time;
        let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending).into_iter().partition(|(t, _)| *t <= now);
        self.pending = keep;
        for (_, p) in due {
            let r = match p {
                Pending::Refill(id) => {
                    if let Some(g) = self.items.iter_mut().find(|g| g.id == id) {
                        if let Some(orig) = &g.respawn {
                            g.count = orig.count;
                        }
                    }
                    continue;
                }
                Pending::Spawn(r) => r,
            };
            let weapon = (db.item(r.item).kind == Kind::MainWeapon).then(|| Weapon::new(db, r.item).with_clip(db, r.clip));
            self.spawn(r.item, r.count, r.pos, r.angles, weapon, None, true);
        }
        for g in &mut self.items {
            let Some(v) = g.vel.as_mut() else { continue };
            v.z -= 750.0 * dt as f32;
            let next = g.pos + *v * dt as f32;
            match floor(g.pos, next) {
                Some(hit) => {
                    g.pos = hit + Vec3::new(0.0, 0.0, 1.0);
                    g.vel = None;
                }
                None => g.pos = next,
            }
        }
    }

    /// The item the crosshair is on (eye, normalised view dir): of the items whose use line the view passes within
    /// range of, the one closest to the crosshair (ties: the nearer one).
    pub fn target(&self, db: &LootDb, eye: Vec3, dir: Vec3) -> Option<u32> {
        let mut best: Option<((f32, f32), u32)> = None;
        for g in &self.items {
            if g.vel.is_some() || self.time - g.spawned < PICKUP_DELAY {
                continue;
            }
            if (g.pos - eye).length() > USE_RANGE + LINE_TOP {
                continue;
            }
            let (d, t) = ray_segment(eye, dir, USE_RANGE, g.pos + Vec3::new(0.0, 0.0, LINE_BOTTOM), g.pos + Vec3::new(0.0, 0.0, LINE_TOP));
            let mut hit = (d <= LINE_RADIUS).then_some((d, t));
            if db.item(g.item).kind == Kind::MainWeapon {
                let (dw, tw) = ray_segment(eye, dir, USE_RANGE, g.pos, g.pos);
                if dw <= WEAPON_RADIUS && hit.is_none_or(|(d, _)| dw < d) {
                    hit = Some((dw, tw));
                }
            }
            if let Some(k) = hit {
                if best.is_none_or(|(b, _)| k.0 < b.0 - 0.5 || (k.0 < b.0 + 0.5 && k.1 < b.1)) {
                    best = Some((k, g.id));
                }
            }
        }
        best.map(|(_, id)| id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_the_vertical_line() {
        let (d, t) = ray_segment(Vec3::new(0.0, 0.0, 60.0), Vec3::X, 200.0, Vec3::new(100.0, 5.0, -6.0), Vec3::new(100.0, 5.0, 46.0));
        assert!((d - (5.0f32 * 5.0 + 14.0 * 14.0).sqrt()).abs() < 0.01, "{d}");
        assert!((t - 100.0).abs() < 0.01);
        let (d, _) = ray_segment(Vec3::new(0.0, 0.0, 20.0), Vec3::X, 200.0, Vec3::new(100.0, 3.0, -6.0), Vec3::new(100.0, 3.0, 46.0));
        assert!((d - 3.0).abs() < 0.01, "{d}");
    }
}
