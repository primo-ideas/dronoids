//! The deterministic simulation: a grid world, factories, drones and the
//! economy. No floats, no hash maps iteration, no wall clock: given the same
//! seed and the same commands, two peers produce the exact same world.

pub mod map;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::rng::{mix, Fnv};
use crate::rules::*;
use crate::vm::{self, thing, Action, Program, Senses, Sensor, Vm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Floor,
    Wall,
    Ore(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Empty,
    Drone(u32, u8),
    Factory(u32, u8),
}

#[derive(Clone, Debug)]
pub struct Player {
    pub id: u8,
    pub name: String,
    pub minerals: i32,
    pub alive: bool,
    pub mined: i64,
    pub drones_built: u32,
    pub kills: u32,
}

#[derive(Clone, Debug)]
pub struct Factory {
    pub id: u32,
    pub owner: u8,
    pub x: i32,
    pub y: i32,
    pub hp: i32,
    pub program: Arc<Program>,
    pub timer: i32,
    pub enabled: bool,
    pub mem: [i32; HIVE_MEM],
    pub spawned: u32,
    /// Maximum number of living drones from this factory.
    pub quota: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastAction {
    Idle,
    Move,
    Mine(i32, i32),
    Drop(i32, i32),
    Attack(i32, i32),
    Wait,
    Fail,
    Stall,
}

#[derive(Clone, Debug)]
pub struct Drone {
    pub id: u32,
    pub owner: u8,
    pub x: i32,
    pub y: i32,
    pub prev_x: i32,
    pub prev_y: i32,
    pub facing: u8,
    pub hp: i32,
    pub cargo: i32,
    pub factory: u32,
    pub program: Arc<Program>,
    pub vm: Vm,
    pub last: LastAction,
    pub born: u64,
}

/// Things that happened during a tick, for visual effects and sounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Spawn { owner: u8, x: i32, y: i32, fx: i32, fy: i32 },
    Mine { owner: u8, x: i32, y: i32, tx: i32, ty: i32, depleted: bool },
    Drop { owner: u8, x: i32, y: i32, tx: i32, ty: i32, amount: i32 },
    Attack { owner: u8, x: i32, y: i32, tx: i32, ty: i32 },
    DroneDestroyed { owner: u8, x: i32, y: i32 },
    FactoryDestroyed { owner: u8, x: i32, y: i32 },
    FactoryBuilt { owner: u8, x: i32, y: i32 },
    PlayerEliminated { player: u8 },
}

/// Player intents. They are the only way to influence the world, which is
/// what makes lockstep multiplayer possible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    PlaceFactory { x: i32, y: i32, name: String, source: String },
    /// Give one factory a program; its living drones are reprogrammed too.
    SetProgram { factory: u32, name: String, source: String },
    /// New version of a named program: every factory and drone of this
    /// player running a program with that name switches to it.
    UpdateProgram { name: String, source: String },
    ToggleFactory { factory: u32 },
    SetQuota { factory: u32, quota: u8 },
}

pub struct World {
    pub w: i32,
    pub h: i32,
    pub seed: u64,
    pub tick: u64,
    pub tiles: Vec<Tile>,
    pub cells: Vec<Cell>,
    pub players: Vec<Player>,
    pub factories: Vec<Factory>,
    pub drones: Vec<Drone>,
    pub events: Vec<Event>,
    pub starts: Vec<(i32, i32)>,
    next_id: u32,
}

pub fn dir_delta(d: i32) -> Option<(i32, i32)> {
    if d < 0 {
        return None;
    }
    Some(match d % 4 {
        0 => (0, -1),
        1 => (1, 0),
        2 => (0, 1),
        _ => (-1, 0),
    })
}

/// Naive "which way" towards an offset: the dominant axis wins.
pub fn greedy_dir(dx: i32, dy: i32) -> i32 {
    if dx == 0 && dy == 0 {
        vm::NONE
    } else if dx.abs() >= dy.abs() {
        if dx > 0 { vm::DIR_E } else { vm::DIR_W }
    } else if dy > 0 {
        vm::DIR_S
    } else {
        vm::DIR_N
    }
}

/// Offsets at exact manhattan distance `d`, in a fixed order.
fn ring(d: i32) -> impl Iterator<Item = (i32, i32)> {
    (0..d).flat_map(move |k| [(d - k, k), (-k, d - k), (-d + k, -k), (k, -d + k)])
}

