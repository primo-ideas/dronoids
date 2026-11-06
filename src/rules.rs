//! Game balance constants. Everything the simulation needs to agree on in
//! multiplayer lives here, so both peers run with identical rules.

/// Map size in tiles.
pub const MAP_W: i32 = 80;
pub const MAP_H: i32 = 52;

/// How much ore a drone can carry.
pub const CARGO_CAP: i32 = 5;
pub const DRONE_HP: i32 = 10;
pub const FACTORY_HP: i32 = 150;
pub const ATTACK_DAMAGE: i32 = 2;

pub const DRONE_COST: i32 = 10;
pub const FACTORY_COST: i32 = 120;
pub const START_MINERALS: i32 = 60;

/// Ticks between two drone spawns of a factory.
pub const SPAWN_INTERVAL: i32 = 12;

/// Manhattan radius of drone sensors (factories are visible from anywhere).
pub const SENSOR_RANGE: i32 = 10;
/// A new factory must be within this manhattan distance of one of your units.
pub const BUILD_RANGE: i32 = 6;
/// ...and at least this far from any enemy factory.
pub const ENEMY_BUILD_CLEARANCE: i32 = 7;

pub const MAX_DRONES_PER_PLAYER: usize = 150;
/// Default / maximum number of living drones per factory.
pub const DEFAULT_QUOTA: u8 = 8;
pub const MAX_QUOTA: u8 = 24;
/// Non-action instructions a drone may execute per tick before it stalls.
pub const MAX_OPS_PER_TICK: u32 = 32;
/// Shared memory cells per factory ("hive memory"), reachable with PEEK/POKE.
pub const HIVE_MEM: usize = 16;
/// CALL depth.
pub const STACK_DEPTH: usize = 8;

/// Simulation speed at "1x".
pub const TICKS_PER_SEC: f32 = 8.0;
pub const MAX_PLAYERS: usize = 4;

pub const ORE_MIN: u16 = 30;
pub const ORE_MAX: u16 = 90;
