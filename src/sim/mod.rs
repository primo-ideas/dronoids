//! The deterministic simulation of an immense, continuous world: asteroids
//! and crystal deposits generated as they are discovered (`terrain`),
//! one big base per colony, and dronoids moving freely.
//!
//! Positions are fixed-point integers (`rules::FP` per unit) and angles are
//! whole degrees: given the same seed and the same commands, every machine
//! computes the exact same world, which is what multiplayer relies on.

mod compat;
pub mod fog;
pub mod grid;
pub mod map;
pub mod quests;
pub mod terrain;

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::geo::{self, compass, dir_vec, dist, dist2};
use crate::programs;
use crate::rng::{mix, Fnv, Rng};
use crate::rules::*;
use crate::vm::{self, thing, Action, Program, Senses, Sensor, Vm};
use fog::FogMap;
use grid::Grid;
pub use map::ore_radius;
use terrain::{Terrain, NATURAL};

const GRID_CELL: i32 = 8 * FP;
/// Largest rock radius produced by the generator (for grid queries).
const MAX_ROCK_R: i32 = 8 * FP;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rock {
    pub x: i32,
    pub y: i32,
    pub r: i32,
    pub seed: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ore {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub amount: i32,
}

impl Ore {
    pub fn radius(&self) -> i32 {
        ore_radius(self.amount)
    }
}

/// A deposit as it stands now, from anywhere in the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OreSpot {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub amount: i32,
}

impl OreSpot {
    pub fn radius(&self) -> i32 {
        ore_radius(self.amount)
    }
}

/// A relic as it stands now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelicSpot {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub perk: Perk,
    /// Shards left in it.
    pub left: i32,
}

/// A generated deposit (or relic) that has been mined: all that is ever
/// stored of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worn {
    pub left: i32,
    /// Whole again from this tick on.
    pub until: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Player {
    pub id: u8,
    pub name: String,
    pub minerals: i32,
    pub alive: bool,
    pub bot: bool,
    pub mined: i64,
    // The name in saves written before the rename.
    #[serde(alias = "drones_built")]
    pub dronoids_built: u32,
    pub kills: u32,
    /// Suggested landing place (and camera start).
    pub home: (i32, i32),
    /// Protected (can neither attack nor be attacked) until this tick.
    pub shield_until: u64,
    /// What this player has seen.
    #[serde(default)]
    pub seen: FogMap,
    /// Older saves: the explored first sector, row by row (moved to `seen`).
    #[serde(default)]
    pub explored: Vec<u64>,
    /// First-steps objectives completed (bit i = `quests::QUESTS[i]`).
    #[serde(default)]
    pub quests: u32,
    /// Things done once that objectives care about (`quests::FLAG_*`).
    #[serde(default)]
    pub flags: u32,
}

/// A colony's one base: it lands with the player, builds the dronoids on its
/// production lines, takes what they unload, and holds what relics gave it.
/// When it falls, the colony is gone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Base {
    pub id: u32,
    pub owner: u8,
    pub x: i32,
    pub y: i32,
    pub hp: i32,
    /// How many living dronoids its hangar holds.
    pub capacity: u16,
    /// Hive memory, shared by all its dronoids (PEEK / POKE).
    pub mem: [i32; HIVE_MEM],
    pub lines: Vec<Line>,
    /// Shards received, by kind of perk (`Perk::index`). Kept as a list,
    /// so that saves from when there were fewer kinds still read.
    #[serde(with = "perk_shards")]
    pub perks: [u16; PERKS],
    /// Refinery's extra, in hundredths of a crystal, until a whole one.
    #[serde(default)]
    pub refined: u32,
    /// Dronoids it ever built.
    pub spawned: u32,
}

/// A production line of a base: it builds dronoids running its program,
/// one every so often, as long as it holds fewer than its quota.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Line {
    pub id: u32,
    pub prog: u32,
    pub timer: i32,
    pub enabled: bool,
    pub spawned: u32,
    pub quota: u8,
}

/// A base's shards, as a list of any length (missing kinds have none).
mod perk_shards {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::rules::PERKS;

    pub fn serialize<S: Serializer>(perks: &[u16; PERKS], s: S) -> Result<S::Ok, S::Error> {
        perks.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u16; PERKS], D::Error> {
        let list = Vec::<u16>::deserialize(d)?;
        let mut out = [0; PERKS];
        for (o, v) in out.iter_mut().zip(list) {
            *o = v;
        }
        Ok(out)
    }
}

impl Base {
    /// How many levels of `perk` it has.
    pub fn level(&self, perk: Perk) -> i32 {
        (self.perks[perk.index()] as i32 / SHARDS_PER_LEVEL).min(PERK_MAX_LEVEL)
    }

    /// How many living dronoids it holds: its hangar, and what Hangar adds.
    pub fn room(&self) -> usize {
        (self.capacity as usize + HANGAR_SLOTS as usize * self.level(Perk::Hangar) as usize).min(MAX_DRONOIDS_PER_PLAYER)
    }

    /// Hull of a dronoid it builds now.
    pub fn dronoid_hp(&self) -> i32 {
        DRONOID_HP + self.level(Perk::Armor) / ARMOR_LEVELS
    }

    /// How far it sees.
    pub fn vision(&self) -> i32 {
        VISION_BASE + VISION_PER_LEVEL * self.level(Perk::Vision)
    }

    /// Damage of a shot of one of its dronoids. Between two whole points,
    /// the next one comes as often in ten as the levels past the last.
    pub fn damage(&self, roll: u64) -> i32 {
        let level = self.level(Perk::Arsenal);
        let extra = (roll % ARSENAL_LEVELS as u64) < (level % ARSENAL_LEVELS) as u64;
        ATTACK_DAMAGE + level / ARSENAL_LEVELS + extra as i32
    }

    /// How long one MINE of its dronoids takes.
    pub fn mine_ticks(&self) -> i32 {
        (MINE_TICKS * 100 / (100 + DRILL_PERCENT * self.level(Perk::Drills))).max(1)
    }

    /// Points it mends every `BASE_REPAIR_TICKS`.
    pub fn repair(&self) -> i32 {
        1 + self.level(Perk::Nanites) / NANITE_LEVELS
    }

    /// Shards it holds of `perk` (levels reached included).
    pub fn shards(&self, perk: Perk) -> i32 {
        self.perks[perk.index()] as i32
    }

    pub fn has_perks(&self) -> bool {
        self.perks.iter().any(|s| *s > 0)
    }

    /// Its hull when whole.
    pub fn max_hp(&self) -> i32 {
        BASE_HP + BULWARK_HP * self.level(Perk::Bulwark)
    }

    /// Ticks between two dronoids of one of its lines.
    pub fn spawn_ticks(&self) -> i32 {
        SPAWN_TICKS * 100 / (100 + FORGE_PERCENT * self.level(Perk::Forge))
    }

    /// How many production lines it runs.
    pub fn lines_allowed(&self) -> usize {
        (BASE_LINES + (self.level(Perk::Assembly) / ASSEMBLY_LEVELS) as usize).min(MAX_LINES)
    }

    pub fn line(&self, id: u32) -> Option<&Line> {
        self.lines.iter().find(|l| l.id == id)
    }
}

/// What a dronoid is busy doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Doing {
    Idle,
    Move { fx: i32, fy: i32, tx: i32, ty: i32 },
    Mine { tx: i32, ty: i32 },
    Drop { tx: i32, ty: i32 },
    Attack { tx: i32, ty: i32 },
    Wait,
    Fail,
    Stall,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dronoid {
    pub id: u32,
    pub owner: u8,
    pub x: i32,
    pub y: i32,
    pub prev_x: i32,
    pub prev_y: i32,
    /// Compass angle the dronoid faces.
    pub heading: i32,
    pub hp: i32,
    pub cargo: i32,
    /// The production line that built it (its id). Older saves called it
    /// the dronoid's factory.
    #[serde(alias = "factory")]
    pub line: u32,
    pub prog: u32,
    pub vm: Vm,
    pub doing: Doing,
    /// Ticks left before the current action completes.
    pub busy: i32,
    pub born: u64,
}

/// A program known to the world. Sources travel in commands and snapshots;
/// the compiled form is rebuilt locally.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProgEntry {
    pub name: String,
    pub source: String,
    #[serde(skip)]
    pub code: Option<Arc<Program>>,
}

/// Things that happened during a tick, for visual effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Spawn { owner: u8, x: i32, y: i32 },
    Mine { owner: u8, x: i32, y: i32, tx: i32, ty: i32, depleted: bool },
    Drop { owner: u8, x: i32, y: i32, tx: i32, ty: i32, amount: i32 },
    Attack { owner: u8, x: i32, y: i32, tx: i32, ty: i32 },
    DronoidDestroyed { owner: u8, x: i32, y: i32 },
    BaseDestroyed { owner: u8, x: i32, y: i32 },
    BaseLanded { owner: u8, x: i32, y: i32 },
    /// A base's hangar was enlarged.
    Expanded { owner: u8, capacity: u16 },
    PlayerJoined { player: u8 },
    PlayerEliminated { player: u8 },
    OreGrown { x: i32, y: i32 },
    /// A shard of a relic went to the base of `owner`; `level` is the level
    /// of that perk it just reached, if it did.
    Shard { owner: u8, x: i32, y: i32, tx: i32, ty: i32, perk: Perk, level: Option<i32>, depleted: bool },
    QuestDone { player: u8, quest: u8, reward: i32 },
}

/// Player intents: the only way to influence the world.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Enter the world (or come back after being eliminated).
    Join { name: String, bot: bool },
    /// Land the player's base; its lines start with this program.
    Land { x: i32, y: i32, name: String, source: String },
    /// Give one production line a program; its living dronoids are
    /// reprogrammed too.
    SetProgram { line: u32, name: String, source: String },
    /// New version of a named program: every line and dronoid of this
    /// player running a program with that name switches to it.
    UpdateProgram { name: String, source: String },
    ToggleLine { line: u32 },
    SetQuota { line: u32, quota: u8 },
    /// Write a cell of the base's hive memory (e.g. a target for its dronoids).
    SetMemory { index: u8, value: i32 },
    /// Enlarge the base's hangar (`EXPAND_COST` crystals, `EXPAND_SLOTS` more dronoids).
    Expand,
}

/// Something a sensor found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub dx: i32,
    pub dy: i32,
    /// Gap between the two bodies.
    pub gap: i32,
    pub id: u64,
}

#[derive(Serialize, Deserialize)]
pub struct World {
    /// Side of the first sector of an older world, generated whole (rocks
    /// and ores below); 0 in newer worlds, entirely generated on the way.
    pub size: i32,
    pub seed: u64,
    pub tick: u64,
    /// Static, rebuilt from the seed (never sent over the network).
    #[serde(skip)]
    pub rocks: Vec<Rock>,
    pub ores: Vec<Ore>,
    /// Generated deposits that have been mined (nothing else is stored).
    #[serde(default)]
    pub worn: BTreeMap<u64, Worn>,
    pub players: Vec<Player>,
    /// One per colony that has landed, by id. (Saves from before the bases
    /// held factories instead: `compat` turns them into bases.)
    #[serde(default)]
    pub bases: Vec<Base>,
    // The name in saves written before the rename.
    #[serde(alias = "drones")]
    pub dronoids: Vec<Dronoid>,
    pub programs: Vec<ProgEntry>,
    next_id: u32,
    /// Drives crystal regrowth.
    rng: Rng,
    /// Regrowth keeps the number of deposits around this.
    ore_target: u32,
    #[serde(skip)]
    pub events: Vec<Event>,
    #[serde(skip)]
    rock_grid: Grid,
    #[serde(skip)]
    ore_grid: Grid,
    #[serde(skip)]
    dronoid_grid: Grid,
    #[serde(skip)]
    terrain: Terrain,
}

