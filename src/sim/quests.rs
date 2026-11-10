//! First steps in the sector: a short chain of objectives, each with a
//! small reward. They are part of the simulation (checked on every machine
//! alike and saved with the world), so progress follows a player across
//! sessions and devices. Computer colonies have none.

use super::{Event, World};

pub struct Quest {
    pub title: &'static str,
    pub hint: &'static str,
    /// Crystals granted on completion.
    pub reward: i32,
}

pub const QUESTS: &[Quest] = &[
    Quest { title: "Place your first factory", hint: "left click on a free spot", reward: 0 },
    Quest { title: "Mine 100 crystals", hint: "your dronoids mine on their own", reward: 40 },
    Quest { title: "Apply a program of your own", hint: "Program (E), edit, Ctrl+S", reward: 60 },
    Quest { title: "Map the surroundings", hint: "a scout goes far", reward: 50 },
    Quest { title: "Found a second factory", hint: "B, near your units", reward: 80 },
];

pub const MINED_GOAL: i64 = 100;
/// Explored fog cells (of `FOG_CELL` units) for the mapping objective:
/// roughly four times the arrival scan.
pub const MAPPED_GOAL: i64 = 700;

/// Facts that are not visible in the world state afterwards.
pub const FLAG_OWN_PROGRAM: u32 = 1;

/// Objectives are checked this often (ticks).
pub const CHECK_EVERY: u64 = 4;

/// Index of the player's current objective, if any is left.
pub fn current(w: &World, player: u8) -> Option<usize> {
    let p = w.players.get(player as usize)?;
    if p.bot {
        return None;
    }
    (0..QUESTS.len()).find(|i| p.quests & (1 << i) == 0)
}

/// Progress towards objective `i`, as (done so far, goal).
pub fn progress(w: &World, player: u8, i: usize) -> (i64, i64) {
    let Some(p) = w.players.get(player as usize) else { return (0, 1) };
    match i {
        0 => (w.factory_count(player).min(1) as i64, 1),
        1 => (p.mined.min(MINED_GOAL), MINED_GOAL),
        2 => ((p.flags & FLAG_OWN_PROGRAM != 0) as i64, 1),
        3 => (p.seen.count().min(MAPPED_GOAL), MAPPED_GOAL),
        4 => (w.factory_count(player).min(2) as i64, 2),
        _ => (0, 1),
    }
}

