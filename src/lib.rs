//! Game of Drones — simulation core (no graphics).
//!
//! Everything in this crate root is deterministic and headless so it can be
//! unit-tested and shared by lockstep multiplayer peers.

pub mod ai;
pub mod lockstep;
pub mod net;
pub mod programs;
pub mod rng;
pub mod rules;
pub mod sim;
pub mod vm;
