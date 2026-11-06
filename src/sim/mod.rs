//! The deterministic simulation of a large, continuous world: asteroids,
//! crystal deposits, factories and drones moving freely.
//!
//! Positions are fixed-point integers (`rules::FP` per unit) and angles are
//! whole degrees: given the same seed and the same commands, every machine
//! computes the exact same world, which is what multiplayer relies on.

pub mod grid;
pub mod map;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::geo::{self, compass, dir_vec, dist, dist2};
use crate::programs;
use crate::rng::{mix, Fnv};
use crate::rules::*;
use crate::vm::{self, thing, Action, Program, Senses, Sensor, Vm};
use grid::Grid;
pub use map::ore_radius;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Player {
    pub id: u8,
    pub name: String,
    pub minerals: i32,
    pub alive: bool,
    pub bot: bool,
    pub mined: i64,
    pub drones_built: u32,
    pub kills: u32,
    pub home: (i32, i32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Factory {
    pub id: u32,
    pub owner: u8,
    pub x: i32,
    pub y: i32,
    pub hp: i32,
    pub prog: u32,
    pub timer: i32,
    pub enabled: bool,
    pub mem: [i32; HIVE_MEM],
    pub spawned: u32,
    /// Maximum number of living drones from this factory.
    pub quota: u8,
}

/// What a drone is busy doing.
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
pub struct Drone {
    pub id: u32,
    pub owner: u8,
    pub x: i32,
    pub y: i32,
    pub prev_x: i32,
    pub prev_y: i32,
    /// Compass angle the drone faces.
    pub heading: i32,
    pub hp: i32,
    pub cargo: i32,
    pub factory: u32,
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
    DroneDestroyed { owner: u8, x: i32, y: i32 },
    FactoryDestroyed { owner: u8, x: i32, y: i32 },
    FactoryBuilt { owner: u8, x: i32, y: i32 },
    PlayerJoined { player: u8 },
    PlayerEliminated { player: u8 },
}

/// Player intents: the only way to influence the world.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Enter the world (or come back after being eliminated).
    Join { name: String, bot: bool },
    PlaceFactory { x: i32, y: i32, name: String, source: String },
    /// Give one factory a program; its living drones are reprogrammed too.
    SetProgram { factory: u32, name: String, source: String },
    /// New version of a named program: every factory and drone of this
    /// player running a program with that name switches to it.
    UpdateProgram { name: String, source: String },
    ToggleFactory { factory: u32 },
    SetQuota { factory: u32, quota: u8 },
    /// Write a cell of a factory's hive memory (e.g. a target for its drones).
    SetMemory { factory: u32, index: u8, value: i32 },
}

/// Something a sensor found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub dx: i32,
    pub dy: i32,
    /// Gap between the two bodies.
    pub gap: i32,
    pub id: u32,
}

#[derive(Serialize, Deserialize)]
pub struct World {
    pub size: i32,
    pub seed: u64,
    pub tick: u64,
    pub rocks: Vec<Rock>,
    pub ores: Vec<Ore>,
    pub players: Vec<Player>,
    pub factories: Vec<Factory>,
    pub drones: Vec<Drone>,
    pub programs: Vec<ProgEntry>,
    pub slots: Vec<(i32, i32)>,
    next_id: u32,
    #[serde(skip)]
    pub events: Vec<Event>,
    #[serde(skip)]
    rock_grid: Grid,
    #[serde(skip)]
    ore_grid: Grid,
    #[serde(skip)]
    drone_grid: Grid,
}

fn gap_between(dx: i32, dy: i32, r1: i32, r2: i32) -> i32 {
    (dist(dx, dy) - r1 - r2).max(0)
}

impl World {
    pub fn new(seed: u64) -> World {
        let m = map::generate(seed, WORLD_SIZE);
        let mut next_id = 1;
        let ores = m
            .ores
            .iter()
            .map(|&(x, y, amount)| {
                let id = next_id;
                next_id += 1;
                Ore { id, x, y, amount }
            })
            .collect();
        let mut w = World {
            size: m.size,
            seed,
            tick: 0,
            rocks: m.rocks,
            ores,
            players: vec![],
            factories: vec![],
            drones: vec![],
            programs: vec![],
            slots: m.slots,
            next_id,
            events: vec![],
            rock_grid: Grid::default(),
            ore_grid: Grid::default(),
            drone_grid: Grid::default(),
        };
        w.rebuild_caches();
        w
    }