fn gap_between(dx: i32, dy: i32, r1: i32, r2: i32) -> i32 {
    (dist(dx, dy) - r1 - r2).max(0)
}

impl World {
    pub fn new(seed: u64) -> World {
        let mut w = World {
            size: 0,
            seed,
            tick: 0,
            rocks: vec![],
            ores: vec![],
            worn: BTreeMap::new(),
            players: vec![],
            bases: vec![],
            dronoids: vec![],
            programs: vec![],
            next_id: 1,
            rng: Rng::new(seed ^ 0x5EED),
            ore_target: 0,
            events: vec![],
            rock_grid: Grid::default(),
            ore_grid: Grid::default(),
            dronoid_grid: Grid::default(),
            terrain: Terrain::default(),
        };
        w.rebuild_caches();
        w
    }

    /// A world with a first sector generated whole, like the older ones.
    #[cfg(test)]
    pub(crate) fn with_first_sector(seed: u64) -> World {
        let m = map::generate(seed, WORLD_SIZE);
        let mut w = World::new(seed);
        w.size = m.size;
        w.ore_target = m.ores.len() as u32;
        for (x, y, amount) in m.ores {
            let id = w.new_id();
            w.ores.push(Ore { id, x, y, amount });
        }
        w.rebuild_caches();
        w
    }

    /// Rebuild what is not serialized (compiled programs, spatial grids).
    pub fn rebuild_caches(&mut self) {
        if self.size > 0 && self.rocks.is_empty() {
            self.rocks = map::generate(self.seed, self.size / FP).rocks;
        }
        self.terrain = Terrain::new(self.seed, self.size);
        self.adopt_old_maps();
        for p in &mut self.programs {
            p.code = Some(Arc::new(vm::assemble(&p.name, &p.source).unwrap_or_else(|_| Program::empty(&p.name))));
        }
        self.rock_grid = Grid::new(GRID_CELL);
        for (i, r) in self.rocks.iter().enumerate() {
            self.rock_grid.insert(r.x, r.y, i as u32);
        }
        self.ore_grid = Grid::new(GRID_CELL);
        for o in &self.ores {
            self.ore_grid.insert(o.x, o.y, o.id);
        }
        self.dronoid_grid = Grid::new(GRID_CELL);
        self.index_dronoids();
    }

