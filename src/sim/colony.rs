//! What a colony builds and how it puts its dronoids to work: factories
//! (more dronoids, more room, a place to unload), beacons (more vision), and
//! tasks.
//!
//! Dronoids wait at home until a task takes them: "these 10 go mining until
//! 200 crystals are in", "these 3 explore eastwards for 5 minutes". A task
//! takes as many waiting dronoids as it asks for (and more as they come),
//! runs until its objective is met, then sends them home, where they wait
//! again; a repeated task starts over. Each kind of task runs one of the
//! built-in programs; a custom task runs a program of one's own.

use serde::{Deserialize, Serialize};

use super::{dist2, gap_between, Event, World};
use crate::programs;
use crate::rules::*;

/// What dronoids run while they wait for a task, or come home from one:
/// back to the base (or a factory), unload, wait.
pub const IDLE_NAME: &str = "idle";
pub const IDLE_SOURCE: &str = "; IDLE -- waits at home for a task, after\n; unloading what it carries.\n\nstart:\n    IFNEAR BASE, home\n    GO BASE\n    IFFAIL step\n    JUMP start\nhome:\n    DROP\n    WAIT\n    JUMP start\nstep:\n    EXPLORE\n    JUMP start\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Factory {
    pub id: u32,
    pub owner: u8,
    pub x: i32,
    pub y: i32,
    pub hp: i32,
    pub timer: i32,
    pub spawned: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beacon {
    pub id: u32,
    pub owner: u8,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Structure {
    Factory,
    Beacon,
}

impl Structure {
    pub fn cost(self) -> i32 {
        match self {
            Structure::Factory => FACTORY_COST,
            Structure::Beacon => BEACON_COST,
        }
    }

    pub fn radius(self) -> i32 {
        match self {
            Structure::Factory => FACTORY_RADIUS,
            Structure::Beacon => BEACON_RADIUS,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Structure::Factory => "factory",
            Structure::Beacon => "beacon",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Mine,
    Explore,
    Prospect,
    Guard,
    Raid,
    /// A program of one's own.
    Custom,
}

impl TaskKind {
    pub const ALL: [TaskKind; 6] = [TaskKind::Mine, TaskKind::Explore, TaskKind::Prospect, TaskKind::Guard, TaskKind::Raid, TaskKind::Custom];

    pub fn name(self) -> &'static str {
        match self {
            TaskKind::Mine => "Mine",
            TaskKind::Explore => "Explore",
            TaskKind::Prospect => "Prospect",
            TaskKind::Guard => "Guard",
            TaskKind::Raid => "Raid",
            TaskKind::Custom => "Custom",
        }
    }

    /// What it does, in a few words.
    pub fn summary(self) -> &'static str {
        match self {
            TaskKind::Mine => "mine crystals and bring them home",
            TaskKind::Explore => "head off and map the sector",
            TaskKind::Prospect => "look for relics and mine their shards",
            TaskKind::Guard => "stay by the base and fire at intruders",
            TaskKind::Raid => "hunt enemies, or head for a target",
            TaskKind::Custom => "run a program of your own",
        }
    }

    /// The built-in program it runs (Custom: the one it is given).
    pub fn program(self) -> Option<&'static str> {
        match self {
            TaskKind::Mine => Some("miner"),
            TaskKind::Explore => Some("scout"),
            TaskKind::Prospect => Some("prospector"),
            TaskKind::Guard => Some("guard"),
            TaskKind::Raid => Some("hunter"),
            TaskKind::Custom => None,
        }
    }

    /// The objectives that make sense for it, the first being the usual one.
    pub fn limits(self) -> &'static [Limit] {
        use Limit::*;
        match self {
            TaskKind::Mine => &[Crystals(100), Crystals(250), Crystals(500), Crystals(1000), Minutes(10), Never],
            TaskKind::Explore => &[Minutes(3), Minutes(5), Minutes(10), Minutes(20), Never],
            TaskKind::Prospect => &[Shards(12), Shards(36), Shards(100), Minutes(20), Never],
            TaskKind::Guard | TaskKind::Raid | TaskKind::Custom => &[Minutes(5), Minutes(10), Minutes(30), Never],
        }
    }
}