    /// Rebuild what is not serialized (compiled programs, spatial grids).
    pub fn rebuild_caches(&mut self) {
        for p in &mut self.programs {
            p.code = Some(Arc::new(vm::assemble(&p.name, &p.source).unwrap_or_else(|_| Program::empty(&p.name))));
        }
        self.rock_grid = Grid::new(self.size, GRID_CELL);
        for (i, r) in self.rocks.iter().enumerate() {
            self.rock_grid.insert(r.x, r.y, i as u32);
        }
        self.ore_grid = Grid::new(self.size, GRID_CELL);
        for o in &self.ores {
            self.ore_grid.insert(o.x, o.y, o.id);
        }
        self.drone_grid = Grid::new(self.size, GRID_CELL);
        self.index_drones();
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("world serializes")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<World, String> {
        let mut w: World = bincode::deserialize(bytes).map_err(|e| e.to_string())?;
        w.rebuild_caches();
        Ok(w)
    }

    fn index_drones(&mut self) {
        self.drone_grid.clear();
        for (i, d) in self.drones.iter().enumerate() {
            self.drone_grid.insert(d.x, d.y, i as u32);
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

    pub fn drone_index(&self, id: u32) -> Option<usize> {
        self.drones.binary_search_by_key(&id, |d| d.id).ok()
    }

    pub fn factory_index(&self, id: u32) -> Option<usize> {
        self.factories.binary_search_by_key(&id, |f| f.id).ok()
    }

    fn ore_index(&self, id: u32) -> Option<usize> {
        self.ores.binary_search_by_key(&id, |o| o.id).ok()
    }

    pub fn drone(&self, id: u32) -> Option<&Drone> {
        self.drone_index(id).map(|i| &self.drones[i])
    }

    pub fn factory(&self, id: u32) -> Option<&Factory> {
        self.factory_index(id).map(|i| &self.factories[i])
    }

    pub fn player_by_name(&self, name: &str) -> Option<&Player> {
        self.players.iter().find(|p| p.name == name)
    }

    pub fn drones_of_factory(&self, factory: u32) -> usize {
        self.drones.iter().filter(|d| d.factory == factory && d.hp > 0).count()
    }

    pub fn drone_count(&self, owner: u8) -> usize {
        self.drones.iter().filter(|d| d.owner == owner && d.hp > 0).count()
    }

    pub fn factory_count(&self, owner: u8) -> usize {
        self.factories.iter().filter(|f| f.owner == owner && f.hp > 0).count()
    }

    pub fn inside(&self, x: i32, y: i32, r: i32) -> bool {
        x >= r && y >= r && x <= self.size - r && y <= self.size - r
    }

    /// Would a body of radius `r` moving from `from` to `to` hit something
    /// it was not already touching?
    pub fn blocked(&self, from: (i32, i32), to: (i32, i32), r: i32) -> bool {
        if !self.inside(to.0, to.1, r) {
            return true;
        }
        let hits = |ox: i32, oy: i32, or: i32| {
            let lim = (or + r) as i64 * (or + r) as i64;
            dist2(ox - to.0, oy - to.1) < lim && dist2(ox - from.0, oy - from.1) >= lim
        };
        let mut near = vec![];
        self.rock_grid.query(to.0, to.1, r + MAX_ROCK_R, &mut near);
        if near.iter().any(|&i| {
            let k = &self.rocks[i as usize];
            hits(k.x, k.y, k.r)
        }) {
            return true;
        }
        self.ore_grid.query(to.0, to.1, r + 2 * FP, &mut near);
        if near.iter().any(|&id| {
            self.ore_index(id).is_some_and(|i| {
                let o = &self.ores[i];
                o.amount > 0 && hits(o.x, o.y, o.radius())
            })
        }) {
            return true;
        }
        self.factories.iter().any(|f| f.hp > 0 && hits(f.x, f.y, FACTORY_RADIUS))
    }

    /// Nearest `target` seen by a drone of `owner` at (x, y).
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
                for f in self.factories.iter().filter(|f| f.hp > 0 && (f.owner == owner) == own) {
                    let (dx, dy) = (f.x - x, f.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONE_RADIUS, FACTORY_RADIUS), id: f.id }, range);
                }
            }
            thing::ORE => {
                self.ore_grid.query(x, y, SENSOR_RANGE + 2 * FP, &mut near);
                for &id in &near {
                    let Some(i) = self.ore_index(id) else { continue };
                    let o = &self.ores[i];
                    if o.amount <= 0 {
                        continue;
                    }
                    let (dx, dy) = (o.x - x, o.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONE_RADIUS, o.radius()), id: o.id }, SENSOR_RANGE);
                }
            }
            thing::WALL => {
                self.rock_grid.query(x, y, SENSOR_RANGE + MAX_ROCK_R, &mut near);
                for &i in &near {
                    let k = &self.rocks[i as usize];
                    let (dx, dy) = (k.x - x, k.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONE_RADIUS, k.r), id: i }, SENSOR_RANGE);
                }
            }
            thing::ALLY | thing::ENEMY => {
                let ally = target == thing::ALLY;
                self.drone_grid.query(x, y, SENSOR_RANGE + 2 * FP, &mut near);
                for &i in &near {
                    let Some(d) = self.drones.get(i as usize) else { continue };
                    if d.hp <= 0 || d.id == me || (d.owner == owner) != ally {
                        continue;
                    }
                    let (dx, dy) = (d.x - x, d.y - y);
                    consider(Target { dx, dy, gap: gap_between(dx, dy, DRONE_RADIUS, DRONE_RADIUS), id: d.id }, SENSOR_RANGE);
                }
            }
            _ => {}
        }
        best
    }

    pub fn count_near(&self, x: i32, y: i32, owner: u8, me: u32, target: i32) -> i32 {
        let mut near = vec![];
        let within = |dx: i32, dy: i32, r: i32, range: i32| gap_between(dx, dy, DRONE_RADIUS, r) <= range;
        match target {
            thing::BASE | thing::EBASE => {
                let own = target == thing::BASE;
                let range = if own { SENSOR_RANGE } else { EBASE_RANGE };
                self.factories
                    .iter()
                    .filter(|f| f.hp > 0 && (f.owner == owner) == own && within(f.x - x, f.y - y, FACTORY_RADIUS, range))
                    .count() as i32
            }
            thing::ORE => {
                self.ore_grid.query(x, y, SENSOR_RANGE + 2 * FP, &mut near);
                near.iter()
                    .filter_map(|&id| self.ore_index(id).map(|i| &self.ores[i]))
                    .filter(|o| o.amount > 0 && within(o.x - x, o.y - y, o.radius(), SENSOR_RANGE))
                    .count() as i32
            }
            thing::WALL => {
                self.rock_grid.query(x, y, SENSOR_RANGE + MAX_ROCK_R, &mut near);
                near.iter()
                    .map(|&i| &self.rocks[i as usize])
                    .filter(|k| within(k.x - x, k.y - y, k.r, SENSOR_RANGE))
                    .count() as i32
            }
            thing::ALLY | thing::ENEMY => {
                let ally = target == thing::ALLY;
                self.drone_grid.query(x, y, SENSOR_RANGE + 2 * FP, &mut near);
                near.iter()
                    .filter_map(|&i| self.drones.get(i as usize))
                    .filter(|d| {
                        d.hp > 0
                            && d.id != me
                            && (d.owner == owner) == ally
                            && within(d.x - x, d.y - y, DRONE_RADIUS, SENSOR_RANGE)
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
        if !self.inside(px, py, DRONE_RADIUS) {
            return thing::WALL;
        }
        let touches = |ox: i32, oy: i32, r: i32| dist2(ox - px, oy - py) < (r + DRONE_RADIUS) as i64 * (r + DRONE_RADIUS) as i64;
        let mut near = vec![];
        self.rock_grid.query(px, py, MAX_ROCK_R + FP, &mut near);
        if near.iter().any(|&i| {
            let k = &self.rocks[i as usize];
            touches(k.x, k.y, k.r)
        }) {
            return thing::WALL;
        }
        if let Some(f) = self.factories.iter().find(|f| f.hp > 0 && touches(f.x, f.y, FACTORY_RADIUS)) {
            return if f.owner == owner { thing::BASE } else { thing::EBASE };
        }
        self.ore_grid.query(px, py, 2 * FP, &mut near);
        if near.iter().any(|&id| {
            self.ore_index(id).is_some_and(|i| {
                let o = &self.ores[i];
                o.amount > 0 && touches(o.x, o.y, o.radius())
            })
        }) {
            return thing::ORE;
        }
        self.drone_grid.query(px, py, 2 * FP, &mut near);
        let mut found = thing::EMPTY;
        for &i in &near {
            let Some(d) = self.drones.get(i as usize) else { continue };
            if d.hp > 0 && d.id != me && touches(d.x, d.y, DRONE_RADIUS) {
                found = if d.owner == owner { thing::ALLY } else { thing::ENEMY };
                if found == thing::ENEMY {
                    break;
                }
            }
        }
        found
    }

    /// Why a factory can't be placed there, if it can't.
    pub fn can_place_factory(&self, owner: u8, x: i32, y: i32) -> Result<(), &'static str> {
        let Some(p) = self.players.get(owner as usize) else { return Err("joueur inconnu") };
        if !p.alive {
            return Err("vous êtes éliminé");
        }
        if !self.inside(x, y, FACTORY_RADIUS + FP) {
            return Err("hors du monde");
        }
        if p.minerals < FACTORY_COST {
            return Err("pas assez de minerai");
        }
        let far = (x - 3 * self.size, y - 3 * self.size);
        if self.blocked(far, (x, y), FACTORY_RADIUS + FP / 2) {
            return Err("emplacement encombré");
        }
        let d = |fx: i32, fy: i32| dist2(fx - x, fy - y);
        let sq = |v: i32| v as i64 * v as i64;
        if self.factories.iter().any(|f| d(f.x, f.y) < sq(FACTORY_SPACING + 2 * FACTORY_RADIUS)) {
            return Err("trop près d'une autre usine");
        }
        if self.factories.iter().any(|f| f.owner != owner && d(f.x, f.y) < sq(ENEMY_BUILD_CLEARANCE)) {
            return Err("trop près d'une usine ennemie");
        }
        let reach = sq(BUILD_RANGE + FACTORY_RADIUS);
        let near_own = self.factories.iter().any(|f| f.owner == owner && d(f.x, f.y) <= sq(BUILD_RANGE + 2 * FACTORY_RADIUS))
            || self.drones.iter().any(|dr| dr.owner == owner && dr.hp > 0 && d(dr.x, dr.y) <= reach);
        if !near_own {
            return Err("trop loin de vos unités");
        }
        Ok(())
    }

    pub fn state_hash(&self) -> u64 {
        let mut h = Fnv::default();
        h.add(self.tick as i64);
        h.add(self.next_id as i64);
        h.add(self.programs.len() as i64);
        for p in &self.players {
            h.add(p.minerals as i64);
            h.add(p.alive as i64);
        }
        for f in &self.factories {
            for v in [f.id as i32, f.x, f.y, f.hp, f.timer, f.enabled as i32, f.quota as i32, f.prog as i32] {
                h.add(v as i64);
            }
            for m in f.mem {
                h.add(m as i64);
            }
        }
        for d in &self.drones {
            for v in [d.id as i32, d.x, d.y, d.hp, d.cargo, d.busy, d.heading, d.vm.pc as i32, d.vm.ok, d.prog as i32] {
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

    fn add_factory(&mut self, owner: u8, x: i32, y: i32, prog: u32) -> u32 {
        let id = self.new_id();
        self.factories.push(Factory {
            id,
            owner,
            x,
            y,
            hp: FACTORY_HP,
            prog,
            timer: 0,
            enabled: true,
            mem: [0; HIVE_MEM],
            spawned: 0,
            quota: DEFAULT_QUOTA,
        });
        id
    }

    /// Starting place farthest from every other player's factories.
    fn pick_slot(&self) -> (i32, i32) {
        let score = |s: &(i32, i32)| {
            self.factories
                .iter()
                .map(|f| dist2(f.x - s.0, f.y - s.1))
                .chain(self.players.iter().filter(|p| p.alive).map(|p| dist2(p.home.0 - s.0, p.home.1 - s.1)))
                .min()
                .unwrap_or(i64::MAX)
        };
        let mut best = (i64::MIN, self.slots[0]);
        for s in &self.slots {
            let v = score(s);
            if v > best.0 {
                best = (v, *s);
            }
        }
        best.1
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
        let id = match existing {
            Some(i) => {
                let p = &mut self.players[i];
                p.alive = true;
                p.minerals = START_MINERALS;
                p.home = home;
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
                    drones_built: 0,
                    kills: 0,
                    home,
                });
                id
            }
        };
        let m = programs::builtin("mineur").expect("builtin");
        let prog = self.intern_program(m.name, m.source).unwrap_or(0);
        self.add_factory(id, home.0, home.1, prog);
        self.events.push(Event::PlayerJoined { player: id });
        Some(id)
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
            Command::PlaceFactory { x, y, name, source } => {
                if self.can_place_factory(player, *x, *y).is_ok() {
                    let Some(prog) = self.intern_program(name, source) else { return };
                    self.players[player as usize].minerals -= FACTORY_COST;
                    self.add_factory(player, *x, *y, prog);
                    self.events.push(Event::FactoryBuilt { owner: player, x: *x, y: *y });
                }
            }
            Command::SetProgram { factory, name, source } => {
                let Some(fi) = self.factory_index(*factory) else { return };
                if self.factories[fi].owner != player {
                    return;
                }
                let Some(prog) = self.intern_program(name, source) else { return };
                self.factories[fi].prog = prog;
                for d in self.drones.iter_mut().filter(|d| d.factory == *factory) {
                    d.prog = prog;
                    d.vm.reset();
                }
            }
            Command::UpdateProgram { name, source } => {
                let Some(prog) = self.intern_program(name, source) else { return };
                let same_name: Vec<bool> = self.programs.iter().map(|p| p.name == *name).collect();
                for f in self.factories.iter_mut().filter(|f| f.owner == player && same_name[f.prog as usize]) {
                    f.prog = prog;
                }
                for d in self.drones.iter_mut().filter(|d| d.owner == player && same_name[d.prog as usize]) {
                    if d.prog != prog {
                        d.prog = prog;
                        d.vm.reset();
                    }
                }
            }
            Command::SetQuota { factory, quota } => {
                if let Some(fi) = self.factory_index(*factory)
                    && self.factories[fi].owner == player
                {
                    self.factories[fi].quota = (*quota).min(MAX_QUOTA);
                }
            }
            Command::SetMemory { factory, index, value } => {
                if let Some(fi) = self.factory_index(*factory)
                    && self.factories[fi].owner == player
                    && (*index as usize) < HIVE_MEM
                {
                    self.factories[fi].mem[*index as usize] = *value;
                }
            }
            Command::ToggleFactory { factory } => {
                if let Some(fi) = self.factory_index(*factory)
                    && self.factories[fi].owner == player
                {
                    self.factories[fi].enabled = !self.factories[fi].enabled;
                }
            }
        }
    }

    /// Advance one tick. `cmds` must be in the same order on every machine.
    pub fn step(&mut self, cmds: &[(u8, Command)]) {
        self.events.clear();
        for (p, c) in cmds {
            self.apply(*p, c);
        }
        let tick = self.tick;
        for d in self.drones.iter_mut().filter(|d| d.born != tick) {
            d.prev_x = d.x;
            d.prev_y = d.y;
        }
        self.produce();
        self.index_drones();
        for di in 0..self.drones.len() {
            let d = &self.drones[di];
            if d.hp <= 0 || d.born == self.tick {
                continue;
            }
            if d.busy == 0 {
                self.decide(di);
            }
            self.advance(di);
        }
        self.drones.retain(|d| d.hp > 0);
        self.factories.retain(|f| f.hp > 0);
        self.ores.retain(|o| o.amount > 0);
        self.check_players();
        self.tick += 1;
    }

    #[allow(clippy::needless_range_loop)] // indexes several parallel vectors
    fn produce(&mut self) {
        let mut counts = [0usize; MAX_PLAYERS];
        let mut per_factory = vec![0u32; self.factories.len()];
        for d in &self.drones {
            counts[d.owner as usize] += 1;
            if let Some(fi) = self.factory_index(d.factory) {
                per_factory[fi] += 1;
            }
        }
        for fi in 0..self.factories.len() {
            let f = &mut self.factories[fi];
            if f.timer > 0 {
                f.timer -= 1;
            }
            if !f.enabled || f.timer > 0 || f.hp <= 0 || per_factory[fi] >= f.quota as u32 {
                continue;
            }
            let owner = f.owner as usize;
            if counts[owner] >= MAX_DRONES_PER_PLAYER || self.players[owner].minerals < DRONE_COST {
                continue;
            }
            let (fx, fy, fid) = (f.x, f.y, f.id);
            let base_angle = (f.spawned.wrapping_mul(137).wrapping_add(fid.wrapping_mul(31)) % 360) as i32;
            let r = FACTORY_RADIUS + DRONE_RADIUS + FP / 4;
            let spot = (0..8).map(|k| base_angle + k * 45).find_map(|a| {
                let (dx, dy) = dir_vec(a, r);
                let p = (fx + dx, fy + dy);
                (!self.blocked(p, p, DRONE_RADIUS) && !self.touches_solid(p.0, p.1)).then_some((a, p))
            });
            let Some((angle, (x, y))) = spot else { continue };

            let id = self.new_id();
            let f = &mut self.factories[fi];
            f.timer = SPAWN_TICKS;
            f.spawned += 1;
            let prog = f.prog;
            self.players[owner].minerals -= DRONE_COST;
            self.players[owner].drones_built += 1;
            counts[owner] += 1;
            self.drones.push(Drone {
                id,
                owner: owner as u8,
                x,
                y,
                prev_x: fx,
                prev_y: fy,
                heading: geo::norm_deg(angle),
                hp: DRONE_HP,
                cargo: 0,
                factory: fid,
                prog,
                vm: Vm::new(mix(self.seed, id as u64)),
                doing: Doing::Idle,
                busy: 0,
                born: self.tick,
            });
            self.events.push(Event::Spawn { owner: owner as u8, x, y });
        }
    }

    /// Is (x, y) inside a rock, deposit or factory (for spawn points)?
    fn touches_solid(&self, x: i32, y: i32) -> bool {
        let far = (x - 3 * self.size, y - 3 * self.size);
        self.blocked(far, (x, y), DRONE_RADIUS)
    }

    fn decide(&mut self, di: usize) {
        let d = &self.drones[di];
        let program = self.program(d.prog);
        let mut vm = d.vm.clone();
        let fi = self.factory_index(d.factory);
        let mut mem = fi.map(|i| self.factories[i].mem).unwrap_or([0; HIVE_MEM]);
        let view = DroneView { world: self, owner: d.owner, id: d.id, x: d.x, y: d.y, cargo: d.cargo, hp: d.hp };
        let outcome = vm.run(&program, &view, &mut mem);
        if let Some(i) = fi {
            self.factories[i].mem = mem;
        }
        self.drones[di].vm = vm;
        match outcome {
            None => {
                let d = &mut self.drones[di];
                d.doing = Doing::Stall;
                d.busy = STALL_TICKS;
            }
            Some((action, arg)) => {
                let ok = self.start(di, action, arg);
                let d = &mut self.drones[di];
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
            let d = &self.drones[di];
            (d.x, d.y, d.owner, d.id)
        };
        let face = |w: &mut World, tx: i32, ty: i32| {
            let a = compass(tx - x, ty - y);
            if a >= 0 {
                w.drones[di].heading = a;
            }
        };
        match action {
            Action::Wait => {
                let d = &mut self.drones[di];
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
                if self.blocked((x, y), to, DRONE_RADIUS) {
                    return false;
                }
                let d = &mut self.drones[di];
                d.heading = geo::norm_deg(arg);
                d.doing = Doing::Move { fx: x, fy: y, tx: to.0, ty: to.1 };
                d.busy = MOVE_TICKS;
                true
            }
            Action::Mine => {
                if self.drones[di].cargo >= CARGO_CAP {
                    return false;
                }
                let Some(t) = self.nearest(x, y, owner, id, thing::ORE).filter(|t| t.gap < REACH) else {
                    return false;
                };
                let oi = self.ore_index(t.id).unwrap();
                let o = &mut self.ores[oi];
                o.amount -= 1;
                let (ox, oy, left) = (o.x, o.y, o.amount);
                if left <= 0 {
                    self.ore_grid.remove(ox, oy, t.id);
                }
                self.players[owner as usize].mined += 1;
                face(self, ox, oy);
                let d = &mut self.drones[di];
                d.cargo += 1;
                d.doing = Doing::Mine { tx: ox, ty: oy };
                d.busy = MINE_TICKS;
                self.events.push(Event::Mine { owner, x, y, tx: ox, ty: oy, depleted: left <= 0 });
                true
            }
            Action::Drop => {
                let cargo = self.drones[di].cargo;
                if cargo == 0 {
                    return false;
                }
                let Some(t) = self.nearest(x, y, owner, id, thing::BASE).filter(|t| t.gap < REACH) else {
                    return false;
                };
                let (tx, ty) = (x + t.dx, y + t.dy);
                self.players[owner as usize].minerals += cargo;
                face(self, tx, ty);
                let d = &mut self.drones[di];
                d.cargo = 0;
                d.doing = Doing::Drop { tx, ty };
                d.busy = DROP_TICKS;
                self.events.push(Event::Drop { owner, x, y, tx, ty, amount: cargo });
                true
            }
            Action::Attack => {
                let drone = self.nearest(x, y, owner, id, thing::ENEMY).filter(|t| t.gap <= ATTACK_RANGE);
                let base = self.nearest(x, y, owner, id, thing::EBASE).filter(|t| t.gap <= ATTACK_RANGE);
                let (tx, ty) = match (drone, base) {
                    (Some(t), b) if b.is_none_or(|b| t.gap <= b.gap) => {
                        let ti = self.drone_index(t.id).unwrap();
                        let victim = &mut self.drones[ti];
                        victim.hp -= ATTACK_DAMAGE;
                        let (vx, vy, vo, dead) = (victim.x, victim.y, victim.owner, victim.hp <= 0);
                        if dead {
                            self.players[owner as usize].kills += 1;
                            self.events.push(Event::DroneDestroyed { owner: vo, x: vx, y: vy });
                        }
                        (vx, vy)
                    }
                    (_, Some(b)) => {
                        let fi = self.factory_index(b.id).unwrap();
                        let f = &mut self.factories[fi];
                        f.hp -= ATTACK_DAMAGE;
                        let (fx, fy, fo, dead) = (f.x, f.y, f.owner, f.hp <= 0);
                        if dead {
                            self.events.push(Event::FactoryDestroyed { owner: fo, x: fx, y: fy });
                        }
                        (fx, fy)
                    }
                    _ => return false,
                };
                face(self, tx, ty);
                let d = &mut self.drones[di];
                d.doing = Doing::Attack { tx, ty };
                d.busy = ATTACK_TICKS;
                self.events.push(Event::Attack { owner, x, y, tx, ty });
                true
            }
        }
    }

    fn advance(&mut self, di: usize) {
        let d = &mut self.drones[di];
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

    /// A player without factories (and without the means to build one) is out.
    fn check_players(&mut self) {
        for pi in 0..self.players.len() {
            let p = &self.players[pi];
            if !p.alive {
                continue;
            }
            let owner = p.id;
            let has_factory = self.factories.iter().any(|f| f.owner == owner);
            let has_drones = self.drones.iter().any(|d| d.owner == owner);
            if !has_factory && (!has_drones || p.minerals < FACTORY_COST) {
                self.players[pi].alive = false;
                for d in self.drones.iter_mut().filter(|d| d.owner == owner) {
                    d.hp = 0;
                    self.events.push(Event::DroneDestroyed { owner, x: d.x, y: d.y });
                }
                self.drones.retain(|d| d.hp > 0);
                self.events.push(Event::PlayerEliminated { player: owner });
            }
        }
    }
}

struct DroneView<'a> {
    world: &'a World,
    owner: u8,
    id: u32,
    x: i32,
    y: i32,
    cargo: i32,
    hp: i32,
}

impl Senses for DroneView<'_> {
    fn sense(&self, sensor: Sensor, arg: i32) -> i32 {
        let w = self.world;
        let near = || w.nearest(self.x, self.y, self.owner, self.id, arg);
        match sensor {
            Sensor::Cargo => self.cargo,
            Sensor::Hp => self.hp,
            Sensor::PosX => self.x / FP,
            Sensor::PosY => self.y / FP,
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
mod tests {
    use super::*;

    fn world_with(players: &[&str]) -> World {
        let mut w = World::new(7);
        for p in players {
            w.step(&[(0, Command::Join { name: p.to_string(), bot: false })]);
        }
        w
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
        assert!(w.drone_count(0) >= 4, "drones {}", w.drone_count(0));
    }

    #[test]
    fn snapshot_roundtrip_and_determinism() {
        let mut a = world_with(&["a", "b"]);
        for _ in 0..400 {
            a.step(&[]);
        }
        let mut b = World::from_bytes(&a.to_bytes()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        let hunter = programs::builtin("chasseur").unwrap();
        for t in 0..1500u64 {
            let cmds = if t == 100 {
                vec![(1u8, Command::UpdateProgram { name: "mineur".into(), source: hunter.source.into() })]
            } else {
                vec![]
            };
            a.step(&cmds);
            b.step(&cmds);
        }
        assert_eq!(a.state_hash(), b.state_hash());
    }

    #[test]
    fn rejoin_after_elimination() {
        let mut w = world_with(&["a", "b"]);
        w.factories.retain(|f| f.owner != 1);
        w.step(&[]);
        assert!(!w.players[1].alive);
        w.step(&[(1, Command::Join { name: "b".into(), bot: false })]);
        assert!(w.players[1].alive);
        assert_eq!(w.factory_count(1), 1);
        assert_eq!(w.players.len(), 2);
    }

    #[test]
    fn factory_placement_rules() {
        let mut w = world_with(&["a", "b"]);
        let (hx, hy) = w.players[0].home;
        assert_eq!(w.can_place_factory(0, hx, hy), Err("pas assez de minerai"));
        w.players[0].minerals = 1000;
        assert_eq!(w.can_place_factory(0, hx, hy), Err("emplacement encombré"));
        let (ex, ey) = w.players[1].home;
        assert!(w.can_place_factory(0, ex + 5 * FP, ey).is_err());
        let ok = (0..360).step_by(15).map(|a| {
            let (dx, dy) = dir_vec(a, 7 * FP);
            (hx + dx, hy + dy)
        });
        let spot = ok.into_iter().find(|p| w.can_place_factory(0, p.0, p.1).is_ok()).expect("a free spot");
        let m = programs::builtin("mineur").unwrap();
        w.step(&[(0, Command::PlaceFactory { x: spot.0, y: spot.1, name: m.name.into(), source: m.source.into() })]);
        assert_eq!(w.factory_count(0), 2);
    }
}

#[cfg(test)]
mod better_programs {
    use super::*;

    fn mined_with(source: &str, seed: u64) -> i64 {
        let mut w = World::new(seed);
        w.join("solo", false);
        let f = w.factories[0].id;
        w.step(&[(0, Command::SetProgram { factory: f, name: "p".into(), source: source.into() })]);
        for _ in 0..6000 {
            w.step(&[]);
        }
        w.players[0].mined
    }

    /// The whole point of the game: a smarter program out-mines the default.
    #[test]
    fn example_program_beats_builtin_miner() {
        let better = include_str!("../../examples/mineur_plus.dasm");
        assert!(vm::assemble("p", better).is_ok());
        let base = programs::builtin("mineur").unwrap().source;
        let (mut a, mut b) = (0, 0);
        for seed in 1..=4 {
            a += mined_with(base, seed);
            b += mined_with(better, seed);
        }
        println!("mineur: {a}  mineur+: {b}");
        assert!(b > a, "mineur+ {b} should beat mineur {a}");
    }
}
