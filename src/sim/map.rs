//! Seeded generation of a large open world: asteroid belts, crystal
//! fields and well-spread starting places.

use super::Rock;
use crate::geo::{dir_vec, dist2};
use crate::rng::Rng;
use crate::rules::*;

pub struct GeneratedMap {
    pub size: i32,
    pub rocks: Vec<Rock>,
    /// (x, y, amount)
    pub ores: Vec<(i32, i32, i32)>,
    pub slots: Vec<(i32, i32)>,
}

/// Radius of an ore deposit holding `amount` crystals.
pub fn ore_radius(amount: i32) -> i32 {
    FP * 3 / 10 + FP * 4 / 10 * amount.clamp(0, ORE_MAX) / ORE_MAX
}

/// Smooth value noise in 0..1000 (integer bilinear interpolation).
struct Noise {
    step: i32,
    n: i32,
    v: Vec<i32>,
}

impl Noise {
    fn new(rng: &mut Rng, size: i32, step: i32) -> Noise {
        let n = size / step + 2;
        let v = (0..n * n).map(|_| rng.range(0, 1001)).collect();
        Noise { step, n, v }
    }

    fn at(&self, x: i32, y: i32) -> i32 {
        let (gx, gy) = (x / self.step, y / self.step);
        let (fx, fy) = ((x % self.step) as i64, (y % self.step) as i64);
        let s = self.step as i64;
        let g = |i: i32, j: i32| self.v[((j.min(self.n - 1)) * self.n + i.min(self.n - 1)) as usize] as i64;
        let top = g(gx, gy) * (s - fx) + g(gx + 1, gy) * fx;
        let bot = g(gx, gy + 1) * (s - fx) + g(gx + 1, gy + 1) * fx;
        ((top * (s - fy) + bot * fy) / (s * s)) as i32
    }
}