/// When a task is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Limit {
    /// Until it is stopped.
    Never,
    /// Once its dronoids unloaded that many crystals.
    Crystals(i32),
    /// Once its dronoids mined that many shards of relics.
    Shards(i32),
    /// After that many minutes.
    Minutes(i32),
}

impl Limit {
    pub fn label(self) -> String {
        match self {
            Limit::Never => "until stopped".into(),
            Limit::Crystals(n) => format!("{n} crystals"),
            Limit::Shards(n) => format!("{n} shards"),
            Limit::Minutes(n) => format!("{n} min"),
        }
    }

    /// How far a task with this progress, started then, is: (done, goal).
    pub fn progress(self, progress: i64, started: u64, now: u64) -> (i64, i64) {
        let minute = (60.0 * TICKS_PER_SEC) as u64;
        match self {
            Limit::Never => (0, 0),
            Limit::Crystals(n) | Limit::Shards(n) => (progress.min(n as i64), n as i64),
            Limit::Minutes(n) => (((now.saturating_sub(started)) / minute) as i64, n as i64),
        }
    }
}

/// What is asked of a task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub kind: TaskKind,
    /// How many dronoids it takes.
    pub count: u8,
    pub limit: Limit,
    /// Starts over once done.
    pub repeat: bool,
    /// Explore: the course to set off on (degrees), or -1 for any.
    pub dir: i32,
    /// Raid: where to head for (world units), if anywhere.
    pub target: Option<(i32, i32)>,
    /// Custom: the program (name, source).
    pub program: Option<(String, String)>,
}

impl TaskSpec {
    pub fn new(kind: TaskKind, count: u8) -> TaskSpec {
        TaskSpec { kind, count, limit: kind.limits()[0], repeat: false, dir: -1, target: None, program: None }
    }

    /// How it reads: "Mine · 10 · 250 crystals".
    pub fn title(&self) -> String {
        let mut t = format!("{} · {}", self.kind.name(), self.count);
        if self.kind == TaskKind::Explore && self.dir >= 0 {
            let d = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"][((self.dir + 22) / 45 % 8) as usize];
            t += &format!(" · {d}");
        }
        if let Some((name, _)) = &self.program {
            t += &format!(" · {name}");
        }
        t
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    /// Launched, no dronoid free yet.
    Waiting,
    Running,
    /// Its objective met (or stopped): its dronoids come home.
    Returning,
    /// All home: it can be launched again.
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: u32,
    pub owner: u8,
    pub spec: TaskSpec,
    /// The program its dronoids run.
    pub prog: u32,
    pub state: TaskState,
    /// Crystals unloaded or shards mined by its dronoids, this run.
    pub progress: i64,
    /// When this run started.
    pub started: u64,
    /// How many times it has run to the end.
    pub runs: u32,
}

impl Task {
    pub fn done_and_goal(&self, now: u64) -> (i64, i64) {
        self.spec.limit.progress(self.progress, self.started, now)
    }

    fn reached(&self, now: u64) -> bool {
        let (done, goal) = self.done_and_goal(now);
        self.spec.limit != Limit::Never && done >= goal
    }
}

impl World {
    pub fn factory_index(&self, id: u32) -> Option<usize> {
        self.factories.binary_search_by_key(&id, |f| f.id).ok()
    }

    pub fn factory(&self, id: u32) -> Option<&Factory> {
        self.factory_index(id).map(|i| &self.factories[i])
    }

    pub fn task(&self, id: u32) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// How many living dronoids a colony holds: its base's hangar, what
    /// Hangar adds, and its factories.
    pub fn room(&self, owner: u8) -> usize {
        let base = self.base_of(owner).map_or(0, |b| b.room());
        let factories = self.factories.iter().filter(|f| f.owner == owner).count();
        (base + FACTORY_ROOM * factories).min(MAX_DRONOIDS_PER_PLAYER)
    }

