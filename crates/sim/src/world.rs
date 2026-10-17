//! The world: every shared field, as typed arrays keyed by cell, province or pop.
//!
//! Generation modules fill the cell and province fields once, in the order their manifests give
//! (by default: terrain → climate → biomes → rivers → farmland → provinces). Runtime modules then
//! own the stores of goods and the pops. Every field can also be read and written by name, as
//! [`FieldData`], for contracts, fingerprints and saves.

use crate::biome::{self, Biome};
use crate::characters::Characters;
use crate::config::ModuleConfig;
use crate::diplomacy::{Relations, Wars};
use crate::fields::{FieldData, FieldId, Goods, Quantity, Tier, FIELDS};
use crate::grid::Grid;
use crate::module::{plan, Manifest, Module, Slot};
use crate::pops::Pops;
use crate::provinces::Provinces;
use crate::realms::Realms;
use crate::routes::Routes;
use crate::warfare::Armies;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// World sizes from the design. Cell count and province count are separate settings; presets are
/// just starting points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
    Huge,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldParams {
    /// Cube face size: the planet has 6 × n × n cells.
    pub n: usize,
    /// Target province count (islands can add a few more).
    pub provinces: usize,
    pub seed: u64,
    /// Fraction of cells that are land.
    pub land_fraction: f64,
    pub radius_km: f64,
}

impl WorldParams {
    pub fn preset(size: Size, seed: u64) -> Self {
        let (n, provinces) = match size {
            Size::Small => (128, 500),
            Size::Medium => (256, 2_000),
            Size::Large => (408, 5_000),
            Size::Huge => (576, 20_000),
        };
        WorldParams { n, provinces, seed, land_fraction: 0.36, radius_km: 6371.0 }
    }
}

/// Rivers carrying at least this much water a year (thousand m³; about 630 m³/s, a major river)
/// make the land around them more fertile and are drawn on the map. Much lower and, in rainy
/// regions, nearly every path a few cells long counts as a river.
pub const RIVER_MIN_FLOW: u64 = 20_000_000;

/// Farmland v0: food per cell from its biome, its area and whether a river runs through it.
pub struct Fertility {
    pub params: FarmlandParams,
}

/// Food a square kilometre of each biome yields per year, in rations (one ration feeds one person
/// for one day), with pre-modern farming and herding; ice and sea yield none. A cell with a big
/// river through it yields `river_bonus` times as much.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FarmlandParams {
    pub river_bonus: f64,
    pub tundra: f64,
    pub alpine: f64,
    pub desert: f64,
    pub boreal_forest: f64,
    pub temperate_forest: f64,
    pub grassland: f64,
    pub savanna: f64,
    pub tropical_forest: f64,
}

impl Default for FarmlandParams {
    fn default() -> Self {
        FarmlandParams {
            river_bonus: 1.6,
            tundra: 60.0,
            alpine: 60.0,
            desert: 30.0,
            boreal_forest: 180.0,
            temperate_forest: 780.0,
            grassland: 900.0,
            savanna: 480.0,
            tropical_forest: 360.0,
        }
    }
}

impl FarmlandParams {
    pub fn food_per_km2(&self, b: Biome) -> f64 {
        match b {
            Biome::Ocean | Biome::IceCap => 0.0,
            Biome::Tundra => self.tundra,
            Biome::Alpine => self.alpine,
            Biome::Desert => self.desert,
            Biome::BorealForest => self.boreal_forest,
            Biome::TemperateForest => self.temperate_forest,
            Biome::Grassland => self.grassland,
            Biome::Savanna => self.savanna,
            Biome::TropicalForest => self.tropical_forest,
        }
    }
}

static FERTILITY: Manifest =
    Manifest::generation(Slot::Fertility, "biome yield", crate::fields::Tier::Cell, &["cell.biome", "cell.river_flow", "cell.elevation"], &["cell.food_yield"]);

impl Module for Fertility {
    fn manifest(&self) -> &'static Manifest {
        &FERTILITY
    }
    fn generate(&self, w: &mut World) {
        let km2_per_sr = w.params.radius_km * w.params.radius_km;
        w.food_per_cell = (0..w.grid.cells())
            .map(|i| {
                let river = if w.river_flow[i] >= RIVER_MIN_FLOW && w.elevation[i] >= 0.0 { self.params.river_bonus } else { 1.0 };
                self.params.food_per_km2(w.biome[i]) * w.grid.area[i] * km2_per_sr * river
            })
            .collect();
    }
}

