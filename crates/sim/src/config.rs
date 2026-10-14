//! A world's configuration: its parameters, its calendar, and which version fills each module
//! slot, with that version's parameters. Chosen at world creation and stored in every save.

use crate::biome::Biomes;
use crate::calendar::{Calendar, CalendarDef};
use crate::climate::{LatitudeClimate, NullClimate};
use crate::economy::{Food, FoodParams};
use crate::hydrology::Rivers;
use crate::module::Module;
use crate::pops::{PopParams, Population};
use crate::provinces::ProvinceGrowth;
use crate::terrain::{NoiseTerrain, NullTerrain};
use crate::weather::Droughts;
use crate::world::{Fertility, WorldParams};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TerrainVersion {
    /// Smooth, gentle landmasses; the same for every seed.
    Null,
    #[default]
    Noise,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ClimateVersion {
    /// The same mild climate everywhere.
    Null,
    #[default]
    Latitude,
}

impl FromStr for TerrainVersion {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "null" => Ok(TerrainVersion::Null),
            "noise" => Ok(TerrainVersion::Noise),
            _ => Err(format!("terrain version {s}: expected null or noise")),
        }
    }
}

impl FromStr for ClimateVersion {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "null" => Ok(ClimateVersion::Null),
            "latitude" => Ok(ClimateVersion::Latitude),
            _ => Err(format!("climate version {s}: expected null or latitude")),
        }
    }
}

/// Which version fills each slot. A runtime slot set to `None` is switched off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModuleConfig {
    pub terrain: TerrainVersion,
    pub climate: ClimateVersion,
    /// Economics v0, "food".
    pub economics: Option<FoodParams>,
    /// Population v0, "basic".
    pub population: Option<PopParams>,
    /// Weather v0, "droughts" (no parameters yet). Off means rain is always normal.
    pub weather: bool,
}

impl Default for ModuleConfig {
    fn default() -> Self {
        ModuleConfig {
            terrain: TerrainVersion::default(),
            climate: ClimateVersion::default(),
            economics: Some(FoodParams::default()),
            population: Some(PopParams::default()),
            weather: true,
        }
    }
}

impl ModuleConfig {
    /// The chosen modules, in slot order. Biomes, rivers, farmland and provinces have one version
    /// each for now.
    pub fn build(&self) -> Vec<Box<dyn Module>> {
        let mut out: Vec<Box<dyn Module>> = vec![
            match self.terrain {
                TerrainVersion::Null => Box::new(NullTerrain),
                TerrainVersion::Noise => Box::new(NoiseTerrain),
            },
            match self.climate {
                ClimateVersion::Null => Box::new(NullClimate),
                ClimateVersion::Latitude => Box::new(LatitudeClimate),
            },
            Box::new(Biomes),
            Box::new(Rivers),
            Box::new(Fertility),
            Box::new(ProvinceGrowth),
        ];
        if let Some(params) = &self.economics {
            out.push(Box::new(Food { params: params.clone() }));
        }
        if let Some(params) = &self.population {
            out.push(Box::new(Population { params: params.clone() }));
        }
        if self.weather {
            out.push(Box::new(Droughts::default()));
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub world: WorldParams,
    pub modules: ModuleConfig,
    pub calendar: CalendarDef,
}

impl Config {
    /// Default modules and the standard calendar.
    pub fn new(world: WorldParams) -> Config {
        Config { world, modules: ModuleConfig::default(), calendar: Calendar::standard().def }
    }
}
