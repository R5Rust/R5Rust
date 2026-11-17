//! Content registry: every legend, ability, passive, weapon, ammo type, attachment, consumable and map the game
//! knows about, loaded from data files so base content and mods are added the same way.
//!
//! Layout (one TOML file per entry; the file name is the entry's name):
//!
//! ```text
//! content/<kind>/<name>.toml                 base game, namespace "core"   -> id core:<name>
//! mods/<folder>/mod.toml                     id = "coolmod", name, version, enabled, priority
//! mods/<folder>/content/<kind>/<name>.toml   -> id coolmod:<name>
//! ```
//!
//! `<kind>` is one of the folder names in [`Kind::ALL`]. References between entries (a weapon's ammo type, a legend's
//! abilities) are ids; a reference without a namespace means "same namespace as the file". A mod replaces an existing
//! entry by setting `replaces = "core:r99"` at the top level; anything else with an id that already exists is an error.
//! Fields the engine doesn't know yet are kept in each entry's `extra` table, so mods can carry data before the code
//! that reads it exists.
//!
//! Every entry gets a [`Handle`]: its index in its registry. Handles are assigned in sorted id order after loading, so
//! two machines with the same content agree on them (use the handle, not the id, on the wire).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

/// Namespaced content id, `namespace:name`, lowercase `[a-z0-9_]` on both sides.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Id(String);

impl Id {
    pub fn new(namespace: &str, name: &str) -> Result<Id, String> {
        Id::parse(&format!("{namespace}:{name}"))
    }

    /// Parses `ns:name`. Without a colon the id is taken as `default_ns:name`.
    pub fn resolve(s: &str, default_ns: &str) -> Result<Id, String> {
        if s.contains(':') { Id::parse(s) } else { Id::new(default_ns, s) }
    }

    pub fn parse(s: &str) -> Result<Id, String> {
        let ok_part = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        match s.split_once(':') {
            Some((ns, name)) if ok_part(ns) && ok_part(name) => Ok(Id(s.to_string())),
            _ => Err(format!("bad id '{s}' (want namespace:name, lowercase a-z 0-9 _)")),
        }
    }

    pub fn namespace(&self) -> &str {
        self.0.split_once(':').map(|(ns, _)| ns).unwrap_or("")
    }

