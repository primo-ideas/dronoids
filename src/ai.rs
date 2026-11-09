//! Computer opponents. They play through the same `Command`s as humans and
//! only use the built-in programs — beat them with better code.

use crate::geo::{dir_vec, dist2};
use crate::programs::builtin;
use crate::rules::*;
use crate::sim::{Command, World};

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
            Level::Normal => 6,
            Level::Hard => 9,
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
        let minutes = match self {
            Level::Easy => 16,
            Level::Normal => 10,
            Level::Hard => 5,
        };
        (minutes as f32 * 60.0 * TICKS_PER_SEC) as u64
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
const BUILD_ORDER: &[&str] = &["mineur", "eclaireur", "mineur", "chasseur", "gardien", "mineur", "chasseur", "chasseur"];

const DENSITY_CELL: i32 = 8 * FP;
/// Colonies only send raids this far from home: the world is big enough
/// for newcomers to grow in peace if they settle away from them.
const RAID_RANGE: i32 = 170 * FP;

/// A free spot to land a first factory, as close as possible to home.
fn landing_spot(w: &World, player: u8) -> Option<(i32, i32)> {
    let (hx, hy) = w.players.get(player as usize)?.home;
    (0..12)
        .flat_map(|r| (0..360).step_by(30).map(move |a| (r, a)))
        .map(|(r, a)| {
            let (dx, dy) = dir_vec(a, r * 3 * FP);
            (hx + dx, hy + dy)
        })
        .find(|p| w.can_place_factory(player, p.0, p.1).is_ok())
}

impl Bot {
    pub fn new(player: u8, level: Level) -> Bot {
        Bot { player, level, next_think: 60 + player as u64 * 7, next_convert: 0, built: 0 }
    }

    pub fn think(&mut self, w: &World) -> Vec<Command> {
        if w.tick < self.next_think {
            return vec![];
        }
        self.next_think = w.tick + 25;
        let Some(me) = w.players.get(self.player as usize) else { return vec![] };
        if !me.alive {
            return vec![];
        }
        let mut cmds = vec![];
        // Like everyone, a colony starts with nothing: land a first factory.
        if w.is_landing(self.player) {
            if me.minerals >= FACTORY_COST
                && let Some((x, y)) = landing_spot(w, self.player)
            {
                let b = builtin("mineur").expect("builtin");
                cmds.push(Command::PlaceFactory { x, y, name: b.name.into(), source: b.source.into() });
            }
            return cmds;
        }
        let hunting = w.tick >= self.level.hunter_gate();

        if me.minerals >= FACTORY_COST + 30 && w.factory_count(self.player) < self.level.max_factories()
            && let Some((x, y)) = self.pick_site(w) {
                let mut name = BUILD_ORDER[self.built as usize % BUILD_ORDER.len()];
                if (name == "chasseur" || name == "gardien") && !hunting {
                    name = "mineur";
                }
                let b = builtin(name).expect("builtin");
                cmds.push(Command::PlaceFactory { x, y, name: b.name.into(), source: b.source.into() });
                self.built += 1;
            }

        if hunting && w.tick >= self.next_convert {
            self.next_convert = w.tick + 300;
            let mine: Vec<_> = w.factories.iter().filter(|f| f.owner == self.player).collect();
            let hunters: Vec<_> = mine.iter().filter(|f| w.program_name(f.prog) == "chasseur").collect();
            if hunters.len() < mine.len() / self.level.hunter_share()
                && let Some(f) = mine.iter().rev().find(|f| matches!(w.program_name(f.prog), "mineur" | "eclaireur")) {
                    let b = builtin("chasseur").expect("builtin");
                    cmds.push(Command::SetProgram { factory: f.id, name: b.name.into(), source: b.source.into() });
                }
            // Point every hunter factory at the closest rival's home.
            if let Some(target) = self.closest_rival(w) {
                for f in &hunters {
                    if f.mem[2] != 1 || f.mem[0] != target.0 || f.mem[1] != target.1 {
                        for (i, v) in [(0u8, target.0), (1, target.1), (2, 1)] {
                            cmds.push(Command::SetMemory { factory: f.id, index: i, value: v });
                        }
                    }
                }
                if self.level == Level::Hard && me.minerals > 400 {
                    for f in hunters.iter().filter(|f| f.quota < 14) {
                        cmds.push(Command::SetQuota { factory: f.id, quota: 14 });
                    }
                }
            }
        }
        cmds
    }

    /// Nearest factory (in world units) of a rival worth raiding: close
    /// enough to our home, and not a shielded newcomer.
    fn closest_rival(&self, w: &World) -> Option<(i32, i32)> {
        let me = w.players.get(self.player as usize)?;
        let range = (RAID_RANGE as i64) * (RAID_RANGE as i64);
        w.factories
            .iter()
            .filter(|f| f.owner != self.player && !w.shielded(f.owner))
            .map(|f| (dist2(f.x - me.home.0, f.y - me.home.1), (f.x, f.y)))
            .filter(|(d, _)| *d <= range)
            .min()
            .map(|(_, (x, y))| (x / FP, y / FP))
    }

