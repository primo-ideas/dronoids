//! Uniform bucket grid for neighbourhood queries in the continuous world.

#[derive(Clone, Debug, Default)]
pub struct Grid {
    cell: i32,
    n: i32,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    pub fn new(size: i32, cell: i32) -> Grid {
        let n = (size / cell + 1).max(1);
        Grid { cell, n, cells: vec![vec![]; (n * n) as usize] }
    }

    fn coord(&self, v: i32) -> i32 {
        (v / self.cell).clamp(0, self.n - 1)
    }

    fn slot(&self, x: i32, y: i32) -> usize {
        (self.coord(y) * self.n + self.coord(x)) as usize
    }

    pub fn clear(&mut self) {
        for c in &mut self.cells {
            c.clear();
        }
    }

    pub fn insert(&mut self, x: i32, y: i32, id: u32) {
        if self.cells.is_empty() {
            return;
        }
        let s = self.slot(x, y);
        self.cells[s].push(id);
    }

    pub fn remove(&mut self, x: i32, y: i32, id: u32) {
        if self.cells.is_empty() {
            return;
        }
        let s = self.slot(x, y);
        self.cells[s].retain(|v| *v != id);
    }

    /// Every id stored in a cell touching the square [x-r, x+r]².
    pub fn query(&self, x: i32, y: i32, r: i32, out: &mut Vec<u32>) {
        out.clear();
        if self.cells.is_empty() {
            return;
        }
        let (x0, x1) = (self.coord(x - r), self.coord(x + r));
        let (y0, y1) = (self.coord(y - r), self.coord(y + r));
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                out.extend_from_slice(&self.cells[(cy * self.n + cx) as usize]);
            }
        }
    }
}