/// Cloning shares the grid, which never changes; every field is copied.
#[derive(Clone)]
pub struct World {
    pub params: WorldParams,
    pub grid: Arc<Grid>,
    /// Metres; below 0 is sea.
    pub elevation: Vec<f32>,
    /// Annual mean, °C.
    pub temperature: Vec<f32>,
    /// Annual total, mm.
    pub rainfall: Vec<f32>,
    pub biome: Vec<Biome>,
    /// The cell each cell drains to (field `cell.drainage_receiver`), [`crate::grid::NONE`] for a
    /// sink: the sea, or the low point of land with no sea.
    pub downstream: Vec<u32>,
    /// Water passing through each cell per year, thousand m³.
    pub river_flow: Vec<u64>,
    /// Food a cell yields in an average year, rations (field `cell.food_yield`).
    pub food_per_cell: Vec<f64>,
    pub provinces: Provinces,
    /// Province neighbours and the ways between them. Worked out from the map, not saved: whatever
    /// changes the map calls [`World::rebuild_routes`].
    pub routes: Arc<Routes>,
    /// The last month's trade, for showing it ([`crate::markets::TradeRecord`]): not a field.
    pub trade: Arc<crate::markets::TradeRecord>,
    /// Fields as they stood when today began, for the modules that read them so ([`Earlier`]).
    pub earlier: Arc<Earlier>,
    /// Stored goods, per province per good kept (field `province.stock`; see [`World::goods`]).
    pub stock: Vec<u64>,
    /// The goods tree, and which of its goods the per-good fields keep. Set from the configuration
    /// when the world is made; it never changes.
    pub goods: Arc<Goods>,
    /// Local price of each good kept, in thousandths of a coin per unit (field `province.price`).
    pub price: Vec<u64>,
    /// People working on each good kept (field `province.workers`).
    pub workers: Vec<u64>,
    /// People who starved in each province since the last year start.
    pub starved_this_year: Vec<u64>,
    pub pops: Pops,
    /// Share of the province's normal rain that is missing, per mille (0 = a normal year).
    pub rain_shortfall: Vec<u64>,
    /// Climate painted by the editor, which the climate module keeps over what it works out: °C
    /// and mm a year, NaN where nothing is set.
    pub temperature_set: Vec<f32>,
    pub rainfall_set: Vec<f32>,
    /// Biomes painted by the editor, on land; [`crate::biome::NOT_SET`] where nothing is set.
    pub biome_set: Vec<u8>,
    /// Water the spring of a river drawn by the editor adds to a cell, thousand m³ a year.
    pub spring: Vec<u64>,
    /// The realm that owns each province (field `province.owner`), [`crate::grid::NONE`] where none
    /// does.
    pub owner: Vec<u32>,
    pub realms: Realms,
    /// Each realm's stores, per good kept (field `realm.stock`; its coins are the treasury).
    pub realm_stock: Vec<u64>,
    pub armies: Armies,
    /// Rulers and heirs, living and dead.
    pub characters: Characters,
    /// The enemy realm occupying each province (field `province.occupier`), [`crate::grid::NONE`]
    /// where none does.
    pub occupier: Vec<u32>,
    /// Wars, going on and ended.
    pub wars: Wars,
    /// What realms think of each other (Diplomacy "treaties").
    pub relations: Relations,
    /// How restless each province is (field `province.unrest`): thousandths of the way to an
    /// uprising.
    pub unrest: Vec<u64>,
    /// The province where each province's uprising began (field `province.uprising`),
    /// [`crate::grid::NONE`] where it is calm.
    pub uprising: Vec<u32>,
}

impl World {
    /// A world with its grid built and every field empty.
    pub fn empty(params: &WorldParams) -> World {
        World {
            params: params.clone(),
            grid: Arc::new(Grid::new(params.n)),
            elevation: Vec::new(),
            temperature: Vec::new(),
            rainfall: Vec::new(),
            biome: Vec::new(),
            downstream: Vec::new(),
            river_flow: Vec::new(),
            food_per_cell: Vec::new(),
            provinces: Provinces::default(),
            routes: Arc::new(Routes::default()),
            trade: Arc::default(),
            earlier: Arc::default(),
            stock: Vec::new(),
            goods: Arc::new(Goods::default()),
            price: Vec::new(),
            workers: Vec::new(),
            starved_this_year: Vec::new(),
            pops: Pops::default(),
            rain_shortfall: Vec::new(),
            temperature_set: Vec::new(),
            rainfall_set: Vec::new(),
            biome_set: Vec::new(),
            spring: Vec::new(),
            owner: Vec::new(),
            realms: Realms::default(),
            realm_stock: Vec::new(),
            armies: Armies::default(),
            characters: Characters::default(),
            occupier: Vec::new(),
            wars: Wars::default(),
            relations: Relations::default(),
            unrest: Vec::new(),
            uprising: Vec::new(),
        }
    }

