//! Terrain v0: noise elevation. Domain-warped continents, detail, and ridged mountain chains, then a
//! sea level chosen so a set fraction of the planet is land.

use crate::grid::{Grid, V3};
use crate::rng::{draw, mix, purpose, Module};
use rayon::prelude::*;

/// Integer lattice value in [-1, 1].
fn lattice(x: i64, y: i64, z: i64, key: u64) -> f64 {
    let h = mix(key ^ mix(x as u64 ^ mix(y as u64 ^ mix(z as u64))));
    (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// Trilinear value noise with smoothstep, in [-1, 1].
fn value_noise(p: V3, key: u64) -> f64 {
    let f = [p[0].floor(), p[1].floor(), p[2].floor()];
    let t = [p[0] - f[0], p[1] - f[1], p[2] - f[2]];
    let s = t.map(|v| v * v * (3.0 - 2.0 * v));
    let (x, y, z) = (f[0] as i64, f[1] as i64, f[2] as i64);
    let mut acc = 0.0;
    for dz in 0..2 {
        for dy in 0..2 {
            for dx in 0..2 {
                let w = (if dx == 1 { s[0] } else { 1.0 - s[0] })
                    * (if dy == 1 { s[1] } else { 1.0 - s[1] })
                    * (if dz == 1 { s[2] } else { 1.0 - s[2] });
                acc += w * lattice(x + dx, y + dy, z + dz, key);
            }
        }
    }
    acc
}

/// Fractal noise: `octaves` layers of value noise, each at about twice the frequency and half the
/// amplitude of the last. Each layer has its own key, derived from `key`.
pub fn fbm(p: V3, octaves: u32, key: u64) -> f64 {
    let (mut amp, mut freq, mut sum, mut norm) = (1.0, 1.0, 0.0, 0.0);
    for o in 0..octaves {
        sum += amp * value_noise([p[0] * freq, p[1] * freq, p[2] * freq], mix(key ^ o as u64));
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm
}

fn scale(d: V3, s: f64) -> V3 {
    [d[0] * s, d[1] * s, d[2] * s]
}

/// Raw height before sea level: continents, warped, plus detail and ridges.
fn raw_height(d: V3, seed: u64) -> f64 {
    let key = |name: &str| draw(seed, Module::Terrain, 0, 0, purpose(name));
    let d2 = scale(d, 2.0);
    let warp = [
        fbm([d2[0] + 5.2, d2[1], d2[2]], 3, key("warp x")),
        fbm([d2[0], d2[1] + 1.3, d2[2]], 3, key("warp y")),
        fbm([d2[0], d2[1], d2[2] + 7.1], 3, key("warp z")),
    ];
    let q = [d[0] * 1.7 + warp[0] * 0.6, d[1] * 1.7 + warp[1] * 0.6, d[2] * 1.7 + warp[2] * 0.6];
    let continents = fbm(q, 4, key("continents"));
    let detail = fbm(scale(d, 9.0), 5, key("detail"));
    let ridge = 1.0 - fbm(scale(d, 4.0), 4, key("ridges")).abs() * 2.0;
    continents + 0.25 * detail + 0.12 * ridge.max(0.0).powi(3)
}

/// Elevation in metres per cell; below 0 is sea. `land_fraction` of cells end up at or above 0.
pub fn generate(grid: &Grid, seed: u64, land_fraction: f64) -> Vec<f32> {
    let cells = grid.cells();
    let raw: Vec<f64> = grid.center.par_iter().map(|&d| raw_height(d, seed)).collect();

    let mut sorted = raw.clone();
    sorted.par_sort_unstable_by(|a, b| a.total_cmp(b));
    let sea_rank = (((1.0 - land_fraction) * cells as f64) as usize).min(cells - 1);
    let sea = sorted[sea_rank];
    let (lo, hi) = (sorted[0], sorted[cells - 1]);
    raw.par_iter()
        .map(|&h| {
            if h >= sea {
                (((h - sea) / (hi - sea).max(1e-12)).powf(1.4) * 5200.0) as f32
            } else {
                (-((sea - h) / (sea - lo).max(1e-12)).powf(0.8) * 5500.0 - 1.0) as f32
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn land_fraction_is_respected() {
        let g = Grid::new(24);
        let e = generate(&g, 3, 0.4);
        let land = e.iter().filter(|&&h| h >= 0.0).count() as f64 / g.cells() as f64;
        assert!((land - 0.4).abs() < 0.01, "{land}");
        assert!(e.iter().all(|h| h.is_finite()));
    }

    #[test]
    fn seeds_give_different_worlds() {
        let g = Grid::new(16);
        assert_ne!(generate(&g, 1, 0.4), generate(&g, 2, 0.4));
        assert_eq!(generate(&g, 1, 0.4), generate(&g, 1, 0.4));
    }
}
