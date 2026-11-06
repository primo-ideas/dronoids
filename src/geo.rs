//! Deterministic integer geometry: compass angles in whole degrees
//! (0 = north, 90 = east, clockwise; y grows southwards), a sine table,
//! integer atan2 and square roots. No floats, so every machine agrees.

/// sin(d) * 65536 for d in 0..=90.
const SIN: [i32; 91] = [
    0, 1144, 2287, 3430, 4572, 5712, 6850, 7987, 9121, 10252, 11380, 12505, 13626, 14742, 15855, 16962, 18064,
    19161, 20252, 21336, 22415, 23486, 24550, 25607, 26656, 27697, 28729, 29753, 30767, 31772, 32768, 33754,
    34729, 35693, 36647, 37590, 38521, 39441, 40348, 41243, 42126, 42995, 43852, 44695, 45525, 46341, 47143,
    47930, 48703, 49461, 50203, 50931, 51643, 52339, 53020, 53684, 54332, 54963, 55578, 56175, 56756, 57319,
    57865, 58393, 58903, 59396, 59870, 60326, 60764, 61183, 61584, 61966, 62328, 62672, 62997, 63303, 63589,
    63856, 64104, 64332, 64540, 64729, 64898, 65048, 65177, 65287, 65376, 65446, 65496, 65526, 65536,
];

pub fn norm_deg(d: i32) -> i32 {
    d.rem_euclid(360)
}

/// sin(deg) * 65536.
pub fn sin(deg: i32) -> i32 {
    let d = norm_deg(deg);
    match d {
        0..=90 => SIN[d as usize],
        91..=180 => SIN[(180 - d) as usize],
        181..=270 => -SIN[(d - 180) as usize],
        _ => -SIN[(360 - d) as usize],
    }
}

/// cos(deg) * 65536.
pub fn cos(deg: i32) -> i32 {
    sin(deg + 90)
}

/// Offset of length `len` towards compass angle `deg`.
pub fn dir_vec(deg: i32, len: i32) -> (i32, i32) {
    let dx = (len as i64 * sin(deg) as i64) / 65536;
    let dy = -(len as i64 * cos(deg) as i64) / 65536;
    (dx as i32, dy as i32)
}

/// Compass angle (0..360) of the offset (dx, dy), or -1 for (0, 0).
pub fn compass(dx: i32, dy: i32) -> i32 {
    if dx == 0 && dy == 0 {
        return -1;
    }
    let east = dx as i64;
    let north = -(dy as i64);
    let (ax, ay) = (east.abs(), north.abs());
    // f(d) = ax*cos(d) - ay*sin(d) decreases from ax (d=0) to -ay (d=90);
    // its root is the angle measured from north towards east.
    let f = |d: i32| ax * SIN[(90 - d) as usize] as i64 - ay * SIN[d as usize] as i64;
    let (mut lo, mut hi) = (0, 90);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if f(mid) <= 0 {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let mut base = lo;
    if base > 0 && f(base - 1).abs() < f(base).abs() {
        base -= 1;
    }
    let a = match (east >= 0, north >= 0) {
        (true, true) => base,
        (true, false) => 180 - base,
        (false, false) => 180 + base,
        (false, true) => 360 - base,
    };
    norm_deg(a)
}

pub fn dist2(dx: i32, dy: i32) -> i64 {
    dx as i64 * dx as i64 + dy as i64 * dy as i64
}

pub fn dist(dx: i32, dy: i32) -> i32 {
    (dist2(dx, dy) as u64).isqrt() as i32
}

/// Smallest signed difference `to - from` in degrees (-180..180).
pub fn angle_diff(from: i32, to: i32) -> i32 {
    let d = norm_deg(to - from);
    if d > 180 { d - 360 } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compass_points() {
        assert_eq!(compass(0, -10), 0);
        assert_eq!(compass(10, 0), 90);
        assert_eq!(compass(0, 10), 180);
        assert_eq!(compass(-10, 0), 270);
        assert_eq!(compass(10, -10), 45);
        assert_eq!(compass(-10, 10), 225);
        assert_eq!(compass(0, 0), -1);
    }

    #[test]
    fn compass_inverts_dir_vec() {
        for a in 0..360 {
            let (dx, dy) = dir_vec(a, 100_000);
            assert_eq!(compass(dx, dy), a, "angle {a} -> ({dx},{dy})");
        }
    }

    #[test]
    fn trig_identities() {
        for a in -720..720 {
            let s = sin(a) as i64;
            let c = cos(a) as i64;
            let one = s * s + c * c;
            assert!((one - 65536 * 65536).abs() < 65536 * 4, "angle {a}");
        }
        assert_eq!(dist(3000, 4000), 5000);
        assert_eq!(angle_diff(350, 10), 20);
        assert_eq!(angle_diff(10, 350), -20);
    }
}