    /// Generate with the default module versions (no runtime modules run).
    pub fn generate(params: &WorldParams) -> World {
        let modules = ModuleConfig::default().build();
        let manifests: Vec<&Manifest> = modules.iter().map(|m| m.manifest()).collect();
        let plan = plan(&manifests).expect("the default configuration is valid");
        let mut w = World::empty(params);
        for &i in &plan.generation {
            modules[i].generate(&mut w);
        }
        w.rebuild_routes();
        w
    }

    /// Work out the province neighbours again, after the map changed.
    pub fn rebuild_routes(&mut self) {
        self.routes = Arc::new(Routes::build(self));
    }

    pub fn province_population(&self) -> Vec<u64> {
        let mut out = vec![0u64; self.provinces.count];
        for (k, &p) in self.pops.province.iter().enumerate() {
            out[p as usize] += self.pops.size[k];
        }
        out
    }

    /// A copy of one field's values.
    pub fn field(&self, id: FieldId) -> FieldData {
        use FieldData::*;
        let p = &self.provinces;
        match id.def().name {
            "cell.elevation" => F32(self.elevation.clone()),
            "cell.temperature" => F32(self.temperature.clone()),
            "cell.rainfall" => F32(self.rainfall.clone()),
            "cell.biome" => U8(self.biome.iter().map(|&b| b as u8).collect()),
            "cell.drainage_receiver" => U32(self.downstream.clone()),
            "cell.river_flow" => U64(self.river_flow.clone()),
            "cell.food_yield" => F64(self.food_per_cell.clone()),
            "cell.province" => U32(p.of_cell.clone()),
            "province.cells" => U32(p.cells.clone()),
            "province.area_km2" => F64(p.area_km2.clone()),
            "province.center" => Dir(p.center.clone()),
            "province.lat" => F32(p.lat.clone()),
            "province.food_capacity" => U64(p.food_capacity.clone()),
            "province.stock" => U64(self.stock.clone()),
            "province.starved_this_year" => U64(self.starved_this_year.clone()),
            "pop.province" => U32(self.pops.province.clone()),
            "pop.species" => U16(self.pops.species.clone()),
            "pop.culture" => U16(self.pops.culture.clone()),
            "pop.religion" => U16(self.pops.religion.clone()),
            "pop.class" => U8(self.pops.class.clone()),
            "pop.size" => U64(self.pops.size.clone()),
            "province.rain_shortfall" => U64(self.rain_shortfall.clone()),
            "cell.temperature_set" => F32(self.temperature_set.clone()),
            "cell.rainfall_set" => F32(self.rainfall_set.clone()),
            "cell.biome_set" => U8(self.biome_set.clone()),
            "cell.spring" => U64(self.spring.clone()),
            "province.grazing_capacity" => U64(p.grazing_capacity.clone()),
            "province.forest_km2" => F64(p.forest_km2.clone()),
            "province.coast_km" => F64(p.coast_km.clone()),
            "province.river_km" => F64(p.river_km.clone()),
            "province.upland_km2" => F64(p.upland_km2.clone()),
            "province.ore_km2" => F64(p.ore_km2.clone()),
            "province.price" => U64(self.price.clone()),
            "province.workers" => U64(self.workers.clone()),
            "province.owner" => U32(self.owner.clone()),
            "realm.capital" => U32(self.realms.capital.clone()),
            "realm.stock" => U64(self.realm_stock.clone()),
            "army.realm" => U32(self.armies.realm.clone()),
            "army.province" => U32(self.armies.province.clone()),
            "army.size" => U64(self.armies.size.clone()),
            "realm.ruler" => U32(self.realms.ruler.clone()),
            "realm.heir" => U32(self.realms.heir.clone()),
            "character.realm" => U32(self.characters.realm.clone()),
            "character.parent" => U32(self.characters.parent.clone()),
            "character.name" => U32(self.characters.name.clone()),
            "character.born" => F64(self.characters.born.clone()),
            "character.died" => U64(self.characters.died.clone()),
            "character.crowned" => U64(self.characters.crowned.clone()),
            "character.succession" => U8(self.characters.succession.clone()),
            "character.martial" => U8(self.characters.martial.clone()),
            "character.greed" => U8(self.characters.greed.clone()),
            "character.ability" => U8(self.characters.ability.clone()),
            "province.occupier" => U32(self.occupier.clone()),
            "war.attacker" => U32(self.wars.attacker.clone()),
            "war.defender" => U32(self.wars.defender.clone()),
            "war.cause" => U8(self.wars.cause.clone()),
            "war.began" => U64(self.wars.began.clone()),
            "war.ended" => U64(self.wars.ended.clone()),
            "war.winner" => U32(self.wars.winner.clone()),
            "war.truce_until" => U64(self.wars.truce_until.clone()),
            "province.unrest" => U64(self.unrest.clone()),
            "province.uprising" => U32(self.uprising.clone()),
            "realm.broke_from" => U32(self.realms.broke_from.clone()),
            "army.kind" => U16(self.armies.kind.clone()),
            "army.home" => U32(self.armies.home.clone()),
            "army.role" => U8(self.armies.role.clone()),
            "army.group" => U32(self.armies.group.clone()),
            "army.dest" => U32(self.armies.dest.clone()),
            "army.next" => U32(self.armies.next.clone()),
            "army.days_left" => F32(self.armies.days_left.clone()),
            "army.siege_days" => U32(self.armies.siege_days.clone()),
            "war.target" => U32(self.wars.target.clone()),
            "war.attacker_dead" => U64(self.wars.attacker_dead.clone()),
            "war.defender_dead" => U64(self.wars.defender_dead.clone()),
            "war.attacker_lost" => U32(self.wars.attacker_lost.clone()),
            "war.defender_lost" => U32(self.wars.defender_lost.clone()),
            "relation.a" => U32(self.relations.a.clone()),
            "relation.b" => U32(self.relations.b.clone()),
            "relation.a_of_b" => F32(self.relations.a_of_b.clone()),
            "relation.b_of_a" => F32(self.relations.b_of_a.clone()),
            other => unreachable!("field {other} has no storage"),
        }
    }

