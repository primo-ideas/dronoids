//! Reading saves written by earlier versions, so that the persistent
//! sector survives updates.
//!
//! Since protocol 5 the server saves in JSON, where new fields simply take
//! their default value. Before that, saves were bincode, whose layout is
//! positional: the structures as they were then are kept here, and a file
//! is only accepted by a layout that reads it to the last byte.
//!
//! Until protocol 9, a colony was a set of factories, each building its own
//! dronoids with its own program and its own relic bonuses. Those saves
//! come back with one base per colony (`found_bases`).

use std::collections::BTreeMap;

use bincode::Options;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{Base, Doing, Dronoid, Line, Ore, Player, ProgEntry, World};
use crate::vm::Vm;
use crate::rng::Rng;
use crate::rules::*;

/// How far a factory saw (protocol 9 and earlier).
const VISION_FACTORY: i32 = 18 * FP;

/// A factory, as saved until protocol 9.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Factory {
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
    pub quota: u8,
}

/// A dronoid's virtual machine as bincode saves held it (before EXPLORE).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct VmOld {
    pc: u16,
    regs: [i32; 4],
    ok: i32,
    stack: [u16; crate::rules::STACK_DEPTH],
    sp: u8,
    rng: u64,
    last_pc: u16,
    last_ops: u8,
    stalled: bool,
}

/// A dronoid as bincode saves held it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct DronoidOld {
    id: u32,
    owner: u8,
    x: i32,
    y: i32,
    prev_x: i32,
    prev_y: i32,
    heading: i32,
    hp: i32,
    cargo: i32,
    factory: u32,
    prog: u32,
    vm: VmOld,
    doing: Doing,
    busy: i32,
    born: u64,
}

impl DronoidOld {
    fn into_dronoid(self) -> Dronoid {
        let v = self.vm;
        let mut vm = Vm::new(v.rng);
        (vm.pc, vm.regs, vm.ok, vm.stack, vm.sp, vm.last_pc, vm.last_ops, vm.stalled) =
            (v.pc, v.regs, v.ok, v.stack, v.sp, v.last_pc, v.last_ops, v.stalled);
        Dronoid {
            id: self.id,
            owner: self.owner,
            x: self.x,
            y: self.y,
            prev_x: self.prev_x,
            prev_y: self.prev_y,
            heading: self.heading,
            hp: self.hp,
            cargo: self.cargo,
            line: self.factory,
            prog: self.prog,
            vm,
            doing: self.doing,
            busy: self.busy,
            born: self.born,
        }
    }

    #[cfg(test)]
    fn of(d: &Dronoid) -> DronoidOld {
        let v = &d.vm;
        DronoidOld {
            id: d.id,
            owner: d.owner,
            x: d.x,
            y: d.y,
            prev_x: d.prev_x,
            prev_y: d.prev_y,
            heading: d.heading,
            hp: d.hp,
            cargo: d.cargo,
            factory: d.line,
            prog: d.prog,
            vm: VmOld {
                pc: v.pc,
                regs: v.regs,
                ok: v.ok,
                stack: v.stack,
                sp: v.sp,
                rng: v.rng,
                last_pc: v.last_pc,
                last_ops: v.last_ops,
                stalled: v.stalled,
            },
            doing: d.doing,
            busy: d.busy,
            born: d.born,
        }
    }
}

/// The four bonuses a relic gave a factory, in the order they were saved.
const OLD_BONUSES: [Perk; 4] = [Perk::Forge, Perk::Refinery, Perk::Bulwark, Perk::Arsenal];

/// Player as saved by protocol 3 (before the fog of war).
#[derive(Deserialize)]
struct PlayerV3 {
    id: u8,
    name: String,
    minerals: i32,
    alive: bool,
    bot: bool,
    mined: i64,
    dronoids_built: u32,
    kills: u32,
    home: (i32, i32),
    shield_until: u64,
}

