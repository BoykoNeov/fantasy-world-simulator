//! A world's configuration: its parameters, its calendar, and which version fills each module
//! slot, with that version's parameters. Chosen at world creation and stored in every save.

use crate::biome::{BiomeParams, Biomes};
use crate::calendar::{Calendar, CalendarDef};
use crate::characters::{RulerParams, Rulers};
use crate::diplomacy::{TreatyParams, WarParams, WarState};
use crate::disease::{DiseaseParams, Plagues};
use crate::climate::{ClimateParams, LatitudeClimate, NullClimate};
use crate::culture::{Assimilation, CultureParams};
use crate::economy::{Food, FoodParams};
use crate::hydrology::{RiverParams, Rivers};
use crate::lives::{LifeParams, Lives};
use crate::markets::{MarketParams, Markets};
use crate::module::{Module, Slot};
use crate::nature::{NatureParams, Regrowth};
use crate::pops::{PopParams, Population};
use crate::provinces::{ProvinceGrowth, ProvinceParams};
use crate::realms::{Founding, RealmParams};
use crate::religion::{Conversion, ReligionParams};
use crate::research::{Research, ResearchParams};
use crate::taxonomy::TreeDef;
use crate::terrain::{NoiseTerrain, NullTerrain, TerrainParams};
use crate::warfare::{TroopParams, Troops};
use crate::weather::Droughts;
use crate::world::{FarmlandParams, Fertility, WorldParams};
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
    /// Economics v1, "markets", in the economics slot instead of v0 (at most one of the two is
    /// set). Scenario files written before it existed have neither key and get none.
    #[serde(default)]
    pub markets: Option<MarketParams>,
    /// Realms v1, "basic": the starting realms, their claims, taxes and unrest (step 7). On by
    /// default since step 7's checks; scenario files written before it existed have no key and get
    /// none, so they make the worlds they made.
    #[serde(default)]
    pub realms: Option<RealmParams>,
    /// Characters v1, "rulers": every realm's ruler and heir (step 7). Needs realms. On by default,
    /// off in files that don't name it, as realms are.
    #[serde(default)]
    pub characters: Option<RulerParams>,
    /// Diplomacy v0, "wars": wars, peace and truces between realms, by the player's pokes (step 7);
    /// v1, "treaties", when its parameters hold [`crate::diplomacy::TreatyParams`]: realms choose war
    /// and make peace themselves (needs armies). Needs realms. "treaties" by default, off in files
    /// that don't name it, as realms are.
    #[serde(default)]
    pub diplomacy: Option<WarParams>,
    /// Warfare v1, "troops": provinces raise soldiers, and armies and fleets move and fight (step
    /// 7); "campaigns" (the default since step 8's wave) is "troops" with every part of logistics on
    /// (sieges in depth alone until Oct 18). Needs
    /// realms. Off in files that don't name it, as realms are, and its parts off in files that don't
    /// name them.
    #[serde(default)]
    pub warfare: Option<TroopParams>,
    /// Research v1, "tree": realms study and learn techs (step 9), which do what their effects say
    /// where its "effects" part is on. Needs realms. On by default since techs have effects (it was
    /// off until then: the user, Oct 18, as scholars cost realms about 5% of their taxes for
    /// nothing); off in files that don't name it.
    #[serde(default)]
    pub research: Option<ResearchParams>,
    /// Culture v1, "assimilation": peoples spread and people take up the culture that pulls hardest
    /// (step 9). Needs Population. On by default since the first wave's checks, off in files that
    /// don't name it.
    #[serde(default)]
    pub culture: Option<CultureParams>,
    // The second wave of modules built side by side (Oct 18).
    /// Religion v1, "conversion": faiths spread and people take up the faith that pulls hardest
    /// (step 9). Needs Population. On by default since the second wave's checks (the user, Oct 18),
    /// off in files that don't name it.
    #[serde(default)]
    pub religion: Option<ReligionParams>,
    /// Disease v1, "plagues": plagues break out, spread along the ways, kill and leave survivors
    /// immune (step 10). Needs Population. On by default since Oct 19 (the user's choice: story 2
    /// passes on as many of its 18 worlds with it as without), off in files that don't name it.
    #[serde(default)]
    pub disease: Option<DiseaseParams>,
    /// Natural world v1, "regrowth": woodland, fish and game used up and growing back (step 10).
    /// Needs Population. On by default since the second wave's checks (the user, Oct 18), off in
    /// files that don't name it.
    #[serde(default)]
    pub nature: Option<NatureParams>,
    /// Lives v0 "diary" and v1 "lives" (when its parameters hold [`crate::lives::RoleParams`]): a
    /// few named people followed through their lives, samples of their provinces who change
    /// nothing in the world (step 9). Needs Population. Off by default, and in files that don't
    /// name it.
    #[serde(default)]
    pub lives: Option<LifeParams>,
    /// The settings of the slots that make the map, used once, when the world is made (and again
    /// where an edit makes the map anew). Scenario files written before they were settings get the
    /// numbers the modules had then, so they make the same world.
    #[serde(default)]
    pub terrain_params: TerrainParams,
    #[serde(default)]
    pub climate_params: ClimateParams,
    #[serde(default)]
    pub biome_params: BiomeParams,
    #[serde(default)]
    pub river_params: RiverParams,
    #[serde(default)]
    pub farmland_params: FarmlandParams,
    #[serde(default)]
    pub province_params: ProvinceParams,
}

