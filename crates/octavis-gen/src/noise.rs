//! Seeded gradient (Perlin) noise.
//!
//! Hand-rolled so results are identical on every machine: only IEEE
//! add/sub/mul/div/floor and integer hashing are used (no libm functions
//! such as `sin`, whose last digits can differ between platforms), and the
//! gradient table is literal. The same seed therefore always gives the same
//! terrain, which is what makes seeds shareable.

const S: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Eight unit gradients (axes and diagonals), as literals.
const GRADIENTS: [(f64, f64); 8] =
    [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (S, S), (-S, S), (S, -S), (-S, -S)];

/// 2D Perlin noise with a seed-shuffled permutation table.
#[derive(Clone)]
pub struct Perlin2 {
    perm: [u8; 512],
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

impl Perlin2 {
    pub fn new(seed: u32) -> Self {
        let mut table: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut state = seed as u64 ^ 0xD1B5_4A32_D192_ED03;
        for i in (1..256usize).rev() {
            let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
            table.swap(i, j);
        }
        let mut perm = [0u8; 512];
        for i in 0..512 {
            perm[i] = table[i & 255];
        }
        Self { perm }
    }

    fn gradient(&self, xi: i64, yi: i64, dx: f64, dy: f64) -> f64 {
        let x = xi.rem_euclid(256) as usize;
        let y = yi.rem_euclid(256) as usize;
        let h = self.perm[self.perm[x] as usize + y] as usize;
        let (gx, gy) = GRADIENTS[h & 7];
        gx * dx + gy * dy
    }

    /// Noise in `[-1, 1]`; zero at integer lattice points, smooth between.
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        let (fx, fy) = (x.floor(), y.floor());
        let (xi, yi) = (fx as i64, fy as i64);
        let (dx, dy) = (x - fx, y - fy);
        let (u, v) = (fade(dx), fade(dy));
        let n00 = self.gradient(xi, yi, dx, dy);
        let n10 = self.gradient(xi + 1, yi, dx - 1.0, dy);
        let n01 = self.gradient(xi, yi + 1, dx, dy - 1.0);
        let n11 = self.gradient(xi + 1, yi + 1, dx - 1.0, dy - 1.0);
        let top = n00 + u * (n10 - n00);
        let bottom = n01 + u * (n11 - n01);
        // Unit gradients bound the raw value by sqrt(2)/2; rescale to ±1.
        ((top + v * (bottom - top)) * std::f64::consts::SQRT_2).clamp(-1.0, 1.0)
    }

    /// Fractal Brownian motion: `octaves` layers of noise, each at twice
    /// the frequency and `persistence` times the weight. Result in `[-1, 1]`.
    pub fn fbm(&self, x: f64, y: f64, octaves: u32, persistence: f64) -> f64 {
        let (mut sum, mut norm) = (0.0, 0.0);
        let (mut freq, mut amp) = (1.0, 1.0);
        for _ in 0..octaves.max(1) {
            sum += self.sample(x * freq, y * freq) * amp;
            norm += amp;
            freq *= 2.0;
            amp *= persistence;
        }
        sum / norm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_on_the_lattice_and_bounded_everywhere() {
        let n = Perlin2::new(7);
        for i in -5..5 {
            for j in -5..5 {
                assert_eq!(n.sample(i as f64, j as f64), 0.0);
            }
        }
        let mut max = 0.0f64;
        for i in 0..400 {
            for j in 0..400 {
                let v = n.sample(i as f64 * 0.137 - 20.0, j as f64 * 0.211 + 3.0);
                assert!((-1.0..=1.0).contains(&v));
                max = max.max(v.abs());
            }
        }
        assert!(max > 0.5, "noise should use most of its range, got {max}");
    }

    #[test]
    fn smooth_between_neighbouring_samples() {
        let n = Perlin2::new(1);
        for i in 0..2000 {
            let x = i as f64 * 0.01;
            assert!((n.sample(x, 0.37) - n.sample(x + 0.01, 0.37)).abs() < 0.1);
        }
    }

    #[test]
    fn seed_decides_everything_and_is_stable() {
        let (a, b, a2) = (Perlin2::new(1), Perlin2::new(2), Perlin2::new(1));
        let pts: Vec<(f64, f64)> = (0..50).map(|i| (i as f64 * 0.73 + 0.1, i as f64 * 0.41 - 3.3)).collect();
        assert!(pts.iter().all(|&(x, y)| a.sample(x, y) == a2.sample(x, y)));
        assert!(pts.iter().any(|&(x, y)| a.sample(x, y) != b.sample(x, y)));
        // Negative coordinates work and the table wraps without panicking.
        assert!(a.sample(-1234.5, -9876.25).abs() <= 1.0);
        assert!(a.sample(1e6 + 0.5, -1e6 + 0.5).abs() <= 1.0);
    }

    #[test]
    fn golden_values_guard_cross_platform_stability() {
        let n = Perlin2::new(1234);
        let got: Vec<i64> =
            [(0.3, 0.6), (3.7, -2.2), (-100.25, 55.5), (12.0, 7.9)].iter().map(|&(x, y)| (n.sample(x, y) * 1e9).round() as i64).collect();
        assert_eq!(got, vec![-92_519_578, -182_191_853, 653_136_731, -10_895_101]);
    }

    #[test]
    fn fbm_stays_in_range_and_adds_detail() {
        let n = Perlin2::new(3);
        let rough = |oct: u32| {
            (0..1000)
                .map(|i| {
                    let x = i as f64 * 0.05;
                    (n.fbm(x, 1.5, oct, 0.5) - n.fbm(x + 0.05, 1.5, oct, 0.5)).abs()
                })
                .sum::<f64>()
        };
        for i in 0..500 {
            assert!(n.fbm(i as f64 * 0.31, 2.2, 6, 0.5).abs() <= 1.0);
        }
        assert!(rough(6) > rough(1));
    }
}
