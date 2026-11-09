//! Sparse bucket grid for neighbourhood queries in a world without bounds.

use std::collections::HashMap;

use crate::rules::FP;

#[derive(Clone, Debug)]
pub struct Grid {
    cell: i32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl Default for Grid {
    fn default() -> Grid {
        Grid::new(8 * FP)
    }
}

impl Grid {
    pub fn new(cell: i32) -> Grid {
        Grid { cell, cells: HashMap::new() }
    }

    fn at(&self, x: i32, y: i32) -> (i32, i32) {
        (x.div_euclid(self.cell), y.div_euclid(self.cell))
    }

    pub fn clear(&mut self) {
        self.cells.clear();
    }

    pub fn insert(&mut self, x: i32, y: i32, id: u32) {
        let at = self.at(x, y);
        self.cells.entry(at).or_default().push(id);
    }

    pub fn remove(&mut self, x: i32, y: i32, id: u32) {
        let at = self.at(x, y);
        if let Some(c) = self.cells.get_mut(&at) {
            c.retain(|v| *v != id);
            if c.is_empty() {
                self.cells.remove(&at);
            }
        }
    }

    /// Every id stored in a cell touching the rectangle, cell by cell (so
    /// always in the same order: nothing iterates over the hash map).
    pub fn query_rect(&self, x0: i32, y0: i32, x1: i32, y1: i32, out: &mut Vec<u32>) {
        out.clear();
        if self.cells.is_empty() {
            return;
        }
        let ((cx0, cy0), (cx1, cy1)) = (self.at(x0, y0), self.at(x1, y1));
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                if let Some(c) = self.cells.get(&(cx, cy)) {
                    out.extend_from_slice(c);
                }
            }
        }
    }

    /// Every id stored in a cell touching the square [x-r, x+r]².
    pub fn query(&self, x: i32, y: i32, r: i32, out: &mut Vec<u32>) {
        self.query_rect(x - r, y - r, x + r, y + r, out);
    }
}
