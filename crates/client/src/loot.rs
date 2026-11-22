//! The client side of the inventory (crates/shared/src/loot, docs/inventory.md): loads the retail loot catalogue and the
//! Firing Range's loot, turns the use key into pickups (tap; hold 0.3 s to swap), throws drops into the world, and
//! builds what the HUD and the test UI show (item labels, the use prompt, the inventory screen, the weapon panel).

use std::sync::Arc;

use glam::Vec3;
use shared::assets::Config;
use shared::collision::Collision;
use shared::loot::db::Kind;
use shared::loot::inventory::{GroundView, Hands};
use shared::loot::db::ItemId;
use shared::loot::{range, Inventory, LootDb, LootWorld, Pickup, UseKind};

use crate::highlight::Setting;
use crate::props::{Instance, Props, DYNAMIC_CAP};

/// survival_item_<tier> (retail hcxt): far distance 3000 (CPU-side: farther loot gets no highlight)
const HIGHLIGHT_FAR: f32 = 3000.0;

/// retail colorpalette_table LOOT_TIER1..5 (hcxt colour ids 14..18), as raw sRGB / 255. UNSURE (docs/highlight.md):
/// whether the engine converts the palette row to linear before it becomes params[0].
const LOOT_TIER_COLORS: [[u8; 3]; 5] = [[192, 192, 192], [30, 144, 255], [125, 0, 255], [255, 205, 60], [255, 78, 29]];

/// The highlight of one ground item (SURVIVAL_GetHighlightForTier -> survival_item_common..heirloom): colour from
/// the palette, param1 0 (no unfocused fill). S21 engine (r5apex FUN_14078a320 / FUN_14078a620): the context keeps
/// its radius as a byte, int(r * 255 / 8 + 0.5) (HighlightContext_SetRadius), read back as byte * 8 / 255: hcxt 1.0 ->
/// 32 -> 1.0039. A focused entity takes the context's focused functions (hcxt: 136 / 136) and radius byte 0x80
/// (128 * 8 / 255 = 4.0157 px).
fn highlight_setting(tier: u8, focused: bool) -> Setting {
    let c = LOOT_TIER_COLORS[(tier.clamp(1, 5) - 1) as usize];
    let (f, byte) = if focused { (136, 128.0) } else { (135, (1.0f32 * 255.0 * 0.125 + 0.5).floor()) };
    Setting::new([c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0], 0.0, f, f, byte * 8.0 / 255.0)
}

/// retail colorpalette_table FX_LOOT_TIER0..5 (identical in R5F S30's platform\datatable\colorpalette_table.csv)
const FX_LOOT_TIER_COLORS: [[u8; 3]; 6] = [[0, 0, 0], [155, 150, 150], [80, 137, 235], [146, 73, 172], [255, 198, 100], [255, 78, 29]];

/// The ground entity's rendercolor as InstGpu.pad[1] (0 = white). R5F S30 cl_survival_loot.nut
/// ApplyEquipmentColorAndFXOverrides: body shields and helmets with no skinOverride (all of survival_loot's) get
/// GetFXRarityColorForTier = GetKeyColor(COLORID_FX_LOOT_TIER0, tier). Armor cores (armor_core_pickup_*: depleted =
/// 50,50,50 + a skin per tier) aren't on the range.
fn render_color(item: &shared::loot::db::Item) -> u32 {
    use shared::loot::db::Kind;
    if !matches!(item.kind, Kind::Armor | Kind::Helmet) {
        return 0;
    }
    let c = FX_LOOT_TIER_COLORS[(item.tier as usize).min(5)];
    c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 | 0xFF << 24
}

/// `USE_INPUT_LONG`: a weapon / equipment swap is a 0.3 s hold (sh_survival_loot_actions.gnut:539-575).
pub const HOLD_TIME: f32 = 0.3;
/// The R-99: the only first-person weapon extracted so far (weapon.rs); other guns are carried but not drawn.
pub const R99: &str = "mp_weapon_r97";

