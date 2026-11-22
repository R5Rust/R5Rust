//! Loot / inventory check against the real retail data (docs/inventory.md). Needs `<assets>/data/loot/`
//! (tools/build_loot.py, install step 10) and the map's `bsp/*_script.ent`.
//!     cargo run --release -p shared --example lootcheck

use shared::assets::Config;
use shared::loot::db::Kind;
use shared::loot::inventory::{armor_shield, GroundView, Hands, Inventory, UseKind, Weapon};
use shared::loot::{range, LootDb};

struct Check {
    pass: u32,
    fail: u32,
}

impl Check {
    fn t(&mut self, name: &str, ok: bool, detail: impl std::fmt::Display) {
        if ok {
            self.pass += 1;
            println!("  ok    {name}: {detail}");
        } else {
            self.fail += 1;
            println!("  FAIL  {name}: {detail}");
        }
    }
}

fn main() {
    let cfg = Config::load();
    let db = LootDb::load(&cfg.assets_root().join("data").join("loot")).expect("loot data (install step 10)");
    let mut c = Check { pass: 0, fail: 0 };
    let id = |r: &str| db.get(r).unwrap_or_else(|| panic!("no {r}"));

    println!("catalogue");
    let ammo = id("bullet");
    c.t("light ammo 20 / 60", db.item(ammo).per_drop == 20 && db.item(ammo).stack == 60, format!("{} / {}", db.item(ammo).per_drop, db.item(ammo).stack));
    let r99 = id("mp_weapon_r97");
    c.t("R-99 mag 18, light", db.clip_size(r99, []) == 18 && db.ammo_of(r99) == Some(ammo), db.clip_size(r99, []));
    c.t("R-99 + purple light mag = 27", db.clip_size(r99, ["bullets_mag_l3"]) == 27, db.clip_size(r99, ["bullets_mag_l3"]));
    let hcog = id("optic_cq_hcog_classic");
    let hmag = id("highcal_mag_l1");
    c.t("hcog fits the R-99, heavy mag doesn't", db.fits(hcog, r99) && !db.fits(hmag, r99), "");
    let ord = db.group_refs("ordnance_drops");
    c.t("ordnance group = 3 grenades", ord.len() == 3, format!("{ord:?}"));

    println!("firing range layout");
    let ents: Vec<_> = std::fs::read_dir(cfg.map_dir().join("bsp"))
        .expect("map bsp dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with("_script.ent"))
        .flat_map(|p| range::parse_ents(&std::fs::read_to_string(p).unwrap_or_default()))
        .collect();
    let racks = range::rack_props(&std::fs::read_to_string(cfg.map_dir().join("props.json")).unwrap_or_default());
    c.t("6 rack props in retail's map", racks.len() == 6, format!("{racks:?}"));
    let spawns = range::firing_range(&db, &ents, &racks);
    let weapons = spawns.iter().filter(|s| db.item(s.item).kind == Kind::MainWeapon).count();
    let by = |k: Kind| spawns.iter().filter(|s| db.item(s.item).kind == k).count();
    println!(
        "  {} spawns: {weapons} weapons, {} attachments, {} ammo, {} health, {} armor, {} backpacks, {} knockdowns, {} grenades",
        spawns.len(),
        by(Kind::Attachment),
        by(Kind::Ammo),
        by(Kind::Health),
        by(Kind::Armor),
        by(Kind::Backpack),
        by(Kind::IncapShield),
        by(Kind::Ordnance)
    );
    c.t("every range weapon on a rack", weapons >= 25, weapons);
    c.t("health rows: medkit + 2x2 syringes per row", spawns.iter().any(|s| s.item == id("health_pickup_health_small") && s.count == 2), "");
    c.t("weapons near the racks are clip 500", spawns.iter().filter(|s| db.item(s.item).kind == Kind::MainWeapon).all(|s| s.clip == Some(500)), "");

    println!("inventory");
    let mut inv = Inventory::firing_range_spawn(&db);
    c.t(
        "spawn kit: white helmet/armor/knockdown, 50 shield, 2 syringes + 2 cells, no weapons",
        inv.helmet.is_some() && inv.armor.is_some() && inv.incap.is_some() && inv.shield == 50 && inv.backpack.len() == 2 && inv.weapons.iter().all(Option::is_none) && inv.hands == Hands::Melee,
        format!("{:?}", inv.backpack),
    );
    let rack = Weapon::new(&db, r99).with_clip(&db, Some(500));
    let p = inv.pickup(&db, &GroundView { item: r99, count: 1, weapon: Some(&rack), shield: None }, UseKind::Tap);
    c.t("R-99 off a rack -> slot 0, full mag, in hand", p.switch_to == Some(0) && inv.weapons[0].as_ref().is_some_and(|w| w.clip == 18) && inv.hands == Hands::Weapon(0), format!("{p:?}"));
    let p = inv.pickup(&db, &GroundView { item: ammo, count: 20, weapon: None, shield: None }, UseKind::Tap);
    c.t("20 light ammo -> backpack, reserve 20", p.taken == 20 && inv.reserve(&db, 0) == Some(20), inv.reserve(&db, 0).unwrap_or(0));
    inv.weapons[0].as_mut().unwrap().clip = 3;
    let got = inv.reload(&db, 0);
    c.t("reload 3 -> 18 takes 15 from the pool", got == 15 && inv.reserve(&db, 0) == Some(5), format!("{got}, reserve {:?}", inv.reserve(&db, 0)));
    let mag = id("bullets_mag_l2");
    let p = inv.pickup(&db, &GroundView { item: mag, count: 1, weapon: None, shield: None }, UseKind::Tap);
    c.t("blue light mag attaches, mag max 24", p.taken == 1 && inv.weapons[0].as_ref().unwrap().clip_size(&db) == 24, format!("{p:?}"));
    let p = inv.pickup(&db, &GroundView { item: id("bullets_mag_l1"), count: 1, weapon: None, shield: None }, UseKind::Tap);
    c.t("white mag is no upgrade: refused", p.taken == 0 && p.message.is_some(), format!("{:?}", p.message));
    let p = inv.pickup(&db, &GroundView { item: id("bullets_mag_l1"), count: 1, weapon: None, shield: None }, UseKind::Hold);
    c.t("held: white mag into the backpack", p.taken == 1 && inv.count(id("bullets_mag_l1")) == 1, "");
    let flat = id("mp_weapon_vinson");
    let p = inv.pickup(&db, &GroundView { item: flat, count: 1, weapon: None, shield: None }, UseKind::Tap);
    c.t("Flatline -> empty slot 1", p.switch_to == Some(1) && inv.weapons[1].is_some(), "");
    let alt = id("mp_weapon_alternator_smg");
    let p = inv.pickup(&db, &GroundView { item: alt, count: 1, weapon: None, shield: None }, UseKind::Tap);
    c.t("third gun on a tap: needs a hold", p.needs_hold && p.taken == 0, format!("{p:?}"));
    inv.hands = Hands::Weapon(0);
    let p = inv.pickup(&db, &GroundView { item: alt, count: 1, weapon: None, shield: None }, UseKind::Hold);
    let alt_mag = inv.weapons[0].as_ref().and_then(|w| w.atts.get("mag").copied());
    c.t(
        "held swap: the R-99 drops, its blue light mag moves to the Alternator",
        p.drops.first().is_some_and(|d| d.item == r99) && inv.weapons[0].as_ref().is_some_and(|w| w.item == alt) && alt_mag == Some(mag),
        format!("drops {:?}", p.drops.iter().map(|d| db.item(d.item).r#ref.clone()).collect::<Vec<_>>()),
    );
    let a3 = id("armor_pickup_lv3");
    let p = inv.pickup(&db, &GroundView { item: a3, count: 1, weapon: None, shield: None }, UseKind::Tap);
    c.t("purple armor replaces white: 100 shield, the white drops with its 50", inv.shield == armor_shield(3) && p.drops.first().is_some_and(|d| d.shield == Some(50)), format!("{p:?}"));
    let p = inv.pickup(&db, &GroundView { item: id("armor_pickup_lv1"), count: 1, weapon: None, shield: Some(50) }, UseKind::Tap);
    c.t("white armor after purple: refused", p.taken == 0 && p.message.is_some(), format!("{:?}", p.message));
    // backpack full: 10 slots
    let syr = id("health_pickup_health_small");
    for _ in 0..12 {
        inv.pickup(&db, &GroundView { item: id("shotgun"), count: 10, weapon: None, shield: None }, UseKind::Tap);
    }
    let used = inv.backpack.len();
    let p = inv.pickup(&db, &GroundView { item: syr, count: 6, weapon: None, shield: None }, UseKind::Tap);
    c.t("10-slot backpack fills; extra syringes only top up the partial stack", used == 10 && p.taken <= 4, format!("{used} slots, took {}", p.taken));
    let drops = inv.drop_weapon(&db, 0);
    c.t("dropping the Alternator pops its mag off as its own item", drops[0].item == alt && drops.iter().any(|d| d.item == mag && d.weapon.is_none()), format!("{:?}", drops.iter().map(|d| db.item(d.item).r#ref.clone()).collect::<Vec<_>>()));

    println!("\n{} passed, {} failed", c.pass, c.fail);
    if c.fail > 0 {
        std::process::exit(1);
    }
}