    /// Replace one field's values. Refuses data of the wrong type or length.
    pub fn set_field(&mut self, id: FieldId, data: FieldData) -> Result<(), String> {
        use FieldData::*;
        let def = id.def();
        let expect = match def.tier {
            Tier::Cell => Some(self.grid.cells()),
            // The province count is set by the province fields themselves; pops, realms, armies
            // and characters vary.
            Tier::Province | Tier::Pop | Tier::Realm | Tier::Army | Tier::Character | Tier::War | Tier::Relation => None,
            Tier::ProvinceGood | Tier::RealmGood => {
                let k = self.goods.stride();
                if !data.len().is_multiple_of(k) {
                    return Err(format!("{}: {} values, not a whole number of rows of {k} goods", def.name, data.len()));
                }
                None
            }
        };
        if let Some(n) = expect.filter(|&n| n != data.len()) {
            return Err(format!("{}: {} values for {n} cells", def.name, data.len()));
        }
        let wrong = || Err(format!("{}: wrong value type", def.name));
        let p = &mut self.provinces;
        match (def.name, data) {
            ("cell.elevation", F32(v)) => self.elevation = v,
            ("cell.temperature", F32(v)) => self.temperature = v,
            ("cell.rainfall", F32(v)) => self.rainfall = v,
            ("cell.biome", U8(v)) => {
                self.biome = v.iter().map(|&b| biome::ALL.get(b as usize).copied().ok_or(format!("cell.biome: no biome {b}"))).collect::<Result<_, _>>()?
            }
            ("cell.drainage_receiver", U32(v)) => self.downstream = v,
            ("cell.river_flow", U64(v)) => self.river_flow = v,
            ("cell.food_yield", F64(v)) => self.food_per_cell = v,
            ("cell.province", U32(v)) => p.of_cell = v,
            ("province.cells", U32(v)) => p.cells = v,
            ("province.area_km2", F64(v)) => {
                p.count = v.len();
                p.area_km2 = v
            }
            ("province.center", Dir(v)) => p.center = v,
            ("province.lat", F32(v)) => p.lat = v,
            ("province.food_capacity", U64(v)) => p.food_capacity = v,
            ("province.stock", U64(v)) => self.stock = v,
            ("province.starved_this_year", U64(v)) => self.starved_this_year = v,
            ("pop.province", U32(v)) => self.pops.province = v,
            ("pop.species", U16(v)) => self.pops.species = v,
            ("pop.culture", U16(v)) => self.pops.culture = v,
            ("pop.religion", U16(v)) => self.pops.religion = v,
            ("pop.class", U8(v)) => self.pops.class = v,
            ("pop.size", U64(v)) => self.pops.size = v,
            ("province.rain_shortfall", U64(v)) => self.rain_shortfall = v,
            ("cell.temperature_set", F32(v)) => self.temperature_set = v,
            ("cell.rainfall_set", F32(v)) => self.rainfall_set = v,
            ("cell.biome_set", U8(v)) => self.biome_set = v,
            ("cell.spring", U64(v)) => self.spring = v,
            ("province.grazing_capacity", U64(v)) => p.grazing_capacity = v,
            ("province.forest_km2", F64(v)) => p.forest_km2 = v,
            ("province.coast_km", F64(v)) => p.coast_km = v,
            ("province.river_km", F64(v)) => p.river_km = v,
            ("province.upland_km2", F64(v)) => p.upland_km2 = v,
            ("province.ore_km2", F64(v)) => p.ore_km2 = v,
            ("province.price", U64(v)) => self.price = v,
            ("province.workers", U64(v)) => self.workers = v,
            ("province.owner", U32(v)) => self.owner = v,
            ("realm.capital", U32(v)) => self.realms.capital = v,
            ("realm.stock", U64(v)) => self.realm_stock = v,
            ("army.realm", U32(v)) => self.armies.realm = v,
            ("army.province", U32(v)) => self.armies.province = v,
            ("army.size", U64(v)) => self.armies.size = v,
            ("realm.ruler", U32(v)) => self.realms.ruler = v,
            ("realm.heir", U32(v)) => self.realms.heir = v,
            ("character.realm", U32(v)) => self.characters.realm = v,
            ("character.parent", U32(v)) => self.characters.parent = v,
            ("character.name", U32(v)) => self.characters.name = v,
            ("character.born", F64(v)) => self.characters.born = v,
            ("character.died", U64(v)) => self.characters.died = v,
            ("character.crowned", U64(v)) => self.characters.crowned = v,
            ("character.succession", U8(v)) => self.characters.succession = v,
            ("character.martial", U8(v)) => self.characters.martial = v,
            ("character.greed", U8(v)) => self.characters.greed = v,
            ("character.ability", U8(v)) => self.characters.ability = v,
            ("province.occupier", U32(v)) => self.occupier = v,
            ("war.attacker", U32(v)) => self.wars.attacker = v,
            ("war.defender", U32(v)) => self.wars.defender = v,
            ("war.cause", U8(v)) => self.wars.cause = v,
            ("war.began", U64(v)) => self.wars.began = v,
            ("war.ended", U64(v)) => self.wars.ended = v,
            ("war.winner", U32(v)) => self.wars.winner = v,
            ("war.truce_until", U64(v)) => self.wars.truce_until = v,
            ("province.unrest", U64(v)) => self.unrest = v,
            ("province.uprising", U32(v)) => self.uprising = v,
            ("realm.broke_from", U32(v)) => self.realms.broke_from = v,
            ("army.kind", U16(v)) => self.armies.kind = v,
            ("army.home", U32(v)) => self.armies.home = v,
            ("army.role", U8(v)) => self.armies.role = v,
            ("army.group", U32(v)) => self.armies.group = v,
            ("army.dest", U32(v)) => self.armies.dest = v,
            ("army.next", U32(v)) => self.armies.next = v,
            ("army.days_left", F32(v)) => self.armies.days_left = v,
            ("army.siege_days", U32(v)) => self.armies.siege_days = v,
            ("war.target", U32(v)) => self.wars.target = v,
            ("war.attacker_dead", U64(v)) => self.wars.attacker_dead = v,
            ("war.defender_dead", U64(v)) => self.wars.defender_dead = v,
            ("war.attacker_lost", U32(v)) => self.wars.attacker_lost = v,
            ("war.defender_lost", U32(v)) => self.wars.defender_lost = v,
            ("relation.a", U32(v)) => self.relations.a = v,
            ("relation.b", U32(v)) => self.relations.b = v,
            ("relation.a_of_b", F32(v)) => self.relations.a_of_b = v,
            ("relation.b_of_a", F32(v)) => self.relations.b_of_a = v,
            _ => return wrong(),
        }
        Ok(())
    }