    /// Dronoids of a colony waiting for a task.
    pub fn idle_count(&self, owner: u8) -> usize {
        self.dronoids.iter().filter(|d| d.owner == owner && d.task == 0 && d.hp > 0).count()
    }

    /// Dronoids working on a task now.
    pub fn task_dronoids(&self, task: u32) -> usize {
        self.dronoids.iter().filter(|d| d.task == task && d.hp > 0).count()
    }

    /// What lets a colony see, besides its dronoids: its base (with
    /// Vision) and its beacons, as (x, y, radius).
    pub fn lookouts(&self, owner: u8) -> Vec<(i32, i32, i32)> {
        let base = self.base_of(owner).map(|b| (b.x, b.y, b.vision()));
        base.into_iter().chain(self.beacons.iter().filter(|b| b.owner == owner).map(|b| (b.x, b.y, BEACON_VISION))).collect()
    }

    /// Does `owner` see (x, y) right now?
    pub fn sees(&self, owner: u8, x: i32, y: i32) -> bool {
        let within = |cx: i32, cy: i32, r: i32| dist2(cx - x, cy - y) <= r as i64 * r as i64;
        self.lookouts(owner).iter().any(|&(cx, cy, r)| within(cx, cy, r))
            || self.dronoids.iter().any(|d| d.owner == owner && within(d.x, d.y, VISION_DRONOID))
    }

    /// Why `owner` cannot build that there, if it cannot.
    pub fn can_build(&self, owner: u8, kind: Structure, x: i32, y: i32) -> Result<(), &'static str> {
        let Some(p) = self.players.get(owner as usize) else { return Err("unknown player") };
        if !p.alive || self.base_of(owner).is_none() {
            return Err("land your base first");
        }
        if p.minerals < kind.cost() {
            return Err("not enough crystals");
        }
        let r = kind.radius();
        if !self.inside(x, y, r + FP) {
            return Err("out of the sector");
        }
        match kind {
            Structure::Factory => {
                if self.factories.iter().filter(|f| f.owner == owner).count() >= MAX_FACTORIES {
                    return Err("no more factories");
                }
                // Within what the base or a beacon sees, not a dronoid.
                let near = |&(cx, cy, vr): &(i32, i32, i32)| dist2(cx - x, cy - y) <= vr as i64 * vr as i64;
                if !self.lookouts(owner).iter().any(near) {
                    return Err("only where the base or a beacon sees");
                }
            }
            Structure::Beacon => {
                if self.beacons.iter().filter(|b| b.owner == owner).count() >= MAX_BEACONS {
                    return Err("no more beacons");
                }
                if !self.sees(owner, x, y) {
                    return Err("only where you see now");
                }
            }
        }
        // Room around it: to the buildings of the colony, more to others'.
        let clear = |bx: i32, by: i32, br: i32, other: bool| {
            let gap = gap_between(bx - x, by - y, br, r);
            gap >= if other { ENEMY_BUILD_CLEARANCE } else { BUILD_SPACING }
        };
        let bases = self.bases.iter().map(|b| (b.owner, b.x, b.y, BASE_RADIUS));
        let factories = self.factories.iter().map(|f| (f.owner, f.x, f.y, FACTORY_RADIUS));
        let beacons = self.beacons.iter().map(|b| (b.owner, b.x, b.y, BEACON_RADIUS));
        for (o, bx, by, br) in bases.chain(factories).chain(beacons) {
            if !clear(bx, by, br, o != owner) {
                return Err(if o != owner { "too close to another colony" } else { "too close to another building" });
            }
        }
        if self.occupied((x, y), r + FP / 2) {
            return Err("the spot is taken");
        }
        Ok(())
    }