/// Player as saved by protocol 4 (fog of war, before objectives).
#[derive(Deserialize)]
struct PlayerV4 {
    id: u8,
    name: String,
    minerals: i32,
    alive: bool,
    bot: bool,
    mined: i64,
    dronoids_built: u32,
    kills: u32,
    home: (i32, i32),
    shield_until: u64,
    explored: Vec<u64>,
}

/// World as saved by protocols 3 and 4 (serialized fields only, in order).
#[derive(Deserialize)]
struct WorldOld<P> {
    size: i32,
    seed: u64,
    tick: u64,
    ores: Vec<Ore>,
    players: Vec<P>,
    factories: Vec<Factory>,
    dronoids: Vec<DronoidOld>,
    programs: Vec<ProgEntry>,
    next_id: u32,
    rng: Rng,
    ore_target: u32,
}

/// Exactly what `bincode::serialize` wrote, but refusing leftover bytes:
/// a file read with the wrong layout fails instead of yielding garbage.
fn strict<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    bincode::DefaultOptions::new().with_fixint_encoding().deserialize(bytes).map_err(|e| e.to_string())
}

/// A bincode save of any version, brought up to date (caches not rebuilt).
/// Returns the protocol it was written by.
pub(super) fn from_bincode(bytes: &[u8]) -> Result<(World, u32), String> {
    let current = match strict::<World>(bytes) {
        Ok(w) => return Ok((w, crate::net::PROTOCOL_VERSION)),
        Err(e) => e,
    };
    if let Ok(old) = strict::<WorldOld<PlayerV4>>(bytes) {
        let (mut w, factories) = upgrade(old, PlayerV4::into_player);
        found_bases(&mut w, factories, &BTreeMap::new());
        return Ok((w, 4));
    }
    if let Ok(old) = strict::<WorldOld<PlayerV3>>(bytes) {
        let (mut w, factories) = upgrade(old, PlayerV3::into_player);
        // Nothing was mapped yet: start from what each colony sees today.
        let homes = w.players.iter().filter(|p| p.alive).map(|p| (p.id, p.home.0, p.home.1, ARRIVAL_SCAN));
        let seen = factories.iter().map(|f| (f.owner, f.x, f.y, VISION_FACTORY));
        let dronoids = w.dronoids.iter().map(|d| (d.owner, d.x, d.y, VISION_DRONOID));
        let units: Vec<(u8, i32, i32, i32)> = homes.chain(seen).chain(dronoids).collect();
        for (owner, x, y, r) in units {
            w.reveal(owner, x, y, r);
        }
        found_bases(&mut w, factories, &BTreeMap::new());
        return Ok((w, 3));
    }
    Err(current)
}

/// A JSON save (protocol 5 onwards), brought up to date (caches not
/// rebuilt). Until protocol 9 it held factories and their bonuses.
pub(super) fn from_json(bytes: &[u8]) -> Result<World, String> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let fields = value.as_object_mut().ok_or("not a sector")?;
    let factories = fields.remove("factories");
    let boosts = fields.remove("boosts");
    let mut w: World = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if let Some(factories) = factories {
        let factories: Vec<Factory> = serde_json::from_value(factories).map_err(|e| e.to_string())?;
        let boosts: BTreeMap<u32, [u16; 4]> = match boosts {
            Some(b) => serde_json::from_value(b).map_err(|e| e.to_string())?,
            None => BTreeMap::new(),
        };
        found_bases(&mut w, factories, &boosts);
    }
    Ok(w)
}

