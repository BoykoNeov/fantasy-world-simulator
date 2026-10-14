//! Biomes v0: a lookup from annual temperature, rainfall and elevation.

use crate::module::{Manifest, Module, Slot};
use crate::world::World;
use rayon::prelude::*;

/// Biomes v0: the lookup below.
pub struct Biomes;

static V0: Manifest = Manifest::generation(Slot::Biomes, "lookup", &["cell.elevation", "cell.temperature", "cell.rainfall"], &["cell.biome"]);

impl Module for Biomes {
    fn manifest(&self) -> &'static Manifest {
        &V0
    }
    fn generate(&self, w: &mut World) {
        w.biome = generate(&w.elevation, &w.temperature, &w.rainfall);
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

impl Biome {
    pub fn classify(elevation_m: f32, temperature_c: f32, rainfall_mm: f32) -> Biome {
        let (t, r) = (temperature_c, rainfall_mm);
        if elevation_m < 0.0 {
            Biome::Ocean
        } else if t < -10.0 {
            Biome::IceCap
        } else if elevation_m > 3200.0 {
            Biome::Alpine
        } else if t < -2.0 {
            Biome::Tundra
        } else if r < 250.0 || (t > 18.0 && r < 400.0) {
            Biome::Desert
        } else if t < 4.0 {
            Biome::BorealForest
        } else if t < 19.0 {
            if r < 650.0 { Biome::Grassland } else { Biome::TemperateForest }
        } else if r < 1400.0 {
            Biome::Savanna
        } else {
            Biome::TropicalForest
        }
    }

    /// Food a square kilometre of this biome yields per year, in rations (one ration feeds one
    /// person for one day), with pre-modern farming and herding.
    pub fn food_per_km2(self) -> f64 {
        match self {
            Biome::Ocean | Biome::IceCap => 0.0,
            Biome::Tundra | Biome::Alpine => 60.0,
            Biome::Desert => 30.0,
            Biome::BorealForest => 180.0,
            Biome::TemperateForest => 780.0,
            Biome::Grassland => 900.0,
            Biome::Savanna => 480.0,
            Biome::TropicalForest => 360.0,
        }
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

pub fn generate(elevation: &[f32], temperature: &[f32], rainfall: &[f32]) -> Vec<Biome> {
    (0..elevation.len())
        .into_par_iter()
        .map(|c| Biome::classify(elevation[c], temperature[c], rainfall[c]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_examples() {
        assert_eq!(Biome::classify(-5.0, 20.0, 1000.0), Biome::Ocean);
        assert_eq!(Biome::classify(100.0, 26.0, 2500.0), Biome::TropicalForest);
        assert_eq!(Biome::classify(100.0, 25.0, 150.0), Biome::Desert);
        assert_eq!(Biome::classify(100.0, 12.0, 900.0), Biome::TemperateForest);
        assert_eq!(Biome::classify(100.0, 12.0, 450.0), Biome::Grassland);
        assert_eq!(Biome::classify(100.0, -20.0, 300.0), Biome::IceCap);
        assert_eq!(Biome::classify(4000.0, 0.0, 800.0), Biome::Alpine);
    }

    #[test]
    fn all_is_complete_and_ordered() {
        for (i, b) in ALL.iter().enumerate() {
            assert_eq!(*b as usize, i);
        }
    }
}
