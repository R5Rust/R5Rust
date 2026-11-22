//! Inventory and loot, default survival as the Firing Range runs it (docs/inventory.md): the retail loot catalogue
//! (`db`), the player's inventory and its pickup / drop / reload rules (`inventory`), items lying in the world with the
//! range's respawn and the use-prompt targeting (`ground`), and the range's loot layout from the map (`range`).

pub mod db;
pub mod ground;
pub mod inventory;
pub mod range;

pub use db::{ItemId, Kind, LootDb};
pub use ground::{GroundItem, LootWorld};
pub use inventory::{Drop, Hands, Inventory, Pickup, UseKind, Weapon};
