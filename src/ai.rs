//! A simple computer opponent. It plays through the same `Command`s as a
//! human and only uses the built-in programs — beat it with better code.

use crate::programs::builtin;
use crate::rules::{BUILD_RANGE, FACTORY_COST};
use crate::sim::{Command, Tile, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Easy,
    Normal,
    Hard,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Easy => "Facile",
            Level::Normal => "Normal",
            Level::Hard => "Difficile",
        }
    }
    fn max_factories(self) -> usize {
        match self {
            Level::Easy => 4,
            Level::Normal => 7,
            Level::Hard => 10,
        }
    }
    /// Once hunters are allowed, 1 factory out of this many hunts.
    fn hunter_share(self) -> usize {
        match self {
            Level::Easy => 4,
            Level::Normal => 3,
            Level::Hard => 2,
        }
    }
    /// No hunters before this tick.
    fn hunter_gate(self) -> u64 {
        match self {
            Level::Easy => 7200,
            Level::Normal => 3600,
            Level::Hard => 1000,
        }
    }
}

pub struct Bot {
    pub player: u8,
    pub level: Level,
    next_think: u64,
    next_convert: u64,
    built: u32,
}

/// Programs given to successive new factories.
const BUILD_ORDER: &[&str] = &["mineur", "eclaireur", "chasseur", "mineur", "gardien", "chasseur", "chasseur"];

impl Bot {
    pub fn new(player: u8, level: Level) -> Bot {
        Bot { player, level, next_think: 40, next_convert: 0, built: 0 }
    }

    pub fn think(&mut self, w: &World) -> Vec<Command> {
        if w.tick < self.next_think {
            return vec![];
        }
        self.next_think = w.tick + 16;
        let Some(me) = w.players.get(self.player as usize) else { return vec![] };
        if !me.alive {
            return vec![];
        }
        let mut cmds = vec![];
        let factories = w.factory_count(self.player);
        if me.minerals >= FACTORY_COST + 25 && factories < self.level.max_factories()
            && let Some((x, y)) = self.pick_site(w) {
                let mut name = BUILD_ORDER[self.built as usize % BUILD_ORDER.len()];
                if name != "mineur" && name != "eclaireur" && w.tick < self.level.hunter_gate() {
                    name = "mineur";
                }
                let b = builtin(name).expect("builtin");
                cmds.push(Command::PlaceFactory { x, y, name: b.name.into(), source: b.source.into() });
                self.built += 1;
            }
        // Once allowed, turn some miners into hunters.
        if w.tick >= self.level.hunter_gate() && w.tick >= self.next_convert {
            let mine: Vec<_> = w.factories.iter().filter(|f| f.owner == self.player).collect();
            let hunters = mine.iter().filter(|f| f.program.name == "chasseur").count();
            if hunters < mine.len() / self.level.hunter_share()
                && let Some(f) = mine.iter().rev().find(|f| f.program.name == "mineur" || f.program.name == "eclaireur") {
                    let b = builtin("chasseur").expect("builtin");
                    cmds.push(Command::SetProgram { factory: f.id, name: b.name.into(), source: b.source.into() });
                    self.next_convert = w.tick + 300;
                }
        }
        // Hard bots pour their surplus into bigger armies.
        if self.level == Level::Hard && me.minerals > 300 {
            for f in w.factories.iter().filter(|f| f.owner == self.player && f.quota < 14) {
                if f.program.name == "chasseur" {
                    cmds.push(Command::SetQuota { factory: f.id, quota: 14 });
                }
            }
        }
        cmds
    }

    /// Best buildable tile near our drones: lots of ore around, not too
    /// close to our existing factories.
    fn pick_site(&self, w: &World) -> Option<(i32, i32)> {
        let me = self.player;
        let anchors: Vec<(i32, i32)> = w
            .drones
            .iter()
            .filter(|d| d.owner == me)
            .step_by(3)
            .take(40)
            .map(|d| (d.x, d.y))
            .collect();
        let mut best: Option<(i32, (i32, i32))> = None;
        for (ax, ay) in anchors {
            for dy in (-BUILD_RANGE..=BUILD_RANGE).step_by(2) {
                for dx in (-BUILD_RANGE..=BUILD_RANGE).step_by(2) {
                    if dx.abs() + dy.abs() > BUILD_RANGE {
                        continue;
                    }
                    let (x, y) = (ax + dx, ay + dy);
                    if w.can_place_factory(me, x, y).is_err() {
                        continue;
                    }
                    let mut ore = 0;
                    for oy in -5..=5 {
                        for ox in -5..=5 {
                            if let Tile::Ore(n) = w.tile(x + ox, y + oy) {
                                ore += n as i32;
                            }
                        }
                    }
                    let nearest_own = w
                        .factories
                        .iter()
                        .filter(|f| f.owner == me)
                        .map(|f| (f.x - x).abs() + (f.y - y).abs())
                        .min()
                        .unwrap_or(99);
                    let crowding = if nearest_own < 7 { (7 - nearest_own) * 60 } else { 0 };
                    let score = ore - crowding;
                    if ore > 0 && best.is_none_or(|(s, _)| score > s) {
                        best = Some((score, (x, y)));
                    }
                }
            }
        }
        best.map(|(_, p)| p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs;

    #[test]
    fn bot_expands() {
        let m = programs::builtin("mineur").unwrap();
        let mut w = World::new(5, &["A".into(), "Bot".into()], m.name, m.source);
        let mut bot = Bot::new(1, Level::Normal);
        for _ in 0..2500 {
            let cmds: Vec<(u8, Command)> = bot.think(&w).into_iter().map(|c| (1, c)).collect();
            w.step(&cmds);
        }
        assert!(w.factory_count(1) >= 2, "bot has {} factories", w.factory_count(1));
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::programs;

    /// `cargo test --release --lib bench_battle -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_battle() {
        let m = programs::builtin("mineur").unwrap();
        for (lvl, passive) in [(Level::Hard, false), (Level::Normal, true), (Level::Easy, true)] {
            let mut w = World::new(9, &["A".into(), "B".into()], m.name, m.source);
            let mut bots = [Bot::new(0, lvl), Bot::new(1, lvl)];
            let t0 = std::time::Instant::now();
            let mut max_drones = 0;
            let mut ticks = 0;
            for _ in 0..12000 {
                let mut cmds = vec![];
                for (i, b) in bots.iter_mut().enumerate() {
                    if passive && i == 0 {
                        continue;
                    }
                    let p = b.player;
                    cmds.extend(b.think(&w).into_iter().map(|c| (p, c)));
                }
                w.step(&cmds);
                ticks += 1;
                max_drones = max_drones.max(w.drones.len());
                if w.winner().is_some() {
                    break;
                }
            }
            let el = t0.elapsed();
            println!(
                "{lvl:?} passive_p0={passive}: {ticks} ticks ({:.1} min game) in {:?} = {:.3} ms/tick, max drones {max_drones}, winner {:?}",
                ticks as f32 / 8.0 / 60.0,
                el,
                el.as_secs_f64() * 1000.0 / ticks as f64,
                w.winner()
            );
        }
    }
}