    /// Write `values` (as [`FieldData::gather`] gives them) at entities `at` of one field, in place:
    /// the cost follows the number of entities, not the field's size.
    pub fn scatter_field(&mut self, id: FieldId, at: &[u32], values: &FieldData) -> Result<(), String> {
        use FieldData::*;
        fn put<T: Copy>(v: &mut [T], at: &[u32], x: &[T]) -> Result<(), String> {
            for (&i, &x) in at.iter().zip(x) {
                *v.get_mut(i as usize).ok_or_else(|| format!("no entity {i}"))? = x;
            }
            Ok(())
        }
        let p = &mut self.provinces;
        let pops = &mut self.pops;
        let armies = &mut self.armies;
        let people = &mut self.characters;
        let wars = &mut self.wars;
        let relations = &mut self.relations;
        match (id.def().name, values) {
            ("cell.elevation", F32(x)) => put(&mut self.elevation, at, x),
            ("cell.temperature", F32(x)) => put(&mut self.temperature, at, x),
            ("cell.rainfall", F32(x)) => put(&mut self.rainfall, at, x),
            ("cell.biome", U8(x)) => {
                let x: Vec<Biome> = x.iter().map(|&b| biome::ALL.get(b as usize).copied().ok_or(format!("cell.biome: no biome {b}"))).collect::<Result<_, _>>()?;
                put(&mut self.biome, at, &x)
            }
            ("cell.drainage_receiver", U32(x)) => put(&mut self.downstream, at, x),
            ("cell.river_flow", U64(x)) => put(&mut self.river_flow, at, x),
            ("cell.food_yield", F64(x)) => put(&mut self.food_per_cell, at, x),
            ("cell.province", U32(x)) => put(&mut p.of_cell, at, x),
            ("province.cells", U32(x)) => put(&mut p.cells, at, x),
            ("province.area_km2", F64(x)) => put(&mut p.area_km2, at, x),
            ("province.center", Dir(x)) => put(&mut p.center, at, x),
            ("province.lat", F32(x)) => put(&mut p.lat, at, x),
            ("province.food_capacity", U64(x)) => put(&mut p.food_capacity, at, x),
            ("province.stock", U64(x)) => put(&mut self.stock, at, x),
            ("province.starved_this_year", U64(x)) => put(&mut self.starved_this_year, at, x),
            ("pop.province", U32(x)) => put(&mut pops.province, at, x),
            ("pop.species", U16(x)) => put(&mut pops.species, at, x),
            ("pop.culture", U16(x)) => put(&mut pops.culture, at, x),
            ("pop.religion", U16(x)) => put(&mut pops.religion, at, x),
            ("pop.class", U8(x)) => put(&mut pops.class, at, x),
            ("pop.size", U64(x)) => put(&mut pops.size, at, x),
            ("province.rain_shortfall", U64(x)) => put(&mut self.rain_shortfall, at, x),
            ("cell.temperature_set", F32(x)) => put(&mut self.temperature_set, at, x),
            ("cell.rainfall_set", F32(x)) => put(&mut self.rainfall_set, at, x),
            ("cell.biome_set", U8(x)) => put(&mut self.biome_set, at, x),
            ("cell.spring", U64(x)) => put(&mut self.spring, at, x),
            ("province.grazing_capacity", U64(x)) => put(&mut p.grazing_capacity, at, x),
            ("province.forest_km2", F64(x)) => put(&mut p.forest_km2, at, x),
            ("province.coast_km", F64(x)) => put(&mut p.coast_km, at, x),
            ("province.river_km", F64(x)) => put(&mut p.river_km, at, x),
            ("province.upland_km2", F64(x)) => put(&mut p.upland_km2, at, x),
            ("province.ore_km2", F64(x)) => put(&mut p.ore_km2, at, x),
            ("province.price", U64(x)) => put(&mut self.price, at, x),
            ("province.workers", U64(x)) => put(&mut self.workers, at, x),
            ("province.owner", U32(x)) => put(&mut self.owner, at, x),
            ("realm.capital", U32(x)) => put(&mut self.realms.capital, at, x),
            ("realm.stock", U64(x)) => put(&mut self.realm_stock, at, x),
            ("army.realm", U32(x)) => put(&mut armies.realm, at, x),
            ("army.province", U32(x)) => put(&mut armies.province, at, x),
            ("army.size", U64(x)) => put(&mut armies.size, at, x),
            ("realm.ruler", U32(x)) => put(&mut self.realms.ruler, at, x),
            ("realm.heir", U32(x)) => put(&mut self.realms.heir, at, x),
            ("character.realm", U32(x)) => put(&mut people.realm, at, x),
            ("character.parent", U32(x)) => put(&mut people.parent, at, x),
            ("character.name", U32(x)) => put(&mut people.name, at, x),
            ("character.born", F64(x)) => put(&mut people.born, at, x),
            ("character.died", U64(x)) => put(&mut people.died, at, x),
            ("character.crowned", U64(x)) => put(&mut people.crowned, at, x),
            ("character.succession", U8(x)) => put(&mut people.succession, at, x),
            ("character.martial", U8(x)) => put(&mut people.martial, at, x),
            ("character.greed", U8(x)) => put(&mut people.greed, at, x),
            ("character.ability", U8(x)) => put(&mut people.ability, at, x),
            ("province.occupier", U32(x)) => put(&mut self.occupier, at, x),
            ("war.attacker", U32(x)) => put(&mut wars.attacker, at, x),
            ("war.defender", U32(x)) => put(&mut wars.defender, at, x),
            ("war.cause", U8(x)) => put(&mut wars.cause, at, x),
            ("war.began", U64(x)) => put(&mut wars.began, at, x),
            ("war.ended", U64(x)) => put(&mut wars.ended, at, x),
            ("war.winner", U32(x)) => put(&mut wars.winner, at, x),
            ("war.truce_until", U64(x)) => put(&mut wars.truce_until, at, x),
            ("province.unrest", U64(x)) => put(&mut self.unrest, at, x),
            ("province.uprising", U32(x)) => put(&mut self.uprising, at, x),
            ("realm.broke_from", U32(x)) => put(&mut self.realms.broke_from, at, x),
            ("army.kind", U16(x)) => put(&mut armies.kind, at, x),
            ("army.home", U32(x)) => put(&mut armies.home, at, x),
            ("army.role", U8(x)) => put(&mut armies.role, at, x),
            ("army.group", U32(x)) => put(&mut armies.group, at, x),
            ("army.dest", U32(x)) => put(&mut armies.dest, at, x),
            ("army.next", U32(x)) => put(&mut armies.next, at, x),
            ("army.days_left", F32(x)) => put(&mut armies.days_left, at, x),
            ("army.siege_days", U32(x)) => put(&mut armies.siege_days, at, x),
            ("war.target", U32(x)) => put(&mut wars.target, at, x),
            ("war.attacker_dead", U64(x)) => put(&mut wars.attacker_dead, at, x),
            ("war.defender_dead", U64(x)) => put(&mut wars.defender_dead, at, x),
            ("war.attacker_lost", U32(x)) => put(&mut wars.attacker_lost, at, x),
            ("war.defender_lost", U32(x)) => put(&mut wars.defender_lost, at, x),
            ("relation.a", U32(x)) => put(&mut relations.a, at, x),
            ("relation.b", U32(x)) => put(&mut relations.b, at, x),
            ("relation.a_of_b", F32(x)) => put(&mut relations.a_of_b, at, x),
            ("relation.b_of_a", F32(x)) => put(&mut relations.b_of_a, at, x),
            (name, _) => Err(format!("{name}: wrong value type")),
        }
    }

