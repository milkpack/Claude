//! Deterministic maths: trigonometry from `libm` (the same bits on every platform and on wasm),
//! a SplitMix64-based hash/PRNG and 2D gradient noise. Nothing here reads global state.

use kurbo::{Affine, Point, Vec2};

/// Degrees → radians (a multiplication: exact the same everywhere).
pub fn rad(deg: f64) -> f64 {
    deg * (std::f64::consts::PI / 180.0)
}

pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}
pub fn tan(x: f64) -> f64 {
    libm::tan(x)
}
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}
pub fn sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}
pub fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}
pub fn hypot(v: Vec2) -> f64 {
    libm::hypot(v.x, v.y)
}

/// Rotation by `deg` degrees, counter-clockwise as seen on the page (y points down), like
/// Illustrator's Rotate.
pub fn rotate_deg(deg: f64) -> Affine {
    let t = -rad(deg);
    let (s, c) = (sin(t), cos(t));
    Affine::new([c, s, -s, c, 0.0, 0.0])
}

/// The point at distance `r` from the origin in direction `deg` (counter-clockwise from +x on
/// the page).
pub fn polar(r: f64, deg: f64) -> Point {
    let t = rad(deg);
    Point::new(r * cos(t), -r * sin(t))
}

/// Direction of `v` in page degrees (counter-clockwise from +x).
pub fn angle_deg(v: Vec2) -> f64 {
    atan2(-v.y, v.x) * (180.0 / std::f64::consts::PI)
}

/// `m` applied around `pivot`.
pub fn about(pivot: Point, m: Affine) -> Affine {
    Affine::translate(pivot.to_vec2()) * m * Affine::translate(-pivot.to_vec2())
}

/// Skew by `x`/`y` degrees (horizontal / vertical shear).
pub fn skew_deg(x: f64, y: f64) -> Affine {
    Affine::new([1.0, tan(rad(y)), tan(rad(x)), 1.0, 0.0, 0.0])
}

/// Every coefficient finite.
pub fn affine_finite(a: &Affine) -> bool {
    a.as_coeffs().iter().all(|c| c.is_finite())
}

pub fn point_finite(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

// ---------- hashing / random numbers ----------

/// SplitMix64 finaliser: a strong 64-bit mix.
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Hash of several words (order matters).
pub fn hash(words: &[u64]) -> u64 {
    words.iter().fold(0x2545_F491_4F6C_DD1D, |h, w| mix64(h ^ mix64(*w)))
}

/// A uniform number in [0, 1) from a hash (53 random bits).
pub fn unit(h: u64) -> f64 {
    (h >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// A small SplitMix64 generator: a deterministic stream of numbers.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        unit(self.next_u64())
    }
    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
    /// Uniform in [-1, 1).
    pub fn signed(&mut self) -> f64 {
        self.next_f64() * 2.0 - 1.0
    }
    /// Uniform index below `n` (0 for `n` = 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        // Multiply-shift: no modulo bias worth speaking of, and no division.
        ((self.next_u64() >> 32).wrapping_mul(n as u64) >> 32) as usize
    }
}

// ---------- gradient noise ----------

/// Gradient directions (unit vectors, exact constants so no trigonometry runs).
const GRADS: [(f64, f64); 8] = [
    (1.0, 0.0),
    (std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2),
    (0.0, 1.0),
    (-std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2),
    (-1.0, 0.0),
    (-std::f64::consts::FRAC_1_SQRT_2, -std::f64::consts::FRAC_1_SQRT_2),
    (0.0, -1.0),
    (std::f64::consts::FRAC_1_SQRT_2, -std::f64::consts::FRAC_1_SQRT_2),
];

/// Lattice coordinates are wrapped into this range so huge inputs stay exact integers.
const LATTICE: f64 = 1_048_576.0;

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn grad(seed: u64, ix: i64, iy: i64, dx: f64, dy: f64) -> f64 {
    let h = hash(&[seed, ix as u64, iy as u64]);
    let (gx, gy) = GRADS[(h & 7) as usize];
    gx * dx + gy * dy
}

