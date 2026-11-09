//! What a player has seen of a world without bounds: one bit per
//! `FOG_CELL` square, kept by pages so that only visited places take room.

use serde::{Deserialize, Serialize};

/// Cells per side of a page.
const PAGE: i32 = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Page {
    x: i32,
    y: i32,
    rows: [u32; PAGE as usize],
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FogMap {
    /// Sorted by (y, x).
    pages: Vec<Page>,
}

fn split(v: i32) -> (i32, usize) {
    (v.div_euclid(PAGE), v.rem_euclid(PAGE) as usize)
}

impl FogMap {
    fn find(&self, x: i32, y: i32) -> Result<usize, usize> {
        self.pages.binary_search_by_key(&(y, x), |p| (p.y, p.x))
    }

    pub fn get(&self, cx: i32, cy: i32) -> bool {
        let ((px, bit), (py, row)) = (split(cx), split(cy));
        self.find(px, py).is_ok_and(|i| self.pages[i].rows[row] & (1 << bit) != 0)
    }

    pub fn set(&mut self, cx: i32, cy: i32) {
        let ((px, bit), (py, row)) = (split(cx), split(cy));
        let i = self.find(px, py).unwrap_or_else(|i| {
            self.pages.insert(i, Page { x: px, y: py, rows: [0; PAGE as usize] });
            i
        });
        self.pages[i].rows[row] |= 1 << bit;
    }

    /// How many cells have been seen.
    pub fn count(&self) -> i64 {
        self.pages.iter().flat_map(|p| p.rows).map(|r| r.count_ones() as i64).sum()
    }

    /// A copy limited to the pages touching these cells.
    pub fn around(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> FogMap {
        let (a, b) = (split(x0).0..=split(x1).0, split(y0).0..=split(y1).0);
        FogMap { pages: self.pages.iter().filter(|p| a.contains(&p.x) && b.contains(&p.y)).cloned().collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_remembered_anywhere() {
        let mut m = FogMap::default();
        let spots = [(0, 0), (-1, -1), (31, 32), (-33, 7), (120_000, -90_000), (5, 5)];
        for (x, y) in spots {
            assert!(!m.get(x, y));
            m.set(x, y);
            m.set(x, y);
            assert!(m.get(x, y));
        }
        assert_eq!(m.count(), spots.len() as i64);
        assert!(!m.get(1, 0) && !m.get(-2, -1));
        assert!(m.pages.windows(2).all(|w| (w[0].y, w[0].x) < (w[1].y, w[1].x)));
    }

    #[test]
    fn a_local_copy_keeps_what_is_around() {
        let mut m = FogMap::default();
        m.set(10, 10);
        m.set(5000, 5000);
        let near = m.around(0, 0, 40, 40);
        assert!(near.get(10, 10) && !near.get(5000, 5000));
    }
}