    /// Give the province values that change as the world runs an entry for every province, after an
    /// edit added provinces. New provinces start with nothing. (Add new running province fields
    /// here.)
    pub fn grow_province_fields(&mut self) {
        let n = self.provinces.count;
        for v in [&mut self.starved_this_year, &mut self.rain_shortfall, &mut self.unrest] {
            if v.len() < n {
                v.resize(n, 0);
            }
        }
        let goods = n * self.goods.stride();
        for v in [&mut self.stock, &mut self.price, &mut self.workers] {
            if v.len() < goods {
                v.resize(goods, 0);
            }
        }
        // A new province belongs to nobody, nobody occupies it and nobody rises up in it, until
        // whoever made it says otherwise.
        for v in [&mut self.owner, &mut self.occupier, &mut self.uprising] {
            if v.len() < n {
                v.resize(n, crate::grid::NONE);
            }
        }
    }

    /// Give every realm the values other modules than the one that makes realms keep for it: a
    /// row of stores (empty), a ruler and an heir (nobody); and the realm it broke away from
    /// (none), for realms made by hand. The engine calls it after every stage and edit, so whoever
    /// adds a realm needn't own those, and they are there with their owners off.
    pub fn fit_realm_rows(&mut self) {
        let n = self.realms.count();
        let want = n * self.goods.stride();
        if self.realm_stock.len() < want {
            self.realm_stock.resize(want, 0);
        }
        for v in [&mut self.realms.ruler, &mut self.realms.heir, &mut self.realms.broke_from] {
            if v.len() < n {
                v.resize(n, crate::grid::NONE);
            }
        }
    }

