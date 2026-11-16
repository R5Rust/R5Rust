//! The map's real collision (`collision.bin`, written by `tools/export-map`): Apex's own BVH4
//! collision decoded into triangles with their contents masks. See docs/collision.md.
//!
//! Layout (little endian): magic `R5COLL\0\0`, u32 version 1, u32 vertex count V, u32 triangle
//! count T, u32 0, V x [f32; 3], T x [u32; 3], T x u32 contents, T x u8 source.

use crate::assets::Config;
use crate::collision::Collision;
use crate::glb;
use std::path::Path;

/// `TRACE_MASK_PLAYERSOLID`: SOLID | WINDOW | GRATE | MOVEABLE | PLAYERCLIP | MONSTER.
pub const MASK_PLAYERSOLID: u32 = 0x0201_400B;

pub const SRC_WORLD_POLY: u8 = 0;
pub const SRC_WORLD_HULL: u8 = 1;
/// A static prop's collision tree (its model's tree 0, the one the engine traces).
pub const SRC_PROP: u8 = 2;
/// A static prop's physics tree (the coarse vphysics replica); only in a `--prop-physics`
/// export, never used for player movement.
pub const SRC_PROP_PHYSICS: u8 = 3;

pub struct CollMesh {
    pub positions: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
    pub contents: Vec<u32>,
    pub source: Vec<u8>,
}

impl CollMesh {
    pub fn load(path: &Path) -> Result<CollMesh, String> {
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let u32_at = |o: usize| -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) };
        if b.len() < 24 || &b[..8] != b"R5COLL\0\0" {
            return Err(format!("{}: not a collision.bin", path.display()));
        }
        if u32_at(8) != 1 {
            return Err(format!("{}: unknown version {}", path.display(), u32_at(8)));
        }
        let (nv, nt) = (u32_at(12) as usize, u32_at(16) as usize);
        let need = 24 + nv * 12 + nt * 12 + nt * 4 + nt;
        if b.len() < need {
            return Err(format!("{}: truncated ({} of {need} bytes)", path.display(), b.len()));
        }
        fn words(b: &[u8]) -> impl Iterator<Item = u32> + '_ {
            b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        }
        let mut o = 24;
        let f: Vec<f32> = words(&b[o..o + nv * 12]).map(f32::from_bits).collect();
        let positions: Vec<[f32; 3]> = f.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        o += nv * 12;
        let i: Vec<u32> = words(&b[o..o + nt * 12]).collect();
        let tris: Vec<[u32; 3]> = i.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        o += nt * 12;
        let contents: Vec<u32> = words(&b[o..o + nt * 4]).collect();
        o += nt * 4;
        let source = b[o..o + nt].to_vec();
        if tris.iter().flatten().any(|&i| i as usize >= nv) {
            return Err(format!("{}: index out of range", path.display()));
        }
        Ok(CollMesh { positions, tris, contents, source })
    }

    /// Whether triangle `i` blocks player movement.
    pub fn player_solid(&self, i: usize) -> bool {
        self.contents[i] & MASK_PLAYERSOLID != 0 && self.source[i] != SRC_PROP_PHYSICS
    }

    /// Flat indices of the triangles player movement collides with.
    pub fn player_indices(&self) -> Vec<u32> {
        (0..self.tris.len()).filter(|&i| self.player_solid(i)).flat_map(|i| self.tris[i]).collect()
    }
}

/// Where the collision came from, for logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `collision.bin`: the game's own collision.
    Game,
    /// world.glb's solid render triangles (no collision.bin next to it).
    Render,
}

/// Positions + indices for player collision: `collision.bin` when it is there, else the solid
/// render triangles of world.glb.
pub fn player_geometry(cfg: &Config) -> Result<(Vec<[f32; 3]>, Vec<u32>, Source), String> {
    let bin = cfg.collision_bin();
    if bin.is_file() {
        match CollMesh::load(&bin) {
            Ok(m) => {
                let idx = m.player_indices();
                return Ok((m.positions, idx, Source::Game));
            }
            Err(e) => log::warn!("{e}; falling back to render triangles"),
        }
    }
    let w = glb::load_world(&cfg.world_glb(), glb::Mode::Geometry, true)?;
    let idx = render_indices(&w);
    Ok((w.positions, idx, Source::Render))
}

/// The solid draws' triangles of a loaded world.glb (the old collision source).
pub fn render_indices(w: &glb::World) -> Vec<u32> {
    w.draws
        .iter()
        .filter(|d| w.materials[d.material as usize].solid())
        .flat_map(|d| w.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].iter().copied())
        .collect()
}

/// [`player_geometry`] built into a [`Collision`].
pub fn build_player_collision(cfg: &Config) -> Result<(Collision, Source), String> {
    let (pos, idx, src) = player_geometry(cfg)?;
    Ok((Collision::build(pos, &idx), src))
}