    pub fn name(&self) -> &str {
        self.0.split_once(':').map(|(_, n)| n).unwrap_or(&self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Id {
    type Error = String;
    fn try_from(s: String) -> Result<Id, String> {
        Id::parse(&s)
    }
}

impl From<Id> for String {
    fn from(id: Id) -> String {
        id.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Index of an entry in its registry, stable for a given set of content (sorted by id).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Handle(pub u32);

/// The kinds of content, one folder each.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Legend,
    Ability,
    Passive,
    Weapon,
    AmmoType,
    Attachment,
    Consumable,
    Map,
}

impl Kind {
    pub const ALL: [Kind; 8] = [Kind::Legend, Kind::Ability, Kind::Passive, Kind::Weapon, Kind::AmmoType, Kind::Attachment, Kind::Consumable, Kind::Map];

    /// Folder name under `content/`.
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Legend => "legends",
            Kind::Ability => "abilities",
            Kind::Passive => "passives",
            Kind::Weapon => "weapons",
            Kind::AmmoType => "ammo",
            Kind::Attachment => "attachments",
            Kind::Consumable => "consumables",
            Kind::Map => "maps",
        }
    }
}

// ---------------------------------------------------------------- entry definitions
//
// Fields are the ones the engine will need first; everything else a file contains lands in `extra`. Keep new
// fields `#[serde(default)]` so older files and mods keep loading.

/// A playable character.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LegendDef {
    pub name: String,
    pub description: String,
    /// assault, skirmisher, recon, support, controller
    pub class: String,
    /// The game's player settings file this legend moves with, e.g. `pilot_survival_stim` (Octane). Empty = stock.
    pub settings: String,
    pub tactical: Option<Id>,
    pub ultimate: Option<Id>,
    pub passives: Vec<Id>,
    /// Body model asset path, e.g. `mdl/humans/pilots/...rmdl`
    pub model: String,
    pub replaces: Option<Id>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// A tactical or ultimate ability.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AbilityDef {
    pub name: String,
    pub description: String,
    /// tactical or ultimate
    pub slot: String,
    /// seconds per charge
    pub cooldown: f32,
    pub charges: u32,
    pub replaces: Option<Id>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// An always-on legend perk.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PassiveDef {
    pub name: String,
    pub description: String,
    pub replaces: Option<Id>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// A weapon (guns and melee).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WeaponDef {
    pub name: String,
    pub description: String,
    /// assault_rifle, smg, lmg, marksman, sniper, shotgun, pistol, melee
    pub class: String,
    /// The game's weapon class name, e.g. `mp_weapon_r97`
    pub game_class: String,
    pub ammo: Option<Id>,
    /// auto, semi, burst, single, charge
    pub fire_modes: Vec<String>,
    pub damage: f32,
    pub headshot_multiplier: f32,
    pub rpm: f32,
    pub magazine: u32,
    /// Attachment slot names this weapon has (barrel, mag, optic, stock, hopup)
    pub attachment_slots: Vec<String>,
    /// Move speed scale while this weapon is out (Apex: 1.0 for most guns; holstered/melee is ~1.15)
    pub move_speed_scale: Option<f32>,
    pub replaces: Option<Id>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// An ammo type.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AmmoTypeDef {
    pub name: String,
    /// rounds per inventory slot
    pub stack: u32,
    /// UI colour, "#rrggbb"
    pub color: String,
    pub replaces: Option<Id>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// A weapon attachment.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AttachmentDef {
    pub name: String,
    /// The weapon slot it goes in (must appear in a weapon's attachment_slots to fit it)
    pub slot: String,
    /// 1 common .. 4 legendary, 5 gold
    pub tier: u32,
    /// Weapons it fits, by id. Empty = every weapon with the slot.
    pub weapons: Vec<Id>,
    pub replaces: Option<Id>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// A consumable (heals, shield batteries, grenades).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConsumableDef {
    pub name: String,
    /// heal, shield, grenade, ...
    pub category: String,
    /// seconds
    pub use_time: f32,
    pub health: f32,
    pub shield: f32,
    pub stack: u32,
    pub replaces: Option<Id>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// A playable map.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MapDef {
    pub name: String,
    pub description: String,
    /// The game's map / BSP name, e.g. `mp_rr_canyonlands_staging_mu1`; also the asset folder under `maps/`
    pub bsp: String,
    /// Game modes / playlists it supports
    pub modes: Vec<String>,
    pub replaces: Option<Id>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Lets the loader handle every def type the same way.
pub trait Def: for<'de> Deserialize<'de> + Clone {
    const KIND: Kind;
    fn replaces(&self) -> Option<&Id>;
    fn display_name(&self) -> &str;
    /// Every id this entry points at, with the kind it must be.
    fn refs(&self) -> Vec<(Kind, &Id)> {
        Vec::new()
    }
}

macro_rules! def_impl {
    ($t:ty, $k:expr) => {
        impl Def for $t {
            const KIND: Kind = $k;
            fn replaces(&self) -> Option<&Id> {
                self.replaces.as_ref()
            }
            fn display_name(&self) -> &str {
                &self.name
            }
        }
    };
    ($t:ty, $k:expr, |$s:ident| $refs:expr) => {
        impl Def for $t {
            const KIND: Kind = $k;
            fn replaces(&self) -> Option<&Id> {
                self.replaces.as_ref()
            }
            fn display_name(&self) -> &str {
                &self.name
            }
            fn refs(&self) -> Vec<(Kind, &Id)> {
                let $s = self;
                $refs
            }
        }
    };
}

def_impl!(LegendDef, Kind::Legend, |s| {
    let mut r: Vec<(Kind, &Id)> = Vec::new();
    r.extend(s.tactical.iter().map(|i| (Kind::Ability, i)));
    r.extend(s.ultimate.iter().map(|i| (Kind::Ability, i)));
    r.extend(s.passives.iter().map(|i| (Kind::Passive, i)));
    r
});
def_impl!(AbilityDef, Kind::Ability);
def_impl!(PassiveDef, Kind::Passive);
def_impl!(WeaponDef, Kind::Weapon, |s| s.ammo.iter().map(|i| (Kind::AmmoType, i)).collect());
def_impl!(AmmoTypeDef, Kind::AmmoType);
def_impl!(AttachmentDef, Kind::Attachment, |s| s.weapons.iter().map(|i| (Kind::Weapon, i)).collect());
def_impl!(ConsumableDef, Kind::Consumable);
def_impl!(MapDef, Kind::Map);

// ---------------------------------------------------------------- one registry

/// Where an entry came from (for error messages and mod listings).
#[derive(Clone, Debug)]
pub struct Origin {
    pub namespace: String,
    pub file: PathBuf,
}

/// All entries of one kind.
#[derive(Clone, Debug)]
pub struct Registry<T> {
    entries: Vec<(Id, T, Origin)>,
    index: HashMap<Id, Handle>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Registry { entries: Vec::new(), index: HashMap::new() }
    }
}

impl<T: Def> Registry<T> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn handle(&self, id: &Id) -> Option<Handle> {
        self.index.get(id).copied()
    }

