//! The endless part of the world. Asteroids and crystals are never stored:
//! they are a pure function of the world's seed and of where you look, so
//! every machine computes the same ones, in whatever order it happens to
//! visit them. Only what players change is saved (`World::worn`).
//!
//! Older worlds also hold a first sector generated whole (`World::size`,
//! `map.rs`): this generates everything around it, and nothing inside.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use super::map::ore_radius;
use super::Rock;
use crate::geo::{dir_vec, dist2};
use crate::rng::Rng;
use crate::rules::*;

/// Terrain is generated and cached by squares of this side.
pub const CHUNK: i32 = 64 * FP;
/// One candidate asteroid per square of this side.
const ROCK_CELL: i32 = 6 * FP;
/// No asteroid is bigger than this.
const MAX_ROCK: i32 = FP * 15 / 2;
const ROCK_GAP: i32 = FP * 6 / 5;
/// One starting place per square of this many chunks.
const SITE_CHUNKS: i32 = 5;
/// No asteroid this close to a starting place.
const SITE_CLEAR: i32 = 16 * FP;
/// Crystal fields start this far inside their chunk, so as to stay in it.
const FIELD_MARGIN: i32 = 8 * FP;
/// No asteroid this close to the first sector.
const CORE_MARGIN: i32 = 12 * FP;
/// The cache starts over past this many chunks (it only saves recomputing).
const MAX_CACHED: usize = 8192;

/// Deposits of the endless terrain have ids from here on.
pub const NATURAL: u64 = 1 << 48;

/// A crystal deposit as generated, before anyone mines it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deposit {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub amount: i32,
}

/// Relics have ids from here on.
pub const RELIC: u64 = 1 << 56;
/// One chunk in this many holds a relic.
const RELIC_ONE_IN: u64 = 5;
/// No relic this close to a starting place: they are worth an expedition.
const RELIC_FROM_SITE: i32 = 50 * FP;

/// A relic as generated, before anyone mines it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Relic {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    /// Which perk it gives (`rules::Perk::of`).
    pub kind: u8,
}

type Cached<T> = RefCell<HashMap<(i32, i32), Arc<Vec<T>>>>;