impl Default for ModuleConfig {
    fn default() -> Self {
        ModuleConfig {
            terrain: TerrainVersion::default(),
            climate: ClimateVersion::default(),
            economics: Some(FoodParams::default()),
            population: Some(PopParams::default()),
            weather: true,
            markets: None,
            realms: Some(RealmParams::default()),
            characters: Some(RulerParams::default()),
            diplomacy: Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() }),
            warfare: Some(campaigns(TroopParams::default())),
            research: Some(ResearchParams::default()),
            culture: Some(CultureParams::default()),
            religion: Some(ReligionParams::default()),
            disease: Some(DiseaseParams::default()),
            nature: Some(NatureParams::default()),
            lives: None,
            terrain_params: TerrainParams::default(),
            climate_params: ClimateParams::default(),
            biome_params: BiomeParams::default(),
            river_params: RiverParams::default(),
            farmland_params: FarmlandParams::default(),
            province_params: ProvinceParams::default(),
        }
    }
}

/// Warfare "campaigns": `t` with every part of logistics on: sieges in depth, supply lines, winter,
/// pay and armies acting together (each kept with its numbers if on already). Sieges alone until
/// Oct 18, when the other four were measured against today's world (Research and Culture on): on
/// 18 worlds a rich river city fell in a normal war on 8 with all five against 4 with sieges alone,
/// and the user chose all five.
fn campaigns(mut t: TroopParams) -> TroopParams {
    t.sieges = t.sieges.take().or_else(|| Some(Default::default()));
    t.supply = t.supply.take().or_else(|| Some(Default::default()));
    t.seasons = t.seasons.take().or_else(|| Some(Default::default()));
    t.pay = t.pay.take().or_else(|| Some(Default::default()));
    t.massing = t.massing.take().or_else(|| Some(Default::default()));
    t
}

impl ModuleConfig {
    /// Realms, rulers, diplomacy and armies switched off, and research, culture, religion, disease and
    /// the natural world, which came after them: the world as it was before step 7 (they are on by default since step 7's checks, and
    /// the first wave's). For tests of what came before, and for worlds without people, which
    /// realms and culture need.
    pub fn without_realms(&mut self) -> &mut Self {
        (self.realms, self.characters, self.diplomacy, self.warfare) = (None, None, None, None);
        (self.research, self.culture) = (None, None);
        (self.religion, self.disease, self.nature) = (None, None, None);
        self
    }

