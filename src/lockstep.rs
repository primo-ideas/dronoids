//! Deterministic lockstep for two peers.
//!
//! Commands typed at tick `T` are scheduled for tick `T + INPUT_DELAY`. A
//! peer may only simulate tick `T` once it holds both players' turn for
//! `T`. Periodically each peer attaches its state hash so desyncs are
//! detected instead of silently diverging.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::sim::Command;

pub const INPUT_DELAY: u64 = 3;
pub const HASH_EVERY: u64 = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub tick: u64,
    pub cmds: Vec<Command>,
    /// `(tick, hash)` of the sender's world at that tick.
    pub hash: Option<(u64, u64)>,
}

pub struct Lockstep {
    pub local: u8,
    pub remote: u8,
    pending: Vec<Command>,
    next_send: u64,
    local_turns: BTreeMap<u64, Vec<Command>>,
    remote_turns: BTreeMap<u64, Vec<Command>>,
    local_hashes: BTreeMap<u64, u64>,
    remote_hashes: BTreeMap<u64, u64>,
    pub desync: Option<u64>,
    /// Number of hash comparisons performed.
    pub checks: u32,
}

impl Lockstep {
    pub fn new(local: u8, remote: u8) -> Lockstep {
        let mut ls = Lockstep {
            local,
            remote,
            pending: vec![],
            next_send: INPUT_DELAY,
            local_turns: BTreeMap::new(),
            remote_turns: BTreeMap::new(),
            local_hashes: BTreeMap::new(),
            remote_hashes: BTreeMap::new(),
            desync: None,
            checks: 0,
        };
        for t in 0..INPUT_DELAY {
            ls.local_turns.insert(t, vec![]);
            ls.remote_turns.insert(t, vec![]);
        }
        ls
    }

    pub fn queue(&mut self, cmd: Command) {
        self.pending.push(cmd);
    }

    /// The turn to send now, if any. `hash` is only evaluated when needed.
    pub fn outgoing(&mut self, current_tick: u64, hash: impl FnOnce() -> u64) -> Option<Turn> {
        if self.next_send > current_tick + INPUT_DELAY {
            return None;
        }
        let tick = self.next_send;
        self.next_send += 1;
        let cmds = std::mem::take(&mut self.pending);
        self.local_turns.insert(tick, cmds.clone());
        let hash = if current_tick.is_multiple_of(HASH_EVERY) && !self.local_hashes.contains_key(&current_tick) {
            let h = hash();
            self.local_hashes.insert(current_tick, h);
            self.check(current_tick);
            Some((current_tick, h))
        } else {
            None
        };
        Some(Turn { tick, cmds, hash })
    }

    pub fn receive(&mut self, turn: Turn) {
        if let Some((t, h)) = turn.hash {
            self.remote_hashes.insert(t, h);
            self.check(t);
        }
        self.remote_turns.insert(turn.tick, turn.cmds);
    }

    fn check(&mut self, t: u64) {
        if let (Some(a), Some(b)) = (self.local_hashes.get(&t), self.remote_hashes.get(&t)) {
            self.checks += 1;
            if a != b && self.desync.is_none() {
                self.desync = Some(t);
            }
        }
    }

    /// Commands for `tick` in canonical (player id) order, once both
    /// peers' turns are known.
    pub fn ready(&mut self, tick: u64) -> Option<Vec<(u8, Command)>> {
        if !self.local_turns.contains_key(&tick) || !self.remote_turns.contains_key(&tick) {
            return None;
        }
        let mine = self.local_turns.remove(&tick).unwrap();
        let theirs = self.remote_turns.remove(&tick).unwrap();
        let (first, second) = if self.local < self.remote {
            ((self.local, mine), (self.remote, theirs))
        } else {
            ((self.remote, theirs), (self.local, mine))
        };
        let mut out = vec![];
        out.extend(first.1.into_iter().map(|c| (first.0, c)));
        out.extend(second.1.into_iter().map(|c| (second.0, c)));
        // Old hashes are no longer useful.
        self.local_hashes.retain(|t, _| *t + HASH_EVERY * 8 > tick);
        self.remote_hashes.retain(|t, _| *t + HASH_EVERY * 8 > tick);
        Some(out)
    }

    /// Ticks we are waiting on the other peer for (0 = smooth).
    pub fn waiting(&self, tick: u64) -> bool {
        !self.remote_turns.contains_key(&tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs;
    use crate::sim::World;

    /// Two simulated peers exchanging turns through queues stay in sync.
    #[test]
    fn two_peers_stay_in_sync() {
        let m = programs::builtin("mineur").unwrap();
        let names = ["A".to_string(), "B".to_string()];
        let mut worlds = [World::new(11, &names, m.name, m.source), World::new(11, &names, m.name, m.source)];
        let mut ls = [Lockstep::new(0, 1), Lockstep::new(1, 0)];
        let mut wires: [Vec<Turn>; 2] = [vec![], vec![]];
        let hunter = programs::builtin("chasseur").unwrap();
        let mut steps = [0u64; 2];
        for frame in 0..3000u64 {
            for p in 0..2 {
                // Peer 1 runs a bit slower, to exercise waiting.
                if p == 1 && frame % 3 == 0 {
                    continue;
                }
                if frame == 100 && p == 1 {
                    ls[p].queue(Command::UpdateProgram { name: "mineur".into(), source: hunter.source.into() });
                }
                let w = &worlds[p];
                if let Some(t) = ls[p].outgoing(w.tick, || w.state_hash()) {
                    wires[1 - p].push(t);
                }
                for t in wires[p].drain(..) {
                    ls[p].receive(t);
                }
                if let Some(cmds) = ls[p].ready(worlds[p].tick) {
                    worlds[p].step(&cmds);
                    steps[p] += 1;
                }
            }
        }
        assert!(steps[0] > 1000 && steps[1] > 1000);
        assert!(ls[0].desync.is_none() && ls[1].desync.is_none());
        assert!(ls[0].checks > 10 && ls[1].checks > 10);
    }
}
