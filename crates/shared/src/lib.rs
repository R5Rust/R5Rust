//! R5Rust shared code: asset paths, map loading (GLB + meta.json), collision,
//! the deterministic player simulation and the network protocol.
//! Nothing in here touches the GPU, so the dedicated server can use all of it.

pub mod assets;
pub mod collision;
pub mod glb;
pub mod proto;
pub mod sim;

pub use glam::{Vec2, Vec3};