pub fn compile_or_empty(name: &str, source: &str) -> Arc<Program> {
    Arc::new(vm::assemble(name, source).unwrap_or_else(|_| Program::empty(name)))
}

impl World {
    /// `players` are names; player `i` starts at `starts[i]`.
    pub fn new(seed: u64, players: &[String], start_name: &str, start_source: &str) -> World {
        let m = map::generate(seed, MAP_W, MAP_H);
        let n = m.tiles.len();
        let mut world = World {
            w: m.w,
            h: m.h,
            seed,
            tick: 0,
            tiles: m.tiles,
            cells: vec![Cell::Empty; n],
            players: vec![],
            factories: vec![],
            drones: vec![],
            events: vec![],
            starts: m.starts.clone(),
            next_id: 1,
        };
        let program = compile_or_empty(start_name, start_source);
        for (i, name) in players.iter().enumerate().take(m.starts.len()) {
            world.players.push(Player {
                id: i as u8,
                name: name.clone(),
                minerals: START_MINERALS,
                alive: true,
                mined: 0,
                drones_built: 0,
                kills: 0,
            });
            let (x, y) = m.starts[i];
            world.add_factory(i as u8, x, y, program.clone());
        }
        world
    }

    // ----- queries ---------------------------------------------------------

    pub fn inside(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h
    }

    pub fn idx(&self, x: i32, y: i32) -> usize {
        (y * self.w + x) as usize
    }

    pub fn tile(&self, x: i32, y: i32) -> Tile {
        if self.inside(x, y) { self.tiles[self.idx(x, y)] } else { Tile::Wall }
    }

    pub fn cell(&self, x: i32, y: i32) -> Cell {
        if self.inside(x, y) { self.cells[self.idx(x, y)] } else { Cell::Empty }
    }

    pub fn walkable(&self, x: i32, y: i32) -> bool {
        self.inside(x, y) && self.tile(x, y) == Tile::Floor && self.cell(x, y) == Cell::Empty
    }

    pub fn drone_index(&self, id: u32) -> Option<usize> {
        self.drones.binary_search_by_key(&id, |d| d.id).ok()
    }

    pub fn factory_index(&self, id: u32) -> Option<usize> {
        self.factories.binary_search_by_key(&id, |f| f.id).ok()
    }

    pub fn drone(&self, id: u32) -> Option<&Drone> {
        self.drone_index(id).map(|i| &self.drones[i])
    }

