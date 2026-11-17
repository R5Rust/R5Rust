//! Hang scan: drops the player on a grid around each spot (default the three reported stuck spots), jumps,
//! holds forward+sprint at 8 yaws for 3 s, and counts hangs: 30+ consecutive ticks with no z change while not
//! on the ground (and not mantling). cargo run --release -p shared --example hangscan -- [x,y,z ...]
//! N=9 STEP=24 set the grid; VERBOSE=1 lists every hang.
use glam::Vec3;
use shared::{assets::Config, collmesh, sim::{self, Input, Player}};
fn main() {
    let mut spots: Vec<Vec3> = std::env::args().skip(1).map(|s| { let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect(); Vec3::new(v[0], v[1], v[2]) }).collect();
    if spots.is_empty() {
        spots = vec![Vec3::new(-11019.0, 811.0, 1624.0), Vec3::new(-9426.0, -74.0, 961.0), Vec3::new(-7825.0, -2282.0, 578.0)];
    }
    let n: i32 = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(9);
    let step: f32 = std::env::var("STEP").ok().and_then(|s| s.parse().ok()).unwrap_or(24.0);
    let verbose = std::env::var_os("VERBOSE").is_some();
    let (col, _) = collmesh::build_player_collision(&Config::load()).unwrap();
    let mut total = (0, 0);
    for s in &spots {
        let (mut runs, mut hangs) = (0, 0);
        for i in 0..n {
            for j in 0..n {
                let x = s.x + (i - n / 2) as f32 * step;
                let y = s.y + (j - n / 2) as f32 * step;
                // start a little above the spot, moved out of anything solid (a player can't be inside geometry)
                let start = nearest_free(&col, Vec3::new(x, y, s.z + 8.0));
                for k in 0..8 {
                    let yaw = k as f32 * 45.0;
                    let mut p = Player::new(start);
                    p.speed_scale = 1.15;
                    let (mut flat, mut worst, mut at) = (0, 0, Vec3::ZERO);
                    let mut last_z = p.pos.z;
                    for t in 0..180 {
                        let jump = t == 0;
                        sim::simulate(&mut p, &Input { forward: 1.0, sprint: true, jump, jump_pressed: jump, yaw, ..Default::default() }, 1.0 / 60.0, &col);
                        if !p.on_ground && !p.mantling() && p.pos.z == last_z {
                            flat += 1;
                            if flat > worst { worst = flat; at = p.pos; }
                        } else {
                            flat = 0;
                        }
                        last_z = p.pos.z;
                    }
                    runs += 1;
                    if worst >= 30 {
                        hangs += 1;
                        if verbose {
                            println!("  hang {worst:3} ticks: start {:.0},{:.0},{:.0} yaw {yaw:3} at {:.1},{:.1},{:.1} ({})", start.x, start.y, start.z, at.x, at.y, at.z, p.state_name());
                        }
                    }
                }
            }
        }
        println!("spot {:.0},{:.0},{:.0}: {hangs} hangs / {runs} runs", s.x, s.y, s.z);
        total.0 += hangs;
        total.1 += runs;
    }
    println!("total: {} hangs / {} runs", total.0, total.1);
}

/// Nearest spot where the standing hull is free (a real player can't be inside geometry; reported spots are rounded
/// to whole units and can land a hull a fraction of a unit inside a wall). Searches shells 0.25 units apart.
fn nearest_free(col: &shared::collision::Collision, p: Vec3) -> Vec3 {
    let (mn, mx) = (shared::sim::STOCK.stand.hull_min, shared::sim::STOCK.stand.hull_max);
    let free = |q: Vec3| !col.trace_hull(q, q, mn, mx).startsolid;
    if free(p) {
        return p;
    }
    let mut dirs = vec![];
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                if (x, y, z) != (0, 0, 0) {
                    dirs.push(Vec3::new(x as f32, y as f32, z as f32).normalize());
                }
            }
        }
    }
    for i in 1..=192 {
        let r = i as f32 * 0.25;
        for d in &dirs {
            if free(p + *d * r) {
                return p + *d * r;
            }
        }
    }
    p
}
