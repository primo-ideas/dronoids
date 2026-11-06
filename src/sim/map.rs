//! Seeded, 180°-symmetric map generation (fair for two players).

use super::Tile;
use crate::rng::Rng;
use crate::rules::{ORE_MAX, ORE_MIN};

pub struct GeneratedMap {
    pub w: i32,
    pub h: i32,
    pub tiles: Vec<Tile>,
    /// Start positions; `starts[1]` is the mirror image of `starts[0]`.
    pub starts: Vec<(i32, i32)>,
}

pub fn generate(seed: u64, w: i32, h: i32) -> GeneratedMap {
    let mut rng = Rng::new(seed);
    let n = (w * h) as usize;
    let idx = |x: i32, y: i32| (y * w + x) as usize;
    let mirror = |i: usize| n - 1 - i;
    let inside = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h;

    // 1. Noise, mirrored.
    let mut wall = vec![false; n];
    for i in 0..n {
        if i <= mirror(i) {
            let v = rng.chance(41, 100);
            wall[i] = v;
            wall[mirror(i)] = v;
        }
    }

    // 2. Cellular automaton smoothing (symmetric rule keeps the symmetry).
    for pass in 0..5 {
        let mut next = wall.clone();
        for y in 0..h {
            for x in 0..w {
                let mut c = 0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let (nx, ny) = (x + dx, y + dy);
                        if !inside(nx, ny) || wall[idx(nx, ny)] {
                            c += 1;
                        }
                    }
                }
                let cur = wall[idx(x, y)];
                next[idx(x, y)] = if pass < 3 { c >= 5 || (cur && c >= 4) } else { c >= 5 };
            }
        }
        wall = next;
    }

    // 3. Solid border.
    for x in 0..w {
        wall[idx(x, 0)] = true;
        wall[idx(x, h - 1)] = true;
    }
    for y in 0..h {
        wall[idx(0, y)] = true;
        wall[idx(w - 1, y)] = true;
    }

    // 4. Start areas.
    let s0 = (5, h - 6);
    let s1 = (w - 1 - s0.0, h - 1 - s0.1);
    for (sx, sy) in [s0, s1] {
        for dy in -4..=4 {
            for dx in -4..=4 {
                let (x, y) = (sx + dx, sy + dy);
                if dx * dx + dy * dy <= 18 && x > 0 && y > 0 && x < w - 1 && y < h - 1 {
                    wall[idx(x, y)] = false;
                }
            }
        }
    }

    // 5. Make sure both starts are connected, then seal unreachable pockets.
    let flood = |wall: &Vec<bool>| -> Vec<bool> {
        let mut seen = vec![false; n];
        let mut stack = vec![s0];
        seen[idx(s0.0, s0.1)] = true;
        while let Some((x, y)) = stack.pop() {
            for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                let (nx, ny) = (x + dx, y + dy);
                if inside(nx, ny) && !wall[idx(nx, ny)] && !seen[idx(nx, ny)] {
                    seen[idx(nx, ny)] = true;
                    stack.push((nx, ny));
                }
            }
        }
        seen
    };
    let mut reach = flood(&wall);
    if !reach[idx(s1.0, s1.1)] {
        // Carve a wobbly corridor towards the other start (and its mirror).
        let (mut x, mut y) = s0;
        while (x, y) != s1 {
            let (dx, dy) = (s1.0 - x, s1.1 - y);
            if dx != 0 && (dy == 0 || rng.chance(dx.unsigned_abs(), (dx.abs() + dy.abs()) as u32)) {
                x += dx.signum();
            } else {
                y += dy.signum();
            }
            for (cx, cy) in [(x, y), (x + 1, y)] {
                if cx > 0 && cx < w - 1 {
                    let i = idx(cx, cy);
                    wall[i] = false;
                    wall[mirror(i)] = false;
                }
            }
        }
        reach = flood(&wall);
    }
    for i in 0..n {
        if !reach[i] {
            wall[i] = true;
        }
    }

    let mut tiles: Vec<Tile> = wall.iter().map(|&b| if b { Tile::Wall } else { Tile::Floor }).collect();

    // 6. Ore fields.
    let dist2 = |a: (i32, i32), b: (i32, i32)| (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2);
    let grow = |tiles: &mut Vec<Tile>, rng: &mut Rng, cx: i32, cy: i32, size: usize, rich: bool| {
        // Start from the closest floor tile to the requested centre.
        let mut seed_at = None;
        'search: for r in 0..8 {
            for dy in -r..=r {
                for dx in -r..=r {
                    let (x, y) = (cx + dx, cy + dy);
                    if (dx.abs() == r || dy.abs() == r)
                        && inside(x, y)
                        && tiles[idx(x, y)] == Tile::Floor
                        && dist2((x, y), s0) >= 12
                        && dist2((x, y), s1) >= 12
                    {
                        seed_at = Some((x, y));
                        break 'search;
                    }
                }
            }
        }
        let Some(start) = seed_at else { return };
        let mut frontier = vec![start];
        let mut placed = 0;
        let mut guard = 0;
        while placed < size && !frontier.is_empty() && guard < 400 {
            guard += 1;
            let k = rng.below(frontier.len() as u32) as usize;
            let (x, y) = frontier.swap_remove(k);
            if !inside(x, y) {
                continue;
            }
            let i = idx(x, y);
            if tiles[i] != Tile::Floor || dist2((x, y), s0) < 12 || dist2((x, y), s1) < 12 {
                continue;
            }
            let base = rng.range(ORE_MIN as i32, ORE_MAX as i32 + 1) as u16;
            let amount = if rich { base + ORE_MAX / 2 } else { base };
            tiles[i] = Tile::Ore(amount);
            tiles[mirror(i)] = Tile::Ore(amount);
            placed += 1;
            for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                frontier.push((x + dx, y + dy));
            }
        }
    };

    // A guaranteed field near each base, offset towards the centre.
    grow(&mut tiles, &mut rng, s0.0 + 6, s0.1 - 3, 10, false);
    grow(&mut tiles, &mut rng, s0.0 + 2, s0.1 - 7, 6, false);
    // A rich contested field in the middle.
    grow(&mut tiles, &mut rng, w / 2, h / 2, 14, true);
    // Scattered fields.
    let mut fields = 0;
    let mut tries = 0;
    while fields < 9 && tries < 500 {
        tries += 1;
        let x = rng.range(2, w - 2);
        let y = rng.range(2, h - 2);
        let i = idx(x, y);
        if i > mirror(i) || tiles[i] != Tile::Floor {
            continue;
        }
        if dist2((x, y), s0) < 120 || dist2((x, y), s1) < 120 {
            continue;
        }
        let size = rng.range(5, 13) as usize;
        grow(&mut tiles, &mut rng, x, y, size, false);
        fields += 1;
    }

    GeneratedMap { w, h, tiles, starts: vec![s0, s1] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{MAP_H, MAP_W};

    #[test]
    fn symmetric_and_deterministic() {
        let a = generate(42, MAP_W, MAP_H);
        let b = generate(42, MAP_W, MAP_H);
        assert_eq!(a.tiles, b.tiles);
        let n = a.tiles.len();
        for i in 0..n {
            assert_eq!(a.tiles[i], a.tiles[n - 1 - i]);
        }
        let (sx, sy) = a.starts[0];
        assert_eq!(a.tiles[(sy * a.w + sx) as usize], Tile::Floor);
    }

    #[test]
    #[ignore]
    fn print_map() {
        for seed in [1u64, 2, 3] {
            let m = generate(seed, MAP_W, MAP_H);
            for y in 0..m.h {
                let row: String = (0..m.w)
                    .map(|x| {
                        if m.starts.contains(&(x, y)) {
                            return '@';
                        }
                        match m.tiles[(y * m.w + x) as usize] {
                            Tile::Floor => '.',
                            Tile::Wall => '#',
                            Tile::Ore(_) => '*',
                        }
                    })
                    .collect();
                println!("{row}");
            }
            println!();
        }
    }
}