    /// The conserved quantities this world counts: people, and each class of goods it keeps.
    pub fn quantities(&self) -> Vec<Quantity> {
        std::iter::once(Quantity::People).chain(self.goods.quantities()).collect()
    }

    /// Province `p`'s stored amount of the good named `name`, if the world keeps that good on its own.
    pub fn stock_of(&self, p: usize, name: &str) -> Option<u64> {
        self.goods.column(name).and_then(|c| self.stock.get(self.goods.index(p, c)).copied())
    }

    /// How much of the good or class named `name` province `p` has stored, however finely it is
    /// kept: a class sums its goods. `None` when the tree has no such node, or it is kept only as
    /// part of something bigger.
    pub fn stock_under(&self, p: usize, name: &str) -> Option<u64> {
        let k = self.goods.stride();
        let row = self.stock.get(p * k..(p + 1) * k)?;
        self.goods.sum_under(row, self.goods.tree.find(name)?)
    }

    /// Check that province, pop, realm, army, character, war and relation fields agree on their lengths (after a
    /// load).
    pub fn check_lengths(&self) -> Result<(), String> {
        let n = self.provinces.count;
        let pops = self.pops.size.len();
        for id in FieldId::all() {
            let (def, len) = (id.def(), self.field(id).len());
            let want = match def.tier {
                Tier::Province => n,
                Tier::ProvinceGood => n * self.goods.stride(),
                Tier::Pop => pops,
                Tier::Realm => self.realms.count(),
                Tier::RealmGood => self.realms.count() * self.goods.stride(),
                Tier::Army => self.armies.count(),
                Tier::Character => self.characters.count(),
                Tier::War => self.wars.count(),
                Tier::Relation => self.relations.count(),
                Tier::Cell => continue,
            };
            if len != want {
                return Err(format!("{}: {len} values, expected {want}", def.name));
            }
        }
        Ok(())
    }

