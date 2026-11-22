//! The Firing Range's loot, laid out like Flowstate's staging area (`vs\mp\_staging_area.gnut`, docs/inventory.md
//! section 0) from the retail map's markers in `<map>/bsp/*_script.ent`: CLootV4 rows between `cloot4_<cat>_start` /
//! `_end` info_targets, weapon racks and `ammo_<type>` info_targets.
//!
//! Racks: retail's map places them as static props (`mdl/canyonlands/firing_range_weapon_rack.rmdl`, 6 at x -7228)
//! with each type's ammo markers on the shelf in front of its rack (2 at ~88 u + a 2x2 grid on the floor), so a rack's
//! ammo type is its nearest ammo marker's (within 110 u; none = the bow / no-pool rack), and the map's own ammo
//! markers and grenade row stand in for S21's 6 generated ammo boxes + grenades per rack. Without rack props (the S21
//! map) the `weapon_racks` info_targets and the S21 rules are used. (The 7 `info_survival_weapon_location` script_refs
//! in retail's map sit on the dropship: not racks.)

use std::collections::HashMap;

use glam::Vec3;

use super::db::{ItemId, Kind, LootDb};

#[derive(Clone, Debug, PartialEq)]
pub struct RangeSpawn {
    pub item: ItemId,
    pub count: u32,
    pub pos: Vec3,
    /// Source angles (pitch, yaw, roll), degrees
    pub angles: Vec3,
    /// weapons: the stored clip (racks: 500 = "full", capped at the mag size on pickup)
    pub clip: Option<u32>,
}

/// `{ "key" "value" ... }` blocks of an .ent file.
pub fn parse_ents(text: &str) -> Vec<HashMap<String, String>> {
    let mut out = Vec::new();
    let mut cur: Option<HashMap<String, String>> = None;
    for line in text.lines() {
        let l = line.trim();
        if l == "{" {
            cur = Some(HashMap::new());
        } else if l == "}" {
            if let Some(c) = cur.take() {
                out.push(c);
            }
        } else if let Some(c) = cur.as_mut() {
            let q: Vec<&str> = l.split('"').collect();
            if q.len() >= 4 {
                c.insert(q[1].to_owned(), q[3].to_owned());
            }
        }
    }
    out
}

fn vec3(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    Vec3::new(*v.first().unwrap_or(&0.0), *v.get(1).unwrap_or(&0.0), *v.get(2).unwrap_or(&0.0))
}

/// Source AngleVectors for (pitch, yaw, 0): forward, right, up.
/// Source angles (pitch, yaw, roll degrees) as a rotation: yaw about Z, then pitch about Y (positive = nose down),
/// then roll about X.
fn angles_quat(a: Vec3) -> glam::Quat {
    glam::Quat::from_rotation_z(a.y.to_radians()) * glam::Quat::from_rotation_y(a.x.to_radians()) * glam::Quat::from_rotation_x(a.z.to_radians())
}

/// `AnglesCompose( a, b )`: b applied in a's local frame, back to Source angles (MatrixAngles).
pub fn angles_compose(a: Vec3, b: Vec3) -> Vec3 {
    let m = glam::Mat3::from_quat(angles_quat(a) * angles_quat(b));
    let (f, l, u) = (m.x_axis, m.y_axis, m.z_axis);
    let xy = (f.x * f.x + f.y * f.y).sqrt();
    if xy > 0.001 {
        Vec3::new((-f.z).atan2(xy).to_degrees(), f.y.atan2(f.x).to_degrees(), l.z.atan2(u.z).to_degrees())
    } else {
        Vec3::new((-f.z).atan2(xy).to_degrees(), (-l.x).atan2(l.y).to_degrees(), 0.0)
    }
}

fn basis(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (p, y) = (angles.x.to_radians(), angles.y.to_radians());
    let f = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
    let r = Vec3::new(y.sin(), -y.cos(), 0.0);
    (f, r, r.cross(f).normalize_or(Vec3::Z))
}

const RACK_SPACING: f32 = 32.0;

/// Which rack a weapon goes on (`Staging_RackAmmoType`): crate / tier 5 -> none, else its ammo pool, no pool -> none.
fn rack_ammo(db: &LootDb, w: ItemId) -> String {
    let it = db.item(w);
    if it.base_mods.iter().any(|m| m == "crate") || it.tier >= 5 {
        return "_none".into();
    }
    db.weapon_info(w).map(|i| i.pool.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| "_none".into())
}