    /// Older saves kept what a player had explored of the first sector row
    /// by row.
    fn adopt_old_maps(&mut self) {
        let n = (self.size + FOG_CELL - 1) / FOG_CELL;
        for p in self.players.iter_mut().filter(|p| !p.explored.is_empty()) {
            for cy in 0..n {
                for cx in 0..n {
                    let i = (cy * n + cx) as usize;
                    if p.explored.get(i / 64).is_some_and(|w| w & (1 << (i % 64)) != 0) {
                        p.seen.set(cx, cy);
                    }
                }
            }
            p.explored = vec![];
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("world serializes")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<World, String> {
        let mut w: World = bincode::deserialize(bytes).map_err(|e| e.to_string())?;
        w.rebuild_caches();
        Ok(w)
    }

    /// For the server's save file: JSON, so that later versions can add
    /// fields (with defaults) and still read it.
    pub fn to_save(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("world serializes")
    }

    /// Read any save: JSON, or the bincode files of earlier versions
    /// (see [`World::is_old_save`]).
    pub fn from_save(bytes: &[u8]) -> Result<World, String> {
        let mut w: World = if Self::is_old_save(bytes) {
            compat::from_bincode(bytes)?.0
        } else {
            compat::from_json(bytes)?
        };
        // Saved before the game spoke English: the built-ins under their new
        // names and wording, and the bots under theirs.
        for p in &mut w.programs {
            if let Some((name, source)) = programs::modernized(&p.name, &p.source) {
                p.name = name;
                p.source = source;
            }
        }
        for p in w.players.iter_mut().filter(|p| p.bot) {
            if let Some(star) = p.name.strip_prefix("IA ") {
                p.name = format!("AI {}", star.replace('\u{ef}', "i").replace('\u{e8}', "e"));
            }
        }
        w.rebuild_caches();
        Ok(w)
    }

    /// Written before protocol 5 (bincode): worth keeping a copy of before
    /// it is replaced by the current format.
    pub fn is_old_save(bytes: &[u8]) -> bool {
        bytes.first() != Some(&b'{')
    }

    fn index_dronoids(&mut self) {
        self.dronoid_grid.clear();
        for (i, d) in self.dronoids.iter().enumerate() {
            self.dronoid_grid.insert(d.x, d.y, i as u32);
        }
    }

    // ----- programs ----------------------------------------------------------

    /// Register a program, reusing an identical one. `None` if it does not
    /// assemble.
    pub fn intern_program(&mut self, name: &str, source: &str) -> Option<u32> {
        if let Some(i) = self.programs.iter().position(|p| p.name == name && p.source == source) {
            return Some(i as u32);
        }
        let code = vm::assemble(name, source).ok()?;
        self.programs.push(ProgEntry { name: name.into(), source: source.into(), code: Some(Arc::new(code)) });
        Some(self.programs.len() as u32 - 1)
    }

    pub fn program(&self, id: u32) -> Arc<Program> {
        self.programs
            .get(id as usize)
            .and_then(|p| p.code.clone())
            .unwrap_or_else(|| Arc::new(Program::empty("?")))
    }

    pub fn program_name(&self, id: u32) -> &str {
        self.programs.get(id as usize).map(|p| p.name.as_str()).unwrap_or("?")
    }

    // ----- queries -------------------------------------------------------------

    pub fn dronoid_index(&self, id: u32) -> Option<usize> {
        self.dronoids.binary_search_by_key(&id, |d| d.id).ok()
    }

    pub fn base_index(&self, id: u32) -> Option<usize> {
        self.bases.binary_search_by_key(&id, |b| b.id).ok()
    }

    fn ore_index(&self, id: u32) -> Option<usize> {
        self.ores.binary_search_by_key(&id, |o| o.id).ok()
    }

    pub fn dronoid(&self, id: u32) -> Option<&Dronoid> {
        self.dronoid_index(id).map(|i| &self.dronoids[i])
    }

    pub fn base(&self, id: u32) -> Option<&Base> {
        self.base_index(id).map(|i| &self.bases[i])
    }

    /// The base of a colony, once it has landed.
    pub fn base_of(&self, owner: u8) -> Option<&Base> {
        self.bases.iter().find(|b| b.owner == owner)
    }

    fn base_index_of(&self, owner: u8) -> Option<usize> {
        self.bases.iter().position(|b| b.owner == owner)
    }

    pub fn player_by_name(&self, name: &str) -> Option<&Player> {
        self.players.iter().find(|p| p.name == name)
    }

    pub fn dronoids_of_line(&self, line: u32) -> usize {
        self.dronoids.iter().filter(|d| d.line == line && d.hp > 0).count()
    }

    pub fn dronoid_count(&self, owner: u8) -> usize {
        self.dronoids.iter().filter(|d| d.owner == owner && d.hp > 0).count()
    }

    pub fn inside(&self, x: i32, y: i32, r: i32) -> bool {
        x.abs() + r <= WORLD_EXTENT && y.abs() + r <= WORLD_EXTENT
    }

    /// Every asteroid centred in the rectangle, with something to tell them apart.
    pub fn rocks_in(&self, x0: i32, y0: i32, x1: i32, y1: i32, mut f: impl FnMut(u64, &Rock)) {
        let mut near = vec![];
        self.rock_grid.query_rect(x0, y0, x1, y1, &mut near);
        for &i in &near {
            f(i as u64, &self.rocks[i as usize]);
        }
        self.terrain.each_rock(x0, y0, x1, y1, |k| f(k.seed as u64, k));
    }

    /// Every deposit still there and centred in the rectangle.
    pub fn ores_in(&self, x0: i32, y0: i32, x1: i32, y1: i32, mut f: impl FnMut(OreSpot)) {
        let mut near = vec![];
        self.ore_grid.query_rect(x0, y0, x1, y1, &mut near);
        for &id in &near {
            let Some(i) = self.ore_index(id) else { continue };
            let o = &self.ores[i];
            if o.amount > 0 {
                f(OreSpot { id: o.id as u64, x: o.x, y: o.y, amount: o.amount });
            }
        }
        self.terrain.each_deposit(x0, y0, x1, y1, |d| {
            let amount = self.worn.get(&d.id).map_or(d.amount, |w| w.left);
            if amount > 0 {
                f(OreSpot { id: d.id, x: d.x, y: d.y, amount });
            }
        });
    }

    /// Every relic with shards left and centred in the rectangle. One that
    /// a base was built over is out of reach.
    pub fn relics_in(&self, x0: i32, y0: i32, x1: i32, y1: i32, mut f: impl FnMut(RelicSpot)) {
        self.terrain.each_relic(x0, y0, x1, y1, |r| {
            let left = self.worn.get(&r.id).map_or(RELIC_SHARDS, |w| w.left);
            let gap = (RELIC_RADIUS + BASE_RADIUS) as i64;
            if left > 0 && !self.bases.iter().any(|b| dist2(b.x - r.x, b.y - r.y) < gap * gap) {
                f(RelicSpot { id: r.id, x: r.x, y: r.y, perk: Perk::of(r.kind), left });
            }
        });
    }

    pub fn relics_near(&self, x: i32, y: i32, reach: i32, f: impl FnMut(RelicSpot)) {
        self.relics_in(x - reach, y - reach, x + reach, y + reach, f);
    }

    fn rocks_near(&self, x: i32, y: i32, reach: i32, f: impl FnMut(u64, &Rock)) {
        self.rocks_in(x - reach, y - reach, x + reach, y + reach, f);
    }

    pub fn ores_near(&self, x: i32, y: i32, reach: i32, f: impl FnMut(OreSpot)) {
        self.ores_in(x - reach, y - reach, x + reach, y + reach, f);
    }

    /// Would a body of radius `r` moving from `from` to `to` hit something
    /// it was not already touching?
    pub fn blocked(&self, from: (i32, i32), to: (i32, i32), r: i32) -> bool {
        !self.inside(to.0, to.1, r) || self.hits_solid(Some(from), to, r)
    }

    /// Is there something in the way of a body of radius `r` at `at`?
    fn occupied(&self, at: (i32, i32), r: i32) -> bool {
        !self.inside(at.0, at.1, r) || self.hits_solid(None, at, r)
    }

    fn hits_solid(&self, from: Option<(i32, i32)>, to: (i32, i32), r: i32) -> bool {
        let hits = |ox: i32, oy: i32, or: i32| {
            let lim = (or + r) as i64 * (or + r) as i64;
            dist2(ox - to.0, oy - to.1) < lim && from.is_none_or(|f| dist2(ox - f.0, oy - f.1) >= lim)
        };
        let mut hit = false;
        self.rocks_near(to.0, to.1, r + MAX_ROCK_R, |_, k| hit = hit || hits(k.x, k.y, k.r));
        if hit {
            return true;
        }
        self.ores_near(to.0, to.1, r + 2 * FP, |o| hit = hit || hits(o.x, o.y, o.radius()));
        self.relics_near(to.0, to.1, r + RELIC_RADIUS, |k| hit = hit || hits(k.x, k.y, RELIC_RADIUS));
        hit || self.bases.iter().any(|b| b.hp > 0 && hits(b.x, b.y, BASE_RADIUS))
    }

    /// Nearest `target` seen by a dronoid of `owner` at (x, y).
    pub fn nearest(&self, x: i32, y: i32, owner: u8, me: u32, target: i32) -> Option<Target> {
        let mut best: Option<Target> = None;
        let mut consider = |t: Target, range: i32| {
            if t.gap <= range && best.is_none_or(|b| (t.gap, t.id) < (b.gap, b.id)) {
                best = Some(t);
            }
        };
        let mut near = vec![];
        match target {
            thing::BASE | thing::EBASE => {
                let own = target == thing::BASE;
                let range = if own { i32::MAX } else { EBASE_RANGE };
                for b in self.bases.iter().filter(|b| b.hp > 0 && (b.owner == owner) == own) {
                    let (dx, dy) = (b.x - x, b.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONOID_RADIUS, BASE_RADIUS), id: b.id as u64 }, range);
                }
            }
            thing::ORE => {
                self.ores_near(x, y, SENSOR_RANGE + 2 * FP, |o| {
                    let (dx, dy) = (o.x - x, o.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONOID_RADIUS, o.radius()), id: o.id }, SENSOR_RANGE);
                });
            }
            thing::WALL => {
                self.rocks_near(x, y, SENSOR_RANGE + MAX_ROCK_R, |id, k| {
                    let (dx, dy) = (k.x - x, k.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONOID_RADIUS, k.r), id }, SENSOR_RANGE);
                });
            }
            thing::RELIC => {
                self.relics_near(x, y, SENSOR_RANGE + RELIC_RADIUS, |k| {
                    let (dx, dy) = (k.x - x, k.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONOID_RADIUS, RELIC_RADIUS), id: k.id }, SENSOR_RANGE);
                });
            }
            thing::ALLY | thing::ENEMY => {
                let ally = target == thing::ALLY;
                self.dronoid_grid.query(x, y, SENSOR_RANGE + 2 * FP, &mut near);
                for &i in &near {
                    let Some(d) = self.dronoids.get(i as usize) else { continue };
                    if d.hp <= 0 || d.id == me || (d.owner == owner) != ally {
                        continue;
                    }
                    let (dx, dy) = (d.x - x, d.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONOID_RADIUS, DRONOID_RADIUS), id: d.id as u64 }, SENSOR_RANGE);
                }
            }
            _ => {}
        }
        best
    }

    pub fn count_near(&self, x: i32, y: i32, owner: u8, me: u32, target: i32) -> i32 {
        let mut near = vec![];
        let within = |dx: i32, dy: i32, r: i32, range: i32| gap_between(dx, dy, DRONOID_RADIUS, r) <= range;
        match target {
            thing::BASE | thing::EBASE => {
                let own = target == thing::BASE;
                let range = if own { SENSOR_RANGE } else { EBASE_RANGE };
                self.bases
                    .iter()
                    .filter(|b| b.hp > 0 && (b.owner == owner) == own && within(b.x - x, b.y - y, BASE_RADIUS, range))
                    .count() as i32
            }
            thing::ORE => {
                let mut n = 0;
                self.ores_near(x, y, SENSOR_RANGE + 2 * FP, |o| n += within(o.x - x, o.y - y, o.radius(), SENSOR_RANGE) as i32);
                n
            }
            thing::WALL => {
                let mut n = 0;
                self.rocks_near(x, y, SENSOR_RANGE + MAX_ROCK_R, |_, k| n += within(k.x - x, k.y - y, k.r, SENSOR_RANGE) as i32);
                n
            }
            thing::RELIC => {
                let mut n = 0;
                self.relics_near(x, y, SENSOR_RANGE + RELIC_RADIUS, |k| n += within(k.x - x, k.y - y, RELIC_RADIUS, SENSOR_RANGE) as i32);
                n
            }
            thing::ALLY | thing::ENEMY => {
                let ally = target == thing::ALLY;
                self.dronoid_grid.query(x, y, SENSOR_RANGE + 2 * FP, &mut near);
                near.iter()
                    .filter_map(|&i| self.dronoids.get(i as usize))
                    .filter(|d| {
                        d.hp > 0
                            && d.id != me
                            && (d.owner == owner) == ally
                            && within(d.x - x, d.y - y, DRONOID_RADIUS, SENSOR_RANGE)
                    })
                    .count() as i32
            }
            _ => 0,
        }
    }

    /// What lies `LOOK_DIST` ahead of (x, y) in direction `angle`.
    pub fn look(&self, x: i32, y: i32, owner: u8, me: u32, angle: i32) -> i32 {
        if angle < 0 {
            return vm::NONE;
        }
        let (dx, dy) = dir_vec(angle, LOOK_DIST);
        let (px, py) = (x + dx, y + dy);
        if !self.inside(px, py, DRONOID_RADIUS) {
            return thing::WALL;
        }
        let touches = |ox: i32, oy: i32, r: i32| dist2(ox - px, oy - py) < (r + DRONOID_RADIUS) as i64 * (r + DRONOID_RADIUS) as i64;
        let mut wall = false;
        self.rocks_near(px, py, MAX_ROCK_R + FP, |_, k| wall = wall || touches(k.x, k.y, k.r));
        if wall {
            return thing::WALL;
        }
        if let Some(b) = self.bases.iter().find(|b| b.hp > 0 && touches(b.x, b.y, BASE_RADIUS)) {
            return if b.owner == owner { thing::BASE } else { thing::EBASE };
        }
        let mut ore = false;
        self.ores_near(px, py, 2 * FP, |o| ore = ore || touches(o.x, o.y, o.radius()));
        if ore {
            return thing::ORE;
        }
        let mut relic = false;
        self.relics_near(px, py, 2 * FP, |k| relic = relic || touches(k.x, k.y, RELIC_RADIUS));
        if relic {
            return thing::RELIC;
        }
        let mut near = vec![];
        self.dronoid_grid.query(px, py, 2 * FP, &mut near);
        let mut found = thing::EMPTY;
        for &i in &near {
            let Some(d) = self.dronoids.get(i as usize) else { continue };
            if d.hp > 0 && d.id != me && touches(d.x, d.y, DRONOID_RADIUS) {
                found = if d.owner == owner { thing::ALLY } else { thing::ENEMY };
                if found == thing::ENEMY {
                    break;
                }
            }
        }
        found
    }

    /// Why the base of `owner` cannot land there, if it cannot.
    pub fn can_land(&self, owner: u8, x: i32, y: i32) -> Result<(), &'static str> {
        let Some(p) = self.players.get(owner as usize) else { return Err("unknown player") };
        if !p.alive {
            return Err("you are eliminated");
        }
        if !self.is_landing(owner) {
            return Err("your base has landed already");
        }
        if !self.inside(x, y, BASE_RADIUS + FP) {
            return Err("out of the sector");
        }
        let clear = LANDING_CLEARANCE as i64 * LANDING_CLEARANCE as i64;
        if self.bases.iter().any(|b| dist2(b.x - x, b.y - y) < clear) {
            return Err("too close to another colony");
        }
        if self.occupied((x, y), BASE_RADIUS + FP / 2) {
            return Err("the spot is taken");
        }
        Ok(())
    }

    /// A player owning nothing yet: their base has still to land.
    pub fn is_landing(&self, owner: u8) -> bool {
        self.base_of(owner).is_none() && !self.dronoids.iter().any(|d| d.owner == owner)
    }

    pub fn shielded(&self, owner: u8) -> bool {
        self.players.get(owner as usize).is_some_and(|p| p.shield_until > self.tick)
    }

    pub fn state_hash(&self) -> u64 {
        let mut h = Fnv::default();
        h.add(self.tick as i64);
        h.add(self.next_id as i64);
        h.add(self.rng.state() as i64);
        h.add(self.programs.len() as i64);
        for p in &self.players {
            h.add(p.minerals as i64);
            h.add(p.alive as i64);
            h.add(p.shield_until as i64);
            h.add(p.quests as i64);
            h.add(p.flags as i64);
        }
        for b in &self.bases {
            for v in [b.id as i32, b.owner as i32, b.x, b.y, b.hp, b.capacity as i32, b.spawned as i32] {
                h.add(v as i64);
            }
            for m in b.mem {
                h.add(m as i64);
            }
            for s in b.perks {
                h.add(s as i64);
            }
            h.add(b.refined as i64);
            for l in &b.lines {
                for v in [l.id as i32, l.prog as i32, l.timer, l.enabled as i32, l.spawned as i32, l.quota as i32] {
                    h.add(v as i64);
                }
            }
        }
        for d in &self.dronoids {
            for v in [d.id as i32, d.x, d.y, d.hp, d.cargo, d.busy, d.heading, d.vm.pc as i32, d.vm.ok, d.prog as i32, d.line as i32] {
                h.add(v as i64);
            }
            for r in d.vm.regs {
                h.add(r as i64);
            }
            h.add(d.vm.rng as i64);
        }
        for o in &self.ores {
            h.add(o.id as i64 * 977 + o.amount as i64);
        }
        for (id, w) in &self.worn {
            h.add((*id as i64).wrapping_mul(977).wrapping_add(w.left as i64));
        }
        h.0
    }

    /// The only player left standing, once there were at least two.
    pub fn winner(&self) -> Option<u8> {
        if self.players.len() < 2 {
            return None;
        }
        let mut alive = self.players.iter().filter(|p| p.alive);
        match (alive.next(), alive.next()) {
            (Some(p), None) => Some(p.id),
            _ => None,
        }
    }

    // ----- mutation ------------------------------------------------------------

    fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Has `owner` ever seen the point (x, y)?
    pub fn explored(&self, owner: u8, x: i32, y: i32) -> bool {
        self.players.get(owner as usize).is_some_and(|p| p.seen.get(x.div_euclid(FOG_CELL), y.div_euclid(FOG_CELL)))
    }

    /// Mark everything within `r` of (x, y) as explored by `owner`.
    fn reveal(&mut self, owner: u8, x: i32, y: i32, r: i32) {
        let Some(p) = self.players.get_mut(owner as usize) else { return };
        let r2 = r as i64 * r as i64;
        let (x0, x1) = ((x - r).div_euclid(FOG_CELL), (x + r).div_euclid(FOG_CELL));
        let (y0, y1) = ((y - r).div_euclid(FOG_CELL), (y + r).div_euclid(FOG_CELL));
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                let (mx, my) = (cx * FOG_CELL + FOG_CELL / 2, cy * FOG_CELL + FOG_CELL / 2);
                if dist2(mx - x, my - y) <= r2 {
                    p.seen.set(cx, cy);
                }
            }
        }
    }

    fn new_line(&mut self, prog: u32) -> Line {
        Line { id: self.new_id(), prog, timer: 0, enabled: true, spawned: 0, quota: DEFAULT_QUOTA }
    }

    fn add_base(&mut self, owner: u8, x: i32, y: i32, prog: u32) {
        self.reveal(owner, x, y, VISION_BASE);
        let id = self.new_id();
        let lines = (0..BASE_LINES).map(|_| self.new_line(prog)).collect();
        // Ids only grow: the list stays sorted.
        self.bases.push(Base {
            id,
            owner,
            x,
            y,
            hp: BASE_HP,
            capacity: BASE_CAPACITY,
            mem: [0; HIVE_MEM],
            lines,
            perks: [0; PERKS],
            refined: 0,
            spawned: 0,
        });
    }

    /// The nearest starting place, working outwards, far enough from
    /// everyone already settled.
    fn pick_slot(&self) -> (i32, i32) {
        let origin = (self.size / 2, self.size / 2);
        let (ci, cj) = Terrain::site_cell(origin.0, origin.1);
        let clear = NEWCOMER_GAP as i64 * NEWCOMER_GAP as i64;
        let free = |s: (i32, i32)| {
            let far = |x: i32, y: i32| dist2(x - s.0, y - s.1) >= clear;
            self.bases.iter().all(|b| far(b.x, b.y)) && self.players.iter().filter(|p| p.alive).all(|p| far(p.home.0, p.home.1))
        };
        for ring in 0..200i32 {
            for dj in -ring..=ring {
                // Only the edge of the ring is new.
                let step = if dj.abs() == ring { 1 } else { (2 * ring).max(1) };
                for di in (-ring..=ring).step_by(step as usize) {
                    if let Some(s) = self.terrain.site(ci + di, cj + dj).filter(|s| free(*s)) {
                        return s;
                    }
                }
            }
        }
        origin
    }

    /// Bring a player into the world (or back to life). Returns its id.
    pub fn join(&mut self, name: &str, bot: bool) -> Option<u8> {
        let existing = self.players.iter().position(|p| p.name == name);
        if let Some(i) = existing {
            if self.players[i].alive {
                return Some(i as u8);
            }
        } else if self.players.len() >= MAX_PLAYERS {
            return None;
        }
        let home = self.pick_slot();
        let shield_until = self.tick + SHIELD_TICKS;
        let id = match existing {
            Some(i) => {
                let p = &mut self.players[i];
                p.alive = true;
                p.minerals = p.minerals.max(START_MINERALS);
                p.home = home;
                p.shield_until = shield_until;
                i as u8
            }
            None => {
                let id = self.players.len() as u8;
                self.players.push(Player {
                    id,
                    name: name.to_string(),
                    minerals: START_MINERALS,
                    alive: true,
                    bot,
                    mined: 0,
                    dronoids_built: 0,
                    kills: 0,
                    home,
                    shield_until,
                    seen: FogMap::default(),
                    explored: vec![],
                    quests: 0,
                    flags: 0,
                });
                id
            }
        };
        self.reveal(id, home.0, home.1, ARRIVAL_SCAN);
        // Newcomers own nothing yet: they choose where their base lands.
        self.events.push(Event::PlayerJoined { player: id });
        Some(id)
    }

    /// Mined deposits of the generated terrain end up whole again, except
    /// under a base landed in the meantime.
    fn restore_deposits(&mut self) {
        if !self.tick.is_multiple_of(REGROW_TICKS) {
            return;
        }
        let tick = self.tick;
        let due: Vec<(u64, Worn)> = self.worn.iter().filter(|(_, w)| w.until <= tick).map(|(id, w)| (*id, *w)).collect();
        for (id, w) in due {
            let spot = self.terrain.deposit(id);
            let covered = spot.is_some_and(|d| {
                let gap = (ore_radius(d.amount) + BASE_RADIUS) as i64;
                self.bases.iter().any(|b| dist2(b.x - d.x, b.y - d.y) < gap * gap)
            });
            if covered {
                self.worn.insert(id, Worn { until: tick + DEPOSIT_REGROW_TICKS, ..w });
                continue;
            }
            self.worn.remove(&id);
            if let Some(d) = spot.filter(|_| w.left <= 0) {
                self.events.push(Event::OreGrown { x: d.x, y: d.y });
            }
            if let Some(r) = self.terrain.relic(id).filter(|_| w.left <= 0) {
                self.events.push(Event::OreGrown { x: r.x, y: r.y });
            }
        }
    }

    /// From time to time a fresh crystal field appears in the first sector
    /// of an older world, away from colonies.
    fn regrow(&mut self) {
        self.restore_deposits();
        if self.size == 0 || !self.tick.is_multiple_of(REGROW_TICKS) || self.ores.len() as u32 >= self.ore_target {
            return;
        }
        let margin = 12 * FP;
        for _ in 0..12 {
            let x = self.rng.range(margin, self.size - margin);
            let y = self.rng.range(margin, self.size - margin);
            let clear = (25 * FP) as i64;
            if self.bases.iter().any(|b| dist2(b.x - x, b.y - y) < clear * clear) {
                continue;
            }
            let n = self.rng.range(3, 8);
            let mut placed = 0;
            for _ in 0..n * 4 {
                if placed >= n {
                    break;
                }
                let (dx, dy) = dir_vec(self.rng.range(0, 360), self.rng.range(0, 4 * FP));
                let (ox, oy) = (x + dx, y + dy);
                let amount = self.rng.range(ORE_MIN, ORE_MAX + 1);
                if ox < 2 * FP || oy < 2 * FP || ox > self.size - 2 * FP || oy > self.size - 2 * FP || self.occupied((ox, oy), ore_radius(amount) + FP / 4) {
                    continue;
                }
                let id = self.new_id();
                self.ores.push(Ore { id, x: ox, y: oy, amount });
                self.ore_grid.insert(ox, oy, id);
                placed += 1;
            }
            if placed > 0 {
                self.events.push(Event::OreGrown { x, y });
                return;
            }
        }
    }

    /// Remember that a player runs a program of their own (not a built-in).
    fn note_program(&mut self, player: u8, source: &str) {
        let own = !programs::BUILTINS.iter().any(|b| b.source.trim() == source.trim());
        if own && let Some(p) = self.players.get_mut(player as usize) {
            p.flags |= quests::FLAG_OWN_PROGRAM;
        }
    }

    fn apply(&mut self, player: u8, cmd: &Command) {
        if let Command::Join { name, bot } = cmd {
            self.join(name, *bot);
            return;
        }
        if !self.players.get(player as usize).is_some_and(|p| p.alive) {
            return;
        }
        match cmd {
            Command::Join { .. } => {}
            Command::Land { x, y, name, source } => {
                if self.can_land(player, *x, *y).is_ok() {
                    let Some(prog) = self.intern_program(name, source) else { return };
                    let tick = self.tick;
                    let p = &mut self.players[player as usize];
                    // Newcomers are protected from the moment they settle.
                    p.shield_until = p.shield_until.max(tick + SHIELD_TICKS);
                    self.note_program(player, source);
                    self.add_base(player, *x, *y, prog);
                    self.events.push(Event::BaseLanded { owner: player, x: *x, y: *y });
                }
            }
            Command::SetProgram { line, name, source } => {
                let Some(bi) = self.base_index_of(player) else { return };
                let Some(li) = self.bases[bi].lines.iter().position(|l| l.id == *line) else { return };
                let Some(prog) = self.intern_program(name, source) else { return };
                self.note_program(player, source);
                self.bases[bi].lines[li].prog = prog;
                for d in self.dronoids.iter_mut().filter(|d| d.line == *line) {
                    d.prog = prog;
                    d.vm.reset();
                }
            }
            Command::UpdateProgram { name, source } => {
                let Some(prog) = self.intern_program(name, source) else { return };
                self.note_program(player, source);
                let same_name: Vec<bool> = self.programs.iter().map(|p| p.name == *name).collect();
                if let Some(bi) = self.base_index_of(player) {
                    for l in self.bases[bi].lines.iter_mut().filter(|l| same_name[l.prog as usize]) {
                        l.prog = prog;
                    }
                }
                for d in self.dronoids.iter_mut().filter(|d| d.owner == player && same_name[d.prog as usize]) {
                    if d.prog != prog {
                        d.prog = prog;
                        d.vm.reset();
                    }
                }
            }
            Command::SetQuota { line, quota } => {
                if let Some(l) = self.my_line(player, *line) {
                    l.quota = (*quota).min(MAX_QUOTA);
                }
            }
            Command::ToggleLine { line } => {
                if let Some(l) = self.my_line(player, *line) {
                    l.enabled = !l.enabled;
                }
            }
            Command::SetMemory { index, value } => {
                if let Some(bi) = self.base_index_of(player)
                    && (*index as usize) < HIVE_MEM
                {
                    self.bases[bi].mem[*index as usize] = *value;
                }
            }
            Command::Expand => {
                let Some(bi) = self.base_index_of(player) else { return };
                let p = &mut self.players[player as usize];
                let b = &mut self.bases[bi];
                if p.minerals >= EXPAND_COST && b.room() < MAX_DRONOIDS_PER_PLAYER {
                    p.minerals -= EXPAND_COST;
                    b.capacity = (b.capacity + EXPAND_SLOTS).min(MAX_DRONOIDS_PER_PLAYER as u16);
                    let capacity = b.room() as u16;
                    self.events.push(Event::Expanded { owner: player, capacity });
                }
            }
        }
    }

    /// A production line of `player`'s base.
    fn my_line(&mut self, player: u8, line: u32) -> Option<&mut Line> {
        let bi = self.base_index_of(player)?;
        self.bases[bi].lines.iter_mut().find(|l| l.id == line)
    }

    /// Advance one tick. `cmds` must be in the same order on every machine.
    pub fn step(&mut self, cmds: &[(u8, Command)]) {
        self.events.clear();
        for (p, c) in cmds {
            self.apply(*p, c);
        }
        let tick = self.tick;
        for d in self.dronoids.iter_mut().filter(|d| d.born != tick) {
            d.prev_x = d.x;
            d.prev_y = d.y;
        }
        self.produce();
        self.index_dronoids();
        for di in 0..self.dronoids.len() {
            let d = &self.dronoids[di];
            if d.hp <= 0 || d.born == self.tick {
                continue;
            }
            if d.busy == 0 {
                self.decide(di);
            }
            self.advance(di);
        }
        self.dronoids.retain(|d| d.hp > 0);
        // A base's perks go with it.
        self.bases.retain(|b| b.hp > 0);
        self.ores.retain(|o| o.amount > 0);
        // Fog of war: dronoids map what they pass by, for their owner.
        for i in 0..self.dronoids.len() {
            let (o, x, y) = (self.dronoids[i].owner, self.dronoids[i].x, self.dronoids[i].y);
            self.reveal(o, x, y, VISION_DRONOID);
        }
        self.regrow();
        self.check_players();
        self.check_quests();
        self.tick += 1;
    }

    /// Every production line under its quota builds a dronoid when its
    /// time comes, as long as the base has room and crystals.
    fn produce(&mut self) {
        let mut counts = [0usize; MAX_PLAYERS];
        let mut per_line: BTreeMap<u32, u32> = BTreeMap::new();
        for d in &self.dronoids {
            counts[d.owner as usize] += 1;
            *per_line.entry(d.line).or_default() += 1;
        }
        let repair = self.tick.is_multiple_of(BASE_REPAIR_TICKS);
        for bi in 0..self.bases.len() {
            let b = &self.bases[bi];
            let (max_hp, wait, mend, hull, room) = (b.max_hp(), b.spawn_ticks(), b.repair(), b.dronoid_hp(), b.room());
            let b = &mut self.bases[bi];
            if repair && b.hp > 0 && b.hp < max_hp {
                b.hp = (b.hp + mend).min(max_hp);
            }
            // (A hull from when Bulwark gave more stays until it is hit.)
            if b.hp <= 0 {
                continue;
            }
            let owner = b.owner as usize;
            for li in 0..self.bases[bi].lines.len() {
                let b = &mut self.bases[bi];
                let l = &mut b.lines[li];
                if l.timer > 0 {
                    l.timer -= 1;
                }
                let held = per_line.get(&l.id).copied().unwrap_or(0);
                if !l.enabled || l.timer > 0 || held >= l.quota as u32 {
                    continue;
                }
                if counts[owner] >= room || self.players[owner].minerals < DRONOID_COST {
                    continue;
                }
                let (bx, by, bid) = (b.x, b.y, b.id);
                let base_angle = (b.spawned.wrapping_mul(137).wrapping_add(bid.wrapping_mul(31)) % 360) as i32;
                let r = BASE_RADIUS + DRONOID_RADIUS + FP / 4;
                let spot = (0..12).map(|k| base_angle + k * 30).find_map(|a| {
                    let (dx, dy) = dir_vec(a, r);
                    let p = (bx + dx, by + dy);
                    (!self.blocked(p, p, DRONOID_RADIUS) && !self.touches_solid(p.0, p.1)).then_some((a, p))
                });
                let Some((angle, (x, y))) = spot else { continue };

                let id = self.new_id();
                let b = &mut self.bases[bi];
                b.spawned += 1;
                let l = &mut b.lines[li];
                // A new line gets its first dronoid out quickly.
                l.timer = if l.spawned < 1 { SPAWN_TICKS / 4 } else { wait };
                l.spawned += 1;
                let (prog, line) = (l.prog, l.id);
                self.players[owner].minerals -= DRONOID_COST;
                self.players[owner].dronoids_built += 1;
                counts[owner] += 1;
                *per_line.entry(line).or_default() += 1;
                self.dronoids.push(Dronoid {
                    id,
                    owner: owner as u8,
                    x,
                    y,
                    prev_x: bx,
                    prev_y: by,
                    heading: geo::norm_deg(angle),
                    hp: hull,
                    cargo: 0,
                    line,
                    prog,
                    vm: Vm::new(mix(self.seed, id as u64)),
                    doing: Doing::Idle,
                    busy: 0,
                    born: self.tick,
                });
                self.events.push(Event::Spawn { owner: owner as u8, x, y });
            }
        }
    }

    /// Is (x, y) inside a rock, deposit or base (for spawn points)?
    fn touches_solid(&self, x: i32, y: i32) -> bool {
        self.occupied((x, y), DRONOID_RADIUS)
    }

    fn decide(&mut self, di: usize) {
        let d = &self.dronoids[di];
        let program = self.program(d.prog);
        let mut vm = d.vm.clone();
        let bi = self.base_index_of(d.owner);
        let mut mem = bi.map(|i| self.bases[i].mem).unwrap_or([0; HIVE_MEM]);
        let view = DronoidView { world: self, owner: d.owner, id: d.id, x: d.x, y: d.y, cargo: d.cargo, hp: d.hp };
        let outcome = vm.run(&program, &view, &mut mem);
        if let Some(i) = bi {
            self.bases[i].mem = mem;
        }
        self.dronoids[di].vm = vm;
        match outcome {
            None => {
                let d = &mut self.dronoids[di];
                d.doing = Doing::Stall;
                d.busy = STALL_TICKS;
            }
            Some((action, arg)) => {
                let ok = self.start(di, action, arg);
                let d = &mut self.dronoids[di];
                d.vm.ok = ok as i32;
                if !ok {
                    d.doing = Doing::Fail;
                    d.busy = FAIL_TICKS;
                }
            }
        }
    }

    fn start(&mut self, di: usize, action: Action, arg: i32) -> bool {
        let (x, y, owner, id) = {
            let d = &self.dronoids[di];
            (d.x, d.y, d.owner, d.id)
        };
        let face = |w: &mut World, tx: i32, ty: i32| {
            let a = compass(tx - x, ty - y);
            if a >= 0 {
                w.dronoids[di].heading = a;
            }
        };
        match action {
            Action::Wait => {
                let d = &mut self.dronoids[di];
                d.doing = Doing::Wait;
                d.busy = WAIT_TICKS;
                true
            }
            Action::Move => {
                if arg < 0 {
                    return false;
                }
                let (dx, dy) = dir_vec(arg, STEP);
                let to = (x + dx, y + dy);
                if self.blocked((x, y), to, DRONOID_RADIUS) {
                    return false;
                }
                let d = &mut self.dronoids[di];
                d.heading = geo::norm_deg(arg);
                d.doing = Doing::Move { fx: x, fy: y, tx: to.0, ty: to.1 };
                d.busy = MOVE_TICKS;
                true
            }
            Action::Mine => {
                let mine_ticks = self.base_of(owner).map_or(MINE_TICKS, |b| b.mine_ticks());
                // A relic within reach, and no crystal nearer: a shard of
                // it, for the dronoid's base.
                let ore_gap = self.nearest(x, y, owner, id, thing::ORE).map_or(i32::MAX, |t| t.gap);
                if let Some(t) = self.nearest(x, y, owner, id, thing::RELIC).filter(|t| t.gap < REACH && t.gap <= ore_gap)
                    && let Some(shard) = self.take_shard(owner, t.id)
                {
                    let (tx, ty) = (x + t.dx, y + t.dy);
                    face(self, tx, ty);
                    let d = &mut self.dronoids[di];
                    d.doing = Doing::Mine { tx, ty };
                    d.busy = mine_ticks;
                    let (perk, level, depleted) = shard;
                    self.events.push(Event::Shard { owner, x, y, tx, ty, perk, level, depleted });
                    return true;
                }
                if self.dronoids[di].cargo >= CARGO_CAP {
                    return false;
                }
                let Some(t) = self.nearest(x, y, owner, id, thing::ORE).filter(|t| t.gap < REACH) else {
                    return false;
                };
                let (ox, oy) = (x + t.dx, y + t.dy);
                let left = self.take_crystal(t.id);
                self.players[owner as usize].mined += 1;
                face(self, ox, oy);
                let d = &mut self.dronoids[di];
                d.cargo += 1;
                d.doing = Doing::Mine { tx: ox, ty: oy };
                d.busy = mine_ticks;
                self.events.push(Event::Mine { owner, x, y, tx: ox, ty: oy, depleted: left <= 0 });
                true
            }
            Action::Drop => {
                let cargo = self.dronoids[di].cargo;
                if cargo == 0 {
                    return false;
                }
                let Some(t) = self.nearest(x, y, owner, id, thing::BASE).filter(|t| t.gap < REACH) else {
                    return false;
                };
                let (tx, ty) = (x + t.dx, y + t.dy);
                // A refinery makes more of what is unloaded there.
                let refined = self.base_index(t.id as u32).map_or(0, |bi| {
                    let b = &mut self.bases[bi];
                    b.refined += (cargo * REFINERY_PERCENT * b.level(Perk::Refinery)) as u32;
                    let whole = b.refined / 100;
                    b.refined %= 100;
                    whole as i32
                });
                self.players[owner as usize].minerals += cargo + refined;
                face(self, tx, ty);
                let d = &mut self.dronoids[di];
                d.cargo = 0;
                d.doing = Doing::Drop { tx, ty };
                d.busy = DROP_TICKS;
                self.events.push(Event::Drop { owner, x, y, tx, ty, amount: cargo + refined });
                true
            }
            Action::Attack => {
                // Shielded newcomers can neither shoot nor be shot.
                if self.shielded(owner) {
                    return false;
                }
                let dronoid = self
                    .nearest(x, y, owner, id, thing::ENEMY)
                    .filter(|t| t.gap <= ATTACK_RANGE && self.dronoid(t.id as u32).is_some_and(|d| !self.shielded(d.owner)));
                let base = self
                    .nearest(x, y, owner, id, thing::EBASE)
                    .filter(|t| t.gap <= ATTACK_RANGE && self.base(t.id as u32).is_some_and(|b| !self.shielded(b.owner)));
                let roll = mix(self.tick, id as u64);
                let damage = self.base_of(owner).map_or(ATTACK_DAMAGE, |b| b.damage(roll));
                let (tx, ty) = match (dronoid, base) {
                    (Some(t), b) if b.is_none_or(|b| t.gap <= b.gap) => {
                        let ti = self.dronoid_index(t.id as u32).unwrap();
                        let victim = &mut self.dronoids[ti];
                        victim.hp -= damage;
                        let (vx, vy, vo, dead) = (victim.x, victim.y, victim.owner, victim.hp <= 0);
                        if dead {
                            self.players[owner as usize].kills += 1;
                            self.events.push(Event::DronoidDestroyed { owner: vo, x: vx, y: vy });
                        }
                        (vx, vy)
                    }
                    (_, Some(b)) => {
                        let bi = self.base_index(b.id as u32).unwrap();
                        let base = &mut self.bases[bi];
                        let was_standing = base.hp > 0;
                        base.hp -= damage;
                        let (bx, by, bo, dead) = (base.x, base.y, base.owner, base.hp <= 0);
                        if dead && was_standing {
                            self.events.push(Event::BaseDestroyed { owner: bo, x: bx, y: by });
                        }
                        (bx, by)
                    }
                    _ => return false,
                };
                face(self, tx, ty);
                let d = &mut self.dronoids[di];
                d.doing = Doing::Attack { tx, ty };
                d.busy = ATTACK_TICKS;
                self.events.push(Event::Attack { owner, x, y, tx, ty });
                true
            }
        }
    }

    /// One shard of a relic, for the base of `owner`. Returns the perk, the
    /// level the base reached if it just did, and whether that was the
    /// relic's last shard; `None` if there is no base or it cannot take
    /// more of that kind (the relic is then left as it is).
    fn take_shard(&mut self, owner: u8, relic: u64) -> Option<(Perk, Option<i32>, bool)> {
        let perk = Perk::of(self.terrain.relic(relic)?.kind);
        let bi = self.base_index_of(owner)?;
        let before = self.bases[bi].level(perk);
        if self.bases[bi].shards(perk) >= SHARDS_PER_LEVEL * PERK_MAX_LEVEL {
            return None;
        }
        self.bases[bi].perks[perk.index()] += 1;
        let until = self.tick + RELIC_REGROW_TICKS;
        let worn = self.worn.entry(relic).or_insert(Worn { left: RELIC_SHARDS, until });
        worn.left -= 1;
        worn.until = until;
        let depleted = worn.left <= 0;
        let level = Some(self.bases[bi].level(perk)).filter(|l| *l > before);
        if level.is_some() {
            match perk {
                // The new plating comes whole.
                Perk::Bulwark => self.bases[bi].hp += BULWARK_HP,
                Perk::Assembly => self.add_lines(bi),
                // It sees further at once.
                Perk::Vision => {
                    let b = &self.bases[bi];
                    let (x, y, r) = (b.x, b.y, b.vision());
                    self.reveal(owner, x, y, r);
                }
                _ => {}
            }
        }
        Some((perk, level, depleted))
    }

    /// New production lines up to what the base runs, each running what
    /// its first line runs.
    fn add_lines(&mut self, bi: usize) {
        while self.bases[bi].lines.len() < self.bases[bi].lines_allowed() {
            let prog = self.bases[bi].lines.first().map_or(0, |l| l.prog);
            let line = self.new_line(prog);
            self.bases[bi].lines.push(line);
        }
    }

    /// One crystal less in a deposit. Returns how many are left.
    fn take_crystal(&mut self, id: u64) -> i32 {
        if id >= NATURAL {
            let whole = self.terrain.deposit(id).map_or(0, |d| d.amount);
            let until = self.tick + DEPOSIT_REGROW_TICKS;
            let worn = self.worn.entry(id).or_insert(Worn { left: whole, until });
            worn.left -= 1;
            worn.until = until;
            return worn.left;
        }
        let oi = self.ore_index(id as u32).unwrap();
        let o = &mut self.ores[oi];
        o.amount -= 1;
        let (ox, oy, left) = (o.x, o.y, o.amount);
        if left <= 0 {
            self.ore_grid.remove(ox, oy, id as u32);
        }
        left
    }

    fn advance(&mut self, di: usize) {
        let d = &mut self.dronoids[di];
        if d.busy <= 0 {
            return;
        }
        if let Doing::Move { fx, fy, tx, ty } = d.doing {
            let k = MOVE_TICKS - d.busy + 1;
            d.x = fx + (tx - fx) * k / MOVE_TICKS;
            d.y = fy + (ty - fy) * k / MOVE_TICKS;
        }
        d.busy -= 1;
        if d.busy == 0 && d.doing != Doing::Stall {
            d.doing = Doing::Idle;
        }
    }

    /// A colony whose base has fallen is out: its dronoids go with it.
    fn check_players(&mut self) {
        for pi in 0..self.players.len() {
            let p = &self.players[pi];
            let owner = p.id;
            if !p.alive || self.base_of(owner).is_some() || !self.dronoids.iter().any(|d| d.owner == owner) {
                continue;
            }
            self.players[pi].alive = false;
            for d in self.dronoids.iter_mut().filter(|d| d.owner == owner) {
                d.hp = 0;
                self.events.push(Event::DronoidDestroyed { owner, x: d.x, y: d.y });
            }
            self.dronoids.retain(|d| d.hp > 0);
            self.events.push(Event::PlayerEliminated { player: owner });
        }
    }
}