    /// Bit-exact hash of every field.
    pub fn fingerprint(&self, h: &mut Fingerprint) {
        for id in FieldId::all() {
            self.field(id).fingerprint(h);
        }
    }

    /// Fill fields no active module owns with their documented defaults.
    pub fn fill_defaults(&mut self, owned: &[bool]) {
        for (k, def) in FIELDS.iter().enumerate() {
            if owned[k] {
                continue;
            }
            let id = FieldId(k as u16);
            let rows = if matches!(def.tier, Tier::Realm | Tier::RealmGood) { self.realms.count() } else { self.provinces.count };
            let per_row = if def.tier.per_good() { self.goods.stride() } else { 1 };
            let data = match (def.default, self.field(id)) {
                (crate::fields::Default::Zero, FieldData::U64(_)) => FieldData::U64(vec![0; rows * per_row]),
                (crate::fields::Default::NoOne, FieldData::U32(_)) => FieldData::U32(vec![crate::grid::NONE; rows]),
                (crate::fields::Default::Empty, d) => match d {
                    FieldData::U64(_) => FieldData::U64(Vec::new()),
                    FieldData::U32(_) => FieldData::U32(Vec::new()),
                    FieldData::U16(_) => FieldData::U16(Vec::new()),
                    FieldData::U8(_) => FieldData::U8(Vec::new()),
                    FieldData::F64(_) => FieldData::F64(Vec::new()),
                    other => other,
                },
                // Required fields are always owned in a valid configuration.
                (_, d) => d,
            };
            self.set_field(id, data).expect("defaults have the field's own type");
        }
    }
}

/// Fields as they stood when the day began: after the day's edits, before its events were
/// delivered. The engine copies the fields that the modules due today list in their manifests'
/// `reads_earlier`, on such a day only, and those modules read the copy instead of the field. So a
/// module can read what is written after it runs (or what is written by those who read it) without
/// a loop in the day's order.
///
/// Not a field: not saved, not fingerprinted. Saves and checkpoints are taken between days, and the
/// copy is made again from them when the day runs, so a loaded world reads the same copy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Earlier {
    /// The day it was copied.
    pub day: u64,
    fields: Vec<(FieldId, FieldData)>,
}

impl Earlier {
    pub fn copy(w: &World, day: u64, fields: &[FieldId]) -> Earlier {
        Earlier { day, fields: fields.iter().map(|&id| (id, w.field(id))).collect() }
    }

    /// The copy of the field named `name`. Panics if it wasn't copied today: it isn't in the
    /// reader's `reads_earlier`.
    pub fn get(&self, name: &str) -> &FieldData {
        let id = crate::fields::known(name);
        self.fields.iter().find(|f| f.0 == id).map(|f| &f.1).unwrap_or_else(|| panic!("{name} is not read as the day began (add it to the manifest's reads_earlier)"))
    }

    pub fn u32s(&self, name: &str) -> &[u32] {
        match self.get(name) {
            FieldData::U32(v) => v,
            _ => panic!("{name} doesn't hold u32 values"),
        }
    }

    pub fn u64s(&self, name: &str) -> &[u64] {
        match self.get(name) {
            FieldData::U64(v) => v,
            _ => panic!("{name} doesn't hold u64 values"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// FNV-1a, 64-bit: a stable hash for comparing worlds bit for bit.
pub struct Fingerprint(pub u64);

impl Default for Fingerprint {
    fn default() -> Self {
        Fingerprint(0xCBF2_9CE4_8422_2325)
    }
}

impl Fingerprint {
    pub fn bytes(&mut self, b: &[u8]) {
        // Length first, so moving bytes between neighbouring fields changes the hash.
        for x in (b.len() as u64).to_le_bytes().iter().chain(b) {
            self.0 ^= *x as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    pub fn u32s(&mut self, v: &[u32]) {
        self.bytes(&v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>());
    }
    pub fn u64s(&mut self, v: &[u64]) {
        self.bytes(&v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>());
    }
    pub fn f32s(&mut self, v: &[f32]) {
        self.u32s(&v.iter().map(|x| x.to_bits()).collect::<Vec<_>>());
    }
    pub fn f64s(&mut self, v: &[f64]) {
        self.u64s(&v.iter().map(|x| x.to_bits()).collect::<Vec<_>>());
    }
}
