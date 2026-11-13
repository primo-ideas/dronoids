//! Computer opponents. They play through the same `Command`s as humans and
//! only use the built-in programs — beat them with better code.

use crate::geo::{dir_vec, dist2};
use crate::programs::builtin;
use crate::rules::*;
use crate::sim::{Command, Limit, Structure, TaskKind, TaskSpec, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Easy,
    Normal,
    Hard,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Easy => "Easy",
            Level::Normal => "Normal",
            Level::Hard => "Hard",
        }
    }
    /// How many factories the colony builds.
    fn max_factories(self) -> usize {
        match self {
            Level::Easy => 2,
            Level::Normal => 4,
            Level::Hard => 8,
        }
    }
    /// Once hunters are allowed, the share (in percent) of the colony's
    /// dronoids that go to them.
    fn hunter_percent(self) -> u32 {
        match self {
            Level::Easy => 25,
            Level::Normal => 34,
            Level::Hard => 50,
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
    next_plan: u64,
}

/// Colonies only send raids this far from home (the hardest ones also let
/// idle hunters roam): the world is big enough
/// for newcomers to grow in peace if they settle away from them.
const RAID_RANGE: i32 = 170 * FP;

/// A free spot for a factory around the base, within what it sees.
fn factory_spot(w: &World, player: u8, bx: i32, by: i32) -> Option<(i32, i32)> {
    (0..4)
        .flat_map(|r| (0..360).step_by(30).map(move |a| (r, a + r * 15)))
        .map(|(r, a)| {
            let (dx, dy) = dir_vec(a, (9 + r * 4) * FP);
            (bx + dx, by + dy)
        })
        .find(|p| w.can_build(player, Structure::Factory, p.0, p.1).is_ok())
}

/// A free spot to land the base, as close as possible to home.
fn landing_spot(w: &World, player: u8) -> Option<(i32, i32)> {
    let (hx, hy) = w.players.get(player as usize)?.home;
    (0..12)
        .flat_map(|r| (0..360).step_by(30).map(move |a| (r, a)))
        .map(|(r, a)| {
            let (dx, dy) = dir_vec(a, r * 3 * FP);
            (hx + dx, hy + dy)
        })
        .find(|p| w.can_land(player, p.0, p.1).is_ok())
}

impl Bot {
    pub fn new(player: u8, level: Level) -> Bot {
        Bot { player, level, next_think: 60 + player as u64 * 7, next_plan: 0 }
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
        // Like everyone, a colony starts with nothing: land the base.
        if w.is_landing(self.player) {
            if let Some((x, y)) = landing_spot(w, self.player) {
                let b = builtin("miner").expect("builtin");
                cmds.push(Command::Land { x, y, name: b.name.into(), source: b.source.into() });
            }
            return cmds;
        }
        let Some(base) = w.base_of(self.player) else { return cmds };
        let hunting = w.tick >= self.level.hunter_gate();

        // Crystals go into factories: more dronoids, and room for them.
        let factories = w.factories.iter().filter(|f| f.owner == self.player).count();
        if me.minerals >= FACTORY_COST + 30
            && factories < self.level.max_factories()
            && let Some((x, y)) = factory_spot(w, self.player, base.x, base.y)
        {
            cmds.push(Command::Build { kind: Structure::Factory, x, y });
        }

        if w.tick >= self.next_plan {
            self.next_plan = w.tick + 300;
            // A mining task that never ends; once hunting is allowed, a raid
            // (a guard, for the easiest colonies) takes a share.
            let fighter = if self.level == Level::Easy { TaskKind::Guard } else { TaskKind::Raid };
            let room = w.room(self.player) as u32;
            let fighters = if hunting { room * self.level.hunter_percent() / 100 } else { 0 };
            for (kind, count) in [(TaskKind::Mine, room - fighters), (fighter, fighters)] {
                let count = count.min(MAX_QUOTA as u32) as u8;
                let task = w.tasks.iter().find(|t| t.owner == self.player && t.spec.kind == kind);
                match task {
                    None if count > 0 => {
                        let mut spec = TaskSpec::new(kind, count);
                        spec.limit = Limit::Never;
                        cmds.push(Command::NewTask { spec });
                    }
                    Some(t) if t.spec.count != count && count > 0 => {
                        cmds.push(Command::EditTask { id: t.id, count, limit: Limit::Never, repeat: false });
                    }
                    _ => {}
                }
            }
            // Point the hunters at the closest rival within reach. With
            // none, all but the hardest colonies call their hunters home,
            // where they stand guard: left to patrol, they would end up at
            // the door of a newcomer settled far away.
            if hunting {
                let rival = self.closest_rival(w);
                let rally = (self.level != Level::Hard).then_some((me.home.0.div_euclid(FP), me.home.1.div_euclid(FP)));
                let target = rival.or(rally);
                let set = target.map_or((base.mem[0], base.mem[1], 0), |t| (t.0, t.1, 1));
                if (base.mem[0], base.mem[1], base.mem[2]) != set {
                    for (i, v) in [(0u8, set.0), (1, set.1), (2, set.2)] {
                        cmds.push(Command::SetMemory { index: i, value: v });
                    }
                }
            }
        }
        cmds
    }

    /// Nearest base (in world units) of a rival worth raiding: close
    /// enough to our home, and not a shielded newcomer.
    fn closest_rival(&self, w: &World) -> Option<(i32, i32)> {
        let me = w.players.get(self.player as usize)?;
        let range = (RAID_RANGE as i64) * (RAID_RANGE as i64);
        w.bases
            .iter()
            .filter(|b| b.owner != self.player && !w.shielded(b.owner))
            .map(|b| (dist2(b.x - me.home.0, b.y - me.home.1), (b.x, b.y)))
            .filter(|(d, _)| *d <= range)
            .min()
            .map(|(_, (x, y))| (x.div_euclid(FP), y.div_euclid(FP)))
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
            let id = w.join(&format!("AI {i}"), true).unwrap();
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

    /// With no rival within reach, a colony keeps its hunters at home,
    /// except the hardest ones, whose hunters roam.
    #[test]
    fn idle_hunters_are_called_home() {
        for level in [Level::Normal, Level::Hard] {
            let (mut w, mut bots) = bot_world(level, 1, false);
            let gate = level.hunter_gate();
            run(&mut w, &mut bots, gate + 2000);
            let home = w.players[0].home;
            let b = w.base_of(0).expect("landed");
            assert!(w.tasks.iter().any(|t| t.spec.kind == TaskKind::Raid), "{level:?}: a colony ends up raiding");
            assert!(w.dronoids.iter().any(|d| w.program_name(d.prog) == "hunter"), "{level:?}: with hunters"); 
            if level == Level::Hard {
                assert_eq!(b.mem[2], 0, "the hardest colonies let them roam");
            } else {
                assert_eq!((b.mem[0], b.mem[1], b.mem[2]), (home.0.div_euclid(FP), home.1.div_euclid(FP), 1));
            }
        }
    }

    #[test]
    fn bot_grows() {
        let (mut w, mut bots) = bot_world(Level::Normal, 1, false);
        run(&mut w, &mut bots, 6000);
        assert!(w.factories.iter().any(|f| f.owner == 0), "a colony builds factories");
        assert!(w.dronoid_count(0) > BASE_CAPACITY as usize, "{} dronoids", w.dronoid_count(0));
        assert!(w.tasks.iter().any(|t| t.owner == 0 && t.spec.kind == TaskKind::Mine), "and puts them to mining");
    }

    /// Does anything ever happen between colonies left to themselves?
    /// `cargo test --release --lib balance_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn balance_report_wars() {
        for level in [Level::Easy, Level::Normal, Level::Hard] {
            let (mut w, mut bots) = bot_world(level, 4, true);
            let miner = builtin("miner").unwrap();
            crate::sim::tests::land(&mut w, 0, miner.source);
            let minute = 60 * TICKS_PER_SEC as u64;
            let mut out = None;
            let mut first_hit = None;
            for m in 1..=90 {
                run(&mut w, &mut bots, minute);
                if out.is_none() && !w.players[0].alive {
                    out = Some(m);
                }
                if first_hit.is_none() && w.base_of(0).is_some_and(|b| b.hp < BASE_HP) {
                    first_hit = Some(m);
                }
            }
            let home = w.players[0].home;
            let gaps: Vec<i32> = w.players.iter().skip(1).map(|p| crate::geo::dist(p.home.0 - home.0, p.home.1 - home.1) / FP).collect();
            let summary: Vec<String> = w
                .players
                .iter()
                .map(|p| {
                    let gone = if p.alive { "" } else { "x" };
                    let cap = w.room(p.id);
                    format!("{}:{}cap/{}d/{}k/{}c{gone}", p.id, cap, w.dronoid_count(p.id), p.kills, p.minerals)
                })
                .collect();
            println!("{level:?}: human first hit {first_hit:?} min, out {out:?} min, AIs at {gaps:?}; {}", summary.join(" "));
        }
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
                .map(|p| format!("{}:{}d{}", p.id, w.dronoid_count(p.id), if p.alive { "" } else { "✗" }))
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