/// Each colony's factories become one base, where its oldest factory
/// stood: one production line per factory (as many as a base can run),
/// with the factory's id, program, quota and state, so that its dronoids
/// carry on as they were; the bonuses of all its factories, added up, as
/// perks; a hangar as large as their quotas together. Dronoids of a
/// factory that got no line of its own join the line running their
/// program, or the first one. A colony left with no factory but dronoids
/// loses them, and lands anew.
pub(super) fn found_bases(w: &mut World, mut factories: Vec<Factory>, boosts: &BTreeMap<u32, [u16; 4]>) {
    factories.retain(|f| f.hp > 0);
    factories.sort_by_key(|f| f.id);
    let full = (SHARDS_PER_LEVEL * PERK_MAX_LEVEL) as u16;
    // Factory id -> line id.
    let mut moved: BTreeMap<u32, u32> = BTreeMap::new();
    let mut first_line: BTreeMap<u8, u32> = BTreeMap::new();
    for owner in 0..w.players.len() as u8 {
        let mine: Vec<&Factory> = factories.iter().filter(|f| f.owner == owner).collect();
        let Some(first) = mine.first() else { continue };
        let mut perks = [0u16; PERKS];
        for b in mine.iter().filter_map(|f| boosts.get(&f.id)) {
            for (kind, shards) in b.iter().enumerate() {
                let p = &mut perks[OLD_BONUSES[kind].index()];
                *p = p.saturating_add(*shards).min(full);
            }
        }
        let lines: Vec<Line> = mine
            .iter()
            .take(MAX_LINES)
            .map(|f| Line { id: f.id, prog: f.prog, timer: f.timer, enabled: f.enabled, spawned: f.spawned, quota: f.quota })
            .collect();
        for f in &mine {
            let line = lines.iter().find(|l| l.id == f.id).or_else(|| lines.iter().find(|l| l.prog == f.prog)).unwrap_or(&lines[0]);
            moved.insert(f.id, line.id);
        }
        first_line.insert(owner, lines[0].id);
        let quotas: u32 = mine.iter().map(|f| f.quota as u32).sum();
        let capacity = quotas.clamp(BASE_CAPACITY as u32, MAX_DRONOIDS_PER_PLAYER as u32) as u16;
        let id = w.new_id();
        let mut base = Base {
            id,
            owner,
            x: first.x,
            y: first.y,
            hp: 0,
            capacity,
            mem: first.mem,
            lines,
            perks,
            refined: 0,
            spawned: mine.iter().map(|f| f.spawned).sum(),
        };
        base.hp = base.max_hp();
        w.bases.push(base);
        let bi = w.bases.len() - 1;
        w.add_lines(bi);
    }
    w.bases.sort_by_key(|b| b.id);
    w.dronoids.retain(|d| first_line.contains_key(&d.owner));
    for d in &mut w.dronoids {
        d.line = moved.get(&d.line).copied().unwrap_or(first_line[&d.owner]);
    }
}

fn upgrade<P>(old: WorldOld<P>, player: impl Fn(P) -> Player) -> (World, Vec<Factory>) {
    let mut w = World::new(old.seed);
    w.size = old.size;
    w.tick = old.tick;
    w.ores = old.ores;
    w.players = old.players.into_iter().map(player).collect();
    w.dronoids = old.dronoids.into_iter().map(DronoidOld::into_dronoid).collect();
    w.programs = old.programs;
    w.next_id = old.next_id;
    w.rng = old.rng;
    w.ore_target = old.ore_target;
    // The map comes from the seed and the saved size, as for any snapshot.
    w.rocks.clear();
    (w, old.factories)
}

impl PlayerV3 {
    fn into_player(self) -> Player {
        Player {
            id: self.id,
            name: self.name,
            minerals: self.minerals,
            alive: self.alive,
            bot: self.bot,
            mined: self.mined,
            dronoids_built: self.dronoids_built,
            kills: self.kills,
            home: self.home,
            shield_until: self.shield_until,
            explored: vec![],
            seen: Default::default(),
            quests: 0,
            flags: 0,
        }
    }
}