/// 2D gradient (Perlin-style) noise, about −1..1, 0 at lattice points.
pub fn noise2(seed: u64, x: f64, y: f64) -> f64 {
    if !(x.is_finite() && y.is_finite()) {
        return 0.0;
    }
    let (x, y) = (x.rem_euclid(LATTICE), y.rem_euclid(LATTICE));
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i64, fy as i64);
    let (dx, dy) = (x - fx, y - fy);
    let (u, v) = (fade(dx), fade(dy));
    let n00 = grad(seed, ix, iy, dx, dy);
    let n10 = grad(seed, ix + 1, iy, dx - 1.0, dy);
    let n01 = grad(seed, ix, iy + 1, dx, dy - 1.0);
    let n11 = grad(seed, ix + 1, iy + 1, dx - 1.0, dy - 1.0);
    let a = n00 + u * (n10 - n00);
    let b = n01 + u * (n11 - n01);
    // Scale so the extremes come close to ±1.
    (a + v * (b - a)) * std::f64::consts::SQRT_2
}

/// Fractal (fBm) noise: `octaves` layers, each at twice the frequency and half the amplitude,
/// normalised to about −1..1.
pub fn fbm(seed: u64, x: f64, y: f64, octaves: u32) -> f64 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves.clamp(1, 8) {
        sum += amp * noise2(seed.wrapping_add(o as u64 * 0x9E37), x * freq, y * freq);
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    if norm > 0.0 { sum / norm } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_uniform() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let xs: Vec<f64> = (0..1000).map(|_| a.next_f64()).collect();
        let ys: Vec<f64> = (0..1000).map(|_| b.next_f64()).collect();
        assert_eq!(xs, ys);
        assert!(xs.iter().all(|x| (0.0..1.0).contains(x)));
        let mean = xs.iter().sum::<f64>() / xs.len() as f64;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
        let mut r = Rng::new(3);
        assert!((0..1000).all(|_| r.below(7) < 7));
        assert_eq!(r.below(0), 0);
    }

    #[test]
    fn known_values_pin_the_generator() {
        // SplitMix64's published first output for seed 0.
        assert_eq!(Rng::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(mix64(0), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn noise_is_bounded_smooth_and_seeded() {
        let mut max: f64 = 0.0;
        for i in 0..2000 {
            let (x, y) = (i as f64 * 0.137, i as f64 * 0.071);
            let n = noise2(5, x, y);
            assert!(n.abs() <= 1.0001, "{n}");
            max = max.max(n.abs());
            // Continuity: a tiny step changes little.
            assert!((noise2(5, x + 1e-4, y) - n).abs() < 1e-2);
            assert!(fbm(5, x, y, 4).abs() <= 1.0001);
        }
        assert!(max > 0.3, "noise too flat: {max}");
        assert_eq!(noise2(1, 3.0, 4.0), 0.0);
        assert_ne!(noise2(1, 3.3, 4.6), noise2(2, 3.3, 4.6));
        assert_eq!(noise2(1, f64::NAN, 0.0), 0.0);
        assert!(noise2(1, 1e300, -1e300).is_finite());
    }

    #[test]
    fn trig_conventions() {
        let p = rotate_deg(90.0) * Point::new(1.0, 0.0);
        // Counter-clockwise on a y-down page: +x turns to −y.
        assert!((p.x).abs() < 1e-12 && (p.y + 1.0).abs() < 1e-12, "{p:?}");
        let q = polar(2.0, 90.0);
        assert!(q.x.abs() < 1e-12 && (q.y + 2.0).abs() < 1e-12);
        assert!((angle_deg(Vec2::new(0.0, -1.0)) - 90.0).abs() < 1e-12);
        let a = about(Point::new(5.0, 5.0), rotate_deg(180.0));
        let r = a * Point::new(6.0, 5.0);
        assert!((r.x - 4.0).abs() < 1e-12 && (r.y - 5.0).abs() < 1e-12);
    }
}
