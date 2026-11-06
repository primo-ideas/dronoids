//! Game of Drones — simulation core (no graphics).
//!
//! Everything in this crate root is deterministic and headless so it can be
//! unit-tested, run by a dedicated server, and replayed identically by
//! every client of a multiplayer world.

pub mod ai;
pub mod geo;
pub mod net;
pub mod programs;
pub mod rng;
pub mod rules;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
pub mod sim;
pub mod vm;
