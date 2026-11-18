//! Lists everything in the content registry and checks it: parse errors, duplicate ids, broken references.
//! cargo run --release -p shared --example content [-- --all]     (--all also loads disabled mods, e.g. example-mod)
//! Exit code 1 if anything is wrong.
use shared::registry::{self, Content, Def, Registry};

fn list<T: Def>(title: &str, reg: &Registry<T>) {
    if reg.is_empty() {
        println!("{title}: (none)");
        return;
    }
    println!("{title}:");
    for (h, id, d) in reg.iter() {
        let from = reg.origin(id).map(|o| o.file.display().to_string()).unwrap_or_default();
        println!("  #{:<3} {:<32} {:<28} {from}", h.0, id.to_string(), d.display_name());
    }
}

fn main() {
    let all = std::env::args().any(|a| a == "--all");
    let root = registry::find_root().unwrap_or_else(|| ".".into());
    let mut sources = registry::discover(&root, all);
    println!("content root: {}", root.display());
    for s in &sources {
        let counts: Vec<String> = registry::count_files(s).into_iter().filter(|(_, n)| *n > 0).map(|(k, n)| format!("{n} {k}")).collect();
        println!(
            "  {:<8} {:<20} v{:<8} {}{}",
            s.namespace,
            s.name,
            s.version,
            if counts.is_empty() { "empty".into() } else { counts.join(", ") },
            if s.enabled { "" } else if all { "  (disabled, loaded for --all)" } else { "  (disabled, skipped)" }
        );
    }
    if !all {
        sources.retain(|s| s.enabled);
    }
    let c = Content::load(&sources);
    println!();
    list("legends", &c.legends);
    list("abilities", &c.abilities);
    list("passives", &c.passives);
    list("weapons", &c.weapons);
    list("ammo", &c.ammo);
    list("attachments", &c.attachments);
    list("consumables", &c.consumables);
    list("maps", &c.maps);
    list("legend skins", &c.legend_skins);
    list("weapon skins", &c.weapon_skins);
    list("heirlooms", &c.heirlooms);
    list("charms", &c.charms);
    list("cosmetics", &c.cosmetics);
    println!("\n{}", c.summary());
    for e in &c.errors {
        println!("ERROR {e}");
    }
    std::process::exit(if c.errors.is_empty() { 0 } else { 1 });
}