#[derive(Default)]
pub struct Terrain {
    seed: u64,
    /// Side of the first sector, not generated here (0 if there is none).
    core: i32,
    rocks: Cached<Rock>,
    deposits: Cached<Deposit>,
    relics: Cached<Relic>,
}

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash(seed: u64, salt: u64, a: i32, b: i32) -> u64 {
    let h = mix(seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let h = mix(h ^ (a as i64 as u64).wrapping_mul(0xA24B_AED4_963E_E407));
    mix(h ^ (b as i64 as u64).wrapping_mul(0x9FB2_1C65_1E98_DF25))
}

/// Smooth value noise in 0..=1000 (integer bilinear interpolation).
fn noise(seed: u64, salt: u64, step: i32, x: i32, y: i32) -> i32 {
    let (gx, gy) = (x.div_euclid(step), y.div_euclid(step));
    let (fx, fy) = (x.rem_euclid(step) as i64, y.rem_euclid(step) as i64);
    let s = step as i64;
    let g = |i: i32, j: i32| (hash(seed, salt, i, j) % 1001) as i64;
    let top = g(gx, gy) * (s - fx) + g(gx + 1, gy) * fx;
    let bottom = g(gx, gy + 1) * (s - fx) + g(gx + 1, gy + 1) * fx;
    ((top * (s - fy) + bottom * fy) / (s * s)) as i32
}

#[derive(Clone, Copy)]
struct Candidate {
    x: i32,
    y: i32,
    r: i32,
    rank: u64,
}

impl Candidate {
    /// Of two overlapping candidates, only the higher ranked exists.
    fn beats(&self, other: &Candidate) -> bool {
        let gap = (self.r + other.r + ROCK_GAP) as i64;
        (self.rank, self.x, self.y) > (other.rank, other.x, other.y) && dist2(self.x - other.x, self.y - other.y) < gap * gap
    }
}

impl Terrain {
    pub fn new(seed: u64, core: i32) -> Terrain {
        Terrain { seed, core, ..Default::default() }
    }

    fn in_core(&self, cx: i32, cy: i32) -> bool {
        self.core > 0 && cx >= 0 && cy >= 0 && cx * CHUNK < self.core && cy * CHUNK < self.core
    }

    fn near_core(&self, x: i32, y: i32, m: i32) -> bool {
        self.core > 0 && x > -m && y > -m && x < self.core + m && y < self.core + m
    }

    // ----- starting places --------------------------------------------------

    /// The square of the lattice of starting places that holds (x, y).
    pub fn site_cell(x: i32, y: i32) -> (i32, i32) {
        let side = SITE_CHUNKS * CHUNK;
        (x.div_euclid(side), y.div_euclid(side))
    }

    /// The starting place of a square of the lattice. It lies near the middle
    /// of one chunk, which also holds its first crystals.
    pub fn site(&self, i: i32, j: i32) -> Option<(i32, i32)> {
        let h = hash(self.seed, 31, i, j);
        let cx = i * SITE_CHUNKS + 1 + (h % 3) as i32;
        let cy = j * SITE_CHUNKS + 1 + ((h >> 16) % 3) as i32;
        if self.in_core(cx, cy) {
            return None;
        }
        let jitter = |v: u64| (v % (16 * FP) as u64) as i32 - 8 * FP;
        let x = cx * CHUNK + CHUNK / 2 + jitter(h >> 24);
        let y = cy * CHUNK + CHUNK / 2 + jitter(h >> 44);
        (x.abs() < WORLD_EXTENT - CHUNK && y.abs() < WORLD_EXTENT - CHUNK).then_some((x, y))
    }

    fn site_in_chunk(&self, cx: i32, cy: i32) -> Option<(i32, i32)> {
        let site = self.site(cx.div_euclid(SITE_CHUNKS), cy.div_euclid(SITE_CHUNKS))?;
        (site.0.div_euclid(CHUNK) == cx && site.1.div_euclid(CHUNK) == cy).then_some(site)
    }

    fn sites_around(&self, x: i32, y: i32) -> impl Iterator<Item = (i32, i32)> + '_ {
        let (i, j) = Terrain::site_cell(x, y);
        (j - 1..=j + 1).flat_map(move |b| (i - 1..=i + 1).filter_map(move |a| self.site(a, b)))
    }

    fn near_site(&self, x: i32, y: i32, m: i32) -> bool {
        self.sites_around(x, y).any(|s| dist2(s.0 - x, s.1 - y) < m as i64 * m as i64)
    }

    // ----- asteroids --------------------------------------------------------

    /// How thick with asteroids space is here: belts, and on a far larger
    /// scale whole expanses, either crowded or empty.
    fn density(&self, x: i32, y: i32) -> i32 {
        let belts = noise(self.seed, 1, 56 * FP, x, y);
        let detail = noise(self.seed, 2, 13 * FP, x, y);
        belts * 2 / 3 + detail / 3 + (self.expanse(x, y) - 500) / 3
    }

    fn expanse(&self, x: i32, y: i32) -> i32 {
        noise(self.seed, 3, 1100 * FP, x, y)
    }

    fn candidate(&self, i: i32, j: i32) -> Option<Candidate> {
        let h = hash(self.seed, 11, i, j);
        let x = i * ROCK_CELL + (h % ROCK_CELL as u64) as i32;
        let y = j * ROCK_CELL + ((h >> 20) % ROCK_CELL as u64) as i32;
        let d = self.density(x, y);
        if d < 560 || 560 + ((h >> 40) % 441) as i32 > d {
            return None;
        }
        let roll = hash(self.seed, 12, i, j);
        let spread = (d - 520).min(480) as i64 * MAX_ROCK.min(6 * FP) as i64 / 480;
        let r = FP * 3 / 2 + ((roll % 1000) as i64 * spread / 1000) as i32;
        if self.near_core(x, y, r + CORE_MARGIN) || self.near_site(x, y, r + SITE_CLEAR) {
            return None;
        }
        Some(Candidate { x, y, r, rank: hash(self.seed, 13, i, j) })
    }

    fn make_rocks(&self, cx: i32, cy: i32) -> Vec<Rock> {
        if self.in_core(cx, cy) {
            return vec![];
        }
        let (x0, y0) = (cx * CHUNK, cy * CHUNK);
        let (x1, y1) = (x0 + CHUNK, y0 + CHUNK);
        // Anything that could overlap one of ours competes with it.
        let m = 2 * MAX_ROCK + ROCK_GAP;
        let mut all = vec![];
        for j in (y0 - m).div_euclid(ROCK_CELL)..=(y1 + m).div_euclid(ROCK_CELL) {
            for i in (x0 - m).div_euclid(ROCK_CELL)..=(x1 + m).div_euclid(ROCK_CELL) {
                all.extend(self.candidate(i, j));
            }
        }
        all.iter()
            .filter(|c| c.x >= x0 && c.x < x1 && c.y >= y0 && c.y < y1 && !all.iter().any(|o| o.beats(c)))
            .map(|c| Rock { x: c.x, y: c.y, r: c.r, seed: c.rank as u32 })
            .collect()
    }

    fn rocks_at(&self, cx: i32, cy: i32) -> Arc<Vec<Rock>> {
        if let Some(r) = self.rocks.borrow().get(&(cx, cy)) {
            return r.clone();
        }
        let made = Arc::new(self.make_rocks(cx, cy));
        let mut map = self.rocks.borrow_mut();
        if map.len() >= MAX_CACHED {
            map.clear();
        }
        map.insert((cx, cy), made.clone());
        made
    }

    // ----- crystals ---------------------------------------------------------

    fn make_deposits(&self, cx: i32, cy: i32) -> Vec<Deposit> {
        if self.in_core(cx, cy) {
            return vec![];
        }
        let mut rng = Rng::new(hash(self.seed, 21, cx, cy));
        let mut around = vec![];
        for b in cy - 1..=cy + 1 {
            for a in cx - 1..=cx + 1 {
                around.push(self.rocks_at(a, b));
            }
        }
        let site = self.site_in_chunk(cx, cy);
        let mut field = FieldMaker { cx, cy, rocks: &around, site, out: vec![] };
        if let Some((sx, sy)) = site {
            // Every starting place has crystals right at hand.
            let a = rng.range(0, 360);
            let (dx, dy) = dir_vec(a, 8 * FP);
            field.place(&mut rng, sx + dx, sy + dy, 10, 100);
            let (dx, dy) = dir_vec(a + rng.range(100, 260), 15 * FP);
            field.place(&mut rng, sx + dx, sy + dy, 6, 100);
            return field.out;
        }
        let (x0, y0) = (cx * CHUNK + FIELD_MARGIN, cy * CHUNK + FIELD_MARGIN);
        let side = CHUNK - 2 * FIELD_MARGIN;
        let expanse = self.expanse(x0 + side / 2, y0 + side / 2);
        // Some expanses are rich, others next to barren.
        let fields = rng.range(0, 3) + (expanse > 560) as i32 - (expanse < 400) as i32;
        for _ in 0..fields {
            let (x, y) = (x0 + rng.range(0, side), y0 + rng.range(0, side));
            // Far from every start is richer: worth an expedition.
            let rich = if self.near_site(x, y, 60 * FP) {
                100
            } else if expanse > 650 {
                220
            } else {
                160
            };
            let n = rng.range(5, 15);
            field.place(&mut rng, x, y, n, rich);
        }
        field.out
    }

    fn deposits_at(&self, cx: i32, cy: i32) -> Arc<Vec<Deposit>> {
        if let Some(d) = self.deposits.borrow().get(&(cx, cy)) {
            return d.clone();
        }
        let made = Arc::new(self.make_deposits(cx, cy));
        let mut map = self.deposits.borrow_mut();
        if map.len() >= MAX_CACHED {
            map.clear();
        }
        map.insert((cx, cy), made.clone());
        made
    }

    // ----- relics -----------------------------------------------------------

    /// At most one relic in a chunk, on open ground, away from the
    /// starting places.
    fn make_relics(&self, cx: i32, cy: i32) -> Vec<Relic> {
        let h = hash(self.seed, 41, cx, cy);
        if self.in_core(cx, cy) || self.site_in_chunk(cx, cy).is_some() || !h.is_multiple_of(RELIC_ONE_IN) {
            return vec![];
        }
        let deposits = self.deposits_at(cx, cy);
        let side = (CHUNK - 2 * FIELD_MARGIN) as u64;
        for k in 0..8u64 {
            let roll = hash(self.seed, 42 + k, cx, cy);
            let x = cx * CHUNK + FIELD_MARGIN + (roll % side) as i32;
            let y = cy * CHUNK + FIELD_MARGIN + ((roll >> 24) % side) as i32;
            if x.abs() > WORLD_EXTENT - CHUNK || y.abs() > WORLD_EXTENT - CHUNK || self.near_site(x, y, RELIC_FROM_SITE) {
                continue;
            }
            let apart = |ox: i32, oy: i32, gap: i32| dist2(ox - x, oy - y) >= gap as i64 * gap as i64;
            let mut clear = deposits.iter().all(|d| apart(d.x, d.y, ore_radius(d.amount) + RELIC_RADIUS + FP));
            for b in cy - 1..=cy + 1 {
                for a in cx - 1..=cx + 1 {
                    clear = clear && self.rocks_at(a, b).iter().all(|r| apart(r.x, r.y, r.r + RELIC_RADIUS + FP));
                }
            }
            if clear {
                let id = RELIC | (cx as u16 as u64) << 32 | (cy as u16 as u64) << 16;
                return vec![Relic { id, x, y, kind: ((h >> 8) % crate::rules::PERKS as u64) as u8 }];
            }
        }
        vec![]
    }

    fn relics_at(&self, cx: i32, cy: i32) -> Arc<Vec<Relic>> {
        if let Some(r) = self.relics.borrow().get(&(cx, cy)) {
            return r.clone();
        }
        let made = Arc::new(self.make_relics(cx, cy));
        let mut map = self.relics.borrow_mut();
        if map.len() >= MAX_CACHED {
            map.clear();
        }
        map.insert((cx, cy), made.clone());
        made
    }

    /// Every relic centred in the rectangle, as generated.
    pub fn each_relic(&self, x0: i32, y0: i32, x1: i32, y1: i32, mut f: impl FnMut(&Relic)) {
        for (cx, cy) in Terrain::chunks(x0, y0, x1, y1) {
            for r in self.relics_at(cx, cy).iter().filter(|r| r.x >= x0 && r.x <= x1 && r.y >= y0 && r.y <= y1) {
                f(r);
            }
        }
    }

    /// A relic by its id, as generated.
    pub fn relic(&self, id: u64) -> Option<Relic> {
        let key = id.checked_sub(RELIC)?;
        let (cx, cy) = ((key >> 32) as u16 as i16 as i32, (key >> 16) as u16 as i16 as i32);
        self.relics_at(cx, cy).first().copied().filter(|r| r.id == id)
    }

    // ----- what the world asks ----------------------------------------------

    fn chunks(x0: i32, y0: i32, x1: i32, y1: i32) -> impl Iterator<Item = (i32, i32)> {
        let c = |v: i32| v.clamp(-WORLD_EXTENT, WORLD_EXTENT).div_euclid(CHUNK);
        let (cx0, cx1) = (c(x0), c(x1));
        (c(y0)..=c(y1)).flat_map(move |cy| (cx0..=cx1).map(move |cx| (cx, cy)))
    }

    /// Every asteroid centred in the rectangle.
    pub fn each_rock(&self, x0: i32, y0: i32, x1: i32, y1: i32, mut f: impl FnMut(&Rock)) {
        for (cx, cy) in Terrain::chunks(x0, y0, x1, y1) {
            for k in self.rocks_at(cx, cy).iter().filter(|k| k.x >= x0 && k.x <= x1 && k.y >= y0 && k.y <= y1) {
                f(k);
            }
        }
    }

    /// Every deposit centred in the rectangle, as generated.
    pub fn each_deposit(&self, x0: i32, y0: i32, x1: i32, y1: i32, mut f: impl FnMut(&Deposit)) {
        for (cx, cy) in Terrain::chunks(x0, y0, x1, y1) {
            for d in self.deposits_at(cx, cy).iter().filter(|d| d.x >= x0 && d.x <= x1 && d.y >= y0 && d.y <= y1) {
                f(d);
            }
        }
    }

    /// A deposit by its id, as generated.
    pub fn deposit(&self, id: u64) -> Option<Deposit> {
        if id >= RELIC {
            return None;
        }
        let key = id.checked_sub(NATURAL)?;
        let (cx, cy, i) = ((key >> 32) as u16 as i16 as i32, (key >> 16) as u16 as i16 as i32, (key & 0xFFFF) as usize);
        self.deposits_at(cx, cy).get(i).copied()
    }
}

