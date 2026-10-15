//! The world: every shared field, as typed arrays keyed by cell, province or pop.
//!
//! Generation modules fill the cell and province fields once, in the order their manifests give
//! (by default: terrain → climate → biomes → rivers → farmland → provinces). Runtime modules then
//! own the stores of goods and the pops. Every field can also be read and written by name, as
//! [`FieldData`], for contracts, fingerprints and saves.

use crate::biome::{self, Biome};
use crate::config::ModuleConfig;
use crate::fields::{FieldData, FieldId, Goods, Quantity, Tier, FIELDS};
use crate::grid::Grid;
use crate::module::{plan, Manifest, Module, Slot};
use crate::pops::Pops;
use crate::provinces::Provinces;
use crate::routes::Routes;
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

/// How much more food a cell yields when a river runs through it.
const RIVER_FOOD_BONUS: f64 = 1.6;

/// Farmland v0: food per cell from its biome, its area and whether a river runs through it.
pub struct Fertility;

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
                let river = if w.river_flow[i] >= RIVER_MIN_FLOW && w.elevation[i] >= 0.0 { RIVER_FOOD_BONUS } else { 1.0 };
                w.biome[i].food_per_km2() * w.grid.area[i] * km2_per_sr * river
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
    /// Stored goods, per province per good kept (field `province.stock`; see [`World::goods`]).
    pub stock: Vec<u64>,
    /// The goods tree, and which of its goods the per-good fields keep. Set from the configuration
    /// when the world is made; it never changes.
    pub goods: Arc<Goods>,
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
            stock: Vec::new(),
            goods: Arc::new(Goods::default()),
            starved_this_year: Vec::new(),
            pops: Pops::default(),
            rain_shortfall: Vec::new(),
            temperature_set: Vec::new(),
            rainfall_set: Vec::new(),
            biome_set: Vec::new(),
            spring: Vec::new(),
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
            other => unreachable!("field {other} has no storage"),
        }
    }

    /// Replace one field's values. Refuses data of the wrong type or length.
    pub fn set_field(&mut self, id: FieldId, data: FieldData) -> Result<(), String> {
        use FieldData::*;
        let def = id.def();
        let expect = match def.tier {
            Tier::Cell => Some(self.grid.cells()),
            // The province count is set by the province fields themselves; pops vary.
            Tier::Province | Tier::Pop => None,
            Tier::ProvinceGood => {
                let k = self.goods.stride();
                if !data.len().is_multiple_of(k) {
                    return Err(format!("{}: {} values, not a whole number of provinces of {k} goods", def.name, data.len()));
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
            (name, _) => Err(format!("{name}: wrong value type")),
        }
    }

    /// Give the province values that change as the world runs an entry for every province, after an
    /// edit added provinces. New provinces start with nothing. (Add new running province fields
    /// here.)
    pub fn grow_province_fields(&mut self) {
        let n = self.provinces.count;
        for v in [&mut self.starved_this_year, &mut self.rain_shortfall] {
            if v.len() < n {
                v.resize(n, 0);
            }
        }
        let goods = n * self.goods.stride();
        if self.stock.len() < goods {
            self.stock.resize(goods, 0);
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

    /// Check that province and pop fields agree on their lengths (after a load).
    pub fn check_lengths(&self) -> Result<(), String> {
        let n = self.provinces.count;
        let pops = self.pops.size.len();
        for id in FieldId::all() {
            let (def, len) = (id.def(), self.field(id).len());
            let want = match def.tier {
                Tier::Province => n,
                Tier::ProvinceGood => n * self.goods.stride(),
                Tier::Pop => pops,
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
            let per_province = if def.tier == Tier::ProvinceGood { self.goods.stride() } else { 1 };
            let data = match (def.default, self.field(id)) {
                (crate::fields::Default::Zero, FieldData::U64(_)) => FieldData::U64(vec![0; self.provinces.count * per_province]),
                (crate::fields::Default::Empty, d) => match d {
                    FieldData::U64(_) => FieldData::U64(Vec::new()),
                    FieldData::U32(_) => FieldData::U32(Vec::new()),
                    FieldData::U16(_) => FieldData::U16(Vec::new()),
                    FieldData::U8(_) => FieldData::U8(Vec::new()),
                    other => other,
                },
                // Required fields are always owned in a valid configuration.
                (_, d) => d,
            };
            self.set_field(id, data).expect("defaults have the field's own type");
        }
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
