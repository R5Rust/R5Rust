//! R5Rust shared code: asset paths, map loading (GLB + meta.json), collision and the
//! deterministic player simulation. Nothing in here touches the GPU, so a future dedicated
//! server (v2) can use all of it.

pub mod assets;
pub mod bsplight;
pub mod collision;
pub mod collmesh;
pub mod glb;
pub mod registry;
pub mod settings;
pub mod sim;

pub use glam::{Vec2, Vec3};