/// Lays out the crystal fields of one chunk.
struct FieldMaker<'a> {
    cx: i32,
    cy: i32,
    rocks: &'a [Arc<Vec<Rock>>],
    site: Option<(i32, i32)>,
    out: Vec<Deposit>,
}

impl FieldMaker<'_> {
    fn place(&mut self, rng: &mut Rng, fx: i32, fy: i32, n: i32, rich: i32) {
        let mut placed = 0;
        for k in 0..n * 6 {
            if placed >= n {
                break;
            }
            let spread = FP * 2 + (k / 2) * FP / 3;
            let (dx, dy) = dir_vec(rng.range(0, 360), rng.range(0, spread.min(6 * FP)));
            let (x, y) = (fx + dx, fy + dy);
            if x.div_euclid(CHUNK) != self.cx || y.div_euclid(CHUNK) != self.cy {
                continue;
            }
            let amount = (rng.range(ORE_MIN, ORE_MAX + 1) * rich / 100).min(ORE_MAX * 2);
            let r = ore_radius(amount);
            let apart = |ox: i32, oy: i32, gap: i32| dist2(ox - x, oy - y) >= gap as i64 * gap as i64;
            if self.site.is_some_and(|s| !apart(s.0, s.1, r + 4 * FP)) {
                continue;
            }
            if !self.rocks.iter().all(|c| c.iter().all(|k| apart(k.x, k.y, k.r + r + FP / 2))) {
                continue;
            }
            if !self.out.iter().all(|o| apart(o.x, o.y, ore_radius(o.amount) + r + FP / 4)) {
                continue;
            }
            let id = NATURAL | (self.cx as u16 as u64) << 32 | (self.cy as u16 as u64) << 16 | self.out.len() as u64;
            self.out.push(Deposit { id, x, y, amount });
            placed += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: std::ops::RangeInclusive<i32> = -6..=6;

    fn all_rocks(t: &Terrain) -> Vec<Rock> {
        let mut v = vec![];
        for cy in AREA {
            for cx in AREA {
                v.extend(t.rocks_at(cx, cy).iter().cloned());
            }
        }
        v
    }

    #[test]
    fn it_does_not_depend_on_the_order_of_visits() {
        let (a, b) = (Terrain::new(7, 0), Terrain::new(7, 0));
        let forward: Vec<_> = (-5..5).map(|c| (a.rocks_at(c, 3), a.deposits_at(c, 3))).collect();
        let backward: Vec<_> = (-5..5).rev().map(|c| (b.deposits_at(c, 3), b.rocks_at(c, 3))).collect();
        for (f, r) in forward.iter().zip(backward.iter().rev()) {
            assert_eq!(*f.0, *r.1);
            assert_eq!(*f.1, *r.0);
        }
        assert_ne!(*Terrain::new(8, 0).rocks_at(1, 1), *a.rocks_at(1, 1));
        // Forgetting everything changes nothing.
        let before = a.deposits_at(2, 2);
        a.rocks.borrow_mut().clear();
        a.deposits.borrow_mut().clear();
        assert_eq!(before, a.deposits_at(2, 2));
    }

    #[test]
    fn asteroids_never_overlap_even_across_chunks() {
        let rocks = all_rocks(&Terrain::new(3, 0));
        assert!(rocks.len() > 300, "{} asteroids", rocks.len());
        for (i, a) in rocks.iter().enumerate() {
            assert!(a.r <= MAX_ROCK);
            for b in &rocks[i + 1..] {
                let gap = (a.r + b.r) as i64;
                assert!(dist2(a.x - b.x, a.y - b.y) >= gap * gap, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn starting_places_are_clear_and_have_crystals() {
        let t = Terrain::new(5, 0);
        for j in -3..3 {
            for i in -3..3 {
                let (x, y) = t.site(i, j).unwrap();
                let mut near = 0;
                t.each_deposit(x - 16 * FP, y - 16 * FP, x + 16 * FP, y + 16 * FP, |_| near += 1);
                assert!(near >= 3, "{near} deposits by site {i},{j}");
                t.each_rock(x - 24 * FP, y - 24 * FP, x + 24 * FP, y + 24 * FP, |k| {
                    let gap = (k.r + SITE_CLEAR) as i64;
                    assert!(dist2(k.x - x, k.y - y) >= gap * gap);
                });
            }
        }
    }

    #[test]
    fn deposits_are_on_open_ground_and_can_be_found_by_id() {
        let t = Terrain::new(11, 0);
        let rocks = all_rocks(&t);
        let mut seen = std::collections::HashSet::new();
        for cy in -4..=4 {
            for cx in -4..=4 {
                for d in t.deposits_at(cx, cy).iter() {
                    assert!(seen.insert(d.id), "two deposits share an id");
                    assert_eq!(t.deposit(d.id), Some(*d));
                    assert!((d.x.div_euclid(CHUNK), d.y.div_euclid(CHUNK)) == (cx, cy));
                    for k in &rocks {
                        let gap = (k.r + ore_radius(d.amount)) as i64;
                        assert!(dist2(k.x - d.x, k.y - d.y) >= gap * gap);
                    }
                }
            }
        }
        assert!(seen.len() > 100, "{} deposits", seen.len());
    }

    #[test]
    fn relics_are_rare_far_from_starts_on_open_ground_and_found_by_id() {
        let t = Terrain::new(11, 0);
        let rocks = all_rocks(&t);
        let (mut n, mut kinds) = (0, [0; crate::rules::PERKS]);
        for cy in -5..=5 {
            for cx in -5..=5 {
                let here = t.relics_at(cx, cy);
                assert!(here.len() <= 1);
                for r in here.iter() {
                    n += 1;
                    kinds[r.kind as usize] += 1;
                    assert_eq!(t.relic(r.id), Some(*r));
                    assert_eq!(t.deposit(r.id), None, "a relic is not a deposit");
                    assert!((r.x.div_euclid(CHUNK), r.y.div_euclid(CHUNK)) == (cx, cy));
                    assert!(!t.near_site(r.x, r.y, RELIC_FROM_SITE));
                    for k in &rocks {
                        let gap = (k.r + RELIC_RADIUS) as i64;
                        assert!(dist2(k.x - r.x, k.y - r.y) >= gap * gap);
                    }
                    for d in t.deposits_at(cx, cy).iter() {
                        let gap = (ore_radius(d.amount) + RELIC_RADIUS) as i64;
                        assert!(dist2(d.x - r.x, d.y - r.y) >= gap * gap);
                    }
                }
            }
        }
        // 121 chunks: about one in five, less those by a starting place.
        assert!((8..=40).contains(&n), "{n} relics");
        assert!(kinds.iter().all(|k| *k > 0), "{kinds:?}");
        assert_eq!(t.relic(RELIC | 3 << 32 | 0xFFFF), None);
    }

    #[test]
    fn the_first_sector_is_left_alone_and_the_far_reaches_work() {
        let core = 768 * FP;
        let t = Terrain::new(1, core);
        assert!(t.rocks_at(5, 5).is_empty() && t.deposits_at(11, 0).is_empty());
        for cy in -2..14 {
            for cx in -2..14 {
                for k in t.rocks_at(cx, cy).iter() {
                    assert!(k.x < -CORE_MARGIN + 1 || k.y < -CORE_MARGIN + 1 || k.x >= core + CORE_MARGIN || k.y >= core + CORE_MARGIN);
                }
            }
        }
        let far = WORLD_EXTENT - 10 * CHUNK;
        let mut found = 0;
        t.each_rock(far, -far, far + 40 * CHUNK, -far + 40 * CHUNK, |_| found += 1);
        t.each_deposit(-far - 40 * CHUNK, far, -far, far + 40 * CHUNK, |d| {
            assert_eq!(t.deposit(d.id), Some(*d));
            found += 1;
        });
        assert!(found > 0);
    }
}