struct DronoidView<'a> {
    world: &'a World,
    owner: u8,
    id: u32,
    x: i32,
    y: i32,
    cargo: i32,
    hp: i32,
}

impl Senses for DronoidView<'_> {
    fn sense(&self, sensor: Sensor, arg: i32) -> i32 {
        let w = self.world;
        let near = || w.nearest(self.x, self.y, self.owner, self.id, arg);
        match sensor {
            Sensor::Cargo => self.cargo,
            Sensor::Hp => self.hp,
            Sensor::PosX => self.x.div_euclid(FP),
            Sensor::PosY => self.y.div_euclid(FP),
            Sensor::Look => w.look(self.x, self.y, self.owner, self.id, arg),
            Sensor::Scan => near().map(|t| compass(t.dx, t.dy)).unwrap_or(vm::NONE),
            Sensor::Dist => near().map(|t| t.gap / FP).unwrap_or(vm::NONE),
            Sensor::Dx => near().map(|t| t.dx / FP).unwrap_or(0),
            Sensor::Dy => near().map(|t| t.dy / FP).unwrap_or(0),
            Sensor::Count => w.count_near(self.x, self.y, self.owner, self.id, arg),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Land the base of `player` at (or near) its suggested home.
    pub(crate) fn land(w: &mut World, player: u8, source: &str) {
        let (hx, hy) = w.players[player as usize].home;
        let spot = (0..8)
            .flat_map(|r| (0..360).step_by(30).map(move |a| (r, a)))
            .map(|(r, a)| {
                let (dx, dy) = dir_vec(a, r * 2 * FP);
                (hx + dx, hy + dy)
            })
            .find(|p| w.can_land(player, p.0, p.1).is_ok())
            .expect("somewhere to land");
        w.step(&[(player, Command::Land { x: spot.0, y: spot.1, name: "miner".into(), source: source.into() })]);
        assert!(w.base_of(player).is_some());
    }

    fn world_with(players: &[&str]) -> World {
        let mut w = World::new(7);
        let m = programs::builtin("miner").unwrap();
        for (i, p) in players.iter().enumerate() {
            w.step(&[(0, Command::Join { name: p.to_string(), bot: false })]);
            land(&mut w, i as u8, m.source);
        }
        w
    }

    #[test]
    fn saves_from_before_the_rename_still_load() {
        let m = programs::builtin("miner").unwrap();
        let mut w = World::new(3);
        let id = w.join("moi", false).unwrap();
        tests::land(&mut w, id, m.source);
        for _ in 0..40 {
            w.step(&[]);
        }
        assert!(!w.dronoids.is_empty());
        let now = String::from_utf8(w.to_save()).unwrap();
        let before = now.replace("\"dronoids\":", "\"drones\":").replace("\"dronoids_built\":", "\"drones_built\":");
        assert_ne!(before, now);
        let back = World::from_save(before.as_bytes()).unwrap();
        assert_eq!(back.state_hash(), w.state_hash());
        assert_eq!(back.dronoids.len(), w.dronoids.len());
        assert_eq!(back.players[id as usize].dronoids_built, w.players[id as usize].dronoids_built);
    }

    /// Saved before the game spoke English: a former built-in's name and a
    /// bot's name take their new form, a player's own code stays as it is.
    #[test]
    fn saved_names_take_their_english_form() {
        let mut w = World::new(1);
        let bot = w.join("IA Antar\u{e8}s", true).unwrap();
        let own = "; my own guard\nWAIT\n";
        w.programs.push(ProgEntry { name: "gardien".into(), source: own.into(), code: None });
        let back = World::from_save(&w.to_save()).unwrap();
        let p = back.programs.last().unwrap();
        assert_eq!((p.name.as_str(), p.source.as_str()), ("guard", own));
        assert_eq!(back.players[bot as usize].name, "AI Antares");
    }

    #[test]
    fn newcomers_own_nothing_but_a_landing() {
        let mut w = World::new(3);
        let id = w.join("nouveau", false).unwrap();
        assert!(w.base_of(id).is_none());
        assert_eq!(w.dronoid_count(id), 0);
        assert_eq!(w.players[0].minerals, START_MINERALS);
        assert!(w.is_landing(id));
        for _ in 0..50 {
            w.step(&[]);
        }
        assert!(w.players[0].alive, "waiting to land is not losing");
        // The landing may be far from everything: we own nothing yet.
        let (hx, hy) = w.players[0].home;
        let far = hx + 200 * FP;
        let m = programs::builtin("miner").unwrap();
        let spot = (0..40)
            .map(|k| (far - k * FP, hy))
            .find(|p| w.can_land(id, p.0, p.1).is_ok())
            .expect("a free spot");
        w.step(&[(id, Command::Land { x: spot.0, y: spot.1, name: m.name.into(), source: m.source.into() })]);
        let b = w.base_of(id).expect("landed");
        assert_eq!((b.x, b.y, b.hp, b.capacity), (spot.0, spot.1, BASE_HP, BASE_CAPACITY));
        assert_eq!(b.lines.len(), BASE_LINES);
        // The landing is free; every line spends on its first dronoid right away.
        assert_eq!(w.players[0].minerals, START_MINERALS - BASE_LINES as i32 * DRONOID_COST);
        assert_eq!(w.dronoid_count(id), BASE_LINES);
        assert_eq!(w.can_land(id, spot.0 + 50 * FP, spot.1), Err("your base has landed already"));
    }

    /// Lines build in parallel, each up to its quota, all together up to
    /// what the hangar holds; crystals enlarge it.
    #[test]
    fn production_lines_share_the_hangar() {
        let mut w = world_with(&["a"]);
        w.players[0].minerals = 10_000;
        let lines: Vec<u32> = w.bases[0].lines.iter().map(|l| l.id).collect();
        let scout = programs::builtin("scout").unwrap();
        w.step(&[(0, Command::SetProgram { line: lines[1], name: scout.name.into(), source: scout.source.into() })]);
        w.step(&[(0, Command::SetQuota { line: lines[0], quota: 3 })]);
        w.step(&[(0, Command::SetQuota { line: lines[1], quota: 20 })]);
        for _ in 0..SPAWN_TICKS as u64 * 12 {
            w.step(&[]);
        }
        let on = |w: &World, l: u32| w.dronoids_of_line(l);
        assert_eq!(on(&w, lines[0]), 3, "a line stops at its quota");
        assert_eq!(on(&w, lines[1]), BASE_CAPACITY as usize - 3, "and the hangar at its size");
        assert!(w.dronoids.iter().filter(|d| d.line == lines[1]).all(|d| w.program_name(d.prog) == "scout"));

        let before = w.players[0].minerals;
        w.step(&[(0, Command::ToggleLine { line: lines[1] }), (0, Command::Expand)]);
        assert_eq!(w.bases[0].capacity, BASE_CAPACITY + EXPAND_SLOTS);
        assert_eq!(w.players[0].minerals, before - EXPAND_COST);
        for _ in 0..SPAWN_TICKS as u64 * 8 {
            w.step(&[]);
        }
        assert_eq!(w.dronoid_count(0), BASE_CAPACITY as usize, "a paused line builds nothing");
        w.step(&[(0, Command::SetQuota { line: lines[0], quota: 50 })]);
        for _ in 0..SPAWN_TICKS as u64 * 8 {
            w.step(&[]);
        }
        assert_eq!(w.dronoid_count(0), (BASE_CAPACITY + EXPAND_SLOTS) as usize);
        // Nobody else's line, and never without the crystals.
        w.players[0].minerals = EXPAND_COST - 1;
        w.step(&[(0, Command::Expand)]);
        assert_eq!(w.bases[0].capacity, BASE_CAPACITY + EXPAND_SLOTS);
    }

    #[test]
    fn exploration_is_remembered_per_player() {
        let mut w = world_with(&["a", "b"]);
        let (ax, ay) = w.players[0].home;
        let (bx, by) = w.players[1].home;
        assert!(w.explored(0, ax, ay), "arrival scan");
        assert!(!w.explored(0, bx, by), "the other colony is far away and unknown");
        // A dronoid far from home maps what it passes by.
        let far = (ax + 80 * FP, ay);
        assert!(!w.explored(0, far.0, far.1));
        let di = w.dronoids.iter().position(|d| d.owner == 0).expect("a dronoid");
        w.dronoids[di].x = far.0;
        w.dronoids[di].y = far.1;
        w.dronoids[di].doing = Doing::Idle;
        w.dronoids[di].busy = 1;
        w.step(&[]);
        assert!(w.explored(0, far.0, far.1));
        assert!(w.explored(0, far.0 + VISION_DRONOID / 2, far.1));
        assert!(!w.explored(1, far.0, far.1), "exploration is per player");
        let copy = World::from_bytes(&w.to_bytes()).unwrap();
        assert_eq!(copy.players[0].seen, w.players[0].seen);
        assert!(!w.explored(0, -9000 * FP, 3) && !w.explored(0, 9000 * FP, 0));
    }

    #[test]
    fn mined_deposits_are_all_that_is_kept_and_they_come_back() {
        let mut w = world_with(&["a"]);
        let (hx, hy) = w.players[0].home;
        let mut found = None;
        w.ores_near(hx, hy, 40 * FP, |o| found = found.or(Some(o)));
        let o = found.expect("crystals by the starting place");
        assert_eq!(w.take_crystal(o.id), o.amount - 1);

        let copy = World::from_bytes(&w.to_bytes()).unwrap();
        assert_eq!(copy.worn.get(&o.id), w.worn.get(&o.id));
        let mut amount = None;
        copy.ores_near(o.x, o.y, FP, |q| {
            if q.id == o.id {
                amount = Some(q.amount);
            }
        });
        assert_eq!(amount, Some(o.amount - 1));

        w.tick = w.worn[&o.id].until.next_multiple_of(REGROW_TICKS);
        w.step(&[]);
        assert!(!w.worn.contains_key(&o.id));
    }

    #[test]
    fn newcomers_to_an_older_world_arrive_around_its_first_sector() {
        let mut w = World::with_first_sector(4);
        let id = w.join("nouveau", false).unwrap();
        let (x, y) = w.players[id as usize].home;
        assert!(x < 0 || y < 0 || x >= w.size || y >= w.size, "{},{}", x / FP, y / FP);
        assert!(w.explored(id, x, y));
        assert!(!w.rocks.is_empty(), "the first sector keeps its asteroids");
        let mut crystals = 0;
        w.ores_near(x, y, 30 * FP, |_| crystals += 1);
        assert!(crystals > 0);
    }

    #[test]
    fn the_far_reaches_work_like_anywhere_else() {
        let mut w = world_with(&["a"]);
        let scout = programs::builtin("scout").unwrap();
        for l in w.bases[0].lines.iter().map(|l| l.id).collect::<Vec<_>>() {
            w.step(&[(0, Command::SetProgram { line: l, name: scout.name.into(), source: scout.source.into() })]);
        }
        let far = (-WORLD_EXTENT + 60 * FP, WORLD_EXTENT - 60 * FP);
        for (i, d) in w.dronoids.iter_mut().enumerate() {
            d.x = far.0 + i as i32 * 2 * FP;
            d.y = far.1;
            d.doing = Doing::Idle;
            d.busy = 1;
        }
        let at_start = w.dronoids.len();
        for _ in 0..400 {
            w.step(&[]);
        }
        assert!(at_start > 0 && w.explored(0, far.0, far.1));
        assert!(w.dronoids.iter().any(|d| d.x < -WORLD_EXTENT / 2 && (d.x, d.y) != far), "a scout got moving out there");
        // The very edge stops bodies, and only that.
        assert!(w.blocked((WORLD_EXTENT - FP, 0), (WORLD_EXTENT, 0), DRONOID_RADIUS));
    }

    #[test]
    fn players_spawn_far_apart() {
        let w = world_with(&["a", "b", "c", "d"]);
        assert_eq!(w.players.len(), 4);
        for a in &w.players {
            for b in w.players.iter().filter(|b| b.id != a.id) {
                let d = dist(a.home.0 - b.home.0, a.home.1 - b.home.1) / FP;
                assert!(d > 80, "players {} and {} only {d} units apart", a.id, b.id);
            }
        }
    }

    #[test]
    fn miners_bring_minerals_home() {
        let mut w = world_with(&["a"]);
        for _ in 0..3000 {
            w.step(&[]);
        }
        assert!(w.players[0].mined > 40, "mined {}", w.players[0].mined);
        assert!(w.dronoid_count(0) >= 3, "dronoids {}", w.dronoid_count(0));
    }

    #[test]
    fn snapshot_roundtrip_and_determinism() {
        let mut a = world_with(&["a", "b"]);
        for _ in 0..400 {
            a.step(&[]);
        }
        let bytes = a.to_bytes();
        let mut b = World::from_bytes(&bytes).unwrap();
        assert_eq!(a.rocks, b.rocks, "rocks are rebuilt from the seed");
        assert_eq!(a.state_hash(), b.state_hash());
        let hunter = programs::builtin("hunter").unwrap();
        for t in 0..1500u64 {
            let cmds = if t == 100 {
                vec![(1u8, Command::UpdateProgram { name: "miner".into(), source: hunter.source.into() })]
            } else {
                vec![]
            };
            a.step(&cmds);
            b.step(&cmds);
        }
        assert_eq!(a.state_hash(), b.state_hash());
    }

    #[test]
    fn crystals_grow_back() {
        let mut w = World::with_first_sector(9);
        let target = w.ores.len();
        w.ores.truncate(target / 2);
        w.rebuild_caches();
        for _ in 0..REGROW_TICKS * 30 {
            w.step(&[]);
        }
        assert!(w.ores.len() > target / 2 + 20, "{} deposits, target {target}", w.ores.len());
    }

    #[test]
    fn shield_protects_newcomers() {
        let mut w = world_with(&["vieux", "nouveau"]);
        w.players[0].shield_until = 0;
        let target = w.base_of(1).unwrap().clone();
        let hunter = programs::builtin("hunter").unwrap();
        w.step(&[(0, Command::UpdateProgram { name: "miner".into(), source: hunter.source.into() })]);
        let park = |w: &mut World| {
            for d in w.dronoids.iter_mut().filter(|d| d.owner == 0) {
                d.x = target.x + BASE_RADIUS + 2 * FP;
                d.y = target.y;
                d.busy = 0;
            }
            w.index_dronoids();
        };
        park(&mut w);
        for _ in 0..200 {
            w.step(&[]);
        }
        assert_eq!(w.base(target.id).unwrap().hp, BASE_HP, "shielded base took damage");
        // Once the shield is over, the same hunters do hurt.
        w.players[1].shield_until = 0;
        park(&mut w);
        for _ in 0..200 {
            w.step(&[]);
        }
        assert!(w.base(target.id).is_none_or(|b| b.hp < BASE_HP));
    }

    #[test]
    fn a_damaged_base_mends_itself() {
        let mut w = world_with(&["a"]);
        let id = w.bases[0].id;
        w.bases[0].hp = BASE_HP - 3;
        w.tick = w.tick.next_multiple_of(BASE_REPAIR_TICKS) + 1;
        for _ in 0..BASE_REPAIR_TICKS {
            w.step(&[]);
        }
        assert_eq!(w.base(id).unwrap().hp, BASE_HP - 2, "one point per period");
        for _ in 0..4 * BASE_REPAIR_TICKS {
            w.step(&[]);
        }
        assert_eq!(w.base(id).unwrap().hp, BASE_HP, "and never more than whole");
    }

    /// When its base falls, a colony is gone, dronoids and all; it may land
    /// again.
    #[test]
    fn a_fallen_base_takes_the_colony_with_it() {
        let mut w = world_with(&["a", "b"]);
        for _ in 0..40 {
            w.step(&[]);
        }
        assert!(w.dronoid_count(1) > 0);
        w.bases.iter_mut().find(|b| b.owner == 1).unwrap().hp = 0;
        w.step(&[]);
        assert!(!w.players[1].alive);
        assert_eq!(w.dronoid_count(1), 0);
        assert!(w.events.contains(&Event::PlayerEliminated { player: 1 }));
        assert!(w.players[0].alive);
        w.step(&[(1, Command::Join { name: "b".into(), bot: false })]);
        assert!(w.players[1].alive);
        assert!(w.is_landing(1));
        assert!(w.players[1].minerals >= START_MINERALS);
        assert_eq!(w.players.len(), 2);
    }

    #[test]
    fn landing_rules() {
        let mut w = world_with(&["a"]);
        let b0 = w.bases[0].clone();
        w.step(&[(0, Command::Join { name: "b".into(), bot: false })]);
        assert_eq!(w.can_land(1, b0.x, b0.y), Err("too close to another colony"));
        assert_eq!(w.can_land(1, b0.x + 20 * FP, b0.y), Err("too close to another colony"));
        assert_eq!(w.can_land(1, WORLD_EXTENT, 0), Err("out of the sector"));
        assert_eq!(w.can_land(9, b0.x + 300 * FP, b0.y), Err("unknown player"));
        // Another player's line is not ours to change.
        let theirs = b0.lines[0].id;
        let land_far = (b0.x + 300 * FP..b0.x + 340 * FP).step_by(FP as usize).find(|x| w.can_land(1, *x, b0.y).is_ok()).unwrap();
        let m = programs::builtin("miner").unwrap();
        w.step(&[(1, Command::Land { x: land_far, y: b0.y, name: m.name.into(), source: m.source.into() })]);
        w.step(&[(1, Command::SetQuota { line: theirs, quota: 0 })]);
        assert_eq!(w.base_of(0).unwrap().lines[0].quota, DEFAULT_QUOTA);
    }
}

#[cfg(test)]
mod better_programs {
    use super::*;

    fn mined_with(source: &str, seed: u64, ticks: u64) -> i64 {
        let mut w = World::new(seed);
        w.join("solo", false);
        tests::land(&mut w, 0, source);
        for _ in 0..ticks {
            w.step(&[]);
        }
        w.players[0].mined
    }

    #[test]
    #[ignore]
    fn compare_horizons() {
        let better = include_str!("../../examples/miner_plus.dasm");
        let base = programs::builtin("miner").unwrap().source;
        for ticks in [4000u64, 8000, 14000] {
            let (mut a, mut b) = (0, 0);
            for seed in 1..=4 {
                a += mined_with(base, seed, ticks);
                b += mined_with(better, seed, ticks);
            }
            println!("{ticks} ticks: miner {a}  miner+ {b}");
        }
    }

    /// What a newcomer who touches nothing lives through, minute by minute.
    /// `cargo test --release --lib balance_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn balance_report() {
        let base = programs::builtin("miner").unwrap().source;
        for seed in 1..=6u64 {
            let mut w = World::new(seed);
            w.join("solo", false);
            tests::land(&mut w, 0, base);
            let minute = (60.0 * TICKS_PER_SEC) as u64;
            let mut line = format!("seed {seed}:");
            let mut goal = None;
            let mut second = None;
            for t in 1..=30 * minute {
                w.step(&[]);
                if goal.is_none() && w.players[0].mined >= quests::MINED_GOAL {
                    goal = Some(t as f32 / minute as f32);
                }
                if second.is_none() && w.players[0].minerals >= EXPAND_COST {
                    second = Some(t as f32 / minute as f32);
                }
                if t % (5 * minute) == 0 || t == 2 * minute {
                    let p = &w.players[0];
                    line += &format!("  {}min {}c/{}m/{}d/{}seen", t / minute, p.minerals, p.mined, w.dronoid_count(0), p.seen.count());
                }
            }
            println!("{line}  | quest {goal:?} min, hangar affordable {second:?} min");
        }
    }

    /// How long scouts take to map the surroundings, and how long a base
    /// lasts under a raid, guarded or not.
    #[test]
    #[ignore]
    fn balance_report_scouts_and_raids() {
        let minute = (60.0 * TICKS_PER_SEC) as u64;
        let scout = programs::builtin("scout").unwrap();
        for seed in 1..=4u64 {
            let mut w = World::new(seed);
            w.join("solo", false);
            tests::land(&mut w, 0, scout.source);
            let mut at = None;
            for t in 1..=30 * minute {
                w.step(&[]);
                if at.is_none() && w.players[0].seen.count() >= quests::MAPPED_GOAL {
                    at = Some(t as f32 / minute as f32);
                }
            }
            println!("seed {seed}: scouts map {} cells in {at:?} min; {} after 30 min", quests::MAPPED_GOAL, w.players[0].seen.count());
        }
        let hunter = programs::builtin("hunter").unwrap();
        let guard = programs::builtin("guard").unwrap();
        for (raiders, guards) in [(8usize, 0usize), (8, 8), (16, 8), (4, 0)] {
            let mut w = World::new(7);
            w.join("a", false);
            w.join("d", false);
            tests::land(&mut w, 0, hunter.source);
            tests::land(&mut w, 1, guard.source);
            for p in &mut w.players {
                p.shield_until = 0;
                p.minerals = 0;
            }
            let target = w.base_of(1).unwrap().clone();
            let home = w.base_of(0).unwrap().clone();
            let (gbase, gline, gprog) = (target.id, target.lines[0].id, target.lines[0].prog);
            let (hline, hprog) = (home.lines[0].id, home.lines[0].prog);
            w.dronoids.clear();
            let put = |w: &mut World, owner: u8, line: u32, prog: u32, n: usize, r: i32| {
                for k in 0..n {
                    let (dx, dy) = dir_vec(k as i32 * 360 / n.max(1) as i32, r);
                    let id = w.new_id();
                    w.dronoids.push(Dronoid {
                        id,
                        owner,
                        x: target.x + dx,
                        y: target.y + dy,
                        prev_x: target.x + dx,
                        prev_y: target.y + dy,
                        heading: 0,
                        hp: DRONOID_HP,
                        cargo: 0,
                        line,
                        prog,
                        vm: Vm::new(id as u64),
                        doing: Doing::Idle,
                        busy: 0,
                        born: 0,
                    });
                }
            };
            put(&mut w, 1, gline, gprog, guards, BASE_RADIUS + 2 * FP);
            put(&mut w, 0, hline, hprog, raiders, 12 * FP);
            let mut fell = None;
            for t in 1..=30 * minute {
                w.step(&[]);
                if w.base(gbase).is_none() {
                    fell = Some(t as f32 / TICKS_PER_SEC);
                    break;
                }
            }
            println!(
                "{raiders} hunters vs {guards} guards: base fell after {fell:?} s; left {} hunters, {} guards, {:?} hp",
                w.dronoid_count(0),
                w.dronoid_count(1),
                w.base(gbase).map(|b| b.hp)
            );
        }
    }

    /// The whole point of the game: a smarter program out-mines the default.
    #[test]
    fn example_program_beats_builtin_miner() {
        let better = include_str!("../../examples/miner_plus.dasm");
        assert!(vm::assemble("p", better).is_ok());
        let base = programs::builtin("miner").unwrap().source;
        let (mut a, mut b) = (0, 0);
        for seed in 1..=4 {
            a += mined_with(base, seed, 8000);
            b += mined_with(better, seed, 8000);
        }
        println!("miner: {a}  miner+: {b}");
        assert!(b > a, "miner+ {b} should beat miner {a}");
    }
}