    pub fn factory(&self, id: u32) -> Option<&Factory> {
        self.factory_index(id).map(|i| &self.factories[i])
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

    /// What a unit of `owner` perceives at (x, y).
    pub fn thing_at(&self, x: i32, y: i32, owner: u8) -> i32 {
        if !self.inside(x, y) {
            return thing::WALL;
        }
        match self.cells[self.idx(x, y)] {
            Cell::Drone(_, o) => {
                if o == owner { thing::ALLY } else { thing::ENEMY }
            }
            Cell::Factory(_, o) => {
                if o == owner { thing::BASE } else { thing::EBASE }
            }
            Cell::Empty => match self.tiles[self.idx(x, y)] {
                Tile::Floor => thing::EMPTY,
                Tile::Wall => thing::WALL,
                Tile::Ore(_) => thing::ORE,
            },
        }
    }

    /// Offset and distance to the nearest `target` seen from (x, y).
    /// Factories broadcast a beacon and are found at any range; everything
    /// else must be within `SENSOR_RANGE`.
    pub fn nearest(&self, x: i32, y: i32, owner: u8, target: i32) -> Option<(i32, i32, i32)> {
        if target == thing::BASE || target == thing::EBASE {
            let want_own = target == thing::BASE;
            return self
                .factories
                .iter()
                .filter(|f| f.hp > 0 && (f.owner == owner) == want_own)
                .map(|f| (f.x - x, f.y - y, (f.x - x).abs() + (f.y - y).abs()))
                .min_by_key(|&(_, _, d)| d);
        }
        if !(thing::EMPTY..=thing::EBASE).contains(&target) {
            return None;
        }
        for d in 1..=SENSOR_RANGE {
            for (dx, dy) in ring(d) {
                if self.thing_at(x + dx, y + dy, owner) == target {
                    return Some((dx, dy, d));
                }
            }
        }
        None
    }

    pub fn count_near(&self, x: i32, y: i32, owner: u8, target: i32) -> i32 {
        let mut n = 0;
        for d in 1..=SENSOR_RANGE {
            for (dx, dy) in ring(d) {
                if self.thing_at(x + dx, y + dy, owner) == target {
                    n += 1;
                }
            }
        }
        n
    }

    /// Why a factory can't be placed there, if it can't.
    pub fn can_place_factory(&self, owner: u8, x: i32, y: i32) -> Result<(), &'static str> {
        let Some(p) = self.players.get(owner as usize) else { return Err("joueur inconnu") };
        if !p.alive {
            return Err("vous êtes éliminé");
        }
        if !self.inside(x, y) {
            return Err("hors de la carte");
        }
        if self.tile(x, y) != Tile::Floor || self.cell(x, y) != Cell::Empty {
            return Err("case occupée");
        }
        if p.minerals < FACTORY_COST {
            return Err("pas assez de minerai");
        }
        let manhattan = |ax: i32, ay: i32| (ax - x).abs() + (ay - y).abs();
        if self.factories.iter().any(|f| manhattan(f.x, f.y) <= 2) {
            return Err("trop près d'une autre usine");
        }
        if self
            .factories
            .iter()
            .any(|f| f.owner != owner && manhattan(f.x, f.y) < ENEMY_BUILD_CLEARANCE)
        {
            return Err("trop près d'une usine ennemie");
        }
        let near_own = self.factories.iter().any(|f| f.owner == owner && manhattan(f.x, f.y) <= BUILD_RANGE)
            || self.drones.iter().any(|d| d.owner == owner && d.hp > 0 && manhattan(d.x, d.y) <= BUILD_RANGE);
        if !near_own {
            return Err("trop loin de vos unités");
        }
        Ok(())
    }

    pub fn state_hash(&self) -> u64 {
        let mut h = Fnv::default();
        h.add(self.tick as i64);
        h.add(self.next_id as i64);
        for p in &self.players {
            h.add(p.minerals as i64);
            h.add(p.alive as i64);
        }
        for f in &self.factories {
            for v in [f.id as i32, f.x, f.y, f.hp, f.timer, f.enabled as i32, f.quota as i32] {
                h.add(v as i64);
            }
            for m in f.mem {
                h.add(m as i64);
            }
        }
        for d in &self.drones {
            for v in [d.id as i32, d.x, d.y, d.hp, d.cargo, d.vm.pc as i32, d.vm.ok] {
                h.add(v as i64);
            }
            for r in d.vm.regs {
                h.add(r as i64);
            }
            h.add(d.vm.rng as i64);
        }
        for (i, t) in self.tiles.iter().enumerate() {
            if let Tile::Ore(n) = t {
                h.add(i as i64 * 131 + *n as i64);
            }
        }
        h.0
    }

    // ----- mutation --------------------------------------------------------

    fn add_factory(&mut self, owner: u8, x: i32, y: i32, program: Arc<Program>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.factories.push(Factory {
            id,
            owner,
            x,
            y,
            hp: FACTORY_HP,
            program,
            timer: 0,
            enabled: true,
            mem: [0; HIVE_MEM],
            spawned: 0,
            quota: DEFAULT_QUOTA,
        });
        let i = self.idx(x, y);
        self.cells[i] = Cell::Factory(id, owner);
        id
    }