    /// Best buildable spot near our units: lots of crystals around, not
    /// too close to our other factories.
    fn pick_site(&self, w: &World) -> Option<(i32, i32)> {
        let me = self.player;
        let n = w.size / DENSITY_CELL + 1;
        let mut density = vec![0i32; (n * n) as usize];
        for o in &w.ores {
            density[((o.y / DENSITY_CELL) * n + o.x / DENSITY_CELL) as usize] += o.amount;
        }
        let ore_around = |x: i32, y: i32| {
            let (cx, cy) = (x / DENSITY_CELL, y / DENSITY_CELL);
            let mut s = 0;
            for j in (cy - 1).max(0)..=(cy + 1).min(n - 1) {
                for i in (cx - 1).max(0)..=(cx + 1).min(n - 1) {
                    s += density[(j * n + i) as usize];
                }
            }
            s
        };
        let anchors: Vec<(i32, i32)> = w
            .factories
            .iter()
            .filter(|f| f.owner == me)
            .map(|f| (f.x, f.y))
            .chain(w.dronoids.iter().filter(|d| d.owner == me).step_by(3).take(30).map(|d| (d.x, d.y)))
            .collect();
        let mut best: Option<(i32, (i32, i32))> = None;
        for (k, (ax, ay)) in anchors.iter().enumerate() {
            for step in 0..8 {
                let angle = step * 45 + (k as i32 * 17) % 45;
                for r in [5 * FP, 7 * FP] {
                    let (dx, dy) = dir_vec(angle, r);
                    let (x, y) = (ax + dx, ay + dy);
                    if w.can_place_factory(me, x, y).is_err() {
                        continue;
                    }
                    let ore = ore_around(x, y);
                    if ore == 0 {
                        continue;
                    }
                    let nearest_own = w
                        .factories
                        .iter()
                        .filter(|f| f.owner == me)
                        .map(|f| dist2(f.x - x, f.y - y))
                        .min()
                        .unwrap_or(i64::MAX);
                    let crowd = (16 * FP) as i64;
                    let penalty = if nearest_own < crowd * crowd { 300 } else { 0 };
                    let score = ore - penalty;
                    if best.is_none_or(|(s, _)| score > s) {
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

    fn bot_world(level: Level, bots: usize, human: bool) -> (World, Vec<Bot>) {
        let mut w = World::new(5);
        if human {
            w.join("humain", false);
        }
        let mut list = vec![];
        for i in 0..bots {
            let id = w.join(&format!("IA {i}"), true).unwrap();
            list.push(Bot::new(id, level));
        }
        (w, list)
    }

    fn run(w: &mut World, bots: &mut [Bot], ticks: u64) {
        for _ in 0..ticks {
            let mut cmds = vec![];
            for b in bots.iter_mut() {
                let p = b.player;
                cmds.extend(b.think(w).into_iter().map(|c| (p, c)));
            }
            w.step(&cmds);
            if w.winner().is_some() {
                break;
            }
        }
    }

    #[test]
    fn bot_expands() {
        let (mut w, mut bots) = bot_world(Level::Normal, 1, false);
        run(&mut w, &mut bots, 6000);
        assert!(w.factory_count(0) >= 2, "bot has {} factories", w.factory_count(0));
    }

    /// `cargo test --release --lib bench_world -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_world() {
        for level in [Level::Easy, Level::Normal, Level::Hard] {
            let (mut w, mut bots) = bot_world(level, 6, true);
            let t0 = std::time::Instant::now();
            let ticks = 30 * 60 * TICKS_PER_SEC as u64;
            let mut human_out = None;
            for _ in 0..ticks / 100 {
                run(&mut w, &mut bots, 100);
                if human_out.is_none() && !w.players[0].alive {
                    human_out = Some(w.tick as f32 / TICKS_PER_SEC / 60.0);
                }
            }
            println!("  idle human eliminated at {human_out:?} min");
            let el = t0.elapsed();
            let summary: Vec<String> = w
                .players
                .iter()
                .map(|p| format!("{}:{}f/{}d{}", p.id, w.factory_count(p.id), w.dronoid_count(p.id), if p.alive { "" } else { "✗" }))
                .collect();
            println!(
                "{level:?}: {} ticks ({:.1} min) in {:?} = {:.3} ms/tick, {} dronoids; {}",
                w.tick,
                w.tick as f32 / TICKS_PER_SEC / 60.0,
                el,
                el.as_secs_f64() * 1000.0 / w.tick as f64,
                w.dronoids.len(),
                summary.join(" ")
            );
        }
    }
}
