//! The loot catalogue: retail's survival_loot table, loot groups, weapon mods, the attachment matrix and the weapon
//! settings that matter to the inventory (`tools/build_loot.py` writes them to `<assets>/data/loot/`). docs/inventory.md
//! has the rules and where every number comes from.

use std::collections::{HashMap, HashSet};
use std::path::Path;

pub type ItemId = u16;

/// survival_loot `type` (Flowstate's eLootType).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    MainWeapon,
    Ammo,
    Health,
    Armor,
    IncapShield,
    Ordnance,
    Attachment,
    Backpack,
    Helmet,
    Gadget,
    Other,
}

impl Kind {
    fn parse(s: &str) -> Kind {
        match s {
            "main_weapon" => Kind::MainWeapon,
            "ammo" => Kind::Ammo,
            "health" => Kind::Health,
            "armor" => Kind::Armor,
            "incapshield" => Kind::IncapShield,
            "ordnance" => Kind::Ordnance,
            "attachment" => Kind::Attachment,
            "backpack" => Kind::Backpack,
            "helmet" => Kind::Helmet,
            "gadget" => Kind::Gadget,
            _ => Kind::Other,
        }
    }
    /// Lives in a backpack stack (everything else is equipped or in a weapon slot).
    pub fn stacks(self) -> bool {
        matches!(self, Kind::Ammo | Kind::Health | Kind::Ordnance | Kind::Attachment | Kind::Gadget | Kind::Other)
    }
    /// Backpack sort category (sh_survival_loot.gnut:7010-7140): attachments, consumables, special, equipment,
    /// weapons, ammo.
    pub fn sort_category(self) -> u8 {
        match self {
            Kind::Attachment => 0,
            Kind::Health | Kind::Ordnance => 1,
            Kind::Gadget | Kind::Other => 2,
            Kind::Armor | Kind::Helmet | Kind::Backpack | Kind::IncapShield => 3,
            Kind::MainWeapon => 4,
            Kind::Ammo => 5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: ItemId,
    pub r#ref: String,
    pub kind: Kind,
    /// 0 none, 1 white, 2 blue, 3 purple, 4 gold, 5 red
    pub tier: u8,
    /// units in one ground item
    pub per_drop: u32,
    /// max units in one backpack stack
    pub stack: u32,
    pub disabled: bool,
    pub base_weapon: String,
    pub base_mods: Vec<String>,
    /// weapons: the attachment points they take
    pub supported: Vec<String>,
    pub loot_tags: String,
    pub pickup_model: String,
    pub icon: String,
    /// attachments: their point (sight, mag, barrel, grip, hopup)
    pub point: Option<String>,
    /// mags / bolts: the ammo they're locked to
    pub ammo_lock: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct WeaponInfo {
    pub printname: String,
    pub clip: u32,
    /// ammo pool type: bullet / highcal / special / shotgun / sniper / arrows
    pub pool: String,
    pub uses_pool: bool,
    pub stockpile_max: u32,
    pub reload: f32,
    pub reload_empty: f32,
    /// the world model (what it looks like on the ground) and the HUD icon
    #[serde(default)]
    pub playermodel: String,
    #[serde(default)]
    pub hud_icon: String,
    /// mod name -> the keys it changes (ammo_clip_size, ammo_stockpile_max) as KeyValues ops ("21", "++3", "*1.25")
    #[serde(default)]
    pub mods: HashMap<String, HashMap<String, String>>,
}

pub struct LootDb {
    pub items: Vec<Item>,
    by_ref: HashMap<String, ItemId>,
    groups: HashMap<String, Vec<String>>,
    /// attachment ref -> weapon refs it fits
    matrix: HashMap<String, HashSet<String>>,
    matrix_weapons: HashSet<String>,
    pub weapons: HashMap<String, WeaponInfo>,
}

/// A small CSV reader (quoted fields, doubled quotes, CRLF): rows of fields, header first.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Rows as header -> value maps, skipping retail's column-type row ("string", "int", ...) after the header.
fn records(text: &str) -> Vec<HashMap<String, String>> {
    let rows = parse_csv(text);
    let Some((head, body)) = rows.split_first() else { return Vec::new() };
    body.iter()
        .filter(|r| !r.iter().all(|f| f.is_empty()) && r.get(1).is_none_or(|f| f != "string"))
        .map(|r| head.iter().cloned().zip(r.iter().cloned().chain(std::iter::repeat(String::new()))).collect())
        .collect()
}

/// Applies one KeyValues mod op to a value: "21" sets, "++3" / "--4" add / subtract, "*1.25" multiplies.
pub fn apply_op(v: f32, op: &str) -> f32 {
    if let Some(n) = op.strip_prefix("++") {
        v + n.parse::<f32>().unwrap_or(0.0)
    } else if let Some(n) = op.strip_prefix("--") {
        v - n.parse::<f32>().unwrap_or(0.0)
    } else if let Some(n) = op.strip_prefix('*') {
        v * n.parse::<f32>().unwrap_or(1.0)
    } else {
        op.parse().unwrap_or(v)
    }
}

impl LootDb {
    /// Reads `<assets>/data/loot/` (tools/build_loot.py).
    pub fn load(dir: &Path) -> Result<Self, String> {
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).map_err(|e| format!("{}: {e}", dir.join(f).display()));
        let mods: HashMap<String, (String, String)> = records(&read("weapon_mods.csv")?)
            .into_iter()
            .map(|r| {
                let point = r["attachmentPoint"].split_whitespace().next().unwrap_or("").to_owned();
                (r["mod"].clone(), (point, r["ammoType"].clone()))
            })
            .collect();
        let mut items = Vec::new();
        let mut by_ref = HashMap::new();
        for r in records(&read("survival_loot.csv")?) {
            let rf = r["ref"].clone();
            if rf.is_empty() || by_ref.contains_key(&rf) {
                continue;
            }
            let id = items.len() as ItemId;
            let num = |k: &str| r.get(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            let (point, lock) = mods.get(&rf).cloned().unzip();
            items.push(Item {
                id,
                kind: Kind::parse(&r["type"]),
                tier: num("tier") as u8,
                per_drop: num("countPerDrop").max(1),
                stack: num("inventorySlotCount").max(1),
                disabled: r["isDisabled"] == "true",
                base_weapon: r["baseWeapon"].clone(),
                base_mods: r["baseMods"].split_whitespace().map(str::to_owned).collect(),
                supported: r["supportedAttachments"].split_whitespace().map(str::to_owned).collect(),
                loot_tags: r["lootTags"].clone(),
                pickup_model: r["pickupModel"].clone(),
                icon: r["icon"].clone(),
                point: point.filter(|p| !p.is_empty()),
                ammo_lock: lock.filter(|a| !a.is_empty()),
                r#ref: rf.clone(),
            });
            by_ref.insert(rf, id);
        }
        // loot groups: a group row, then its entries (entries may name other groups)
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        let mut cur = String::new();
        for r in records(&read("loot_groups.csv")?) {
            if !r["groupRef"].is_empty() {
                cur = r["groupRef"].clone();
                groups.entry(cur.clone()).or_default();
            } else if !cur.is_empty() && !r["entryRef"].is_empty() {
                groups.entry(cur.clone()).or_default().push(r["entryRef"].clone());
            }
        }
        let mut matrix: HashMap<String, HashSet<String>> = HashMap::new();
        let mtext = read("attachment_matrix.csv")?;
        let mrows = parse_csv(&mtext);
        let mut matrix_weapons = HashSet::new();
        if let Some((head, body)) = mrows.split_first() {
            matrix_weapons = head.iter().skip(2).cloned().collect();
            for r in body {
                let Some(att) = r.first() else { continue };
                let fits = head.iter().zip(r.iter()).skip(2).filter(|(_, v)| *v == "true").map(|(w, _)| w.clone()).collect();
                matrix.insert(att.clone(), fits);
            }
        }
        let weapons: HashMap<String, WeaponInfo> =
            serde_json::from_str(&read("weapons.json")?).map_err(|e| format!("weapons.json: {e}"))?;
        Ok(LootDb { items, by_ref, groups, matrix, matrix_weapons, weapons })
    }

    pub fn get(&self, r: &str) -> Option<ItemId> {
        self.by_ref.get(r).copied()
    }
    pub fn item(&self, id: ItemId) -> &Item {
        &self.items[id as usize]
    }

    /// Every ref in a loot group, nested groups expanded, in order, deduplicated.
    pub fn group_refs(&self, group: &str) -> Vec<String> {
        fn walk(db: &LootDb, g: &str, out: &mut Vec<String>, seen: &mut HashSet<String>) {
            if !seen.insert(g.to_owned()) {
                return;
            }
            for e in db.groups.get(g).into_iter().flatten() {
                if db.groups.contains_key(e) {
                    walk(db, e, out, seen);
                } else if !out.contains(e) {
                    out.push(e.clone());
                }
            }
        }
        let mut out = Vec::new();
        walk(self, group, &mut out, &mut HashSet::new());
        out
    }

    /// The weapon settings name of a weapon item (`baseWeapon`, else its ref).
    pub fn weapon_ref(&self, id: ItemId) -> &str {
        let it = self.item(id);
        if it.base_weapon.is_empty() { &it.r#ref } else { &it.base_weapon }
    }
    pub fn weapon_info(&self, id: ItemId) -> Option<&WeaponInfo> {
        self.weapons.get(self.weapon_ref(id))
    }
    /// The ammo item a weapon reloads from (None: no pool, e.g. energy LMGs carry their own).
    pub fn ammo_of(&self, weapon: ItemId) -> Option<ItemId> {
        let w = self.weapon_info(weapon)?;
        if !w.uses_pool { None } else { self.get(&w.pool) }
    }

    /// Whether an attachment fits a weapon: the weapon supports its point, the attachment matrix says so (when it
    /// has a column for the weapon), and a mag's ammo matches.
    pub fn fits(&self, att: ItemId, weapon: ItemId) -> bool {
        let a = self.item(att);
        let w = self.item(weapon);
        let Some(point) = &a.point else { return false };
        if !w.supported.iter().any(|p| p == point) {
            return false;
        }
        let wref = self.weapon_ref(weapon);
        if self.matrix_weapons.contains(wref) {
            if !self.matrix.get(&a.r#ref).is_some_and(|s| s.contains(wref)) {
                return false;
            }
        }
        match (&a.ammo_lock, self.weapon_info(weapon)) {
            (Some(lock), Some(info)) => *lock == info.pool,
            _ => true,
        }
    }

    /// Mag size with these attachments (base clip, then each attachment's mod op).
    pub fn clip_size<'a>(&self, weapon: ItemId, atts: impl IntoIterator<Item = &'a str>) -> u32 {
        let Some(info) = self.weapon_info(weapon) else { return 0 };
        let mut v = info.clip as f32;
        for a in atts {
            if let Some(op) = info.mods.get(a).and_then(|m| m.get("ammo_clip_size")) {
                v = apply_op(v, op);
            }
        }
        v.round().max(0.0) as u32
    }

    /// What an item looks like on the ground: its pickupModel, weapons their world model (lowercase, `/`).
    pub fn model_of(&self, id: ItemId) -> Option<String> {
        let it = self.item(id);
        let m = if !it.pickup_model.is_empty() { it.pickup_model.as_str() } else { self.weapon_info(id).map_or("", |w| w.playermodel.as_str()) };
        m.starts_with("mdl/").then(|| m.replace('\\', "/").to_ascii_lowercase())
    }

    /// Its icon (a rui image path; tools/build_loot.py copies them to `<assets>/images/loot/<path>.png`).
    pub fn icon_of(&self, id: ItemId) -> Option<&str> {
        let it = self.item(id);
        let i = if !it.icon.is_empty() { it.icon.as_str() } else { self.weapon_info(id).map_or("", |w| w.hud_icon.as_str()) };
        i.starts_with("rui/").then_some(i)
    }

    /// A pickable item: not disabled and not a placeholder row.
    pub fn spawnable(&self, id: ItemId) -> bool {
        let it = self.item(id);
        !it.disabled && it.r#ref != "blank"
    }

    /// Refs of one kind, spawnable, sorted by tier (the range rows' order).
    pub fn refs_of(&self, f: impl Fn(&Item) -> bool) -> Vec<ItemId> {
        let mut v: Vec<ItemId> = self.items.iter().filter(|i| self.spawnable(i.id) && f(i)).map(|i| i.id).collect();
        v.sort_by_key(|&i| (self.item(i).tier, i));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_and_crlf() {
        let r = parse_csv("a,b\r\n\"x, y\",\"he said \"\"hi\"\"\"\r\n");
        assert_eq!(r, vec![vec!["a".to_string(), "b".into()], vec!["x, y".into(), "he said \"hi\"".into()]]);
    }

    #[test]
    fn mod_ops() {
        assert_eq!(apply_op(18.0, "21"), 21.0);
        assert_eq!(apply_op(5.0, "++2"), 7.0);
        assert_eq!(apply_op(18.0, "--4"), 14.0);
        assert_eq!(apply_op(20.0, "*1.25"), 25.0);
    }
}