    /// The chosen modules, in slot order. Biomes, rivers, farmland and provinces have one version
    /// each for now.
    pub fn build(&self) -> Vec<Box<dyn Module>> {
        let mut out: Vec<Box<dyn Module>> = vec![
            match self.terrain {
                TerrainVersion::Null => Box::new(NullTerrain { params: self.terrain_params.clone() }),
                TerrainVersion::Noise => Box::new(NoiseTerrain { params: self.terrain_params.clone() }),
            },
            match self.climate {
                ClimateVersion::Null => Box::new(NullClimate { params: self.climate_params.clone() }),
                ClimateVersion::Latitude => Box::new(LatitudeClimate { params: self.climate_params.clone() }),
            },
            Box::new(Biomes { params: self.biome_params.clone() }),
            Box::new(Rivers { params: self.river_params.clone() }),
            Box::new(Fertility { params: self.farmland_params.clone() }),
            Box::new(ProvinceGrowth { params: self.province_params.clone() }),
        ];
        // What techs do, for the modules their effects reach; none without Research or its effects.
        let know = self.research.as_ref().and_then(crate::research::Know::new);
        if let Some(params) = &self.economics {
            out.push(Box::new(Food { params: params.clone(), know: know.clone() }));
        }
        // Both set fills the economics slot twice, which the engine refuses.
        if let Some(params) = &self.markets {
            let mut m = Markets::new(params.clone());
            m.know = know.clone();
            out.push(Box::new(m));
        }
        if let Some(params) = &self.population {
            out.push(Box::new(Population { params: params.clone() }));
        }
        if self.weather {
            out.push(Box::new(Droughts::default()));
        }
        if let Some(params) = &self.realms {
            // Realms tax coins only where the world keeps them.
            let mut m = Founding::new(params.clone(), self.markets.is_some());
            m.know = know.clone();
            out.push(Box::new(m));
        }
        if let Some(params) = &self.characters {
            out.push(Box::new(Rulers { params: params.clone() }));
        }
        if let Some(params) = &self.diplomacy {
            out.push(Box::new(WarState::new(params.clone())));
        }
        if let Some(params) = &self.warfare {
            // Soldiers use up goods and are paid for only where the world keeps them.
            let mut m = Troops::new(params.clone(), self.markets.is_some());
            m.know = know.clone();
            out.push(Box::new(m));
        }
        if let Some(params) = &self.research {
            // Scholars are paid only where the world keeps coins.
            out.push(Box::new(Research::new(params.clone(), self.markets.is_some())));
        }
        if let Some(params) = &self.culture {
            out.push(Box::new(Assimilation::new(params.clone())));
        }
        if let Some(params) = &self.religion {
            out.push(Box::new(Conversion::new(params.clone())));
        }
        if let Some(params) = &self.disease {
            out.push(Box::new(Plagues::new(params.clone())));
        }
        if let Some(params) = &self.nature {
            out.push(Box::new(Regrowth::new(params.clone())));
        }
        if let Some(params) = &self.lives {
            // The jobs open to followed people: those the modules that are on offer.
            let jobs = out.iter().filter_map(|m| m.jobs()).collect();
            out.push(Box::new(Lives::new(params.clone(), jobs)));
        }
        out
    }
}

