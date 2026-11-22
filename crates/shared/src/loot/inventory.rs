//! The player's inventory, default survival rules (docs/inventory.md sections 1-6): backpack stacks, two main weapon
//! slots with attachments, armor / helmet / knockdown shield / backpack, ammo as backpack stacks (= the AmmoPool the
//! HUD's reserve shows). Pure logic: the client turns `Drop`s into ground items and decides when a hold is a hold.

use std::collections::BTreeMap;

use super::db::{ItemId, Kind, LootDb};

/// AmmoPool capacity per type (sh_survival_loot.gnut:557).
pub const POOL_MAX: u32 = 2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stack {
    pub item: ItemId,
    pub count: u32,
}

/// A main weapon (in a slot or on the ground).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Weapon {
    pub item: ItemId,
    /// rounds in the mag
    pub clip: u32,
    /// weapons without an ammo pool (energy LMGs, Kraber, bow) carry their reserve themselves
    pub reserve: u32,
    /// attachment point -> attachment
    pub atts: BTreeMap<String, ItemId>,
}

impl Weapon {
    /// A fresh weapon: full mag (and full own reserve), the loot row's locked attachments.
    pub fn new(db: &LootDb, item: ItemId) -> Self {
        let mut w = Weapon { item, clip: 0, reserve: 0, atts: BTreeMap::new() };
        // locked-set rows (`*_blueset` ...) list their attachments in baseMods; `dormant_hopup_*` there is a weapon mod
        // waiting for its hop-up, not an attachment
        for m in db.item(item).base_mods.iter().filter(|m| !m.starts_with("dormant_")) {
            if let Some(a) = db.get(m).filter(|&a| db.item(a).kind == Kind::Attachment) {
                if let Some(p) = db.item(a).point.clone() {
                    w.atts.insert(p, a);
                }
            }
        }
        w.clip = w.clip_size(db);
        if let Some(info) = db.weapon_info(item).filter(|i| !i.uses_pool) {
            w.reserve = info.stockpile_max;
        }
        w
    }
    pub fn clip_size(&self, db: &LootDb) -> u32 {
        db.clip_size(self.item, self.atts.values().map(|&a| db.item(a).r#ref.as_str()))
    }
    /// A ground weapon's stored mag (racks spawn clip 500 = "full"): capped at what the mag holds.
    pub fn with_clip(mut self, db: &LootDb, clip: Option<u32>) -> Self {
        let max = self.clip_size(db);
        self.clip = clip.map_or(max, |c| c.min(max));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hands {
    Weapon(usize),
    Melee,
}

/// What comes out onto the ground.
#[derive(Clone, Debug, PartialEq)]
pub struct Drop {
    pub item: ItemId,
    pub count: u32,
    pub weapon: Option<Weapon>,
    /// armor: the shield it holds
    pub shield: Option<u32>,
}

impl Drop {
    fn stack(item: ItemId, count: u32) -> Self {
        Drop { item, count, weapon: None, shield: None }
    }
}

/// How the use key was pressed: tap = the main action, hold = swaps / alternative (attachment into the backpack).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseKind {
    Tap,
    Hold,
}

/// A ground item as the pickup sees it.
#[derive(Clone, Debug)]
pub struct GroundView<'a> {
    pub item: ItemId,
    pub count: u32,
    pub weapon: Option<&'a Weapon>,
    pub shield: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pickup {
    /// units taken off the ground item (it keeps count - taken)
    pub taken: u32,
    /// things the pickup put on the ground (the replaced armor, the swapped weapon, ...)
    pub drops: Vec<Drop>,
    /// the game's message for a refused pickup ("You already have a better ...")
    pub message: Option<String>,
    /// a weapon went into this slot: switch to it
    pub switch_to: Option<usize>,
    /// a tap that needs a hold (swap a weapon)
    pub needs_hold: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Inventory {
    pub backpack: Vec<Stack>,
    pub weapons: [Option<Weapon>; 2],
    pub hands: Hands,
    /// the last weapon slot held (a swap from melee replaces this one)
    pub last_weapon: usize,
    pub armor: Option<ItemId>,
    pub shield: u32,
    pub helmet: Option<ItemId>,
    pub incap: Option<ItemId>,
    pub bag: Option<ItemId>,
    /// Firing Range toggle (default off): reloads take nothing
    pub infinite_ammo: bool,
}

impl Default for Inventory {
    fn default() -> Self {
        Inventory {
            backpack: Vec::new(),
            weapons: [None, None],
            hands: Hands::Melee,
            last_weapon: 0,
            armor: None,
            shield: 0,
            helmet: None,
            incap: None,
            bag: None,
            infinite_ammo: false,
        }
    }
}

/// Shield an armor tier holds (sh_survival_loot.gnut:6366-6450): 0 25, 1 50, 2 75, 3/4 100, 5 125.
pub fn armor_shield(tier: u8) -> u32 {
    match tier {
        0 => 25,
        1 => 50,
        2 => 75,
        3 | 4 => 100,
        _ => 125,
    }
}

/// Backpack slots by bag tier (sh_consts.gnut:2504-2507): none 10, 1 12, 2 14, 3/4 16.
pub fn bag_slots(tier: Option<u8>) -> usize {
    match tier {
        None => 10,
        Some(1) => 12,
        Some(2) => 14,
        Some(_) => 16,
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Armor => "body shield",
        Kind::Helmet => "helmet",
        Kind::Backpack => "backpack",
        Kind::IncapShield => "knockdown shield",
        Kind::Attachment => "attachment",
        _ => "item",
    }
}

impl Inventory {
    /// The Firing Range spawn kit (playlist survival_firingrange_base, firing_range.gnut:3840-3874): white helmet,
    /// white body shield (full), white knockdown shield, 2 syringes, 2 shield cells. No backpack, no weapons.
    pub fn firing_range_spawn(db: &LootDb) -> Self {
        let mut inv = Inventory::default();
        inv.helmet = db.get("helmet_pickup_lv1");
        inv.armor = db.get("armor_pickup_lv1");
        inv.shield = inv.armor.map_or(0, |a| armor_shield(db.item(a).tier));
        inv.incap = db.get("incapshield_pickup_lv1");
        for (r, n) in [("health_pickup_health_small", 2), ("health_pickup_combo_small", 2)] {
            if let Some(id) = db.get(r) {
                inv.add_stack(db, id, n);
            }
        }
        inv
    }

    pub fn capacity(&self, db: &LootDb) -> usize {
        bag_slots(self.bag.map(|b| db.item(b).tier))
    }

    pub fn count(&self, item: ItemId) -> u32 {
        self.backpack.iter().filter(|s| s.item == item).map(|s| s.count).sum()
    }

    /// Adds units: tops up partial stacks of the same item, then opens new stacks while there are free slots
    /// (sh_survival_loot.gnut:2786-2835). Returns how many went in. Ammo also stops at the pool's 2000.
    pub fn add_stack(&mut self, db: &LootDb, item: ItemId, count: u32) -> u32 {
        let it = db.item(item);
        let mut left = count;
        if it.kind == Kind::Ammo {
            left = left.min(POOL_MAX.saturating_sub(self.count(item)));
        }
        let room_total = left;
        for s in self.backpack.iter_mut().filter(|s| s.item == item) {
            let n = left.min(it.stack.saturating_sub(s.count));
            s.count += n;
            left -= n;
        }
        let cap = self.capacity(db);
        while left > 0 && self.backpack.len() < cap {
            let n = left.min(it.stack);
            self.backpack.push(Stack { item, count: n });
            left -= n;
        }
        self.sort(db);
        room_total - left
    }

    /// Takes units out (the last, smallest stacks first). Returns how many came out.
    pub fn remove(&mut self, item: ItemId, count: u32) -> u32 {
        let mut left = count;
        for i in (0..self.backpack.len()).rev() {
            if left == 0 {
                break;
            }
            if self.backpack[i].item != item {
                continue;
            }
            let n = left.min(self.backpack[i].count);
            self.backpack[i].count -= n;
            left -= n;
        }
        self.backpack.retain(|s| s.count > 0);
        count - left
    }

    /// Backpack order (sh_survival_loot.gnut:7010-7140): category, kind, tier high first, item, count high first.
    pub fn sort(&mut self, db: &LootDb) {
        self.backpack.sort_by(|a, b| {
            let (ia, ib) = (db.item(a.item), db.item(b.item));
            (ia.kind.sort_category(), ia.kind as u8, std::cmp::Reverse(ia.tier), a.item, std::cmp::Reverse(a.count))
                .cmp(&(ib.kind.sort_category(), ib.kind as u8, std::cmp::Reverse(ib.tier), b.item, std::cmp::Reverse(b.count)))
        });
    }

    pub fn active(&self) -> Option<usize> {
        match self.hands {
            Hands::Weapon(i) if self.weapons[i].is_some() => Some(i),
            _ => None,
        }
    }

    /// The reserve the HUD shows for a weapon slot: backpack ammo of its type (pool weapons), else its own reserve.
    pub fn reserve(&self, db: &LootDb, slot: usize) -> Option<u32> {
        let w = self.weapons[slot].as_ref()?;
        Some(match db.ammo_of(w.item) {
            Some(ammo) => self.count(ammo),
            None => w.reserve,
        })
    }

    /// Reload: fills the mag from the pool (or the weapon's own reserve). Returns the rounds loaded.
    pub fn reload(&mut self, db: &LootDb, slot: usize) -> u32 {
        let Some(w) = self.weapons[slot].as_ref() else { return 0 };
        let need = w.clip_size(db).saturating_sub(w.clip);
        let ammo = db.ammo_of(w.item);
        let got = if self.infinite_ammo {
            need
        } else {
            match ammo {
                Some(a) => self.remove(a, need),
                None => {
                    let w = self.weapons[slot].as_mut().unwrap();
                    let n = need.min(w.reserve);
                    w.reserve -= n;
                    n
                }
            }
        };
        if let Some(w) = self.weapons[slot].as_mut() {
            w.clip += got;
        }
        got
    }

    /// Whether a reload could load anything.
    pub fn can_reload(&self, db: &LootDb, slot: usize) -> bool {
        let Some(w) = self.weapons[slot].as_ref() else { return false };
        w.clip < w.clip_size(db) && (self.infinite_ammo || self.reserve(db, slot).unwrap_or(0) > 0)
    }

    /// Puts compatible backpack attachments on a weapon's empty points, best tier first (survival_loot.gnut:4255).
    fn take_backpack_attachments(&mut self, db: &LootDb, slot: usize) {
        let Some(wid) = self.weapons[slot].as_ref().map(|w| w.item) else { return };
        let mut cands: Vec<ItemId> = self
            .backpack
            .iter()
            .map(|s| s.item)
            .filter(|&a| db.item(a).kind == Kind::Attachment && db.fits(a, wid))
            .collect();
        cands.sort_by_key(|&a| std::cmp::Reverse(db.item(a).tier));
        for a in cands {
            let p = db.item(a).point.clone().unwrap_or_default();
            let w = self.weapons[slot].as_mut().unwrap();
            if !w.atts.contains_key(&p) && self.count(a) > 0 {
                self.weapons[slot].as_mut().unwrap().atts.insert(p, a);
                self.remove(a, 1);
            }
        }
    }

    /// Rounds above the mag's size after an attachment change go back to the backpack; what doesn't fit is thrown.
    fn trim_clip(&mut self, db: &LootDb, slot: usize, drops: &mut Vec<Drop>) {
        let Some(w) = self.weapons[slot].as_ref() else { return };
        let max = w.clip_size(db);
        if w.clip <= max {
            return;
        }
        let extra = w.clip - max;
        self.weapons[slot].as_mut().unwrap().clip = max;
        if let Some(ammo) = db.ammo_of(self.weapons[slot].as_ref().unwrap().item) {
            let back = self.add_stack(db, ammo, extra);
            if back < extra {
                drops.push(Drop::stack(ammo, extra - back));
            }
        }
    }

    /// Picks up (part of) a ground item. docs/inventory.md section 4.
    pub fn pickup(&mut self, db: &LootDb, g: &GroundView, how: UseKind) -> Pickup {
        let it = db.item(g.item);
        let mut out = Pickup::default();
        match it.kind {
            Kind::MainWeapon => self.pickup_weapon(db, g, how, &mut out),
            Kind::Attachment => self.pickup_attachment(db, g, how, &mut out),
            Kind::Armor | Kind::Helmet | Kind::IncapShield | Kind::Backpack => self.pickup_gear(db, g, &mut out),
            _ => {
                out.taken = self.add_stack(db, g.item, g.count);
                if out.taken == 0 {
                    out.message = Some("Inventory full".into());
                }
            }
        }
        out
    }

    fn pickup_gear(&mut self, db: &LootDb, g: &GroundView, out: &mut Pickup) {
        let it = db.item(g.item);
        let cur = match it.kind {
            Kind::Armor => self.armor,
            Kind::Helmet => self.helmet,
            Kind::IncapShield => self.incap,
            _ => self.bag,
        };
        let ground_shield = g.shield.unwrap_or_else(|| armor_shield(it.tier));
        let better = match cur {
            None => true,
            Some(c) => {
                let ct = db.item(c).tier;
                it.tier > ct || (it.tier == ct && c != g.item) || (it.kind == Kind::Armor && ground_shield > self.shield)
            }
        };
        if !better {
            out.message = Some(format!("You already have a better {}", kind_name(it.kind)));
            return;
        }
        if let Some(old) = cur {
            let shield = (it.kind == Kind::Armor).then_some(self.shield);
            out.drops.push(Drop { item: old, count: 1, weapon: None, shield });
        }
        match it.kind {
            Kind::Armor => {
                self.armor = Some(g.item);
                self.shield = ground_shield;
            }
            Kind::Helmet => self.helmet = Some(g.item),
            Kind::IncapShield => self.incap = Some(g.item),
            _ => self.bag = Some(g.item),
        }
        out.taken = 1;
    }

    fn pickup_weapon(&mut self, db: &LootDb, g: &GroundView, how: UseKind, out: &mut Pickup) {
        let mut new = g.weapon.cloned().unwrap_or_else(|| Weapon::new(db, g.item));
        let slot = match self.weapons.iter().position(Option::is_none) {
            Some(free) => free,
            None => {
                if how != UseKind::Hold {
                    out.needs_hold = true;
                    out.message = Some("Hold to swap".into());
                    return;
                }
                let slot = self.active().unwrap_or(self.last_weapon);
                let old = self.weapons[slot].take().unwrap();
                let (drops, moved) = self.strip_for_swap(db, old, &new);
                out.drops.extend(drops);
                new.atts.extend(moved);
                slot
            }
        };
        self.weapons[slot] = Some(new);
        // the swapped-out gun's mods that fit are on it already; now the backpack's compatible attachments, then the mag
        // tops up from the pool
        let mut dummy = Vec::new();
        self.take_backpack_attachments(db, slot);
        self.trim_clip(db, slot, &mut dummy);
        out.drops.extend(dummy);
        self.reload(db, slot);
        self.hands = Hands::Weapon(slot);
        self.last_weapon = slot;
        out.switch_to = Some(slot);
        out.taken = 1;
    }

    /// A weapon lost to a swap: its attachments that fit the new weapon's empty points move over (returned as
    /// `moved`), the rest pop off as their own ground items; the ground gun keeps its mag minus rounds above its base
    /// clip, which go to the backpack (survival_loot.gnut:3520-3545, 4036, 4098-4225).
    fn strip_for_swap(&mut self, db: &LootDb, mut old: Weapon, new: &Weapon) -> (Vec<Drop>, Vec<(String, ItemId)>) {
        let mut drops = Vec::new();
        let mut moved: Vec<(String, ItemId)> = Vec::new();
        for (p, a) in std::mem::take(&mut old.atts) {
            if !new.atts.contains_key(&p) && db.fits(a, new.item) {
                moved.push((p, a));
            } else {
                drops.push(Drop::stack(a, 1));
            }
        }
        let base = old.clip_size(db);
        if old.clip > base {
            let extra = old.clip - base;
            old.clip = base;
            if let Some(ammo) = db.ammo_of(old.item) {
                let back = self.add_stack(db, ammo, extra);
                if back < extra {
                    drops.push(Drop::stack(ammo, extra - back));
                }
            }
        }
        drops.insert(0, Drop { item: old.item, count: 1, weapon: Some(old), shield: None });
        (drops, moved)
    }

    fn pickup_attachment(&mut self, db: &LootDb, g: &GroundView, how: UseKind, out: &mut Pickup) {
        let it = db.item(g.item);
        let point = it.point.clone().unwrap_or_default();
        let always = point == "sight" || point.starts_with("hopup");
        let order: Vec<usize> = match self.active() {
            Some(a) => vec![a, 1 - a],
            None => vec![self.last_weapon, 1 - self.last_weapon],
        };
        let mut fits_somewhere = false;
        if how == UseKind::Tap {
            for &slot in &order {
                let Some(w) = self.weapons[slot].as_ref() else { continue };
                if !db.fits(g.item, w.item) {
                    continue;
                }
                fits_somewhere = true;
                let cur = w.atts.get(&point).copied();
                let upgrade = match cur {
                    None => true,
                    Some(c) => c != g.item && (always || it.tier > db.item(c).tier),
                };
                if !upgrade {
                    continue;
                }
                self.weapons[slot].as_mut().unwrap().atts.insert(point.clone(), g.item);
                if let Some(old) = cur {
                    // the replaced one: onto the other weapon if it's better there or that point is empty, else ground
                    let other = 1 - slot;
                    let placed = match self.weapons[other].as_mut() {
                        Some(o) if db.fits(old, o.item) => match o.atts.get(&point) {
                            None => {
                                o.atts.insert(point.clone(), old);
                                true
                            }
                            Some(&oc) if db.item(old).tier > db.item(oc).tier => {
                                o.atts.insert(point.clone(), old);
                                out.drops.push(Drop::stack(oc, 1));
                                true
                            }
                            _ => false,
                        },
                        _ => false,
                    };
                    if !placed {
                        out.drops.push(Drop::stack(old, 1));
                    }
                }
                let mut d = Vec::new();
                self.trim_clip(db, slot, &mut d);
                self.trim_clip(db, other_slot(slot), &mut d);
                out.drops.extend(d);
                out.taken = 1;
                return;
            }
        }
        if fits_somewhere && how == UseKind::Tap {
            out.message = Some(format!("You already have a better {}", kind_name(Kind::Attachment)));
            return;
        }
        out.taken = self.add_stack(db, g.item, 1);
        if out.taken == 0 {
            out.message = Some("Inventory full".into());
        }
    }

    /// Drops `count` of backpack stack `index` (one ground item; survival_loot.gnut:6444-6655).
    pub fn drop_stack(&mut self, index: usize, count: u32) -> Option<Drop> {
        let s = self.backpack.get_mut(index)?;
        let n = count.min(s.count);
        if n == 0 {
            return None;
        }
        s.count -= n;
        let item = s.item;
        self.backpack.retain(|s| s.count > 0);
        Some(Drop::stack(item, n))
    }

    /// Drops a weapon: the ground gun keeps its mag (rounds above its base clip are thrown as ammo); its attachments
    /// pop off as their own items (survival_loot.gnut:3289-3364, 4941-5105). Last gun gone -> melee.
    pub fn drop_weapon(&mut self, db: &LootDb, slot: usize) -> Vec<Drop> {
        let Some(mut w) = self.weapons[slot].take() else { return Vec::new() };
        let mut drops = Vec::new();
        for (_, a) in std::mem::take(&mut w.atts) {
            drops.push(Drop::stack(a, 1));
        }
        let base = w.clip_size(db);
        if w.clip > base {
            if let Some(ammo) = db.ammo_of(w.item) {
                drops.push(Drop::stack(ammo, w.clip - base));
            }
            w.clip = base;
        }
        drops.insert(0, Drop { item: w.item, count: 1, weapon: Some(w), shield: None });
        if self.hands == Hands::Weapon(slot) {
            self.hands = match self.weapons[1 - slot] {
                Some(_) => Hands::Weapon(1 - slot),
                None => Hands::Melee,
            };
        }
        drops
    }

    /// Takes an attachment off a weapon into the backpack (or onto the ground when it's full).
    pub fn detach(&mut self, db: &LootDb, slot: usize, point: &str) -> Vec<Drop> {
        let mut drops = Vec::new();
        let Some(a) = self.weapons[slot].as_mut().and_then(|w| w.atts.remove(point)) else { return drops };
        if self.add_stack(db, a, 1) == 0 {
            drops.push(Drop::stack(a, 1));
        }
        self.trim_clip(db, slot, &mut drops);
        drops
    }
}

fn other_slot(s: usize) -> usize {
    1 - s
}