    pub(super) fn build(&mut self, owner: u8, kind: Structure, x: i32, y: i32) {
        if self.can_build(owner, kind, x, y).is_err() {
            return;
        }
        self.players[owner as usize].minerals -= kind.cost();
        let id = self.new_id();
        match kind {
            Structure::Factory => {
                self.factories.push(Factory { id, owner, x, y, hp: FACTORY_HP, timer: SPAWN_TICKS / 4, spawned: 0 });
            }
            Structure::Beacon => {
                self.beacons.push(Beacon { id, owner, x, y });
                self.reveal(owner, x, y, BEACON_VISION);
            }
        }
        if let Some(p) = self.players.get_mut(owner as usize) {
            p.flags |= match kind {
                Structure::Factory => super::quests::FLAG_FACTORY,
                Structure::Beacon => super::quests::FLAG_BEACON,
            };
        }
        self.events.push(Event::Built { owner, kind, x, y });
    }

    /// The program waiting dronoids run.
    pub(super) fn idle_prog(&mut self) -> u32 {
        self.intern_program(IDLE_NAME, IDLE_SOURCE).expect("the idle program assembles")
    }

    /// Factories build waiting dronoids when their time comes, as long as
    /// the colony has room and crystals. They mend like the base.
    pub(super) fn produce_factories(&mut self, counts: &mut [usize; MAX_PLAYERS], rooms: &[usize; MAX_PLAYERS]) {
        if self.factories.is_empty() {
            return;
        }
        let idle = self.idle_prog();
        let repair = self.tick.is_multiple_of(BASE_REPAIR_TICKS);
        for fi in 0..self.factories.len() {
            let owner = self.factories[fi].owner;
            let Some(base) = self.base_of(owner) else { continue };
            let (wait, hull) = (base.spawn_ticks(), base.dronoid_hp());
            let f = &mut self.factories[fi];
            if repair && f.hp > 0 && f.hp < FACTORY_HP {
                f.hp += 1;
            }
            if f.timer > 0 {
                f.timer -= 1;
            }
            let o = owner as usize;
            if f.timer > 0 || f.hp <= 0 || counts[o] >= rooms[o] || self.players[o].minerals < DRONOID_COST {
                continue;
            }
            let (fx, fy, fid, spawned) = (f.x, f.y, f.id, f.spawned);
            let first = (spawned.wrapping_mul(137).wrapping_add(fid.wrapping_mul(31)) % 360) as i32;
            if self.spawn(owner, fid, (fx, fy), FACTORY_RADIUS, first, idle, hull) {
                let f = &mut self.factories[fi];
                f.spawned += 1;
                f.timer = wait;
                counts[o] += 1;
            }
        }
    }

    /// Run every task one tick: give waiting tasks free dronoids, end those
    /// whose objective is met, let home those of the returning ones.
    pub(super) fn run_tasks(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let idle = self.idle_prog();
        let now = self.tick;
        for ti in 0..self.tasks.len() {
            let (id, owner, state) = (self.tasks[ti].id, self.tasks[ti].owner, self.tasks[ti].state);
            match state {
                TaskState::Waiting | TaskState::Running => {
                    if state == TaskState::Running && self.tasks[ti].reached(now) {
                        self.end_task(ti, idle);
                        continue;
                    }
                    let want = self.tasks[ti].spec.count as usize;
                    let mut have = self.task_dronoids(id);
                    if have >= want {
                        continue;
                    }
                    let prog = self.tasks[ti].prog;
                    let dir = self.tasks[ti].spec.dir;
                    for d in self.dronoids.iter_mut().filter(|d| d.owner == owner && d.task == 0 && d.hp > 0) {
                        if have >= want {
                            break;
                        }
                        d.task = id;
                        d.prog = prog;
                        d.vm.reset();
                        // Explorers set off on the course asked for.
                        if (0..360).contains(&dir) {
                            d.vm.course = dir + 1;
                        }
                        have += 1;
                    }
                    if state == TaskState::Waiting && have > 0 {
                        self.start_task(ti);
                    }
                }
                TaskState::Returning => {
                    let homes: Vec<(i32, i32, i32)> = self
                        .base_of(owner)
                        .map(|b| (b.x, b.y, BASE_RADIUS))
                        .into_iter()
                        .chain(self.factories.iter().filter(|f| f.owner == owner).map(|f| (f.x, f.y, FACTORY_RADIUS)))
                        .collect();
                    let mut left = 0;
                    for d in self.dronoids.iter_mut().filter(|d| d.task == id && d.hp > 0) {
                        let home = homes.iter().any(|&(x, y, r)| gap_between(x - d.x, y - d.y, r, DRONOID_RADIUS) <= HOME_GAP);
                        if home && d.cargo == 0 {
                            d.task = 0;
                        } else {
                            left += 1;
                        }
                    }
                    if left == 0 {
                        let t = &mut self.tasks[ti];
                        t.runs += 1;
                        t.state = if t.spec.repeat { TaskState::Waiting } else { TaskState::Done };
                        t.progress = 0;
                        self.events.push(Event::TaskDone { owner, task: id });
                    }
                }
                TaskState::Done => {}
            }
        }
    }