impl PlayerV4 {
    fn into_player(self) -> Player {
        Player {
            id: self.id,
            name: self.name,
            minerals: self.minerals,
            alive: self.alive,
            bot: self.bot,
            mined: self.mined,
            dronoids_built: self.dronoids_built,
            kills: self.kills,
            home: self.home,
            shield_until: self.shield_until,
            explored: self.explored,
            seen: Default::default(),
            quests: 0,
            flags: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::programs;
    use crate::sim::tests::land;

    #[derive(Serialize)]
    struct PlayerOut<'a> {
        id: u8,
        name: &'a str,
        minerals: i32,
        alive: bool,
        bot: bool,
        mined: i64,
        dronoids_built: u32,
        kills: u32,
        home: (i32, i32),
        shield_until: u64,
        /// Absent before protocol 4.
        #[serde(skip_serializing_if = "Option::is_none", serialize_with = "inner")]
        explored: Option<&'a [u64]>,
    }

    fn inner<S: serde::Serializer>(v: &Option<&[u64]>, s: S) -> Result<S::Ok, S::Error> {
        v.unwrap_or_default().serialize(s)
    }

    #[derive(Serialize)]
    struct WorldOut<'a> {
        size: i32,
        seed: u64,
        tick: u64,
        ores: &'a [Ore],
        players: Vec<PlayerOut<'a>>,
        factories: &'a [Factory],
        dronoids: Vec<DronoidOld>,
        programs: &'a [ProgEntry],
        next_id: u32,
        rng: &'a Rng,
        ore_target: u32,
    }

    /// The factories a colony of the time would have had: one per line of
    /// today's bases, the first where the base is, the others around it.
    fn factories_of(w: &World) -> Vec<Factory> {
        let mut out = vec![];
        for b in &w.bases {
            for (i, l) in b.lines.iter().enumerate() {
                out.push(Factory {
                    id: l.id,
                    owner: b.owner,
                    x: b.x + i as i32 * 6 * FP,
                    y: b.y,
                    hp: 300,
                    prog: l.prog,
                    timer: l.timer,
                    enabled: l.enabled,
                    mem: b.mem,
                    spawned: l.spawned,
                    quota: l.quota,
                });
            }
        }
        out
    }

    /// A player's explored map as it used to be stored.
    fn old_bits(w: &World, p: &Player) -> Vec<u64> {
        let n = (w.size + crate::rules::FOG_CELL - 1) / crate::rules::FOG_CELL;
        let mut bits = vec![0u64; ((n * n) as usize).div_ceil(64)];
        for cy in 0..n {
            for cx in 0..n {
                if p.seen.get(cx, cy) {
                    let i = (cy * n + cx) as usize;
                    bits[i / 64] |= 1 << (i % 64);
                }
            }
        }
        bits
    }

    /// What a protocol-3 or protocol-4 server would have written for this world.
    fn as_old(w: &World, version: u32) -> Vec<u8> {
        let factories = factories_of(w);
        let bits: Vec<Vec<u64>> = w.players.iter().map(|p| old_bits(w, p)).collect();
        let players = w
            .players
            .iter()
            .zip(&bits)
            .map(|(p, bits)| PlayerOut {
                id: p.id,
                name: &p.name,
                minerals: p.minerals,
                alive: p.alive,
                bot: p.bot,
                mined: p.mined,
                dronoids_built: p.dronoids_built,
                kills: p.kills,
                home: p.home,
                shield_until: p.shield_until,
                explored: (version >= 4).then_some(&bits[..]),
            })
            .collect();
        bincode::serialize(&WorldOut {
            size: w.size,
            seed: w.seed,
            tick: w.tick,
            ores: &w.ores,
            players,
            factories: &factories,
            dronoids: w.dronoids.iter().map(DronoidOld::of).collect(),
            programs: &w.programs,
            next_id: w.next_id,
            rng: &w.rng,
            ore_target: w.ore_target,
        })
        .unwrap()
    }