    pub fn get(&self, id: &Id) -> Option<&T> {
        self.handle(id).map(|h| &self.entries[h.0 as usize].1)
    }

    pub fn by_handle(&self, h: Handle) -> Option<&T> {
        self.entries.get(h.0 as usize).map(|e| &e.1)
    }

    pub fn id_of(&self, h: Handle) -> Option<&Id> {
        self.entries.get(h.0 as usize).map(|e| &e.0)
    }

    pub fn origin(&self, id: &Id) -> Option<&Origin> {
        self.handle(id).map(|h| &self.entries[h.0 as usize].2)
    }

    /// (handle, id, def) in handle order.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &Id, &T)> {
        self.entries.iter().enumerate().map(|(i, (id, d, _))| (Handle(i as u32), id, d))
    }

    /// Adds or replaces an entry while loading; handles are assigned by `finish`.
    fn put(&mut self, id: Id, def: T, origin: Origin, errors: &mut Vec<String>) {
        if let Some(target) = def.replaces().cloned() {
            match self.entries.iter().position(|e| e.0 == target) {
                Some(i) => {
                    // the replacement keeps the original id, so everything that points at it now gets the mod's data
                    self.entries[i] = (target, def, origin);
                }
                None => errors.push(format!("{}: replaces {target}, which doesn't exist (yet: check mod priority)", origin.file.display())),
            }
            return;
        }
        if let Some(prev) = self.entries.iter().find(|e| e.0 == id) {
            errors.push(format!("{}: {} {id} already defined in {} (use `replaces` to override it)", origin.file.display(), T::KIND.dir(), prev.2.file.display()));
            return;
        }
        self.entries.push((id, def, origin));
    }

    fn finish(&mut self) {
        self.entries.sort_by(|a, b| a.0.cmp(&b.0));
        self.index = self.entries.iter().enumerate().map(|(i, e)| (e.0.clone(), Handle(i as u32))).collect();
    }
}

// ---------------------------------------------------------------- all content + loading

/// A folder of content to load: the base game or one mod.
#[derive(Clone, Debug)]
pub struct Source {
    pub namespace: String,
    pub name: String,
    pub version: String,
    /// the folder holding `<kind>/<name>.toml`
    pub content_dir: PathBuf,
    /// lower loads first; a mod that replaces another mod's entry needs a higher priority
    pub priority: i32,
    pub enabled: bool,
}

/// `mods/<folder>/mod.toml`
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ModManifest {
    id: String,
    name: String,
    version: String,
    enabled: Option<bool>,
    priority: i32,
}

/// Every registry, plus what was loaded and what went wrong.
#[derive(Clone, Debug, Default)]
pub struct Content {
    pub legends: Registry<LegendDef>,
    pub abilities: Registry<AbilityDef>,
    pub passives: Registry<PassiveDef>,
    pub weapons: Registry<WeaponDef>,
    pub ammo: Registry<AmmoTypeDef>,
    pub attachments: Registry<AttachmentDef>,
    pub consumables: Registry<ConsumableDef>,
    pub maps: Registry<MapDef>,
    /// the sources that were loaded, in load order
    pub sources: Vec<Source>,
    /// parse errors, duplicate ids, broken references (content still loads; bad entries are skipped)
    pub errors: Vec<String>,
}

