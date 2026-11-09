//! Game balance constants. Everything the simulation needs to agree on in
//! multiplayer lives here, so every peer runs with identical rules.
//!
//! Distances are fixed-point integers: one world unit is `FP` sub-units.
//! A dronoid is about 0.7 unit wide, a factory 2.4.

/// Fixed-point scale: 1 world unit.
pub const FP: i32 = 1024;

/// The world stretches this far from the origin in every direction: a
/// million units across, generated as it is discovered (`sim::terrain`).
pub const WORLD_EXTENT: i32 = 500_000 * FP;

/// Side, in units, of the first sector of older worlds, generated whole.
pub const WORLD_SIZE: i32 = 768;

/// Simulation speed at "x1". Slow on purpose: a persistent world where
/// dronoids work while you think (and while you are away).
pub const TICKS_PER_SEC: f32 = 4.0;

pub const DRONOID_RADIUS: i32 = FP * 35 / 100;
pub const FACTORY_RADIUS: i32 = FP * 12 / 10;

/// Length of one MOVE, and how long actions take (in ticks).
pub const STEP: i32 = FP;
pub const MOVE_TICKS: i32 = 4;
pub const MINE_TICKS: i32 = 8;
pub const DROP_TICKS: i32 = 4;
pub const ATTACK_TICKS: i32 = 6;
pub const WAIT_TICKS: i32 = 4;
pub const FAIL_TICKS: i32 = 2;
pub const STALL_TICKS: i32 = 1;

/// MINE / DROP work when the gap between the dronoid and its target is
/// below this (so `DIST a, ORE` then `JLE a, 1, ...` is always in reach).
pub const REACH: i32 = 2 * FP;
pub const ATTACK_RANGE: i32 = 3 * FP;
/// How far ahead LOOK probes.
pub const LOOK_DIST: i32 = FP * 3 / 2;
/// Dronoid sensors range. Own factories are always visible, enemy ones
/// from `EBASE_RANGE`.
pub const SENSOR_RANGE: i32 = 12 * FP;
pub const EBASE_RANGE: i32 = 40 * FP;

pub const CARGO_CAP: i32 = 5;
pub const DRONOID_HP: i32 = 10;
pub const FACTORY_HP: i32 = 200;
pub const ATTACK_DAMAGE: i32 = 2;

pub const DRONOID_COST: i32 = 10;
pub const FACTORY_COST: i32 = 150;
/// Newcomers own nothing but this: one factory and its first two dronoids.
pub const START_MINERALS: i32 = FACTORY_COST + 2 * DRONOID_COST;

/// Ticks between two dronoid spawns of a factory (30 s).
pub const SPAWN_TICKS: i32 = 120;

/// Newcomers can neither be attacked nor attack for this long (10 min).
pub const SHIELD_TICKS: u64 = (10.0 * 60.0 * TICKS_PER_SEC) as u64;

/// Every so often a new crystal field appears somewhere in a first sector,
/// so that it never runs dry.
pub const REGROW_TICKS: u64 = 40;
/// Elsewhere, a deposit is whole again this long after it was last mined (6 h).
pub const DEPOSIT_REGROW_TICKS: u64 = (6.0 * 3600.0 * TICKS_PER_SEC) as u64;
/// A newcomer starts at least this far from every colony (raids go this far).
pub const NEWCOMER_GAP: i32 = 200 * FP;

/// A new factory must be this close to one of your units...
pub const BUILD_RANGE: i32 = 8 * FP;
/// ...this far from enemy factories...
pub const ENEMY_BUILD_CLEARANCE: i32 = 25 * FP;
/// ...and leave this gap to any other factory.
pub const FACTORY_SPACING: i32 = 4 * FP;
/// A first factory (landing, owning nothing yet) can go anywhere at least
/// this far from other players' factories.
pub const LANDING_CLEARANCE: i32 = 40 * FP;

pub const MAX_DRONOIDS_PER_PLAYER: usize = 150;
pub const DEFAULT_QUOTA: u8 = 8;
pub const MAX_QUOTA: u8 = 24;
pub const MAX_PLAYERS: usize = 200;

/// Non-action instructions a dronoid may execute per decision before it
/// stalls for a moment.
pub const MAX_OPS_PER_TICK: u32 = 32;
/// Shared memory cells per factory ("hive memory"), reachable with PEEK/POKE.
pub const HIVE_MEM: usize = 16;
/// CALL depth.
pub const STACK_DEPTH: usize = 8;

pub const ORE_MIN: i32 = 40;
pub const ORE_MAX: i32 = 120;

/// Fog of war: how far units see (players only see around their units).
pub const VISION_DRONOID: i32 = SENSOR_RANGE;
pub const VISION_FACTORY: i32 = 18 * FP;
/// On arrival, the area around the suggested landing place is scanned.
pub const ARRIVAL_SCAN: i32 = 30 * FP;
/// Resolution of each player's map of explored space.
pub const FOG_CELL: i32 = 4 * FP;

/// Number of possible starting places generated with a first sector.
pub const SPAWN_SLOTS: usize = 24;