    /// A sector with a bit of history: two colonies, dronoids out, a bot.
    fn lived_in() -> World {
        let mut w = World::with_first_sector(11);
        let m = programs::builtin("miner").unwrap();
        // Everyone lives in the first sector, as in the worlds of that time.
        let slots = crate::sim::map::generate(11, crate::rules::WORLD_SIZE).slots;
        let settle = |w: &mut World, id: u8, i: usize| {
            let p = &mut w.players[id as usize];
            p.home = slots[i];
            p.seen = Default::default();
            w.reveal(id, slots[i].0, slots[i].1, ARRIVAL_SCAN);
        };
        for (i, name) in ["ana", "bob"].into_iter().enumerate() {
            let id = w.join(name, false).unwrap();
            settle(&mut w, id, i);
            land(&mut w, id, m.source);
        }
        let id = w.join("AI Vega", true).unwrap();
        settle(&mut w, id, 2);
        for _ in 0..300 {
            w.step(&[]);
        }
        assert!(w.dronoids.len() >= 2);
        w
    }

    /// Read like the server does.
    fn read(bytes: &[u8]) -> (World, u32) {
        let (mut w, version) = from_bincode(bytes).unwrap();
        w.rebuild_caches();
        (w, version)
    }

    /// The parts of the state every version shares.
    fn same_sector(a: &World, b: &World) {
        assert_eq!((a.tick, a.seed, a.size), (b.tick, b.seed, b.size));
        assert_eq!(a.ores, b.ores);
        assert_eq!(a.bases.len(), b.bases.len());
        assert_eq!(a.dronoids.len(), b.dronoids.len());
        for (x, y) in a.bases.iter().zip(&b.bases) {
            assert_eq!((x.owner, x.x, x.y), (y.owner, y.x, y.y));
            let lines = |b: &Base| b.lines.iter().map(|l| (l.id, l.prog, l.quota)).collect::<Vec<_>>();
            assert_eq!(lines(x), lines(y), "each factory became a line");
        }
        for (d, e) in a.dronoids.iter().zip(&b.dronoids) {
            assert_eq!((d.id, d.line, d.prog, d.x, d.y), (e.id, e.line, e.prog, e.x, e.y));
        }
        assert_eq!(a.rocks, b.rocks, "the map comes back from the seed");
        for (p, q) in a.players.iter().zip(&b.players) {
            assert_eq!((&p.name, p.minerals, p.alive, p.bot, p.mined), (&q.name, q.minerals, q.alive, q.bot, q.mined));
        }
    }

    #[test]
    fn protocol_4_saves_keep_their_map() {
        let w = lived_in();
        let (old, version) = read(&as_old(&w, 4));
        assert_eq!(version, 4);
        same_sector(&w, &old);
        for (p, q) in w.players.iter().zip(&old.players) {
            // (Those maps stopped at the edge of the sector.)
            let n = (w.size + crate::rules::FOG_CELL - 1) / crate::rules::FOG_CELL;
            for cy in 0..n {
                for cx in 0..n {
                    assert_eq!(p.seen.get(cx, cy), q.seen.get(cx, cy));
                }
            }
            assert!(q.seen.count() > 0 && q.explored.is_empty());
            assert_eq!((q.quests, q.flags), (0, 0));
        }
    }

    #[test]
    fn protocol_3_saves_get_a_map_of_what_colonies_see() {
        let w = lived_in();
        let (old, version) = read(&as_old(&w, 3));
        assert_eq!(version, 3);
        same_sector(&w, &old);
        let b = old.base_of(1).unwrap();
        assert!(old.explored(1, b.x, b.y));
        assert!(!old.explored(0, b.x, b.y), "nobody learns where the others are");
        let d = old.dronoids.iter().find(|d| d.owner == 0).unwrap();
        assert!(old.explored(0, d.x, d.y));
    }