    fn apply(&mut self, player: u8, cmd: &Command) {
        if !self.players.get(player as usize).is_some_and(|p| p.alive) {
            return;
        }
        match cmd {
            Command::PlaceFactory { x, y, name, source } => {
                if self.can_place_factory(player, *x, *y).is_ok() {
                    self.players[player as usize].minerals -= FACTORY_COST;
                    self.add_factory(player, *x, *y, compile_or_empty(name, source));
                    self.events.push(Event::FactoryBuilt { owner: player, x: *x, y: *y });
                }
            }
            Command::SetProgram { factory, name, source } => {
                let Some(fi) = self.factory_index(*factory) else { return };
                if self.factories[fi].owner != player {
                    return;
                }
                let Ok(p) = vm::assemble(name, source) else { return };
                let p = Arc::new(p);
                self.factories[fi].program = p.clone();
                for d in self.drones.iter_mut().filter(|d| d.factory == *factory) {
                    d.program = p.clone();
                    d.vm.reset();
                }
            }
            Command::UpdateProgram { name, source } => {
                let Ok(p) = vm::assemble(name, source) else { return };
                let p = Arc::new(p);
                for f in self.factories.iter_mut().filter(|f| f.owner == player && f.program.name == *name) {
                    f.program = p.clone();
                }
                for d in self.drones.iter_mut().filter(|d| d.owner == player && d.program.name == *name) {
                    d.program = p.clone();
                    d.vm.reset();
                }
            }
            Command::SetQuota { factory, quota } => {
                if let Some(fi) = self.factory_index(*factory)
                    && self.factories[fi].owner == player {
                        self.factories[fi].quota = (*quota).min(MAX_QUOTA);
                    }
            }
            Command::ToggleFactory { factory } => {
                if let Some(fi) = self.factory_index(*factory)
                    && self.factories[fi].owner == player {
                        self.factories[fi].enabled = !self.factories[fi].enabled;
                    }
            }
        }
    }

