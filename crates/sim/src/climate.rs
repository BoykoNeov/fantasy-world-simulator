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
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub struct LatitudeClimate {
    pub params: ClimateParams,
}
pub struct NullClimate {
    pub params: ClimateParams,
}

/// The climate's numbers: temperatures by latitude and height, rain by latitude band and distance
/// from the sea, and the null version's one climate everywhere.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClimateParams {
    /// Yearly mean at sea level on the equator, °C.
    pub equator_c: f64,
    /// How much colder the poles are than the equator, °C.
    pub pole_drop_c: f64,
    /// The cold rises toward the poles as (latitude / 90°) to this power: above 1, warm lands
    /// reach far from the equator and the cold gathers near the poles.
    pub cooling_curve: f64,
    /// Degrees lost per kilometre climbed.
    pub lapse_c_per_km: f64,
    /// Warm and cold patches, up to this many degrees.
    pub temperature_noise_c: f64,
    /// Rain a year everywhere on a coast, mm.
    pub base_rain_mm: f64,
    /// Extra rain on the equator, mm, fading over `tropical_width_deg` degrees of latitude.
    pub tropical_rain_mm: f64,
    pub tropical_width_deg: f64,
    /// Extra rain in the mid-latitudes' belt of storms, mm, centred on `storm_lat_deg` and fading
    /// over `storm_width_deg`.
    pub storm_rain_mm: f64,
    pub storm_lat_deg: f64,
    pub storm_width_deg: f64,
    /// Rain far inland, as a share of a coast's, and the distance over which it falls to that, km.
    pub inland_rain: f64,
    pub inland_km: f64,
    /// Wet and dry patches: rain is up to this share more or less.
    pub rain_noise: f64,
    /// The null version's temperature (°C) and rain (mm) everywhere.
    pub null_temperature_c: f64,
    pub null_rain_mm: f64,
}

impl Default for ClimateParams {
    fn default() -> Self {
        ClimateParams {
            equator_c: 27.0,
            pole_drop_c: 50.0,
            cooling_curve: 1.6,
            lapse_c_per_km: 6.5,
            temperature_noise_c: 3.0,
            base_rain_mm: 200.0,
            tropical_rain_mm: 1900.0,
            tropical_width_deg: 13.0,
            storm_rain_mm: 850.0,
            storm_lat_deg: 52.0,
            storm_width_deg: 14.0,
            inland_rain: 0.3,
            inland_km: 1400.0,
            rain_noise: 0.35,
            null_temperature_c: NULL_TEMPERATURE as f64,
            null_rain_mm: NULL_RAINFALL as f64,
        }
    }
}

const OWNS: &[&str] = &["cell.temperature", "cell.rainfall", "cell.temperature_set", "cell.rainfall_set"];
static LATITUDE: Manifest = Manifest::generation(Slot::Climate, "latitude", crate::fields::Tier::Cell, &["cell.elevation"], OWNS);
static NULL: Manifest = Manifest::generation(Slot::Climate, "null", crate::fields::Tier::Cell, &[], OWNS);

/// The null climate by default: a temperate place with enough rain to farm.
pub const NULL_TEMPERATURE: f32 = 15.0;
pub const NULL_RAINFALL: f32 = 800.0;

impl SimModule for LatitudeClimate {
    fn manifest(&self) -> &'static Manifest {
        &LATITUDE
    }
    fn generate(&self, w: &mut World) {
        let c = generate(&w.grid, &w.elevation, w.params.seed, w.params.radius_km, &self.params);
        w.temperature = c.temperature;
        w.rainfall = c.rainfall;
        keep_painted(w);
    }
}

impl SimModule for NullClimate {
    fn manifest(&self) -> &'static Manifest {
        &NULL
    }
    fn generate(&self, w: &mut World) {
        w.temperature = vec![self.params.null_temperature_c as f32; w.grid.cells()];
        w.rainfall = vec![self.params.null_rain_mm as f32; w.grid.cells()];
        keep_painted(w);
    }
}

