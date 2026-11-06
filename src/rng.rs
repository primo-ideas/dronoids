//! Tiny deterministic RNG (splitmix64). The simulation must never use a
//! platform RNG: lockstep multiplayer relies on bit-identical runs.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Rng { state: seed ^ 0x2545_F491_4F6C_DD1D };
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        splitmix(&mut self.state)
    }

    /// Uniform in `0..n` (0 when `n == 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as u32
        }
    }

    /// Uniform in `lo..hi` (returns `lo` when the range is empty).
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            lo
        } else {
            lo + self.below((hi - lo) as u32) as i32
        }
    }

    /// True with probability `num / den`.
    pub fn chance(&mut self, num: u32, den: u32) -> bool {
        self.below(den) < num
    }
}

pub fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Mix two values into a well distributed seed.
pub fn mix(a: u64, b: u64) -> u64 {
    let mut s = a ^ b.rotate_left(32) ^ 0xD6E8_FEB8_6659_FD93;
    splitmix(&mut s)
}

/// FNV-1a hasher used for desync detection.
#[derive(Clone, Copy)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn add(&mut self, v: i64) {
        for b in v.to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}