    fn start_task(&mut self, ti: usize) {
        let t = &mut self.tasks[ti];
        t.state = TaskState::Running;
        t.started = self.tick;
        t.progress = 0;
        // Hunters read where to go in the base's memory.
        if t.spec.kind == TaskKind::Raid {
            let (owner, target) = (t.owner, t.spec.target);
            if let Some(bi) = self.base_index_of(owner) {
                let mem = &mut self.bases[bi].mem;
                match target {
                    Some((x, y)) => (mem[0], mem[1], mem[2]) = (x, y, 1),
                    None => mem[2] = 0,
                }
            }
        }
    }

    /// The objective is met, or the task stopped: its dronoids come home.
    fn end_task(&mut self, ti: usize, idle: u32) {
        let id = self.tasks[ti].id;
        self.tasks[ti].state = TaskState::Returning;
        for d in self.dronoids.iter_mut().filter(|d| d.task == id) {
            d.prog = idle;
            d.vm.reset();
        }
    }

    /// Crystals unloaded or shards mined by a dronoid count for its task.
    pub(super) fn task_progress(&mut self, dronoid: usize, amount: i64) {
        let task = self.dronoids[dronoid].task;
        if task != 0
            && let Some(t) = self.tasks.iter_mut().find(|t| t.id == task && t.state == TaskState::Running)
        {
            t.progress += amount;
        }
    }

    pub(super) fn new_task(&mut self, owner: u8, spec: &TaskSpec) {
        if self.tasks.iter().filter(|t| t.owner == owner).count() >= MAX_TASKS || spec.count == 0 {
            return;
        }
        let mut spec = spec.clone();
        spec.count = spec.count.min(MAX_DRONOIDS_PER_PLAYER as u8);
        let prog = match (spec.kind.program(), &spec.program) {
            (Some(name), _) => {
                spec.program = None;
                let b = programs::builtin(name).expect("a built-in");
                self.intern_program(b.name, b.source)
            }
            (None, Some((name, source))) => {
                let p = self.intern_program(name, source);
                if p.is_some() {
                    self.note_program(owner, source);
                }
                p
            }
            (None, None) => None,
        };
        let Some(prog) = prog else { return };
        let id = self.new_id();
        self.tasks.push(Task { id, owner, spec, prog, state: TaskState::Waiting, progress: 0, started: self.tick, runs: 0 });
        if let Some(p) = self.players.get_mut(owner as usize) {
            p.flags |= super::quests::FLAG_TASK;
        }
    }

    fn my_task(&self, owner: u8, id: u32) -> Option<usize> {
        self.tasks.iter().position(|t| t.id == id && t.owner == owner)
    }

    pub(super) fn stop_task(&mut self, owner: u8, id: u32) {
        let Some(ti) = self.my_task(owner, id) else { return };
        self.tasks[ti].spec.repeat = false;
        match self.tasks[ti].state {
            TaskState::Running => {
                let idle = self.idle_prog();
                self.end_task(ti, idle);
            }
            TaskState::Waiting => self.tasks[ti].state = TaskState::Done,
            _ => {}
        }
    }

    pub(super) fn relaunch_task(&mut self, owner: u8, id: u32) {
        let Some(ti) = self.my_task(owner, id) else { return };
        if self.tasks[ti].state == TaskState::Done {
            self.tasks[ti].state = TaskState::Waiting;
            self.tasks[ti].progress = 0;
        }
    }

