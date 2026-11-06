//! Reading saves written by earlier versions, so that the persistent
//! sector survives updates.
//!
//! Since protocol 5 the server saves in JSON, where new fields simply take
//! their default value. Before that, saves were bincode, whose layout is
//! positional: the structures as they were then are kept here, and a file
//! is only accepted by a layout that reads it to the last byte.

use bincode::Options;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::{Drone, Factory, Ore, Player, ProgEntry, World};
use crate::rng::Rng;
use crate::rules::{ARRIVAL_SCAN, VISION_DRONE, VISION_FACTORY};

/// Player as saved by protocol 3 (before the fog of war).
#[derive(Deserialize)]
struct PlayerV3 {
    id: u8,
    name: String,
    minerals: i32,
    alive: bool,
    bot: bool,
    mined: i64,
    drones_built: u32,
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
    drones_built: u32,
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
    drones: Vec<Drone>,
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
        return Ok((upgrade(old, PlayerV4::into_player), 4));
    }
    if let Ok(old) = strict::<WorldOld<PlayerV3>>(bytes) {
        let mut w = upgrade(old, PlayerV3::into_player);
        // Nothing was mapped yet: start from what each colony sees today.
        let homes = w.players.iter().filter(|p| p.alive).map(|p| (p.id, p.home.0, p.home.1, ARRIVAL_SCAN));
        let factories = w.factories.iter().map(|f| (f.owner, f.x, f.y, VISION_FACTORY));
        let drones = w.drones.iter().map(|d| (d.owner, d.x, d.y, VISION_DRONE));
        let units: Vec<(u8, i32, i32, i32)> = homes.chain(factories).chain(drones).collect();
        for (owner, x, y, r) in units {
            w.reveal(owner, x, y, r);
        }
        return Ok((w, 3));
    }
    Err(current)
}

fn upgrade<P>(old: WorldOld<P>, player: impl Fn(P) -> Player) -> World {
    let mut w = World::new(old.seed);
    w.size = old.size;
    w.tick = old.tick;
    w.ores = old.ores;
    w.players = old.players.into_iter().map(player).collect();
    w.factories = old.factories;
    w.drones = old.drones;
    w.programs = old.programs;
    w.next_id = old.next_id;
    w.rng = old.rng;
    w.ore_target = old.ore_target;
    // The map comes from the seed and the saved size, as for any snapshot.
    w.rocks.clear();
    w.slots.clear();
    w
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
            drones_built: self.drones_built,
            kills: self.kills,
            home: self.home,
            shield_until: self.shield_until,
            explored: vec![],
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
            drones_built: self.drones_built,
            kills: self.kills,
            home: self.home,
            shield_until: self.shield_until,
            explored: self.explored,
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
    use crate::sim::Command;

    #[derive(Serialize)]
    struct PlayerOut<'a> {
        id: u8,
        name: &'a str,
        minerals: i32,
        alive: bool,
        bot: bool,
        mined: i64,
        drones_built: u32,
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
        drones: &'a [Drone],
        programs: &'a [ProgEntry],
        next_id: u32,
        rng: &'a Rng,
        ore_target: u32,
    }

    /// What a protocol-3 or protocol-4 server would have written for this world.
    fn as_old(w: &World, version: u32) -> Vec<u8> {
        let players = w
            .players
            .iter()
            .map(|p| PlayerOut {
                id: p.id,
                name: &p.name,
                minerals: p.minerals,
                alive: p.alive,
                bot: p.bot,
                mined: p.mined,
                drones_built: p.drones_built,
                kills: p.kills,
                home: p.home,
                shield_until: p.shield_until,
                explored: (version >= 4).then_some(&p.explored[..]),
            })
            .collect();
        bincode::serialize(&WorldOut {
            size: w.size,
            seed: w.seed,
            tick: w.tick,
            ores: &w.ores,
            players,
            factories: &w.factories,
            drones: &w.drones,
            programs: &w.programs,
            next_id: w.next_id,
            rng: &w.rng,
            ore_target: w.ore_target,
        })
        .unwrap()
    }

    /// A sector with a bit of history: two colonies, drones out, a bot.
    fn lived_in() -> World {
        let mut w = World::new(11);
        let m = programs::builtin("mineur").unwrap();
        for name in ["ana", "bob"] {
            let id = w.join(name, false).unwrap();
            land(&mut w, id, m.source);
        }
        w.join("IA Vega", true);
        for _ in 0..300 {
            w.step(&[]);
        }
        assert!(w.drones.len() >= 2);
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
        assert_eq!(a.factories.len(), b.factories.len());
        assert_eq!(a.drones.len(), b.drones.len());
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
            assert_eq!(p.explored, q.explored);
            assert_eq!((q.quests, q.flags), (0, 0));
        }
    }

    #[test]
    fn protocol_3_saves_get_a_map_of_what_colonies_see() {
        let w = lived_in();
        let (old, version) = read(&as_old(&w, 3));
        assert_eq!(version, 3);
        same_sector(&w, &old);
        let f = old.factories.iter().find(|f| f.owner == 1).unwrap();
        assert!(old.explored(1, f.x, f.y));
        assert!(!old.explored(0, f.x, f.y), "nobody learns where the others are");
        let d = old.drones.iter().find(|d| d.owner == 0).unwrap();
        assert!(old.explored(0, d.x, d.y));
    }

    #[test]
    fn upgraded_saves_play_on_like_the_original() {
        let w = lived_in();
        for version in [3, 4] {
            let (mut old, _) = read(&as_old(&w, version));
            let mut now = World::from_bytes(&w.to_bytes()).unwrap();
            // Objectives and maps start over; everything else is identical.
            for p in &mut now.players {
                p.quests = 0;
                p.flags = 0;
                p.explored = old.players[p.id as usize].explored.clone();
            }
            let join = [(0, Command::Join { name: "cleo".into(), bot: false })];
            now.step(&join);
            old.step(&join);
            for _ in 0..200 {
                now.step(&[]);
                old.step(&[]);
            }
            assert_eq!(now.state_hash(), old.state_hash(), "protocol {version}");
        }
    }

    #[test]
    fn a_current_snapshot_is_not_mistaken_for_an_old_one() {
        let w = lived_in();
        let (back, version) = read(&w.to_bytes());
        assert_eq!(version, crate::net::PROTOCOL_VERSION);
        assert_eq!(back.players[0].explored, w.players[0].explored);
        assert!(from_bincode(b"pas une sauvegarde").is_err());
        assert!(from_bincode(&[]).is_err());
    }
}