impl Content {
    /// The base `content/` folder and every enabled mod under `mods/`, found in the working directory, next to the exe
    /// or two folders up from it (like `r5rust.toml`).
    pub fn load_default() -> Content {
        let root = find_root().unwrap_or_else(|| PathBuf::from("."));
        let mut sources = discover(&root, false);
        sources.retain(|s| s.enabled);
        Content::load(&sources)
    }

    /// Loads the given sources in priority order and checks references.
    pub fn load(sources: &[Source]) -> Content {
        let mut c = Content::default();
        let mut order: Vec<&Source> = sources.iter().collect();
        order.sort_by_key(|s| (s.priority, s.namespace != "core"));
        for s in order {
            c.load_source(s);
            c.sources.push(s.clone());
        }
        c.legends.finish();
        c.abilities.finish();
        c.passives.finish();
        c.weapons.finish();
        c.ammo.finish();
        c.attachments.finish();
        c.consumables.finish();
        c.maps.finish();
        c.check_refs();
        c
    }

    fn load_source(&mut self, s: &Source) {
        let errors = &mut self.errors;
        load_kind(s, &mut self.legends, errors);
        load_kind(s, &mut self.abilities, errors);
        load_kind(s, &mut self.passives, errors);
        load_kind(s, &mut self.weapons, errors);
        load_kind(s, &mut self.ammo, errors);
        load_kind(s, &mut self.attachments, errors);
        load_kind(s, &mut self.consumables, errors);
        load_kind(s, &mut self.maps, errors);
    }

    fn has(&self, kind: Kind, id: &Id) -> bool {
        match kind {
            Kind::Legend => self.legends.handle(id).is_some(),
            Kind::Ability => self.abilities.handle(id).is_some(),
            Kind::Passive => self.passives.handle(id).is_some(),
            Kind::Weapon => self.weapons.handle(id).is_some(),
            Kind::AmmoType => self.ammo.handle(id).is_some(),
            Kind::Attachment => self.attachments.handle(id).is_some(),
            Kind::Consumable => self.consumables.handle(id).is_some(),
            Kind::Map => self.maps.handle(id).is_some(),
        }
    }

    fn check_refs(&mut self) {
        let mut errs = Vec::new();
        macro_rules! check {
            ($reg:expr) => {
                for (_, id, d) in $reg.iter() {
                    for (kind, r) in d.refs() {
                        if !self.has(kind, r) {
                            errs.push(format!("{id}: points at {} {r}, which doesn't exist", kind.dir()));
                        }
                    }
                }
            };
        }
        check!(self.legends);
        check!(self.abilities);
        check!(self.passives);
        check!(self.weapons);
        check!(self.ammo);
        check!(self.attachments);
        check!(self.consumables);
        check!(self.maps);
        // attachments must name a slot some weapon has
        for (_, id, a) in self.attachments.iter() {
            if !a.slot.is_empty() && !self.weapons.is_empty() && !self.weapons.iter().any(|(_, _, w)| w.attachment_slots.iter().any(|s| s == &a.slot)) {
                errs.push(format!("{id}: slot '{}' isn't on any weapon", a.slot));
            }
        }
        self.errors.extend(errs);
    }

    /// The map whose `bsp` is `bsp_name` (the old config names maps by BSP).
    pub fn map_by_bsp(&self, bsp_name: &str) -> Option<(Handle, &Id, &MapDef)> {
        self.maps.iter().find(|(_, _, m)| m.bsp.eq_ignore_ascii_case(bsp_name))
    }