/// The racks a map places as props (props.json instances of `firing_range_weapon_rack.rmdl`): (position, yaw).
pub type RackProp = (Vec3, f32);

/// Rack props from the exporter's props.json (`variants[].model`, `instances[] = [variant, x, y, z, qx, qy, qz, qw,
/// scale, ...]`).
pub fn rack_props(props_json: &str) -> Vec<RackProp> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(props_json) else { return Vec::new() };
    let racks: Vec<usize> = v["variants"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, x)| x["model"].as_str().is_some_and(|m| m.ends_with("/firing_range_weapon_rack.rmdl")))
        .map(|(i, _)| i)
        .collect();
    v["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let a = x.as_array()?;
            let f = |i: usize| a.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            if !racks.contains(&(f(0) as usize)) {
                return None;
            }
            let (qx, qy, qz, qw) = (f(4), f(5), f(6), f(7));
            let yaw = (2.0 * (qw * qz + qx * qy)).atan2(1.0 - 2.0 * (qy * qy + qz * qz)).to_degrees();
            Some((Vec3::new(f(1), f(2), f(3)), yaw))
        })
        .collect()
}

pub fn firing_range(db: &LootDb, ents: &[HashMap<String, String>], rack_props: &[RackProp]) -> Vec<RangeSpawn> {
    let mut out = Vec::new();
    let by_guid: HashMap<&str, &HashMap<String, String>> =
        ents.iter().filter_map(|e| e.get("link_guid").map(|g| (g.as_str(), e))).collect();
    let refs = |names: &[&str]| -> Vec<ItemId> { names.iter().filter_map(|r| db.get(r)).filter(|&i| db.spawnable(i)).collect() };
    let attach = |sub: &str| -> Vec<ItemId> { db.refs_of(|i| i.kind == Kind::Attachment && i.r#ref.contains(sub)) };

    // ---- the rows' contents (CLootV4_Create_AllStuff_ForRealm)
    let mut rows: HashMap<&str, Vec<(ItemId, u32)>> = HashMap::new();
    let ones = |v: Vec<ItemId>| v.into_iter().map(|i| (i, 1)).collect::<Vec<_>>();
    let mut cq = attach("optic_cq");
    let mut adv = attach("optic_ranged");
    if let Some(digi) = cq.pop() {
        adv.push(digi);
    }
    rows.insert("scopes_basic", ones(cq));
    rows.insert("scopes_advanced", ones(adv));
    rows.insert("scopes_sniper", ones(attach("optic_sniper")));
    rows.insert("mags_light", ones(attach("bullets_mag")));
    rows.insert("mags_heavy", ones(attach("highcal_mag")));
    rows.insert("mags_energy", ones(attach("energy_mag")));
    rows.insert("mags_sniper", ones(attach("sniper_mag")));
    rows.insert("stocks_basic", ones(attach("stock_tactical")));
    rows.insert("stocks_sniper", ones(attach("stock_sniper")));
    rows.insert("barrels", ones(attach("barrel_stabilizer")));
    rows.insert("lasers", ones(attach("laser_sight")));
    rows.insert("shotgunbolts", ones(attach("shotgun_bolt")));
    let mut hop: Vec<String> = db.group_refs("attachments_hopups_only");
    for r in db.group_refs("corrupted_attachments") {
        if !hop.contains(&r) {
            hop.push(r);
        }
    }
    let hop: Vec<&str> = hop.iter().map(String::as_str).collect();
    rows.insert("hopups", ones(refs(&hop)));
    rows.insert("bodyarmor", ones(refs(&["armor_pickup_lv1", "armor_pickup_lv2", "armor_pickup_lv3", "armor_pickup_lv4_all_fast"])));
    rows.insert(
        "evoarmor",
        ones(refs(&["armor_pickup_lv1_evolving", "armor_pickup_lv2_evolving", "armor_pickup_lv3_evolving", "armor_pickup_lv4_all_fast", "armor_pickup_lv5_evolving"])),
    );
    rows.insert("helmets", Vec::new());
    let first = |mut v: Vec<ItemId>, r: &str| {
        if let Some(f) = db.get(r) {
            v.retain(|&i| i != f);
            v.insert(0, f);
        }
        v
    };
    rows.insert("incaps", ones(first(db.refs_of(|i| i.kind == Kind::IncapShield), "incapshield_pickup_lv1")));
    let ord: Vec<String> = db.group_refs("ordnance_drops");
    rows.insert("ordinances", ones(refs(&ord.iter().map(String::as_str).collect::<Vec<_>>())));
    let id = |r: &str| db.get(r);
    let fixed = |list: &[(&str, u32)]| list.iter().filter_map(|&(r, n)| id(r).map(|i| (i, n))).collect::<Vec<_>>();
    rows.insert("batts", fixed(&[("health_pickup_combo_full", 1), ("health_pickup_combo_large", 1), ("health_pickup_combo_small", 1), ("health_pickup_combo_small", 1)]));
    rows.insert("health", fixed(&[("health_pickup_health_large", 1), ("health_pickup_health_small", 2), ("health_pickup_health_small", 2)]));
    rows.insert("specialgear", fixed(&[("health_pickup_ultimate", 1), ("mp_ability_redeploy_balloon", 1)]));
    rows.insert("backpacks", ones(first(db.refs_of(|i| i.kind == Kind::Backpack), "backpack_pickup_lv1")));

    // ---- lay each row out between its start and end (CLootV4_SpawnLootRow_ForRealm)
    for e in ents {
        let Some(name) = e.get("script_name") else { continue };
        let Some(cat) = name.strip_prefix("cloot4_").and_then(|n| n.strip_suffix("_start")) else { continue };
        let Some(list) = rows.get(cat).filter(|l| !l.is_empty()) else { continue };
        let Some(end) = e.get("link_to_guid_0").and_then(|g| by_guid.get(g.as_str())) else { continue };
        let a = vec3(&e["origin"]);
        let b = vec3(end.get("origin").map(String::as_str).unwrap_or(""));
        let width = (b - a).length();
        let dir = (b - a).normalize_or(Vec3::X);
        let (step, mut pos) = if list.len() >= 2 { (width / (list.len() - 1) as f32, a) } else { (width, a + dir * width / 2.0) };
        let yaw = vec3(e.get("angles").map(String::as_str).unwrap_or("")).y;
        for &(item, count) in list {
            out.push(RangeSpawn { item, count, pos, angles: Vec3::new(0.0, yaw, 0.0), clip: None });
            pos += dir * step;
        }
    }

    // ---- weapon racks (CreateWeaponRacksForRealm / CreateWeaponRacksForLoot)
    let retail_rows = ents.iter().any(|e| e.get("script_name").is_some_and(|n| n.starts_with("guns_")));
    let mut groups: Vec<(String, Vec<ItemId>)> = Vec::new();
    for r in db.group_refs("firing_range_all_weapons") {
        let Some(w) = db.get(&r).filter(|&w| db.spawnable(w)) else { continue };
        let mut t = rack_ammo(db, w);
        if (!rack_props.is_empty() || retail_rows) && t == "arrows" {
            // retail has no arrows rack: the bow shares the no-pool one
            t = "_none".into();
        }
        match groups.iter_mut().find(|(g, _)| *g == t) {
            Some((_, v)) => v.push(w),
            None => groups.push((t, vec![w])),
        }
    }
    groups.sort_by_key(|(t, _)| t == "_none");
    let place = |out: &mut Vec<RangeSpawn>, weapons: &[ItemId], org: Vec3, yaw: f32| {
        let (fwd, right, up) = basis(Vec3::new(0.0, yaw, 0.0));
        let start = org + right * RACK_SPACING * -3.0;
        for (i, &w) in weapons.iter().enumerate() {
            let rack = start + right * (RACK_SPACING * i as f32);
            // lootAngles = AnglesCompose( rackAngles, <-85, 180, 0> ): standing nearly upright in the rack
            out.push(RangeSpawn { item: w, count: 1, pos: rack + fwd * 2.0 + up * 47.0, angles: Vec3::new(-85.0, yaw + 180.0, 0.0), clip: Some(500) });
        }
        (start, fwd, right)
    };
    // Retail: each ammo type's guns lie along a `guns_<type>_start` -> `_end` line on the rack's top (`guns__none` for
    // no pool), spread evenly like a CLootV4 row, at the start node's angles (pitch -35: tilted up across the rack).
    let gun_rows: Vec<(&str, &HashMap<String, String>)> = ents
        .iter()
        .filter_map(|e| Some((e.get("script_name")?.strip_prefix("guns_")?.strip_suffix("_start")?, e)))
        .collect();
    if !gun_rows.is_empty() {
        for (ty, e) in gun_rows {
            let Some((_, weapons)) = groups.iter().find(|(g, _)| g == ty) else { continue };
            let Some(end) = e.get("link_to_guid_0").and_then(|g| by_guid.get(g.as_str())) else { continue };
            let a = vec3(&e["origin"]);
            let b = vec3(end.get("origin").map(String::as_str).unwrap_or(""));
            // defaultAngles = <0, 0, -90> + startNode.GetAngles(): on its side on the slope; the bow and the
            // throwing knife AnglesCompose <85, 0, 0> on top (CLootV2_Spawn_WeaponRowAndAmmo_ForRealm)
            let angles = vec3(e.get("angles").map(String::as_str).unwrap_or("")) + Vec3::new(0.0, 0.0, -90.0);
            let n = weapons.len();
            for (i, &w) in weapons.iter().enumerate() {
                let t = if n >= 2 { i as f32 / (n - 1) as f32 } else { 0.5 };
                let r = db.item(w).r#ref.as_str();
                let angles = if matches!(r, "mp_weapon_bow" | "mp_weapon_bow_crate" | "mp_weapon_throwingknife") {
                    angles_compose(angles, Vec3::new(85.0, 0.0, 0.0))
                } else {
                    angles
                };
                out.push(RangeSpawn { item: w, count: 1, pos: a.lerp(b, t), angles, clip: Some(500) });
            }
        }
    } else if !rack_props.is_empty() {
        let markers: Vec<(String, Vec3)> = ents
            .iter()
            .filter_map(|e| Some((e.get("script_name")?.strip_prefix("ammo_")?.to_owned(), vec3(e.get("origin")?))))
            .collect();
        for &(pos, yaw) in rack_props {
            let ty = markers
                .iter()
                .map(|(t, m)| (t, (*m - pos).length()))
                .filter(|(_, d)| *d < 110.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map_or("_none".to_owned(), |(t, _)| t.clone());
            if let Some((_, weapons)) = groups.iter().find(|(g, _)| *g == ty) {
                place(&mut out, weapons, pos, yaw);
            }
        }
    } else {
        let mut nodes: Vec<&HashMap<String, String>> = ents.iter().filter(|e| e.get("script_name").is_some_and(|n| n == "weapon_racks")).collect();
        // ArrayClosest( nodes, <0,0,0> ). UNSURE: S21 iterated a squirrel table for the ammo type -> node order
        nodes.sort_by(|x, y| vec3(&x["origin"]).length().total_cmp(&vec3(&y["origin"]).length()));
        let ordnance: Vec<ItemId> = db.refs_of(|i| i.kind == Kind::Ordnance && !i.r#ref.contains("noncombat"));
        for ((ammo, weapons), node) in groups.iter().zip(nodes) {
            let org = vec3(&node["origin"]);
            let yaw = vec3(node.get("angles").map(String::as_str).unwrap_or("")).y;
            let (start, fwd, mut right) = place(&mut out, weapons, org, yaw);
            // 6 ammo boxes per rack line (not energy / none); the bow's line turns the other way
            if ammo != "_none" && ammo != "special" {
                if let Some(a) = db.get(ammo).filter(|&a| db.spawnable(a)) {
                    if ammo == "arrows" {
                        right = -right;
                    }
                    let base = start + right * (RACK_SPACING * weapons.len() as f32 - 5.0);
                    for i in 0..6 {
                        let pos = base + fwd * ((i % 3) as f32 * 16.0) + right * ((i / 3) as f32 * 25.0);
                        out.push(RangeSpawn { item: a, count: db.item(a).per_drop, pos, angles: Vec3::new(0.0, 90.0, 0.0), clip: None });
                    }
                    // every grenade, two each, after the line (CreateOrdnanceForRealmNode)
                    let ob = start + right * (RACK_SPACING * weapons.len() as f32 + 45.0);
                    for (i, &o) in ordnance.iter().enumerate() {
                        for col in 0..2 {
                            let pos = ob + fwd * (15.0 * i as f32) + right * (10.0 * col as f32) + Vec3::new(0.0, 0.0, 3.0);
                            out.push(RangeSpawn { item: o, count: 1, pos, angles: Vec3::ZERO, clip: None });
                        }
                    }
                }
            }
        }
    }

    // ---- ammo_<type> info_targets: countPerDrop each
    for e in ents {
        let Some(t) = e.get("script_name").and_then(|n| n.strip_prefix("ammo_")) else { continue };
        let Some(a) = db.get(t).filter(|&a| db.spawnable(a) && db.item(a).kind == Kind::Ammo) else { continue };
        let ang = vec3(e.get("angles").map(String::as_str).unwrap_or(""));
        out.push(RangeSpawn { item: a, count: db.item(a).per_drop, pos: vec3(&e["origin"]), angles: ang, clip: None });
    }
    out
}