#[cfg(test)]
mod relic_tests {
    use super::tests::land;
    use super::*;

    /// One player, landed, running `program`, with its first dronoids out.
    fn colony(program: &str) -> World {
        let mut w = World::new(7);
        let p = programs::builtin(program).unwrap();
        w.step(&[(0, Command::Join { name: "a".into(), bot: false })]);
        land(&mut w, 0, p.source);
        for _ in 0..80 {
            w.step(&[]);
        }
        assert!(!w.dronoids.is_empty());
        w
    }

    fn nearest_relic(w: &World) -> RelicSpot {
        let (hx, hy) = w.players[0].home;
        let mut best: Option<RelicSpot> = None;
        w.relics_near(hx, hy, 600 * FP, |r| {
            if best.is_none_or(|b| dist2(r.x - hx, r.y - hy) < dist2(b.x - hx, b.y - hy)) {
                best = Some(r);
            }
        });
        best.expect("a relic within a few hundred units")
    }

    #[test]
    fn prospectors_mine_a_relic_for_their_base() {
        let mut w = colony("prospector");
        let relic = nearest_relic(&w);
        let (hx, hy) = w.players[0].home;
        assert!(dist2(relic.x - hx, relic.y - hy) > (40 * FP as i64).pow(2), "relics are not handed out at the door");
        assert_eq!(relic.left, RELIC_SHARDS);
        // It is a body like any other.
        assert!(w.blocked((relic.x + 3 * FP, relic.y), (relic.x + FP, relic.y), DRONOID_RADIUS));

        let d = &mut w.dronoids[0];
        (d.x, d.y, d.doing, d.busy) = (relic.x + 5 * FP, relic.y, Doing::Idle, 1);
        d.vm.reset();
        w.dronoids.truncate(1);
        for l in &mut w.bases[0].lines {
            l.quota = 0;
        }
        let mut levels = vec![];
        for _ in 0..(RELIC_SHARDS as usize + 4) * MINE_TICKS as usize + 80 {
            w.step(&[]);
            levels.extend(w.events.iter().filter_map(|e| match e {
                Event::Shard { owner, perk, level: Some(l), depleted, .. } => Some((*owner, *perk, *l, *depleted)),
                _ => None,
            }));
        }
        assert_eq!(levels, vec![(0, relic.perk, 1, false), (0, relic.perk, 2, false), (0, relic.perk, 3, true)], "a relic is three levels");
        assert_eq!(w.bases[0].level(relic.perk), 3);
        assert_eq!(w.bases[0].shards(relic.perk), RELIC_SHARDS);
        let mut left = 0;
        w.relics_near(relic.x, relic.y, FP, |_| left += 1);
        assert_eq!(left, 0, "mined out");
        assert!(!w.blocked((relic.x + 3 * FP, relic.y), (relic.x + FP, relic.y), DRONOID_RADIUS));
        assert!(w.dronoids.iter().all(|d| d.cargo == 0), "shards are not cargo");

        // Snapshots and saves carry the perks and the worn relic.
        let copy = World::from_bytes(&w.to_bytes()).unwrap();
        assert_eq!(copy.state_hash(), w.state_hash());
        let saved = World::from_save(&w.to_save()).unwrap();
        assert_eq!(saved.bases[0].level(relic.perk), 3);
        assert_eq!(saved.state_hash(), w.state_hash());

        // A day later it is whole again.
        w.tick = w.worn[&relic.id].until.next_multiple_of(REGROW_TICKS);
        w.step(&[]);
        let mut back = None;
        w.relics_near(relic.x, relic.y, FP, |r| back = Some(r.left));
        assert_eq!(back, Some(RELIC_SHARDS));
    }