    #[test]
    fn upgraded_saves_play_on() {
        let w = lived_in();
        for version in [3, 4] {
            let (mut old, _) = read(&as_old(&w, version));
            for _ in 0..400 {
                old.step(&[]);
            }
            assert!(old.players.iter().filter(|p| !p.bot).all(|p| p.alive), "protocol {version}");
            let copy = World::from_bytes(&old.to_bytes()).unwrap();
            assert_eq!(copy.state_hash(), old.state_hash());
        }
    }

    /// A JSON save of protocol 9: factories, and the bonuses relics gave them.
    #[test]
    fn factories_and_their_bonuses_become_a_base_and_its_perks() {
        let mut w = lived_in();
        let mut factories = factories_of(&w);
        // Ana had nine factories, more than a base runs lines, some of them
        // scout factories; a destroyed one is left out.
        let ana: Vec<u32> = factories.iter().filter(|f| f.owner == 0).map(|f| f.id).collect();
        let scout = w.intern_program("scout", programs::builtin("scout").unwrap().source).unwrap();
        let miner = w.bases[0].lines[0].prog;
        for (i, prog) in [miner, scout, scout, miner, scout, miner, miner].into_iter().enumerate() {
            let mut f = factories[0].clone();
            f.id = 9000 + i as u32;
            f.x += (i as i32 + 2) * 6 * FP;
            f.prog = prog;
            factories.push(f);
        }
        let mut gone = factories[0].clone();
        gone.id = 9100;
        gone.hp = 0;
        factories.push(gone);
        // A dronoid of the ninth factory (whose program is the first's).
        let d = w.dronoids.iter_mut().find(|d| d.owner == 0).unwrap();
        d.line = 9006;
        let moved = d.id;
        let mut json: serde_json::Value = serde_json::from_slice(&w.to_save()).unwrap();
        let fields = json.as_object_mut().unwrap();
        fields.remove("bases");
        fields.insert("factories".into(), serde_json::to_value(&factories).unwrap());
        // Forge, Refinery, Bulwark, Arsenal, on two of Ana's factories.
        let boosts: BTreeMap<u32, [u16; 4]> = [(ana[0], [12, 0, 30, 1]), (9001, [12, 5, 30, 0])].into();
        fields.insert("boosts".into(), serde_json::to_value(&boosts).unwrap());

        let back = World::from_save(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(back.bases.len(), 2, "one base per colony that had a factory");
        let b = back.base_of(0).unwrap();
        assert_eq!((b.x, b.y), (factories[0].x, factories[0].y), "where the oldest factory stood");
        assert_eq!(b.lines.len(), MAX_LINES);
        assert_eq!(b.lines.iter().map(|l| l.id).collect::<Vec<_>>(), vec![ana[0], ana[1], 9000, 9001, 9002, 9003, 9004]);
        assert_eq!(back.dronoid(moved).unwrap().line, ana[0], "with the line running its program");
        assert_eq!(b.level(Perk::Forge), 24 / SHARDS_PER_LEVEL);
        assert_eq!((b.shards(Perk::Refinery), b.shards(Perk::Bulwark), b.shards(Perk::Arsenal)), (5, 60, 1));
        assert_eq!(b.level(Perk::Assembly), 0);
        assert_eq!(b.hp, b.max_hp());
        assert!(b.capacity >= 9 * DEFAULT_QUOTA as u16);
        assert!(back.dronoids.iter().all(|d| back.base_of(d.owner).and_then(|b| b.line(d.line)).is_some()));
        // And it is saved as a base from then on.
        let again = World::from_save(&back.to_save()).unwrap();
        assert_eq!(again.state_hash(), back.state_hash());
    }

    #[test]
    fn a_current_snapshot_is_not_mistaken_for_an_old_one() {
        let w = lived_in();
        let (back, version) = read(&w.to_bytes());
        assert_eq!(version, crate::net::PROTOCOL_VERSION);
        assert_eq!(back.players[0].explored, w.players[0].explored);
        assert!(from_bincode(b"not a save").is_err());
        assert!(from_bincode(&[]).is_err());
    }
}