    /// One line per kind, e.g. for the log.
    pub fn summary(&self) -> String {
        let counts: Vec<String> = [
            ("legends", self.legends.len()),
            ("abilities", self.abilities.len()),
            ("passives", self.passives.len()),
            ("weapons", self.weapons.len()),
            ("ammo", self.ammo.len()),
            ("attachments", self.attachments.len()),
            ("consumables", self.consumables.len()),
            ("maps", self.maps.len()),
        ]
        .iter()
        .map(|(k, n)| format!("{n} {k}"))
        .collect();
        let mods: Vec<&str> = self.sources.iter().filter(|s| s.namespace != "core").map(|s| s.namespace.as_str()).collect();
        format!("{} | mods: {} | {} errors", counts.join(", "), if mods.is_empty() { "none".into() } else { mods.join(", ") }, self.errors.len())
    }
}

fn load_kind<T: Def>(s: &Source, reg: &mut Registry<T>, errors: &mut Vec<String>) {
    let dir = s.content_dir.join(T::KIND.dir());
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    files.sort();
    for f in files {
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let id = match Id::new(&s.namespace, stem) {
            Ok(i) => i,
            Err(e) => {
                errors.push(format!("{}: {e} (rename the file)", f.display()));
                continue;
            }
        };
        let text = match std::fs::read_to_string(&f) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("{}: {e}", f.display()));
                continue;
            }
        };
        // ids inside the file may leave out the namespace: resolve them against this source's
        let text = qualify_ids(&text, &s.namespace);
        match toml::from_str::<T>(&text) {
            Ok(def) => reg.put(id, def, Origin { namespace: s.namespace.clone(), file: f.clone() }, errors),
            Err(e) => errors.push(format!("{}: {e}", f.display())),
        }
    }
}

/// Fields that hold ids; a bare `"name"` in them becomes `"<namespace>:name"`.
const ID_FIELDS: [&str; 7] = ["tactical", "ultimate", "passives", "ammo", "weapons", "replaces", "legends"];

fn qualify_ids(text: &str, ns: &str) -> String {
    let Ok(mut table) = toml::from_str::<toml::Table>(text) else { return text.to_string() };
    let fix = |v: &mut toml::Value| {
        if let toml::Value::String(s) = v {
            if !s.contains(':') {
                *s = format!("{ns}:{s}");
            }
        }
    };
    for f in ID_FIELDS {
        match table.get_mut(f) {
            Some(toml::Value::Array(a)) => a.iter_mut().for_each(fix),
            Some(v) => fix(v),
            None => {}
        }
    }
    toml::to_string(&table).unwrap_or_else(|_| text.to_string())
}

/// The folder that holds `content/` (working dir, next to the exe, or two up from it).
pub fn find_root() -> Option<PathBuf> {
    let mut cands = vec![PathBuf::from(".")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            cands.push(d.to_path_buf());
            cands.push(d.join("../.."));
        }
    }
    cands.into_iter().find(|c| c.join("content").is_dir())
}

/// The base game plus every mod folder under `<root>/mods` (disabled ones included, marked).
pub fn discover(root: &Path, _include_disabled: bool) -> Vec<Source> {
    let mut out = vec![Source {
        namespace: "core".into(),
        name: "R5Rust".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        content_dir: root.join("content"),
        priority: i32::MIN,
        enabled: true,
    }];
    let Ok(rd) = std::fs::read_dir(root.join("mods")) else { return out };
    let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("mod.toml").is_file()).collect();
    dirs.sort();
    for d in dirs {
        let m: ModManifest = std::fs::read_to_string(d.join("mod.toml")).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
        let ns = if m.id.is_empty() { d.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase().replace('-', "_") } else { m.id.clone() };
        if ns == "core" {
            log::warn!("mod {} uses the reserved id 'core'; skipped", d.display());
            continue;
        }
        out.push(Source {
            namespace: ns,
            name: if m.name.is_empty() { d.file_name().and_then(|n| n.to_str()).unwrap_or("").into() } else { m.name },
            version: m.version,
            content_dir: d.join("content"),
            priority: m.priority,
            enabled: m.enabled.unwrap_or(true),
        });
    }
    out
}

/// Content counts per kind for one source (for mod lists).
pub fn count_files(s: &Source) -> BTreeMap<&'static str, usize> {
    Kind::ALL
        .iter()
        .map(|k| (k.dir(), std::fs::read_dir(s.content_dir.join(k.dir())).map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "toml")).count()).unwrap_or(0)))
        .collect()
}
