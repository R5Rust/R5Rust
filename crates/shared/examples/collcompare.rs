//! Compares the game's collision (collision.bin) with the render triangles the client used to
//! collide with (world.glb solid draws): floor heights under the player hull along lines and over
//! a grid, and where they differ.
//!
//!   cargo run --release -p shared --example collcompare [-- <physics collision.bin>]
//!
//! The optional argument is a collision.bin exported with `--prop-physics`; it adds a third
//! column where static props use their coarse physics trees (where they have one) instead.
use glam::Vec3;
use shared::{assets::Config, collision::Collision, collmesh, glb};

const HULL_MIN: Vec3 = Vec3::new(-16.0, -16.0, 0.0);
const HULL_MAX: Vec3 = Vec3::new(16.0, 16.0, 72.0);

/// Where the player hull comes to rest dropping from `top` (None = nothing within 400 units).
fn stand(col: &Collision, x: f32, y: f32, top: f32) -> Option<f32> {
    let s = Vec3::new(x, y, top);
    let t = col.trace_hull(s, s - Vec3::Z * 400.0, HULL_MIN, HULL_MAX);
    (t.hit() && !t.startsolid).then_some(t.endpos.z)
}

/// The surface a thin ray hits straight down.
fn ray_floor(col: &Collision, x: f32, y: f32, top: f32) -> Option<f32> {
    col.ray(Vec3::new(x, y, top), -Vec3::Z, 400.0).map(|t| top - t)
}

fn fmt(v: Option<f32>) -> String {
    v.map(|z| format!("{z:8.2}")).unwrap_or_else(|| "       -".into())
}

fn main() {
    let cfg = Config::load();
    let w = glb::load_world(&cfg.world_glb(), glb::Mode::Geometry, true).expect("world.glb");
    let render = Collision::build(w.positions.clone(), &collmesh::render_indices(&w));
    drop(w);
    let game_mesh = collmesh::CollMesh::load(&cfg.collision_bin()).expect("collision.bin (run tools/export-map --collision)");
    let game = Collision::build(game_mesh.positions.clone(), &game_mesh.player_indices());
    let detail = std::env::args().nth(1).map(|p| {
        let m = collmesh::CollMesh::load(std::path::Path::new(&p)).expect("physics collision.bin");
        // Everything, physics trees included (they encase the main trees, so from outside this
        // is what colliding with the physics shells would look like).
        let idx: Vec<u32> = (0..m.tris.len()).filter(|&i| m.contents[i] & collmesh::MASK_PLAYERSOLID != 0).flat_map(|i| m.tris[i]).collect();
        Collision::build(m.positions, &idx)
    });
    println!(
        "render: {} tris | game (player-solid): {} tris{}",
        render.tri_count(),
        game.tri_count(),
        detail.as_ref().map(|d| format!(" | game + prop physics trees: {} tris", d.tri_count())).unwrap_or_default()
    );

    // Lines through the spots the user floated at, and the open floor near the far spawn.
    let lines: [(&str, Vec3, Vec3); 5] = [
        ("ramp A, along x", Vec3::new(-10567.0, -266.0, 1580.0), Vec3::new(-10311.0, -266.0, 1580.0)),
        ("ramp A, along y", Vec3::new(-10439.0, -394.0, 1580.0), Vec3::new(-10439.0, -138.0, 1580.0)),
        ("ramp B, along x", Vec3::new(-10996.0, 557.0, 1580.0), Vec3::new(-10740.0, 557.0, 1580.0)),
        ("ramp B, along y", Vec3::new(-10868.0, 429.0, 1580.0), Vec3::new(-10868.0, 685.0, 1580.0)),
        ("open floor, along x", Vec3::new(-14775.0, -6266.0, -1035.0), Vec3::new(-14519.0, -6266.0, -1035.0)),
    ];
    for (name, a, b) in lines {
        println!("\n== {name}: ({:.0},{:.0}) -> ({:.0},{:.0}), z ref {:.0}", a.x, a.y, b.x, b.y, a.z);
        println!("       x        y |  hull: render     game   diff{} | ray: render     game", if detail.is_some() { "  +physics" } else { "" });
        let n = ((b - a).length() / 16.0).round() as i32;
        let (mut worst, mut differ) = (0.0f32, 0);
        for i in 0..=n {
            let p = a + (b - a) * (i as f32 / n as f32);
            let top = p.z + 120.0;
            let (hr, hg) = (stand(&render, p.x, p.y, top), stand(&game, p.x, p.y, top));
            let hd = detail.as_ref().map(|d| stand(d, p.x, p.y, top));
            let (rr, rg) = (ray_floor(&render, p.x, p.y, top), ray_floor(&game, p.x, p.y, top));
            let diff = match (hr, hg) {
                (Some(r), Some(g)) => Some(g - r),
                _ => None,
            };
            if let Some(d) = diff {
                worst = worst.max(d.abs());
                if d.abs() > 0.5 {
                    differ += 1;
                }
            }
            println!(
                "{:8.0} {:8.0} | {} {} {}{} | {} {}",
                p.x,
                p.y,
                fmt(hr),
                fmt(hg),
                diff.map(|d| format!("{d:+7.2}")).unwrap_or_else(|| "      ?".into()),
                hd.map(|h| format!(" {}", fmt(h))).unwrap_or_default(),
                fmt(rr),
                fmt(rg)
            );
        }
        println!("   -> {differ}/{} samples differ by > 0.5 (hull stand height), worst {worst:.2}", n + 1);
    }

    // A grid around the spawn corridor: how much of the floor agrees.
    let (c, half, step) = (Vec3::new(-11000.0, 0.0, 1580.0), 1536.0f32, 32.0f32);
    let (mut same, mut higher, mut lower, mut only_r, mut only_g) = (0, 0, 0, 0, 0);
    let mut big: Vec<(f32, Vec3, f32, f32)> = Vec::new();
    let mut y = c.y - half;
    while y <= c.y + half {
        let mut x = c.x - half;
        while x <= c.x + half {
            // From 100 above the spawn floor: the game has invisible clip ceilings higher up.
            let top = c.z + 100.0;
            match (stand(&render, x, y, top), stand(&game, x, y, top)) {
                (Some(r), Some(g)) => {
                    let d = g - r;
                    if d.abs() <= 0.5 {
                        same += 1;
                    } else if d > 0.0 {
                        higher += 1;
                    } else {
                        lower += 1;
                    }
                    if d.abs() > 2.0 {
                        big.push((d, Vec3::new(x, y, r), r, g));
                    }
                }
                (Some(_), None) => only_r += 1,
                (None, Some(_)) => only_g += 1,
                _ => {}
            }
            x += step;
        }
        y += step;
    }
    println!(
        "\n== grid {0}x{0} around ({1:.0},{2:.0}) every {step:.0}: hull stand height same (<=0.5) {same}, game higher {higher}, game lower {lower}, floor only in render {only_r}, only in game {only_g}",
        half * 2.0,
        c.x,
        c.y
    );
    big.sort_by(|a, b| b.0.abs().partial_cmp(&a.0.abs()).unwrap());
    for (d, p, r, g) in big.iter().take(20) {
        println!("   ({:7.0},{:7.0})  render {r:8.2}  game {g:8.2}  ({d:+.2})", p.x, p.y);
    }
}
