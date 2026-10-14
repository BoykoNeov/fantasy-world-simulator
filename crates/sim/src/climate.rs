//! Climate, two versions:
//!
//! - **latitude**: annual mean temperature from latitude and elevation, annual rainfall from
//!   latitude bands and distance to the sea. No wind, no rain shadow, no seasons in the fields.
//! - **null**: the same mild climate everywhere.

use crate::grid::{Grid, NONE};
use crate::module::{Manifest, Module as SimModule, Slot};
use crate::rng::{draw, purpose, Module};
use crate::terrain::fbm;
use crate::world::World;
use rayon::prelude::*;
use std::collections::VecDeque;

pub struct LatitudeClimate;
pub struct NullClimate;

const OWNS: &[&str] = &["cell.temperature", "cell.rainfall"];
static LATITUDE: Manifest = Manifest::generation(Slot::Climate, "latitude", &["cell.elevation"], OWNS);
static NULL: Manifest = Manifest::generation(Slot::Climate, "null", &[], OWNS);

/// The null climate: a temperate place with enough rain to farm.
pub const NULL_TEMPERATURE: f32 = 15.0;
pub const NULL_RAINFALL: f32 = 800.0;

impl SimModule for LatitudeClimate {
    fn manifest(&self) -> &'static Manifest {
        &LATITUDE
    }
    fn generate(&self, w: &mut World) {
        let c = generate(&w.grid, &w.elevation, w.params.seed, w.params.radius_km);
        w.temperature = c.temperature;
        w.rainfall = c.rainfall;
    }
}

impl SimModule for NullClimate {
    fn manifest(&self) -> &'static Manifest {
        &NULL
    }
    fn generate(&self, w: &mut World) {
        w.temperature = vec![NULL_TEMPERATURE; w.grid.cells()];
        w.rainfall = vec![NULL_RAINFALL; w.grid.cells()];
    }
}

pub struct Climate {
    /// Annual mean, °C.
    pub temperature: Vec<f32>,
    /// Annual total, mm.
    pub rainfall: Vec<f32>,
    /// Distance to the nearest sea cell, km (0 for sea).
    pub coast_distance_km: Vec<f32>,
}

/// Lapse rate: degrees lost per metre climbed.
const LAPSE_PER_M: f32 = 0.0065;

/// Sea-level temperature by latitude (degrees, absolute).
fn sea_level_temperature(abs_lat_deg: f64) -> f64 {
    27.0 - 50.0 * (abs_lat_deg / 90.0).powf(1.6)
}

/// Rainfall by latitude for a coastal place: wet tropics, dry subtropics, wet mid-latitudes, dry
/// poles.
fn band_rainfall(abs_lat_deg: f64) -> f64 {
    let g = |centre: f64, width: f64| (-((abs_lat_deg - centre) / width).powi(2)).exp();
    200.0 + 1900.0 * g(0.0, 13.0) + 850.0 * g(52.0, 14.0)
}

pub fn generate(grid: &Grid, elevation: &[f32], seed: u64, radius_km: f64) -> Climate {
    let cell_km = (grid.mean_area()).sqrt() * radius_km;
    let coast_distance_km: Vec<f32> =
        sea_distance_cells(grid, elevation).into_iter().map(|d| (d as f64 * cell_km) as f32).collect();

    let t_key = draw(seed, Module::Climate, 0, 0, purpose("temperature noise"));
    let r_key = draw(seed, Module::Climate, 0, 0, purpose("rainfall noise"));
    let (temperature, rainfall): (Vec<f32>, Vec<f32>) = (0..grid.cells())
        .into_par_iter()
        .map(|c| {
            let d = grid.center[c];
            let lat = (grid.lat[c] as f64).to_degrees().abs();
            let t_noise = fbm([d[0] * 3.0, d[1] * 3.0, d[2] * 3.0], 3, t_key) * 3.0;
            let t = sea_level_temperature(lat) + t_noise - (elevation[c].max(0.0) * LAPSE_PER_M) as f64;
            let continental = 0.3 + 0.7 * (-(coast_distance_km[c] as f64) / 1400.0).exp();
            let r_noise = 1.0 + 0.35 * fbm([d[0] * 4.0, d[1] * 4.0, d[2] * 4.0], 4, r_key);
            let r = band_rainfall(lat) * continental * r_noise;
            (t as f32, r.max(0.0) as f32)
        })
        .unzip();
    Climate { temperature, rainfall, coast_distance_km }
}

/// Breadth-first distance, in steps over edge neighbours, from every cell to the nearest sea cell.
/// A planet with no sea gets `u32::MAX / 2` everywhere.
fn sea_distance_cells(grid: &Grid, elevation: &[f32]) -> Vec<u32> {
    let far = u32::MAX / 2;
    let mut dist = vec![far; grid.cells()];
    let mut queue = VecDeque::new();
    for (c, &h) in elevation.iter().enumerate() {
        if h < 0.0 {
            dist[c] = 0;
            queue.push_back(c);
        }
    }
    while let Some(c) = queue.pop_front() {
        for &m in &grid.neighbors[c] {
            debug_assert_ne!(m, NONE);
            let m = m as usize;
            if dist[m] == far {
                dist[m] = dist[c] + 1;
                queue.push_back(m);
            }
        }
    }
    dist
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poles_are_colder_and_mountains_colder_still() {
        assert!(sea_level_temperature(0.0) > 20.0);
        assert!(sea_level_temperature(90.0) < -15.0);
        let g = Grid::new(20);
        let flat = vec![0.0f32; g.cells()];
        let high = vec![3000.0f32; g.cells()];
        let a = generate(&g, &flat, 1, 6371.0);
        let b = generate(&g, &high, 1, 6371.0);
        for c in 0..g.cells() {
            assert!((a.temperature[c] - b.temperature[c] - 19.5).abs() < 1e-3);
        }
    }

    #[test]
    fn interiors_are_drier_than_coasts() {
        let g = Grid::new(32);
        // One ocean cell: everything else is a continent around it.
        let mut elev = vec![100.0f32; g.cells()];
        let sea = g.dir_to_cell([0.0, 0.0, 1.0]);
        elev[sea] = -100.0;
        let c = generate(&g, &elev, 1, 6371.0);
        assert_eq!(c.coast_distance_km[sea], 0.0);
        let far = g.dir_to_cell([0.0, 0.0, -1.0]);
        assert!(c.coast_distance_km[far] > 15_000.0);
        // Same latitude, near versus far from the sea; noise is far smaller than this factor.
        assert!(c.rainfall[far] < c.rainfall[sea] * 0.7);
    }
}