    /// Advance one tick. `cmds` must be in the same order on every peer.
    pub fn step(&mut self, cmds: &[(u8, Command)]) {
        self.events.clear();
        for (p, c) in cmds {
            self.apply(*p, c);
        }
        for d in &mut self.drones {
            d.prev_x = d.x;
            d.prev_y = d.y;
        }
        self.produce();
        for di in 0..self.drones.len() {
            let d = &self.drones[di];
            if d.hp <= 0 || d.born == self.tick {
                continue;
            }
            self.run_drone(di);
        }
        self.drones.retain(|d| d.hp > 0);
        self.factories.retain(|f| f.hp > 0);
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
            let (fx, fy, start) = (f.x, f.y, f.spawned);
            let spot = (0..4).map(|k| ((start + k) % 4) as i32).find_map(|dir| {
                let (dx, dy) = dir_delta(dir).unwrap();
                self.walkable(fx + dx, fy + dy).then_some((dir, fx + dx, fy + dy))
            });
            let Some((dir, x, y)) = spot else { continue };

            let id = self.next_id;
            self.next_id += 1;
            let f = &mut self.factories[fi];
            f.timer = SPAWN_INTERVAL;
            f.spawned += 1;
            let program = f.program.clone();
            let fid = f.id;
            self.players[owner].minerals -= DRONE_COST;
            self.players[owner].drones_built += 1;
            counts[owner] += 1;
            let i = self.idx(x, y);
            self.cells[i] = Cell::Drone(id, owner as u8);
            self.drones.push(Drone {
                id,
                owner: owner as u8,
                x,
                y,
                prev_x: fx,
                prev_y: fy,
                facing: dir as u8,
                hp: DRONE_HP,
                cargo: 0,
                factory: fid,
                program,
                vm: Vm::new(mix(self.seed, id as u64)),
                last: LastAction::Idle,
                born: self.tick,
            });
            self.events.push(Event::Spawn { owner: owner as u8, x, y, fx, fy });
        }
    }

    fn run_drone(&mut self, di: usize) {
        let d = &self.drones[di];
        let program = d.program.clone();
        let mut vm = d.vm.clone();
        let fi = self.factory_index(d.factory);
        let mut mem = fi.map(|i| self.factories[i].mem).unwrap_or([0; HIVE_MEM]);
        let view = DroneView { world: self, owner: d.owner, x: d.x, y: d.y, cargo: d.cargo, hp: d.hp };
        let outcome = vm.run(&program, &view, &mut mem);
        if let Some(i) = fi {
            self.factories[i].mem = mem;
        }
        self.drones[di].vm = vm;
        match outcome {
            None => self.drones[di].last = LastAction::Stall,
            Some((action, arg)) => {
                let ok = self.perform(di, action, arg);
                self.drones[di].vm.ok = ok as i32;
                if !ok {
                    self.drones[di].last = LastAction::Fail;
                }
            }
        }
    }

    fn perform(&mut self, di: usize, action: Action, arg: i32) -> bool {
        let (x, y, owner) = (self.drones[di].x, self.drones[di].y, self.drones[di].owner);
        if action == Action::Wait {
            self.drones[di].last = LastAction::Wait;
            return true;
        }
        let Some((dx, dy)) = dir_delta(arg) else { return false };
        self.drones[di].facing = (arg % 4) as u8;
        let (tx, ty) = (x + dx, y + dy);
        match action {
            Action::Move => {
                if !self.walkable(tx, ty) {
                    return false;
                }
                let (from, to) = (self.idx(x, y), self.idx(tx, ty));
                self.cells[to] = self.cells[from];
                self.cells[from] = Cell::Empty;
                let d = &mut self.drones[di];
                d.x = tx;
                d.y = ty;
                d.last = LastAction::Move;
                true
            }
            Action::Mine => {
                if self.drones[di].cargo >= CARGO_CAP || self.cell(tx, ty) != Cell::Empty {
                    return false;
                }
                let Tile::Ore(n) = self.tile(tx, ty) else { return false };
                let i = self.idx(tx, ty);
                self.tiles[i] = if n > 1 { Tile::Ore(n - 1) } else { Tile::Floor };
                self.drones[di].cargo += 1;
                self.drones[di].last = LastAction::Mine(tx, ty);
                self.players[owner as usize].mined += 1;
                self.events.push(Event::Mine { owner, x, y, tx, ty, depleted: n <= 1 });
                true
            }
            Action::Drop => {
                let cargo = self.drones[di].cargo;
                match self.cell(tx, ty) {
                    Cell::Factory(_, o) if o == owner && cargo > 0 => {
                        self.players[owner as usize].minerals += cargo;
                        self.drones[di].cargo = 0;
                        self.drones[di].last = LastAction::Drop(tx, ty);
                        self.events.push(Event::Drop { owner, x, y, tx, ty, amount: cargo });
                        true
                    }
                    _ => false,
                }
            }
            Action::Attack => {
                let hit = match self.cell(tx, ty) {
                    Cell::Drone(id, o) if o != owner => {
                        let ti = self.drone_index(id).unwrap();
                        let t = &mut self.drones[ti];
                        t.hp -= ATTACK_DAMAGE;
                        if t.hp <= 0 {
                            let i = self.idx(tx, ty);
                            self.cells[i] = Cell::Empty;
                            self.players[owner as usize].kills += 1;
                            self.events.push(Event::DroneDestroyed { owner: o, x: tx, y: ty });
                        }
                        true
                    }
                    Cell::Factory(id, o) if o != owner => {
                        let fi = self.factory_index(id).unwrap();
                        let f = &mut self.factories[fi];
                        f.hp -= ATTACK_DAMAGE;
                        if f.hp <= 0 {
                            let i = self.idx(tx, ty);
                            self.cells[i] = Cell::Empty;
                            self.events.push(Event::FactoryDestroyed { owner: o, x: tx, y: ty });
                        }
                        true
                    }
                    _ => false,
                };
                if hit {
                    self.drones[di].last = LastAction::Attack(tx, ty);
                    self.events.push(Event::Attack { owner, x, y, tx, ty });
                }
                hit
            }
            Action::Wait => true,
        }
    }

    /// A player without factories (and without the means to build one) is out.
    fn check_players(&mut self) {
        if self.players.len() < 2 {
            return;
        }
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
                    let i = (d.y * self.w + d.x) as usize;
                    self.cells[i] = Cell::Empty;
                    self.events.push(Event::DroneDestroyed { owner, x: d.x, y: d.y });
                }
                self.drones.retain(|d| d.hp > 0);
                self.events.push(Event::PlayerEliminated { player: owner });
            }
        }
    }

    /// The last player standing, once the game is decided.
    pub fn winner(&self) -> Option<u8> {
        if self.players.len() < 2 {
            return None;
        }
        let alive: Vec<&Player> = self.players.iter().filter(|p| p.alive).collect();
        if alive.len() == 1 { Some(alive[0].id) } else { None }
    }
}

struct DroneView<'a> {
    world: &'a World,
    owner: u8,
    x: i32,
    y: i32,
    cargo: i32,
    hp: i32,
}