impl World {
    /// Complete (and reward) the current objective of every player who
    /// reached it. One objective at a time, in order.
    pub(super) fn check_quests(&mut self) {
        if !self.tick.is_multiple_of(CHECK_EVERY) {
            return;
        }
        for pi in 0..self.players.len() {
            let player = pi as u8;
            if !self.players[pi].alive {
                continue;
            }
            let Some(i) = current(self, player) else { continue };
            let (done, goal) = progress(self, player, i);
            if done >= goal {
                let p = &mut self.players[pi];
                p.quests |= 1 << i;
                p.minerals += QUESTS[i].reward;
                self.events.push(Event::QuestDone { player, quest: i as u8, reward: QUESTS[i].reward });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::dir_vec;
    use crate::programs;
    use crate::rules::{FACTORY_COST, FP};
    use crate::sim::tests::land;
    use crate::sim::Command;

    fn done(w: &World, player: u8) -> Vec<usize> {
        (0..QUESTS.len()).filter(|i| w.players[player as usize].quests & (1 << i) != 0).collect()
    }

    fn completed(w: &World) -> Vec<Event> {
        w.events.iter().filter(|e| matches!(e, Event::QuestDone { .. })).cloned().collect()
    }

    /// Step until an objective is completed, checking they come one at a time.
    fn next_quest(w: &mut World) -> Event {
        for _ in 0..2 * CHECK_EVERY {
            w.step(&[]);
            let mut done = completed(w);
            assert!(done.len() <= 1, "one objective at a time: {done:?}");
            if let Some(e) = done.pop() {
                return e;
            }
        }
        panic!("no objective completed");
    }

    fn idle(w: &mut World) {
        for _ in 0..2 * CHECK_EVERY {
            w.step(&[]);
            assert_eq!(completed(w), vec![]);
        }
    }

    #[test]
    fn first_steps_come_one_at_a_time_with_their_reward() {
        let mut w = World::new(5);
        let me = w.join("ana", false).unwrap();
        let bot = w.join("AI Vega", true).unwrap();
        assert_eq!(current(&w, me), Some(0));
        assert_eq!(current(&w, bot), None, "computer colonies have no objectives");
        // Already true, but objectives go in order: landing first.
        w.players[me as usize].mined = MINED_GOAL;
        idle(&mut w);

        let miner = programs::builtin("miner").unwrap();
        land(&mut w, me, miner.source);
        land(&mut w, bot, miner.source);
        while done(&w, me).is_empty() {
            next_quest(&mut w);
        }
        assert_eq!(done(&w, me), vec![0]);
        let before = w.players[me as usize].minerals;
        assert_eq!(next_quest(&mut w), Event::QuestDone { player: me, quest: 1, reward: QUESTS[1].reward });
        assert!(w.players[me as usize].minerals >= before + QUESTS[1].reward);

        // A built-in program does not count as one of our own.
        w.step(&[(me, Command::UpdateProgram { name: "miner".into(), source: miner.source.into() })]);
        idle(&mut w);
        assert_eq!(current(&w, me), Some(2));
        let own = format!("; my own way\n{}", miner.source);
        w.step(&[(me, Command::UpdateProgram { name: "miner".into(), source: own })]);
        assert_eq!(next_quest(&mut w), Event::QuestDone { player: me, quest: 2, reward: QUESTS[2].reward });

        assert!(progress(&w, me, 3).0 < MAPPED_GOAL, "the arrival scan alone is not enough");
        // A scout goes further and further out.
        let (hx, hy) = w.players[me as usize].home;
        let mut far = 30;
        while progress(&w, me, 3).0 < MAPPED_GOAL {
            assert!(far < 300, "the sector is big enough");
            for k in 0..8 {
                let (dx, dy) = dir_vec(k * 45, far * FP);
                w.reveal(me, hx + dx, hy + dy, 12 * FP);
            }
            far += 20;
        }
        assert_eq!(next_quest(&mut w), Event::QuestDone { player: me, quest: 3, reward: QUESTS[3].reward });

        let f = w.factories.iter().find(|f| f.owner == me).unwrap();
        let (fx, fy) = (f.x, f.y);
        w.players[me as usize].minerals += FACTORY_COST;
        let spot = (2..12)
            .flat_map(|r| (0..360).step_by(30).map(move |a| (r, a)))
            .map(|(r, a)| {
                let (dx, dy) = dir_vec(a, r * 2 * FP);
                (fx + dx, fy + dy)
            })
            .find(|p| w.can_place_factory(me, p.0, p.1).is_ok())
            .expect("room for a second factory");
        w.step(&[(me, Command::PlaceFactory { x: spot.0, y: spot.1, name: "miner".into(), source: miner.source.into() })]);
        assert_eq!(next_quest(&mut w), Event::QuestDone { player: me, quest: 4, reward: QUESTS[4].reward });
        assert_eq!(current(&w, me), None);
        idle(&mut w);
        assert_eq!(w.players[bot as usize].quests, 0);

        // Progress travels with the world (snapshots and saves).
        let copy = World::from_bytes(&w.to_bytes()).unwrap();
        assert_eq!(copy.players[me as usize].quests, w.players[me as usize].quests);
        let saved = World::from_save(&w.to_save()).unwrap();
        assert_eq!((saved.players[me as usize].quests, saved.players[me as usize].flags), (0b11111, FLAG_OWN_PROGRAM));
        assert_eq!(saved.state_hash(), w.state_hash());
    }
}