pub fn generate(seed: u64, size_units: i32) -> GeneratedMap {
    let mut rng = Rng::new(seed);
    let size = size_units * FP;
    let margin = 24 * FP;

    // 1. Starting places, spread out by farthest-point sampling.
    let mut slots: Vec<(i32, i32)> = vec![];
    let c = size / 2;
    slots.push((c + rng.range(-40 * FP, 40 * FP), c + rng.range(-40 * FP, 40 * FP)));
    while slots.len() < SPAWN_SLOTS {
        let mut best = (0i64, (0, 0));
        for _ in 0..80 {
            let p = (rng.range(margin, size - margin), rng.range(margin, size - margin));
            let d = slots.iter().map(|s| dist2(s.0 - p.0, s.1 - p.1)).min().unwrap();
            if d > best.0 {
                best = (d, p);
            }
        }
        slots.push(best.1);
    }
    let near_slot = |x: i32, y: i32, r: i32| slots.iter().any(|s| dist2(s.0 - x, s.1 - y) < r as i64 * r as i64);

    // 2. Asteroid belts follow a noise field.
    let belts = Noise::new(&mut rng, size, 56 * FP);
    let detail = Noise::new(&mut rng, size, 13 * FP);
    let mut rocks: Vec<Rock> = vec![];
    let cell = 16 * FP;
    let mut rock_grid = super::grid::Grid::new(cell);
    let mut near = vec![];
    let attempts = (size_units as i64 * size_units as i64 / 30) as usize;
    for _ in 0..attempts {
        let (x, y) = (rng.range(FP * 4, size - FP * 4), rng.range(FP * 4, size - FP * 4));
        let d = belts.at(x, y) * 2 / 3 + detail.at(x, y) / 3;
        if d < 560 || rng.range(560, 1001) > d {
            continue;
        }
        let r = rng.range(FP * 3 / 2, FP * 3 / 2 + (d - 520) * FP * 6 / 480);
        if near_slot(x, y, r + 16 * FP) {
            continue;
        }
        rock_grid.query(x, y, r + 8 * FP, &mut near);
        let overlaps = near.iter().any(|&i| {
            let o: &Rock = &rocks[i as usize];
            let gap = o.r + r + FP * 6 / 5;
            dist2(o.x - x, o.y - y) < gap as i64 * gap as i64
        });
        if overlaps {
            continue;
        }
        rock_grid.insert(x, y, rocks.len() as u32);
        rocks.push(Rock { x, y, r, seed: rng.next_u64() as u32 });
    }

    // 3. Crystal fields.
    let mut ores: Vec<(i32, i32, i32)> = vec![];
    let mut ore_grid = super::grid::Grid::new(cell);
    let mut cluster = |rng: &mut Rng, ores: &mut Vec<(i32, i32, i32)>, cx: i32, cy: i32, n: usize, rich: i32| {
        let mut placed = 0;
        for k in 0..n * 6 {
            if placed >= n {
                break;
            }
            let spread = FP * 2 + (k as i32 / 2) * FP / 3;
            let (dx, dy) = dir_vec(rng.range(0, 360), rng.range(0, spread.min(6 * FP)));
            let (x, y) = (cx + dx, cy + dy);
            if x < FP * 2 || y < FP * 2 || x > size - FP * 2 || y > size - FP * 2 {
                continue;
            }
            let amount = (rng.range(ORE_MIN, ORE_MAX + 1) * rich / 100).min(ORE_MAX * 2);
            let r = ore_radius(amount);
            if near_slot(x, y, r + 4 * FP) {
                continue;
            }
            rock_grid.query(x, y, r + 8 * FP, &mut near);
            if near.iter().any(|&i| {
                let o = &rocks[i as usize];
                let gap = o.r + r + FP / 2;
                dist2(o.x - x, o.y - y) < gap as i64 * gap as i64
            }) {
                continue;
            }
            ore_grid.query(x, y, 2 * FP, &mut near);
            if near.iter().any(|&i| {
                let (ox, oy, oa) = ores[i as usize];
                let gap = ore_radius(oa) + r + FP / 4;
                dist2(ox - x, oy - y) < gap as i64 * gap as i64
            }) {
                continue;
            }
            ore_grid.insert(x, y, ores.len() as u32);
            ores.push((x, y, amount));
            placed += 1;
        }
    };
    let slot_list = slots.clone();
    for &(sx, sy) in &slot_list {
        let a = rng.range(0, 360);
        let (dx, dy) = dir_vec(a, 8 * FP);
        cluster(&mut rng, &mut ores, sx + dx, sy + dy, 10, 100);
        let (dx, dy) = dir_vec(a + rng.range(100, 260), 15 * FP);
        cluster(&mut rng, &mut ores, sx + dx, sy + dy, 6, 100);
    }
    let fields = (size_units as i64 * size_units as i64 / 2600) as usize;
    for _ in 0..fields {
        let (x, y) = (rng.range(margin / 2, size - margin / 2), rng.range(margin / 2, size - margin / 2));
        // Fields far from every start are richer: worth an expedition.
        let far = slot_list.iter().map(|s| dist2(s.0 - x, s.1 - y)).min().unwrap();
        let rich = if far > (60 * FP) as i64 * (60 * FP) as i64 { 160 } else { 100 };
        let n = rng.range(5, 15) as usize;
        cluster(&mut rng, &mut ores, x, y, n, rich);
    }

    GeneratedMap { size, rocks, ores, slots }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_spread_out() {
        let a = generate(42, WORLD_SIZE);
        let b = generate(42, WORLD_SIZE);
        assert_eq!(a.slots, b.slots);
        assert_eq!(a.ores, b.ores);
        assert_eq!(a.rocks.len(), b.rocks.len());
        assert!(a.rocks.len() > 100, "rocks {}", a.rocks.len());
        assert!(a.ores.len() > 300, "ores {}", a.ores.len());
        // Starts are far apart from each other.
        for (i, s) in a.slots.iter().enumerate().take(6) {
            for t in a.slots.iter().skip(i + 1).take(6) {
                assert!(dist2(s.0 - t.0, s.1 - t.1) > (70 * FP) as i64 * (70 * FP) as i64);
            }
        }
        // ...and each has crystals nearby.
        for s in &a.slots {
            assert!(a.ores.iter().any(|o| dist2(o.0 - s.0, o.1 - s.1) < (12 * FP) as i64 * (12 * FP) as i64));
        }
    }
}
