//! A world's configuration: its parameters, its calendar, and which version fills each module
//! slot, with that version's parameters. Chosen at world creation and stored in every save.

use crate::biome::Biomes;
use crate::calendar::{Calendar, CalendarDef};
use crate::climate::{LatitudeClimate, NullClimate};
use crate::economy::{Food, FoodParams};
use crate::hydrology::Rivers;
use crate::module::{Module, Slot};
use crate::pops::{PopParams, Population};
use crate::provinces::ProvinceGrowth;
use crate::taxonomy::TreeDef;
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

/// Every slot that has a choice, with its versions by name; "off" is a runtime slot switched off.
/// The first version of each is the default. This is what the combination tester tries.
pub const CHOICES: &[(Slot, &[&str])] = &[
    (Slot::Terrain, &["noise", "null"]),
    (Slot::Climate, &["latitude", "null"]),
    (Slot::Economics, &["food", "off"]),
    (Slot::Population, &["basic", "off"]),
    (Slot::Weather, &["droughts", "off"]),
];

impl ModuleConfig {
    /// The version chosen for `slot`, by name (see [`CHOICES`]); `None` for a slot with no choice.
    pub fn choice(&self, slot: Slot) -> Option<&'static str> {
        let on = |b: bool, name: &'static str| if b { name } else { "off" };
        Some(match slot {
            Slot::Terrain => match self.terrain {
                TerrainVersion::Null => "null",
                TerrainVersion::Noise => "noise",
            },
            Slot::Climate => match self.climate {
                ClimateVersion::Null => "null",
                ClimateVersion::Latitude => "latitude",
            },
            Slot::Economics => on(self.economics.is_some(), "food"),
            Slot::Population => on(self.population.is_some(), "basic"),
            Slot::Weather => on(self.weather, "droughts"),
            _ => return None,
        })
    }

    /// Choose a version by name. A runtime slot switched on gets its default parameters, unless it
    /// was on already.
    pub fn set_choice(&mut self, slot: Slot, version: &str) -> Result<(), String> {
        let known = CHOICES.iter().find(|c| c.0 == slot).is_some_and(|c| c.1.contains(&version));
        if !known {
            return Err(format!("{slot:?} has no version {version}"));
        }
        let on = version != "off";
        match slot {
            Slot::Terrain => self.terrain = version.parse()?,
            Slot::Climate => self.climate = version.parse()?,
            Slot::Economics => self.economics = if on { self.economics.take().or_else(|| Some(FoodParams::default())) } else { None },
            Slot::Population => self.population = if on { self.population.take().or_else(|| Some(PopParams::default())) } else { None },
            Slot::Weather => self.weather = on,
            _ => unreachable!("checked against CHOICES"),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub world: WorldParams,
    pub modules: ModuleConfig,
    pub calendar: CalendarDef,
    /// The goods tree. Scenario files written before it was saved get the shipped one.
    #[serde(default)]
    pub goods: TreeDef,
}

impl Config {
    /// Default modules and the standard calendar.
    pub fn new(world: WorldParams) -> Config {
        Config { world, modules: ModuleConfig::default(), calendar: Calendar::standard().def, goods: TreeDef::standard_goods() }
    }
}