    pub(super) fn edit_task(&mut self, owner: u8, id: u32, count: u8, limit: Limit, repeat: bool) {
        let Some(ti) = self.my_task(owner, id) else { return };
        let t = &mut self.tasks[ti];
        t.spec.count = count.clamp(1, MAX_DRONOIDS_PER_PLAYER as u8);
        if t.spec.kind.limits().contains(&limit) {
            t.spec.limit = limit;
        }
        t.spec.repeat = repeat;
        let (count, running) = (t.spec.count as usize, t.state == TaskState::Running);
        // Fewer asked for: the extra dronoids go home.
        let idle = self.idle_prog();
        let extra = self.task_dronoids(id).saturating_sub(count);
        if extra > 0 && running {
            for d in self.dronoids.iter_mut().rev().filter(|d| d.task == id).take(extra) {
                d.task = 0;
                d.prog = idle;
                d.vm.reset();
            }
        }
    }

    pub(super) fn delete_task(&mut self, owner: u8, id: u32) {
        let Some(ti) = self.my_task(owner, id) else { return };
        let idle = self.idle_prog();
        for d in self.dronoids.iter_mut().filter(|d| d.task == id) {
            d.task = 0;
            d.prog = idle;
            d.vm.reset();
        }
        self.tasks.remove(ti);
    }

    /// Factories hit hard enough fall.
    pub(super) fn fallen_factories(&mut self) {
        if self.factories.iter().all(|f| f.hp > 0) {
            return;
        }
        for f in self.factories.iter().filter(|f| f.hp <= 0) {
            self.events.push(Event::FactoryDestroyed { owner: f.owner, x: f.x, y: f.y });
        }
        self.factories.retain(|f| f.hp > 0);
    }

