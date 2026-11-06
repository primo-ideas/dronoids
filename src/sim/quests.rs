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
    Quest { title: "Posez votre première usine", hint: "clic gauche sur un espace libre", reward: 0 },
    Quest { title: "Extrayez 100 cristaux", hint: "vos drones minent tout seuls", reward: 40 },
    Quest { title: "Appliquez un programme de votre cru", hint: "« Programme » (E), modifiez, Ctrl+S", reward: 60 },
    Quest { title: "Cartographiez les environs", hint: "un éclaireur va loin", reward: 50 },
    Quest { title: "Fondez une deuxième usine", hint: "B, près de vos unités", reward: 80 },
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
        3 => ((p.explored.iter().map(|w| w.count_ones() as i64).sum::<i64>()).min(MAPPED_GOAL), MAPPED_GOAL),
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
