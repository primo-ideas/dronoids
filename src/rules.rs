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
pub const FACTORY_HP: i32 = 300;
/// A damaged factory mends itself, one point every so many ticks (5 s):
/// nothing against a raid, but it is whole again half an hour later.
pub const FACTORY_REPAIR_TICKS: u64 = 20;
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

/// Relics: rare remains scattered far from the starting places. MINE takes
/// one shard at a time, which goes straight to the factory of the dronoid
/// that mined it; a relic's worth of shards of one kind raises that factory
/// one level of that kind (`Bonus`).
pub const RELIC_RADIUS: i32 = FP * 9 / 10;
pub const RELIC_SHARDS: i32 = 12;
pub const BONUS_MAX_LEVEL: i32 = 3;
/// A relic mined out is whole again this long after its last shard (24 h).
pub const RELIC_REGROW_TICKS: u64 = (24.0 * 3600.0 * TICKS_PER_SEC) as u64;
/// Forge: the wait between two dronoids shrinks by this many percent a level.
pub const FORGE_PERCENT: i32 = 20;
/// Refinery: one more crystal a level for every this many unloaded there.
pub const REFINERY_PER: i32 = 5;
/// Bulwark: more hull a level.
pub const BULWARK_HP: i32 = 150;
/// Arsenal: more damage a level for each shot of the factory's dronoids.
pub const ARSENAL_DAMAGE: i32 = 1;

/// The ways a relic improves the factory whose dronoids mine it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bonus {
    Forge,
    Refinery,
    Bulwark,
    Arsenal,
}

impl Bonus {
    pub const ALL: [Bonus; 4] = [Bonus::Forge, Bonus::Refinery, Bonus::Bulwark, Bonus::Arsenal];

    pub fn of(kind: u8) -> Bonus {
        Bonus::ALL[kind as usize % Bonus::ALL.len()]
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Bonus::Forge => "Forge",
            Bonus::Refinery => "Refinery",
            Bonus::Bulwark => "Bulwark",
            Bonus::Arsenal => "Arsenal",
        }
    }

    /// What `level` levels of it do for a factory.
    pub fn effect(self, level: i32) -> String {
        let level = level.max(1);
        match self {
            Bonus::Forge => format!("builds dronoids {} % faster", FORGE_PERCENT * level),
            Bonus::Refinery => format!("+{} crystal{} for every {REFINERY_PER} unloaded here", level, if level == 1 { "" } else { "s" }),
            Bonus::Bulwark => format!("+{} hull", BULWARK_HP * level),
            Bonus::Arsenal => format!("its dronoids hit {} harder", ARSENAL_DAMAGE * level),
        }
    }
}
