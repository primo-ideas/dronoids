//! Reading saves written by earlier versions, so that the persistent
//! sector survives updates.
//!
//! Since protocol 5 the server saves in JSON, where new fields simply take
//! their default value. Before that, saves were bincode, whose layout is
//! positional: the structures as they were then are kept here.

use serde::Deserialize;

use super::{Drone, Factory, Ore, Player, ProgEntry, World};
use crate::rng::Rng;

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

/// World as saved by protocol 4 (serialized fields only, in order).
#[derive(Deserialize)]
struct WorldV4 {
    size: i32,
    seed: u64,
    tick: u64,
    ores: Vec<Ore>,
    players: Vec<PlayerV4>,
    factories: Vec<Factory>,
    drones: Vec<Drone>,
    programs: Vec<ProgEntry>,
    next_id: u32,
    rng: Rng,
    ore_target: u32,
}

/// A protocol-4 bincode save, brought up to date (caches not rebuilt yet).
pub(super) fn from_v4(bytes: &[u8]) -> Result<World, String> {
    let old: WorldV4 = bincode::deserialize(bytes).map_err(|e| e.to_string())?;
    let players = old
        .players
        .into_iter()
        .map(|p| Player {
            id: p.id,
            name: p.name,
            minerals: p.minerals,
            alive: p.alive,
            bot: p.bot,
            mined: p.mined,
            drones_built: p.drones_built,
            kills: p.kills,
            home: p.home,
            shield_until: p.shield_until,
            explored: p.explored,
            quests: 0,
            flags: 0,
        })
        .collect();
    let mut w = World::new(old.seed);
    w.size = old.size;
    w.tick = old.tick;
    w.ores = old.ores;
    w.players = players;
    w.factories = old.factories;
    w.drones = old.drones;
    w.programs = old.programs;
    w.next_id = old.next_id;
    w.rng = old.rng;
    w.ore_target = old.ore_target;
    Ok(w)
}

#[cfg(test)]
pub(super) mod tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct PlayerV4Out<'a> {
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
        explored: &'a [u64],
    }

    #[derive(Serialize)]
    struct WorldV4Out<'a> {
        size: i32,
        seed: u64,
        tick: u64,
        ores: &'a [Ore],
        players: Vec<PlayerV4Out<'a>>,
        factories: &'a [Factory],
        drones: &'a [Drone],
        programs: &'a [ProgEntry],
        next_id: u32,
        rng: &'a Rng,
        ore_target: u32,
    }

    /// What a protocol-4 server would have written for this world.
    pub(in crate::sim) fn as_v4(w: &World) -> Vec<u8> {
        let players = w
            .players
            .iter()
            .map(|p| PlayerV4Out {
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
                explored: &p.explored,
            })
            .collect();
        bincode::serialize(&WorldV4Out {
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
}
