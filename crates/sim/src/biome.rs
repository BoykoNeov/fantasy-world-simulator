//! Biomes v0: a lookup from annual temperature, rainfall and elevation.

use crate::module::{Manifest, Module, Slot};
use crate::world::World;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Biomes v0: the lookup below.
pub struct Biomes {
    pub params: BiomeParams,
}

/// Where each biome begins: limits of yearly mean temperature (°C), rain (mm a year) and height
/// (m), tried in the order of [`BiomeParams::classify`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BiomeParams {
    /// Colder than this: ice cap.
    pub ice_below_c: f64,
    /// Higher than this: alpine.
    pub alpine_above_m: f64,
    /// Colder than this: tundra.
    pub tundra_below_c: f64,
    /// Drier than this: desert; and warmer than `hot_desert_c` and drier than `hot_desert_rain_mm`.
    pub desert_rain_mm: f64,
    pub hot_desert_c: f64,
    pub hot_desert_rain_mm: f64,
    /// Colder than this: boreal forest.
    pub boreal_below_c: f64,
    /// Colder than this: temperate land, grassland if drier than `grassland_rain_mm`, else forest.
    pub temperate_below_c: f64,
    pub grassland_rain_mm: f64,
    /// Warmer land drier than this is savanna, wetter tropical forest.
    pub savanna_rain_mm: f64,
}

impl Default for BiomeParams {
    fn default() -> Self {
        BiomeParams {
            ice_below_c: -10.0,
            alpine_above_m: 3200.0,
            tundra_below_c: -2.0,
            desert_rain_mm: 250.0,
            hot_desert_c: 18.0,
            hot_desert_rain_mm: 400.0,
            boreal_below_c: 4.0,
            temperate_below_c: 19.0,
            grassland_rain_mm: 650.0,
            savanna_rain_mm: 1400.0,
        }
    }
}

static V0: Manifest =
    Manifest::generation(Slot::Biomes, "lookup", crate::fields::Tier::Cell, &["cell.elevation", "cell.temperature", "cell.rainfall"], &["cell.biome", "cell.biome_set"]);

/// `cell.biome_set` where the editor painted no biome.
pub const NOT_SET: u8 = u8::MAX;

impl Module for Biomes {
    fn manifest(&self) -> &'static Manifest {
        &V0
    }
    fn generate(&self, w: &mut World) {
        w.biome = generate(&w.elevation, &w.temperature, &w.rainfall, &self.params);
        // Biomes the editor painted win on land. A new world has none.
        if w.biome_set.len() != w.biome.len() {
            w.biome_set = vec![NOT_SET; w.biome.len()];
        }
        for (c, &b) in w.biome_set.iter().enumerate() {
            if b != NOT_SET && w.elevation[c] >= 0.0 {
                w.biome[c] = ALL[b as usize];
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Biome {
    Ocean = 0,
    IceCap,
    Tundra,
    BorealForest,
    TemperateForest,
    Grassland,
    Desert,
    Savanna,
    TropicalForest,
    Alpine,
}

pub const ALL: [Biome; 10] = [
    Biome::Ocean,
    Biome::IceCap,
    Biome::Tundra,
    Biome::BorealForest,
    Biome::TemperateForest,
    Biome::Grassland,
    Biome::Desert,
    Biome::Savanna,
    Biome::TropicalForest,
    Biome::Alpine,
];

impl BiomeParams {
    pub fn classify(&self, elevation_m: f32, temperature_c: f32, rainfall_mm: f32) -> Biome {
        let p = self;
        let lim = |x: f64| x as f32;
        let (t, r) = (temperature_c, rainfall_mm);
        if elevation_m < 0.0 {
            Biome::Ocean
        } else if t < lim(p.ice_below_c) {
            Biome::IceCap
        } else if elevation_m > lim(p.alpine_above_m) {
            Biome::Alpine
        } else if t < lim(p.tundra_below_c) {
            Biome::Tundra
        } else if r < lim(p.desert_rain_mm) || (t > lim(p.hot_desert_c) && r < lim(p.hot_desert_rain_mm)) {
            Biome::Desert
        } else if t < lim(p.boreal_below_c) {
            Biome::BorealForest
        } else if t < lim(p.temperate_below_c) {
            if r < lim(p.grassland_rain_mm) { Biome::Grassland } else { Biome::TemperateForest }
        } else if r < lim(p.savanna_rain_mm) {
            Biome::Savanna
        } else {
            Biome::TropicalForest
        }
    }
}

impl Biome {

    /// Share of the food this biome yields that comes better from herds than from fields: all of it
    /// where crops won't grow, little of it in good farming country.
    pub fn grazing_share(self) -> f64 {
        match self {
            Biome::Ocean | Biome::IceCap => 0.0,
            Biome::Tundra | Biome::Alpine => 1.0,
            Biome::Desert => 0.9,
            Biome::Savanna => 0.6,
            Biome::BorealForest => 0.4,
            Biome::Grassland => 0.35,
            Biome::TemperateForest => 0.2,
            Biome::TropicalForest => 0.1,
        }
    }

    /// Whether the land is wooded (timber can be cut).
    pub fn is_forest(self) -> bool {
        matches!(self, Biome::BorealForest | Biome::TemperateForest | Biome::TropicalForest)
    }

    pub fn name(self) -> &'static str {
        match self {
            Biome::Ocean => "ocean",
            Biome::IceCap => "ice cap",
            Biome::Tundra => "tundra",
            Biome::BorealForest => "boreal forest",
            Biome::TemperateForest => "temperate forest",
            Biome::Grassland => "grassland",
            Biome::Desert => "desert",
            Biome::Savanna => "savanna",
            Biome::TropicalForest => "tropical forest",
            Biome::Alpine => "alpine",
        }
    }
}

pub fn generate(elevation: &[f32], temperature: &[f32], rainfall: &[f32], p: &BiomeParams) -> Vec<Biome> {
    (0..elevation.len())
        .into_par_iter()
        .map(|c| p.classify(elevation[c], temperature[c], rainfall[c]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_examples() {
        let b = BiomeParams::default();
        assert_eq!(b.classify(-5.0, 20.0, 1000.0), Biome::Ocean);
        assert_eq!(b.classify(100.0, 26.0, 2500.0), Biome::TropicalForest);
        assert_eq!(b.classify(100.0, 25.0, 150.0), Biome::Desert);
        assert_eq!(b.classify(100.0, 12.0, 900.0), Biome::TemperateForest);
        assert_eq!(b.classify(100.0, 12.0, 450.0), Biome::Grassland);
        assert_eq!(b.classify(100.0, -20.0, 300.0), Biome::IceCap);
        assert_eq!(b.classify(4000.0, 0.0, 800.0), Biome::Alpine);
    }

    #[test]
    fn all_is_complete_and_ordered() {
        for (i, b) in ALL.iter().enumerate() {
            assert_eq!(*b as usize, i);
        }
    }
}