impl Senses for DroneView<'_> {
    fn sense(&self, sensor: Sensor, arg: i32) -> i32 {
        let w = self.world;
        match sensor {
            Sensor::Cargo => self.cargo,
            Sensor::Hp => self.hp,
            Sensor::Look => match dir_delta(arg) {
                Some((dx, dy)) => w.thing_at(self.x + dx, self.y + dy, self.owner),
                None => vm::NONE,
            },
            Sensor::Scan => w
                .nearest(self.x, self.y, self.owner, arg)
                .map(|(dx, dy, _)| greedy_dir(dx, dy))
                .unwrap_or(vm::NONE),
            Sensor::Dist => w.nearest(self.x, self.y, self.owner, arg).map(|t| t.2).unwrap_or(vm::NONE),
            Sensor::Dx => w.nearest(self.x, self.y, self.owner, arg).map(|t| t.0).unwrap_or(0),
            Sensor::Dy => w.nearest(self.x, self.y, self.owner, arg).map(|t| t.1).unwrap_or(0),
            Sensor::Count => w.count_near(self.x, self.y, self.owner, arg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs;

    fn two_player_world(seed: u64) -> World {
        let m = programs::builtin("mineur").unwrap();
        World::new(seed, &["A".into(), "B".into()], m.name, m.source)
    }

    #[test]
    fn ring_covers_exactly_the_diamond() {
        for d in 1..6 {
            let pts: Vec<_> = ring(d).collect();
            assert_eq!(pts.len(), 4 * d as usize);
            assert!(pts.iter().all(|(x, y)| x.abs() + y.abs() == d));
            let mut uniq = pts.clone();
            uniq.sort();
            uniq.dedup();
            assert_eq!(uniq.len(), pts.len());
        }
    }

    #[test]
    fn miners_bring_minerals_home() {
        let mut w = two_player_world(7);
        for _ in 0..600 {
            w.step(&[]);
        }
        assert!(w.players[0].mined > 50, "mined {}", w.players[0].mined);
        assert!(w.drone_count(0) > 3);
    }

    #[test]
    fn simulation_is_deterministic() {
        let mut a = two_player_world(99);
        let mut b = two_player_world(99);
        let hunter = programs::builtin("chasseur").unwrap();
        for t in 0..800u64 {
            let cmds = if t == 200 {
                vec![(1u8, Command::UpdateProgram { name: "mineur".into(), source: hunter.source.into() })]
            } else {
                vec![]
            };
            a.step(&cmds);
            b.step(&cmds);
            if t % 50 == 0 {
                assert_eq!(a.state_hash(), b.state_hash(), "desync at {t}");
            }
        }
        assert_eq!(a.state_hash(), b.state_hash());
    }

    #[test]
    fn factory_placement_rules() {
        let mut w = two_player_world(3);
        let (sx, sy) = w.starts[0];
        assert_eq!(w.can_place_factory(0, sx, sy), Err("case occupée"));
        w.players[0].minerals = 0;
        assert_eq!(w.can_place_factory(0, sx + 3, sy), Err("pas assez de minerai"));
        w.players[0].minerals = 1000;
        let far = w.starts[1];
        assert!(w.can_place_factory(0, far.0 + 3, far.1 + 3).is_err());
        let spot = (sx + 3, sy);
        if w.walkable(spot.0, spot.1) {
            assert_eq!(w.can_place_factory(0, spot.0, spot.1), Ok(()));
            let m = programs::builtin("mineur").unwrap();
            w.step(&[(0, Command::PlaceFactory { x: spot.0, y: spot.1, name: m.name.into(), source: m.source.into() })]);
            assert_eq!(w.factory_count(0), 2);
            assert_eq!(w.players[0].minerals, 1000 - FACTORY_COST - DRONE_COST * 2);
        }
    }
}

#[cfg(test)]
mod better_programs {
    use super::*;
    use crate::programs;

    fn mined_with(source: &str, seed: u64) -> i64 {
        let mut w = World::new(seed, &["solo".into()], "p", source);
        for _ in 0..2400 {
            w.step(&[]);
        }
        w.players[0].mined
    }

    /// The whole point of the game: a smarter program out-mines the default.
    #[test]
    fn example_program_beats_builtin_miner() {
        let better = include_str!("../../examples/mineur_plus.dasm");
        let base = programs::builtin("mineur").unwrap().source;
        let (mut a, mut b) = (0, 0);
        for seed in 1..=6 {
            a += mined_with(base, seed);
            b += mined_with(better, seed);
        }
        println!("mineur: {a}  mineur+: {b}");
        assert!(b > a, "mineur+ {b} should beat mineur {a}");
    }
}
