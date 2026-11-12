//! Game balance constants. Everything the simulation needs to agree on in
//! multiplayer lives here, so every peer runs with identical rules.
//!
//! Distances are fixed-point integers: one world unit is `FP` sub-units.
//! A dronoid is about 0.7 unit wide, a base 6.

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
/// Every colony has one big central base, which builds its dronoids.
pub const BASE_RADIUS: i32 = 3 * FP;

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
/// Dronoid sensors range. Your own base is always visible, enemy ones
/// from `EBASE_RANGE`.
pub const SENSOR_RANGE: i32 = 12 * FP;
pub const EBASE_RANGE: i32 = 40 * FP;

pub const CARGO_CAP: i32 = 5;
pub const DRONOID_HP: i32 = 10;
pub const BASE_HP: i32 = 3000;
/// A damaged base mends itself, one point every so many ticks (1 s):
/// nothing against a raid, but it is whole again within the hour.
pub const BASE_REPAIR_TICKS: u64 = 4;
pub const ATTACK_DAMAGE: i32 = 2;

pub const DRONOID_COST: i32 = 10;
/// Newcomers land their base for free and own just enough for its first
/// dronoids.
pub const START_MINERALS: i32 = 4 * DRONOID_COST;

/// Production lines a new base has. Each one builds dronoids of its own
/// program, in parallel with the others; relics of Assembly add more.
pub const BASE_LINES: usize = 2;
/// Ticks between two dronoids of a production line (30 s).
pub const SPAWN_TICKS: i32 = 120;
/// How many living dronoids a new base can hold...
pub const BASE_CAPACITY: u16 = 12;
/// ...and what each enlargement of its hangar costs and adds.
pub const EXPAND_COST: i32 = 150;
pub const EXPAND_SLOTS: u16 = 6;

/// Newcomers can neither be attacked nor attack for this long (10 min).
pub const SHIELD_TICKS: u64 = (10.0 * 60.0 * TICKS_PER_SEC) as u64;

/// Every so often a new crystal field appears somewhere in a first sector,
/// so that it never runs dry.
pub const REGROW_TICKS: u64 = 40;
/// Elsewhere, a deposit is whole again this long after it was last mined (6 h).
pub const DEPOSIT_REGROW_TICKS: u64 = (6.0 * 3600.0 * TICKS_PER_SEC) as u64;
/// A newcomer starts at least this far from every colony (raids go this far).
pub const NEWCOMER_GAP: i32 = 200 * FP;

/// A base lands anywhere at least this far from the other bases.
pub const LANDING_CLEARANCE: i32 = 40 * FP;

pub const MAX_DRONOIDS_PER_PLAYER: usize = 150;
/// What a new production line may hold of the base's dronoids.
pub const DEFAULT_QUOTA: u8 = 8;
pub const MAX_QUOTA: u8 = MAX_DRONOIDS_PER_PLAYER as u8;
pub const MAX_PLAYERS: usize = 200;

/// Non-action instructions a dronoid may execute per decision before it
/// stalls for a moment.
pub const MAX_OPS_PER_TICK: u32 = 32;
/// Shared memory cells per base ("hive memory"), reachable with PEEK/POKE.
pub const HIVE_MEM: usize = 16;
/// CALL depth.
pub const STACK_DEPTH: usize = 8;

pub const ORE_MIN: i32 = 40;
pub const ORE_MAX: i32 = 120;

/// Fog of war: how far units see (players only see around their units).
pub const VISION_DRONOID: i32 = SENSOR_RANGE;
pub const VISION_BASE: i32 = 24 * FP;
/// On arrival, the area around the suggested landing place is scanned.
pub const ARRIVAL_SCAN: i32 = 30 * FP;
/// Resolution of each player's map of explored space.
pub const FOG_CELL: i32 = 4 * FP;

/// Number of possible starting places generated with a first sector.
pub const SPAWN_SLOTS: usize = 24;

/// Relics: rare remains scattered far from the starting places. MINE takes
/// one shard at a time, which goes straight to the base of the dronoid that
/// mined it; a relic's worth of shards of one kind raises that base one
/// level of that perk (`Perk`).
pub const RELIC_RADIUS: i32 = FP * 9 / 10;
pub const RELIC_SHARDS: i32 = 12;
pub const PERK_MAX_LEVEL: i32 = 3;
/// A base never has more production lines than this.
pub const MAX_LINES: usize = BASE_LINES + PERK_MAX_LEVEL as usize;
/// A relic mined out is whole again this long after its last shard (24 h).
pub const RELIC_REGROW_TICKS: u64 = (24.0 * 3600.0 * TICKS_PER_SEC) as u64;
/// Forge: the wait between two dronoids shrinks by this many percent a level.
pub const FORGE_PERCENT: i32 = 20;
/// Refinery: one more crystal a level for every this many unloaded there.
pub const REFINERY_PER: i32 = 5;
/// Bulwark: more hull a level.
pub const BULWARK_HP: i32 = 1000;
/// Arsenal: more damage a level for each shot of the base's dronoids.
pub const ARSENAL_DAMAGE: i32 = 1;

/// The ways a relic improves the base whose dronoids mine it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perk {
    Forge,
    Assembly,
    Refinery,
    Bulwark,
    Arsenal,
}

/// How many kinds of perks there are.
pub const PERKS: usize = 5;

impl Perk {
    pub const ALL: [Perk; PERKS] = [Perk::Forge, Perk::Assembly, Perk::Refinery, Perk::Bulwark, Perk::Arsenal];

    pub fn of(kind: u8) -> Perk {
        Perk::ALL[kind as usize % PERKS]
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Perk::Forge => "Forge",
            Perk::Assembly => "Assembly",
            Perk::Refinery => "Refinery",
            Perk::Bulwark => "Bulwark",
            Perk::Arsenal => "Arsenal",
        }
    }

    /// What `level` levels of it do for a base.
    pub fn effect(self, level: i32) -> String {
        let level = level.max(1);
        let s = |n: i32| if n == 1 { "" } else { "s" };
        match self {
            Perk::Forge => format!("every line builds {} % faster", FORGE_PERCENT * level),
            Perk::Assembly => format!("+{level} production line{}", s(level)),
            Perk::Refinery => format!("+{level} crystal{} for every {REFINERY_PER} unloaded", s(level)),
            Perk::Bulwark => format!("+{} hull", BULWARK_HP * level),
            Perk::Arsenal => format!("its dronoids hit {} harder", ARSENAL_DAMAGE * level),
        }
    }
}