    #[test]
    fn a_base_takes_a_hundred_levels_of_a_perk_and_no_more() {
        let mut w = colony("miner");
        let relic = nearest_relic(&w);
        let full = (SHARDS_PER_LEVEL * PERK_MAX_LEVEL) as u16;
        // A whole relic is three levels.
        assert_eq!(RELIC_SHARDS / SHARDS_PER_LEVEL, 3);
        w.bases[0].perks[relic.perk.index()] = full - 1;
        assert_eq!(w.bases[0].level(relic.perk), PERK_MAX_LEVEL - 1);
        assert_eq!(w.take_shard(0, relic.id), Some((relic.perk, Some(PERK_MAX_LEVEL), false)));
        assert_eq!(w.take_shard(0, relic.id), None, "nothing more to gain");
        assert_eq!(w.worn[&relic.id].left, RELIC_SHARDS - 1, "and the relic is left to others");
        w.step(&[(1, Command::Join { name: "b".into(), bot: false })]);
        assert_eq!(w.take_shard(1, relic.id), None, "no base, no shard");
    }

    /// Bases from when there were five kinds of perks read back.
    #[test]
    fn a_base_saved_with_fewer_kinds_of_perks_reads_back() {
        let w = colony("miner");
        let mut json: serde_json::Value = serde_json::from_slice(&w.to_save()).unwrap();
        let base = &mut json["bases"][0];
        base["perks"] = serde_json::json!([36, 24, 36, 36, 36]);
        base.as_object_mut().unwrap().remove("refined");
        let back = World::from_save(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(back.bases[0].perks, [36, 24, 36, 36, 36, 0, 0, 0, 0, 0]);
        assert_eq!(back.bases[0].level(Perk::Forge), 9);
    }

    /// Every kind of relic, somewhere in the sector.
    #[test]
    fn relics_of_every_perk_lie_in_the_sector() {
        let w = colony("miner");
        let mut kinds = [0; PERKS];
        w.relics_near(0, 0, 1500 * FP, |r| kinds[r.perk.index()] += 1);
        assert!(kinds.iter().all(|n| *n > 0), "{kinds:?}");
    }

    #[test]
    fn each_perk_does_what_it_says() {
        let mut w = colony("miner");
        let b = &w.bases[0];
        assert_eq!((b.spawn_ticks(), b.max_hp(), b.lines_allowed()), (SPAWN_TICKS, BASE_HP, BASE_LINES));
        assert_eq!((b.room(), b.dronoid_hp(), b.vision(), b.mine_ticks(), b.repair()), (BASE_CAPACITY as usize, DRONOID_HP, VISION_BASE, MINE_TICKS, 1));
        let level = |n: i32| (n * SHARDS_PER_LEVEL) as u16;
        w.bases[0].perks = [level(50); PERKS];
        let b = &w.bases[0];
        assert_eq!(b.spawn_ticks(), SPAWN_TICKS * 100 / 250);
        assert_eq!(b.max_hp(), BASE_HP + 50 * BULWARK_HP);
        assert_eq!(b.lines_allowed(), BASE_LINES + 2);
        assert_eq!(b.room(), BASE_CAPACITY as usize + 50);
        assert_eq!(b.dronoid_hp(), DRONOID_HP + 10);
        assert_eq!(b.vision(), VISION_BASE + 50 * FP);
        assert_eq!(b.mine_ticks(), MINE_TICKS / 2);
        assert_eq!(b.repair(), 11);
        // Arsenal: 5 points more, and the sixth never yet at a round level.
        assert!((0..20).all(|r| b.damage(r) == ATTACK_DAMAGE + 5));
        w.bases[0].perks[Perk::Arsenal.index()] = level(53);
        let hits: i32 = (0..1000).map(|r| w.bases[0].damage(r)).sum();
        assert_eq!(hits, 1000 * (ATTACK_DAMAGE + 5) + 300, "three shots in ten hit a point harder");
        w.bases[0].perks = [level(100); PERKS];
        assert_eq!(w.bases[0].lines_allowed(), MAX_LINES);
        assert_eq!(w.bases[0].mine_ticks(), MINE_TICKS / 3);
        assert!(w.bases[0].room() <= MAX_DRONOIDS_PER_PLAYER);

        // Assembly: twenty levels more and a new line opens, running the first's program.
        w.bases[0].perks = [0; PERKS];
        w.bases[0].perks[Perk::Assembly.index()] = level(ASSEMBLY_LEVELS) - 1;
        let relic = (0..)
            .map(|k| {
                let mut found = None;
                w.relics_near(k * 400 * FP, 0, 400 * FP, |r| {
                    if r.perk == Perk::Assembly {
                        found = Some(r);
                    }
                });
                found
            })
            .take(20)
            .flatten()
            .next()
            .expect("an Assembly relic");
        assert_eq!(w.bases[0].lines.len(), BASE_LINES);
        assert_eq!(w.take_shard(0, relic.id), Some((Perk::Assembly, Some(ASSEMBLY_LEVELS), false)));
        let b = &w.bases[0];
        assert_eq!(b.lines.len(), BASE_LINES + 1);
        assert_eq!(b.lines.last().unwrap().prog, b.lines[0].prog);

        // Refinery: at level 7, 14 % more of every unload, the
        // fractions adding up from one unload to the next.
        w.bases[0].perks[Perk::Refinery.index()] = level(7);
        for l in &mut w.bases[0].lines {
            l.quota = 0;
        }
        w.dronoids.truncate(1);
        let (bx, by) = (w.bases[0].x, w.bases[0].y);
        let before = w.players[0].minerals;
        for _ in 0..5 {
            let d = &mut w.dronoids[0];
            (d.x, d.y, d.cargo, d.doing, d.busy) = (bx + BASE_RADIUS + DRONOID_RADIUS + FP / 2, by, CARGO_CAP, Doing::Idle, 1);
            d.vm.reset();
            for _ in 0..3 {
                w.step(&[]);
            }
            assert_eq!(w.dronoids[0].cargo, 0);
        }
        assert_eq!(w.players[0].minerals, before + 5 * CARGO_CAP + 3, "25 crystals unloaded, 3.5 more");

        // Bulwark and Nanites: the hull mends faster, up to its new size.
        w.bases[0].perks[Perk::Bulwark.index()] = level(2);
        w.bases[0].perks[Perk::Nanites.index()] = level(10);
        w.bases[0].hp = BASE_HP + 2 * BULWARK_HP - 10;
        w.tick = w.tick.next_multiple_of(BASE_REPAIR_TICKS);
        w.step(&[]);
        assert_eq!(w.bases[0].hp, BASE_HP + 2 * BULWARK_HP - 7);
        for _ in 0..4 * BASE_REPAIR_TICKS {
            w.step(&[]);
        }
        assert_eq!(w.bases[0].hp, BASE_HP + 2 * BULWARK_HP, "and never past it");

        // Armor: new dronoids come out tougher.
        w.bases[0].perks[Perk::Armor.index()] = level(25);
        w.players[0].minerals = 1000;
        for l in &mut w.bases[0].lines {
            l.quota = 5;
            l.timer = 0;
        }
        w.step(&[]);
        assert!(w.dronoids.iter().any(|d| d.hp == DRONOID_HP + 5));

        // Vision: the base sees further at once.
        let far = w.bases[0].vision() + 20 * FP;
        assert!(!w.explored(0, bx + far, by));
        w.bases[0].perks[Perk::Vision.index()] = level(40) - 1;
        let vision = (0..)
            .map(|k| {
                let mut found = None;
                w.relics_near(k * 400 * FP, 0, 400 * FP, |r| {
                    if r.perk == Perk::Vision {
                        found = Some(r);
                    }
                });
                found
            })
            .take(20)
            .flatten()
            .next()
            .expect("a Vision relic");
        w.take_shard(0, vision.id);
        assert!(w.explored(0, bx + far, by));
    }

    #[test]
    fn an_arsenal_makes_its_dronoids_hit_harder() {
        let mut w = World::new(7);
        let hunter = programs::builtin("hunter").unwrap();
        for (i, name) in ["a", "b"].iter().enumerate() {
            w.step(&[(0, Command::Join { name: name.to_string(), bot: false })]);
            land(&mut w, i as u8, hunter.source);
        }
        for _ in 0..80 {
            w.step(&[]);
        }
        for p in &mut w.players {
            p.shield_until = 0;
        }
        w.bases.iter_mut().find(|b| b.owner == 0).unwrap().perks[Perk::Arsenal.index()] = (ARSENAL_LEVELS * SHARDS_PER_LEVEL) as u16;
        let shooter = w.dronoids.iter().position(|d| d.owner == 0).unwrap();
        let victim = w.dronoids.iter().position(|d| d.owner == 1).unwrap();
        let (x, y) = (w.dronoids[shooter].x, w.dronoids[shooter].y);
        (w.dronoids[victim].x, w.dronoids[victim].y) = (x + 2 * FP, y);
        w.index_dronoids();
        assert!(w.start(shooter, Action::Attack, 0));
        assert_eq!(w.dronoids[victim].hp, DRONOID_HP - ATTACK_DAMAGE - 1);
    }
}