/// Every slot that has a choice, with its versions by name; "off" is a runtime slot switched off.
/// The first version of each is the default. This is what the combination tester tries.
pub const CHOICES: &[(Slot, &[&str])] = &[
    (Slot::Terrain, &["noise", "null"]),
    (Slot::Climate, &["latitude", "null"]),
    (Slot::Economics, &["food", "markets", "off"]),
    (Slot::Population, &["basic", "off"]),
    (Slot::Weather, &["droughts", "off"]),
    (Slot::Realms, &["basic", "off"]),
    (Slot::Characters, &["rulers", "off"]),
    (Slot::Diplomacy, &["treaties", "wars", "off"]),
    (Slot::Warfare, &["campaigns", "troops", "off"]),
    (Slot::Research, &["tree", "off"]),
    (Slot::Culture, &["assimilation", "off"]),
    (Slot::Religion, &["conversion", "off"]),
    (Slot::Disease, &["plagues", "off"]),
    (Slot::Nature, &["regrowth", "off"]),
    (Slot::Lives, &["off", "lives", "diary"]),
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
            Slot::Economics => match (&self.economics, &self.markets) {
                (_, Some(_)) => "markets",
                (Some(_), None) => "food",
                (None, None) => "off",
            },
            Slot::Population => on(self.population.is_some(), "basic"),
            Slot::Weather => on(self.weather, "droughts"),
            Slot::Realms => on(self.realms.is_some(), "basic"),
            Slot::Characters => on(self.characters.is_some(), "rulers"),
            Slot::Diplomacy => match &self.diplomacy {
                None => "off",
                Some(d) if d.treaties.is_some() => "treaties",
                Some(_) => "wars",
            },
            Slot::Warfare => match &self.warfare {
                None => "off",
                Some(t) if t.sieges.is_some() => "campaigns",
                Some(_) => "troops",
            },
            Slot::Research => on(self.research.is_some(), "tree"),
            Slot::Culture => on(self.culture.is_some(), "assimilation"),
            Slot::Religion => on(self.religion.is_some(), "conversion"),
            Slot::Disease => on(self.disease.is_some(), "plagues"),
            Slot::Nature => on(self.nature.is_some(), "regrowth"),
            Slot::Lives => match &self.lives {
                None => "off",
                Some(l) if l.roles.is_some() => "lives",
                Some(_) => "diary",
            },
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
            Slot::Economics => {
                self.economics = if version == "food" { self.economics.take().or_else(|| Some(FoodParams::default())) } else { None };
                self.markets = if version == "markets" { self.markets.take().or_else(|| Some(MarketParams::default())) } else { None };
            }
            Slot::Population => self.population = if on { self.population.take().or_else(|| Some(PopParams::default())) } else { None },
            Slot::Weather => self.weather = on,
            Slot::Realms => self.realms = if on { self.realms.take().or_else(|| Some(RealmParams::default())) } else { None },
            Slot::Characters => self.characters = if on { self.characters.take().or_else(|| Some(RulerParams::default())) } else { None },
            Slot::Diplomacy => {
                self.diplomacy = match version {
                    "off" => None,
                    _ => {
                        let mut d = self.diplomacy.take().unwrap_or_default();
                        d.treaties = if version == "treaties" { d.treaties.take().or_else(|| Some(TreatyParams::default())) } else { None };
                        Some(d)
                    }
                }
            }
            Slot::Warfare => {
                self.warfare = match version {
                    "off" => None,
                    _ => {
                        // "campaigns" is "troops" with sieges in depth on; "troops" has every part
                        // of logistics off.
                        let mut t = self.warfare.take().unwrap_or_default();
                        if version == "campaigns" {
                            t = campaigns(t);
                        } else {
                            (t.supply, t.seasons, t.pay, t.sieges, t.massing) = (None, None, None, None, None);
                        }
                        Some(t)
                    }
                }
            }
            Slot::Research => self.research = if on { self.research.take().or_else(|| Some(ResearchParams::default())) } else { None },
            Slot::Culture => self.culture = if on { self.culture.take().or_else(|| Some(CultureParams::default())) } else { None },
            Slot::Religion => self.religion = if on { self.religion.take().or_else(|| Some(ReligionParams::default())) } else { None },
            Slot::Disease => self.disease = if on { self.disease.take().or_else(|| Some(DiseaseParams::default())) } else { None },
            Slot::Nature => self.nature = if on { self.nature.take().or_else(|| Some(NatureParams::default())) } else { None },
            Slot::Lives => {
                self.lives = match version {
                    "off" => None,
                    _ => {
                        // "lives" is the diary with jobs and what befalls people.
                        let mut l = self.lives.take().unwrap_or_default();
                        l.roles = if version == "lives" { l.roles.take().or_else(|| Some(Default::default())) } else { None };
                        Some(l)
                    }
                }
            }
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