    /// A colony that is gone leaves nothing behind.
    pub(super) fn forget_colony(&mut self, owner: u8) {
        self.factories.retain(|f| f.owner != owner);
        self.beacons.retain(|b| b.owner != owner);
        self.tasks.retain(|t| t.owner != owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::dir_vec;
    use crate::sim::Command;

    /// A colony landed (with its starting mining task), rich.
    fn colony() -> World {
        let mut w = World::new(7);
        w.step(&[(0, Command::Join { name: "a".into(), bot: false })]);
        let (hx, hy) = w.players[0].home;
        let spot = (0..8)
            .flat_map(|r| (0..360).step_by(30).map(move |a| (r, a)))
            .map(|(r, a)| {
                let (dx, dy) = dir_vec(a, r * 2 * FP);
                (hx + dx, hy + dy)
            })
            .find(|p| w.can_land(0, p.0, p.1).is_ok())
            .unwrap();
        let m = programs::builtin("miner").unwrap();
        w.step(&[(0, Command::Land { x: spot.0, y: spot.1, name: m.name.into(), source: m.source.into() })]);
        w.players[0].minerals = 5_000;
        w
    }

    fn run(w: &mut World, ticks: u64) {
        for _ in 0..ticks {
            w.step(&[]);
        }
    }

    const MINUTE: u64 = (60.0 * TICKS_PER_SEC) as u64;

    /// A free spot for `kind` around the base, `r` units out.
    fn spot(w: &World, kind: Structure, r: i32) -> (i32, i32) {
        let b = w.base_of(0).unwrap();
        (0..360)
            .step_by(15)
            .map(|a| {
                let (dx, dy) = dir_vec(a, r * FP);
                (b.x + dx, b.y + dy)
            })
            .find(|p| w.can_build(0, kind, p.0, p.1).is_ok())
            .expect("a free spot")
    }

    #[test]
    fn a_new_base_lands_with_a_mining_task() {
        let mut w = colony();
        assert_eq!(w.tasks.len(), 1);
        let t = w.tasks[0].clone();
        assert_eq!((t.spec.kind, t.spec.count, t.spec.limit), (TaskKind::Mine, 4, Limit::Never));
        run(&mut w, SPAWN_TICKS as u64 * 3);
        assert_eq!(w.task_dronoids(t.id), 4, "it takes the first four");
        assert!(w.dronoids.iter().filter(|d| d.task == t.id).all(|d| w.program_name(d.prog) == "miner"));
        assert!(w.idle_count(0) > 0, "the others wait at home");
        assert!(w.dronoids.iter().filter(|d| d.task == 0).all(|d| w.program_name(d.prog) == IDLE_NAME));
        assert_eq!(w.tasks[0].state, TaskState::Running);
    }

    /// "These 3 go exploring for 3 minutes": they go, come back, and are
    /// free again; launched again and repeated, the task starts over.
    #[test]
    fn a_task_runs_to_its_objective_then_its_dronoids_come_home() {
        let mut w = colony();
        let mine = w.tasks[0].id;
        w.step(&[(0, Command::DeleteTask { id: mine })]);
        run(&mut w, SPAWN_TICKS as u64 * 3);
        assert!(w.idle_count(0) >= 3);
        let mut spec = TaskSpec::new(TaskKind::Explore, 3);
        spec.limit = Limit::Minutes(3);
        spec.dir = 90;
        w.step(&[(0, Command::NewTask { spec })]);
        let id = w.tasks.last().unwrap().id;
        w.step(&[]);
        assert_eq!(w.task(id).unwrap().state, TaskState::Running);
        assert_eq!(w.task_dronoids(id), 3);
        let bx = w.base_of(0).unwrap().x;
        run(&mut w, MINUTE * 2);
        let far = w.dronoids.iter().filter(|d| d.task == id).map(|d| (d.x - bx) / FP).max().unwrap();
        assert!(far > 20, "they set off eastwards ({far} units)");
        run(&mut w, MINUTE + 10);
        assert_eq!(w.task(id).unwrap().state, TaskState::Returning, "three minutes: home");
        let mut back = false;
        for _ in 0..8000u64 {
            w.step(&[]);
            if w.task(id).unwrap().state == TaskState::Done {
                back = true;
                break;
            }
        }
        assert!(back, "they all came home");
        assert_eq!(w.task_dronoids(id), 0);
        assert_eq!(w.task(id).unwrap().runs, 1);
        // Launched again, repeated.
        w.step(&[(0, Command::EditTask { id, count: 2, limit: Limit::Minutes(3), repeat: true }), (0, Command::RelaunchTask { id })]);
        w.step(&[]);
        assert_eq!(w.task_dronoids(id), 2);
        // Stopped: home again, and no more repeats.
        w.step(&[(0, Command::StopTask { id })]);
        assert_eq!(w.task(id).unwrap().state, TaskState::Returning);
        assert!(!w.task(id).unwrap().spec.repeat);
    }

    #[test]
    fn mining_tasks_count_what_they_bring_home() {
        let mut w = colony();
        let id = w.tasks[0].id;
        w.step(&[(0, Command::EditTask { id, count: 4, limit: Limit::Crystals(100), repeat: false })]);
        let mut ended = false;
        for _ in 0..30_000 {
            w.step(&[]);
            if w.task(id).unwrap().state != TaskState::Running {
                ended = true;
                break;
            }
        }
        assert!(ended, "100 crystals come home");
        assert!(w.task(id).unwrap().progress >= 100);
    }

    #[test]
    fn factories_go_where_the_base_or_a_beacon_sees() {
        let mut w = colony();
        let b = w.base_of(0).unwrap().clone();
        assert_eq!(w.can_build(0, Structure::Factory, b.x + b.vision() + 10 * FP, b.y), Err("only where the base or a beacon sees"));
        assert_eq!(w.can_build(0, Structure::Factory, b.x + 5 * FP, b.y), Err("too close to another building"));
        let f = spot(&w, Structure::Factory, 10);
        let room = w.room(0);
        w.step(&[(0, Command::Build { kind: Structure::Factory, x: f.0, y: f.1 })]);
        assert_eq!(w.factories.len(), 1);
        assert_eq!(w.room(0), room + FACTORY_ROOM);
        // It builds dronoids of its own, which wait for a task.
        run(&mut w, SPAWN_TICKS as u64);
        let fid = w.factories[0].id;
        assert!(w.dronoids.iter().any(|d| d.line == fid));
        // Dronoids unload there: it is home.
        let (fx, fy) = (w.factories[0].x, w.factories[0].y);
        let near = w.nearest(fx + 3 * FP, fy, 0, 0, crate::vm::thing::BASE).map(|t| t.id);
        assert_eq!(near, Some(fid as u64));
        // It does not see: nothing goes beyond the base's sight off it.
        assert!(w.can_build(0, Structure::Factory, b.x + b.vision() + 10 * FP, b.y).is_err());
    }

    #[test]
    fn beacons_see_and_are_built_where_one_sees() {
        let mut w = colony();
        run(&mut w, SPAWN_TICKS as u64 * 2);
        let b = w.base_of(0).unwrap().clone();
        let out = (b.x + b.vision() + 20 * FP, b.y);
        assert!(!w.sees(0, out.0, out.1));
        assert_eq!(w.can_build(0, Structure::Beacon, out.0, out.1), Err("only where you see now"));
        // A dronoid out there sees enough to place one by it.
        let di = w.dronoids.iter().position(|d| d.owner == 0).unwrap();
        (w.dronoids[di].x, w.dronoids[di].y) = (out.0, out.1);
        let at = (0..360)
            .step_by(30)
            .flat_map(|a| [4, 6, 8].map(|r| (a, r)))
            .map(|(a, r)| {
                let (dx, dy) = dir_vec(a, r * FP);
                (out.0 + dx, out.1 + dy)
            })
            .find(|p| w.can_build(0, Structure::Beacon, p.0, p.1).is_ok())
            .expect("a spot by the dronoid");
        w.step(&[(0, Command::Build { kind: Structure::Beacon, x: at.0, y: at.1 })]);
        assert_eq!(w.beacons.len(), 1);
        let far = (at.0 + BEACON_VISION - 2 * FP, at.1);
        assert!(w.explored(0, far.0, far.1), "it maps all around it at once");
        assert!(w.lookouts(0).contains(&(at.0, at.1, BEACON_VISION)));
        assert!(w.sees(0, far.0, far.1));
        // Saved and read back.
        let back = World::from_save(&w.to_save()).unwrap();
        assert_eq!(back.state_hash(), w.state_hash());
        assert_eq!(back.beacons, w.beacons);
    }

    #[test]
    fn a_fallen_colony_leaves_no_building_nor_task() {
        let mut w = colony();
        let f = spot(&w, Structure::Factory, 10);
        w.step(&[(0, Command::Build { kind: Structure::Factory, x: f.0, y: f.1 })]);
        run(&mut w, 40);
        w.bases[0].hp = 0;
        w.step(&[]);
        assert!(!w.players[0].alive);
        assert!(w.factories.is_empty() && w.beacons.is_empty() && w.tasks.is_empty());
    }

    /// Saved before tasks: dronoids running a program for good.
    #[test]
    fn saves_from_before_tasks_put_every_dronoid_in_a_task() {
        let mut w = colony();
        run(&mut w, SPAWN_TICKS as u64 * 2);
        let mut json: serde_json::Value = serde_json::from_slice(&w.to_save()).unwrap();
        let fields = json.as_object_mut().unwrap();
        fields.remove("tasks");
        fields.remove("factories");
        fields.remove("beacons");
        for d in fields["dronoids"].as_array_mut().unwrap() {
            d.as_object_mut().unwrap().remove("task");
        }
        let back = World::from_save(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(back.dronoids.iter().all(|d| d.task != 0), "each dronoid has a task");
        let kinds: Vec<TaskKind> = back.tasks.iter().map(|t| t.spec.kind).collect();
        assert!(kinds.contains(&TaskKind::Mine));
        assert!(back.tasks.iter().all(|t| t.spec.limit == Limit::Never && t.state == TaskState::Running));
        for t in &back.tasks {
            assert!(back.dronoids.iter().filter(|d| d.task == t.id).all(|d| d.prog == t.prog), "they keep their program");
        }
    }
}