/// Every version keeps the climate the editor painted over what it works out. A new world has
/// nothing painted.
fn keep_painted(w: &mut World) {
    let cells = w.grid.cells();
    for v in [&mut w.temperature_set, &mut w.rainfall_set] {
        if v.len() != cells {
            *v = vec![f32::NAN; cells];
        }
    }
    for c in 0..cells {
        if !w.temperature_set[c].is_nan() {
            w.temperature[c] = w.temperature_set[c];
        }
        if !w.rainfall_set[c].is_nan() {
            w.rainfall[c] = w.rainfall_set[c];
        }
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

/// The coldest and hottest yearly mean `cell.temperature` may hold, °C.
const MIN_C: f64 = -90.0;
const MAX_C: f64 = 70.0;

/// Sea-level temperature by latitude (degrees, absolute).
fn sea_level_temperature(abs_lat_deg: f64, p: &ClimateParams) -> f64 {
    p.equator_c - p.pole_drop_c * (abs_lat_deg / 90.0).powf(p.cooling_curve)
}

/// Rainfall by latitude for a coastal place: wet tropics, dry subtropics, wet mid-latitudes, dry
/// poles.
fn band_rainfall(abs_lat_deg: f64, p: &ClimateParams) -> f64 {
    let g = |centre: f64, width: f64| (-((abs_lat_deg - centre) / width).powi(2)).exp();
    p.base_rain_mm + p.tropical_rain_mm * g(0.0, p.tropical_width_deg) + p.storm_rain_mm * g(p.storm_lat_deg, p.storm_width_deg)
}

pub fn generate(grid: &Grid, elevation: &[f32], seed: u64, radius_km: f64, p: &ClimateParams) -> Climate {
    let cell_km = (grid.mean_area()).sqrt() * radius_km;
    let coast_distance_km: Vec<f32> =
        sea_distance_cells(grid, elevation).into_iter().map(|d| (d as f64 * cell_km) as f32).collect();

    let t_key = draw(seed, Module::Climate, 0, 0, purpose("temperature noise"));
    let r_key = draw(seed, Module::Climate, 0, 0, purpose("rainfall noise"));
    let lapse_per_m = (p.lapse_c_per_km / 1000.0) as f32;
    let (temperature, rainfall): (Vec<f32>, Vec<f32>) = (0..grid.cells())
        .into_par_iter()
        .map(|c| {
            let d = grid.center[c];
            let lat = (grid.lat[c] as f64).to_degrees().abs();
            let t_noise = fbm([d[0] * 3.0, d[1] * 3.0, d[2] * 3.0], 3, t_key) * p.temperature_noise_c;
            let t = sea_level_temperature(lat, p) + t_noise - (elevation[c].max(0.0) * lapse_per_m) as f64;
            let continental = p.inland_rain + (1.0 - p.inland_rain) * (-(coast_distance_km[c] as f64) / p.inland_km).exp();
            let r_noise = 1.0 + p.rain_noise * fbm([d[0] * 4.0, d[1] * 4.0, d[2] * 4.0], 4, r_key);
            let r = band_rainfall(lat, p) * continental * r_noise;
            // Extreme settings together (bitter poles, steep cooling with height) could pass the
            // coldest a field may hold; no default world comes near it.
            (t.clamp(MIN_C, MAX_C) as f32, r.max(0.0) as f32)
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
        let p = ClimateParams::default();
        assert!(sea_level_temperature(0.0, &p) > 20.0);
        assert!(sea_level_temperature(90.0, &p) < -15.0);
        let g = Grid::new(20);
        let flat = vec![0.0f32; g.cells()];
        let high = vec![3000.0f32; g.cells()];
        let a = generate(&g, &flat, 1, 6371.0, &p);
        let b = generate(&g, &high, 1, 6371.0, &p);
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
        let c = generate(&g, &elev, 1, 6371.0, &ClimateParams::default());
        assert_eq!(c.coast_distance_km[sea], 0.0);
        let far = g.dir_to_cell([0.0, 0.0, -1.0]);
        assert!(c.coast_distance_km[far] > 15_000.0);
        // Same latitude, near versus far from the sea; noise is far smaller than this factor.
        assert!(c.rainfall[far] < c.rainfall[sea] * 0.7);
    }
}