pub struct Loot {
    pub db: Arc<LootDb>,
    pub inv: Inventory,
    pub world: LootWorld,
    /// the use key held on an item that needs a hold: (ground id, seconds held)
    hold: Option<(u32, f32)>,
    /// the last refused pickup's message, shown for a moment
    pub message: Option<(String, f32)>,
    /// the inventory screen (TAB)
    pub open: bool,
    /// item -> its ground model's prop-library variant (None: no model exported; the test UI labels it instead)
    variant_of: Vec<Option<u32>>,
    /// ground id -> (position, packed lighting probe) last looked up
    probe_cache: std::collections::HashMap<u32, (Vec3, u32)>,
    /// `<assets>/images/loot`: the items' icons (install step 10)
    icon_root: std::path::PathBuf,
}

/// What the game must do after a loot tick.
#[derive(Default)]
pub struct LootEvents {
    /// a weapon went into this slot: bring it up
    pub switch_to: Option<usize>,
}

impl Loot {
    /// The catalogue (`<assets>/data/loot`, install step 10), the range's loot from the map's markers, the range's spawn
    /// kit. None without the loot data.
    pub fn load(cfg: &Config) -> Option<Loot> {
        let db = match LootDb::load(&cfg.assets_root().join("data").join("loot")) {
            Ok(db) => Arc::new(db),
            Err(e) => {
                log::warn!("no loot data ({e}): run the install's loot step (tools\\install.ps1 -Only loot)");
                return None;
            }
        };
        let ents: Vec<_> = std::fs::read_dir(cfg.map_dir().join("bsp"))
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().ends_with("_script.ent"))
            .flat_map(|p| range::parse_ents(&std::fs::read_to_string(p).unwrap_or_default()))
            .collect();
        let mut world = LootWorld::default();
        let racks = range::rack_props(&std::fs::read_to_string(cfg.map_dir().join("props.json")).unwrap_or_default());
        let spawns = range::firing_range(&db, &ents, &racks);
        for s in &spawns {
            let weapon = (db.item(s.item).kind == Kind::MainWeapon).then(|| shared::loot::Weapon::new(&db, s.item).with_clip(&db, s.clip));
            world.spawn(s.item, s.count, s.pos, s.angles, weapon, None, true);
        }
        // the range's loot has been lying there since before the player arrived (no 0.1 s spawn delay on it)
        world.time = 1.0;
        log::info!("loot: {} items in the catalogue, {} on the range", db.items.len(), spawns.len());
        Some(Loot { inv: Inventory::firing_range_spawn(&db), db, world, hold: None, message: None, open: false, variant_of: Vec::new(), probe_cache: Default::default(), icon_root: cfg.assets_root().join("images").join("loot") })
    }

    /// The ground loot as prop instances of their world models (export-map puts the loot's models in the map's prop
    /// library: install step 1 after step 10), into `out` (Props::dynamic). `probe` packs an item's lighting.
    /// `eye` / `dir`: the view, for the highlight slots (nearest first, within HIGHLIGHT_FAR) and the focused item.
    pub fn ground_instances(&mut self, props: &Props, probe: impl Fn(Vec3) -> u32, eye: Vec3, dir: Vec3, out: &mut Vec<Instance>, hl: &mut Vec<Setting>) {
        if self.variant_of.len() != self.db.items.len() {
            self.variant_of = (0..self.db.items.len()).map(|i| self.db.model_of(i as ItemId).and_then(|m| props.model_variant.get(&m).copied())).collect();
            let spawned: std::collections::HashSet<ItemId> = self.world.items.iter().map(|g| g.item).collect();
            let missing: Vec<&str> = spawned.iter().filter(|&&i| self.variant_of[i as usize].is_none()).map(|&i| self.db.item(i).r#ref.as_str()).collect();
            log::info!("loot: {} of {} items have a ground model; on the range without one: {missing:?}", self.variant_of.iter().flatten().count(), self.variant_of.len());
        }
        out.clear();
        // highlight slots (retail: one per highlighted entity, 8-bit id, 0 = none): the nearest 255 items within the
        // context's far distance; the one under the crosshair is focused
        hl.clear();
        hl.push(Setting::default());
        let focus = self.world.target(&self.db, eye, dir);
        let mut near: Vec<(f32, u32)> = self.world.items.iter().map(|g| ((g.pos - eye).length(), g.id)).filter(|&(d, _)| d < HIGHLIGHT_FAR).collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        near.truncate(crate::highlight::MAX_SLOTS - 1);
        let slot_of: std::collections::HashMap<u32, u32> = near.iter().enumerate().map(|(i, &(_, id))| (id, i as u32 + 1)).collect();
        for &(_, id) in &near {
            let Some(g) = self.world.get(id) else { continue };
            hl.push(highlight_setting(self.db.item(g.item).tier, Some(id) == focus));
        }
        for g in &self.world.items {
            let Some(v) = self.variant_of[g.item as usize] else { continue };
            if out.len() >= DYNAMIC_CAP {
                break;
            }
            let lit = match self.probe_cache.get(&g.id) {
                Some(&(p, l)) if p == g.pos => l,
                _ => {
                    let l = probe(g.pos + Vec3::new(0.0, 0.0, 4.0));
                    self.probe_cache.insert(g.id, (g.pos, l));
                    l
                }
            };
            let mut inst = props.instance(v, g.pos, g.angles, lit);
            inst.gpu.pad[0] = slot_of.get(&g.id).copied().unwrap_or(0);
            inst.gpu.pad[1] = render_color(self.db.item(g.item));
            out.push(inst);
        }
        if self.probe_cache.len() > 2 * self.world.items.len() + 64 {
            let live: std::collections::HashSet<u32> = self.world.items.iter().map(|g| g.id).collect();
            self.probe_cache.retain(|id, _| live.contains(id));
        }
    }

    /// An item's icon file (it may not exist: the console shows the name alone then).
    pub fn icon(&self, item: ItemId) -> Option<std::path::PathBuf> {
        self.db.icon_of(item).map(|i| self.icon_root.join(format!("{i}.png")))
    }

    /// The targeted item (for the prompt's icon).
    pub fn target_item(&self, eye: Vec3, dir: Vec3) -> Option<ItemId> {
        let id = self.world.target(&self.db, eye, dir)?;
        self.world.get(id).map(|g| g.item)
    }

    /// Whether an item shows its model on the ground.
    pub fn has_model(&self, item: ItemId) -> bool {
        self.variant_of.get(item as usize).is_some_and(|v| v.is_some())
    }

    /// One sim tick: the world's loot, and the use key (`pressed` this tick, `down` = held).
    pub fn tick(&mut self, dt: f32, eye: Vec3, dir: Vec3, pressed: bool, down: bool, coll: &Collision) -> LootEvents {
        let mut ev = LootEvents::default();
        self.world.tick(&self.db, dt as f64, |a, b| {
            let d = b - a;
            let len = d.length();
            if len < 1e-4 {
                return None;
            }
            coll.ray(a, d / len, len).map(|t| a + d / len * t)
        });
        if let Some((_, t)) = &mut self.message {
            *t -= dt;
        }
        if self.message.as_ref().is_some_and(|(_, t)| *t <= 0.0) {
            self.message = None;
        }
        let target = self.world.target(&self.db, eye, dir);
        if pressed {
            self.hold = None;
            if let Some(id) = target {
                let p = self.pickup(id, UseKind::Tap, eye, dir);
                if p.needs_hold || (p.taken == 0 && self.world.get(id).is_some_and(|g| self.db.item(g.item).kind == Kind::Attachment)) {
                    self.hold = Some((id, 0.0));
                }
                ev.switch_to = p.switch_to;
            }
        } else if down {
            if let Some((id, t)) = &mut self.hold {
                *t += dt;
                if *t >= HOLD_TIME {
                    let id = *id;
                    self.hold = None;
                    if target == Some(id) {
                        let p = self.pickup(id, UseKind::Hold, eye, dir);
                        ev.switch_to = p.switch_to;
                    }
                }
            }
        } else {
            self.hold = None;
        }
        ev
    }

    /// The use key on whatever the crosshair is on (console `use`): what E does, without the timing.
    pub fn use_target(&mut self, eye: Vec3, dir: Vec3, how: UseKind) -> Option<(String, Pickup)> {
        let id = self.world.target(&self.db, eye, dir)?;
        let name = self.world.get(id).map(|g| name(&self.db, g.item)).unwrap_or_default();
        Some((name, self.pickup(id, how, eye, dir)))
    }

    fn pickup(&mut self, id: u32, how: UseKind, eye: Vec3, dir: Vec3) -> Pickup {
        let Some(g) = self.world.get(id) else { return Pickup::default() };
        let view = GroundView { item: g.item, count: g.count, weapon: g.weapon.as_ref(), shield: g.shield };
        let p = self.inv.pickup(&self.db, &view, how);
        if p.taken > 0 {
            self.world.take(id, p.taken);
        }
        if let Some(m) = &p.message {
            if !p.needs_hold {
                self.message = Some((m.clone(), 1.5));
            }
        }
        self.throw(p.drops.clone(), eye, dir);
        p
    }

    /// Puts drops into the world from the player's view.
    pub fn throw(&mut self, drops: Vec<shared::loot::Drop>, eye: Vec3, dir: Vec3) {
        for (n, d) in drops.into_iter().enumerate() {
            self.world.throw(d, eye - Vec3::new(0.0, 0.0, 16.0), dir, n);
        }
    }

    /// Hold progress 0..1 on the targeted item (the use prompt's ring).
    pub fn hold_frac(&self) -> Option<f32> {
        self.hold.map(|(_, t)| (t / HOLD_TIME).min(1.0))
    }

    /// The use prompt for the item under the crosshair.
    pub fn prompt(&self, eye: Vec3, dir: Vec3) -> Option<String> {
        let id = self.world.target(&self.db, eye, dir)?;
        let g = self.world.get(id)?;
        let it = self.db.item(g.item);
        let name = name(&self.db, g.item);
        let count = if it.kind.stacks() && g.count > 1 { format!(" x{}", g.count) } else { String::new() };
        let swap = it.kind == Kind::MainWeapon && self.inv.weapons.iter().all(Option::is_some);
        Some(if swap { format!("[Hold E] Swap for {name}") } else { format!("[E] {name}{count}") })
    }

    /// Whether the active weapon slot holds an R-99 (the weapon.rs viewmodel is drawn).
    pub fn r99_in_hand(&self) -> bool {
        self.inv.active().and_then(|s| self.inv.weapons[s].as_ref()).is_some_and(|w| self.db.weapon_ref(w.item) == R99)
    }

    /// The weapon panel request (tools/rui/bridge.mjs `hud` with two slots): loot refs, mag, reserve, attachments.
    pub fn hud_request(&self, screen: [u32; 2]) -> serde_json::Value {
        let slot = |i: usize| -> serde_json::Value {
            let Some(w) = &self.inv.weapons[i] else { return serde_json::Value::Null };
            let atts: serde_json::Map<String, serde_json::Value> = w
                .atts
                .iter()
                .map(|(p, &a)| (p.clone(), serde_json::json!({ "ref": self.db.item(a).r#ref, "tier": self.db.item(a).tier })))
                .collect();
            let info = self.db.weapon_info(w.item);
            let reserve = if self.inv.infinite_ammo { serde_json::json!("inf") } else { serde_json::json!(self.inv.reserve(&self.db, i).unwrap_or(0)) };
            serde_json::json!({
                "ref": self.db.weapon_ref(w.item), "mag": w.clip, "magMax": w.clip_size(&self.db), "reserve": reserve,
                "reserveMax": info.map_or(1, |i| if i.uses_pool { shared::loot::inventory::POOL_MAX.min(300) } else { i.stockpile_max.max(1) }),
                "tier": self.db.item(w.item).tier.max(1), "attachments": atts,
            })
        };
        // holstered / melee: the last selected primary stays the active tab, drawn as not in hand
        // (cl_weapon_status.gnut UpdateHudDataForMainWeapons: lastSelectedPrimaryWeapon, else slot 0, else slot 1)
        let w = &self.inv.weapons;
        let (active, selected) = match self.inv.hands {
            Hands::Weapon(i) if w[i].is_some() => (i as i64, true),
            _ if w[self.inv.last_weapon.min(1)].is_some() => (self.inv.last_weapon.min(1) as i64, false),
            _ if w[0].is_some() => (0, false),
            _ if w[1].is_some() => (1, false),
            _ => (-1, false),
        };
        serde_json::json!({ "kind": "hud", "screen": screen, "slots": [slot(0), slot(1)], "active": active, "selected": selected })
    }
}

/// A readable name for the test UI (retail's names are localisation tokens we don't load).
pub fn name(db: &LootDb, id: shared::loot::ItemId) -> String {
    let it = db.item(id);
    let r = it.r#ref.as_str();
    let fixed = match r {
        "bullet" => Some("Light Rounds"),
        "highcal" => Some("Heavy Rounds"),
        "special" => Some("Energy Ammo"),
        "shotgun" => Some("Shotgun Shells"),
        "sniper" => Some("Sniper Ammo"),
        "arrows" => Some("Arrows"),
        "health_pickup_health_small" => Some("Syringe"),
        "health_pickup_health_large" => Some("Med Kit"),
        "health_pickup_combo_small" => Some("Shield Cell"),
        "health_pickup_combo_large" => Some("Shield Battery"),
        "health_pickup_combo_full" => Some("Phoenix Kit"),
        "health_pickup_ultimate" => Some("Ultimate Accelerant"),
        "mp_weapon_frag_grenade" => Some("Frag Grenade"),
        "mp_weapon_thermite_grenade" => Some("Thermite Grenade"),
        "mp_weapon_grenade_emp" => Some("Arc Star"),
        _ => None,
    };
    if let Some(n) = fixed {
        return n.to_owned();
    }
    let lv = it.tier;
    match it.kind {
        Kind::Armor => format!("Body Shield Lv.{lv}{}", if r.contains("evolving") { " (Evo)" } else { "" }),
        Kind::Helmet => format!("Helmet Lv.{lv}"),
        Kind::Backpack => format!("Backpack Lv.{lv}"),
        Kind::IncapShield => format!("Knockdown Shield Lv.{lv}"),
        Kind::MainWeapon => weapon_name(db.weapon_ref(id)).to_owned(),
        _ => {
            let words: Vec<String> = r
                .split('_')
                .filter(|w| !w.is_empty() && !(w.len() == 2 && w.starts_with('l') && w[1..].chars().all(|c| c.is_ascii_digit())))
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                })
                .collect();
            if it.kind == Kind::Attachment && lv > 0 { format!("{} Lv.{lv}", words.join(" ")) } else { words.join(" ") }
        }
    }
}

fn weapon_name(r: &str) -> &str {
    match r {
        "mp_weapon_r97" => "R-99",
        "mp_weapon_rspn101" => "R-301",
        "mp_weapon_vinson" => "Flatline",
        "mp_weapon_hemlok" => "Hemlok",
        "mp_weapon_energy_ar" => "Havoc",
        "mp_weapon_nemesis" => "Nemesis",
        "mp_weapon_alternator_smg" => "Alternator",
        "mp_weapon_pdw" => "Prowler",
        "mp_weapon_volt_smg" => "Volt",
        "mp_weapon_car" => "C.A.R.",
        "mp_weapon_esaw" => "Devotion",
        "mp_weapon_lstar" => "L-STAR",
        "mp_weapon_lmg" => "Spitfire",
        "mp_weapon_dragon_lmg" => "Rampage",
        "mp_weapon_g2" => "G7 Scout",
        "mp_weapon_doubletake" => "Triple Take",
        "mp_weapon_3030" => "30-30 Repeater",
        "mp_weapon_bow" => "Bocek",
        "mp_weapon_defender" => "Charge Rifle",
        "mp_weapon_dmr" => "Longbow",
        "mp_weapon_sentinel" => "Sentinel",
        "mp_weapon_sniper" => "Kraber",
        "mp_weapon_shotgun" => "EVA-8",
        "mp_weapon_mastiff" => "Mastiff",
        "mp_weapon_energy_shotgun" => "Peacekeeper",
        "mp_weapon_shotgun_pistol" => "Mozambique",
        "mp_weapon_autopistol" => "RE-45",
        "mp_weapon_semipistol" => "P2020",
        "mp_weapon_wingman" => "Wingman",
        "mp_weapon_throwingknife" => "Throwing Knife",
        other => other,
    }
}

/// Apex's rarity colours (test UI).
pub fn tier_color(tier: u8) -> [u8; 3] {
    match tier {
        1 => [220, 220, 220],
        2 => [60, 160, 255],
        3 => [180, 80, 255],
        4 => [255, 210, 60],
        5 => [255, 70, 60],
        _ => [150, 150, 150],
    }
}
