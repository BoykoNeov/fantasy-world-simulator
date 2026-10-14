//! The world: every shared field, as typed arrays keyed by cell, province or pop.
//!
//! Generation modules fill the cell and province fields once, in the order their manifests give
//! (by default: terrain → climate → biomes → rivers → farmland → provinces). Runtime modules then
//! own the food stores and the pops. Every field can also be read and written by name, as
//! [`FieldData`], for contracts, fingerprints and saves.

use crate::biome::{self, Biome};
use crate::config::ModuleConfig;
use crate::fields::{FieldData, FieldId, Tier, FIELDS};
use crate::grid::Grid;
use crate::hydrology::Drainage;
use crate::module::{plan, Manifest, Module, Slot};
use crate::pops::Pops;
use crate::provinces::Provinces;
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
    pub drainage: Drainage,
    /// Water passing through each cell per year, thousand m³.
    pub river_flow: Vec<u64>,
    /// Food a cell yields in an average year, rations (field `cell.food_yield`).
    pub food_per_cell: Vec<f64>,
    pub provinces: Provinces,
    /// Stored food per province, rations.
    pub food_stock: Vec<u64>,
    /// People who starved in each province since the last year start.
    pub starved_this_year: Vec<u64>,
    pub pops: Pops,
    /// Share of the province's normal rain that is missing, per mille (0 = a normal year).
    pub rain_shortfall: Vec<u64>,
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
            drainage: Drainage::default(),
            river_flow: Vec::new(),
            food_per_cell: Vec::new(),
            provinces: Provinces::default(),
            food_stock: Vec::new(),
            starved_this_year: Vec::new(),
            pops: Pops::default(),
            rain_shortfall: Vec::new(),
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
        w
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
            "cell.drainage_receiver" => U32(self.drainage.receiver.clone()),
            "cell.drainage_order" => U32(self.drainage.order.clone()),
            "cell.river_flow" => U64(self.river_flow.clone()),
            "cell.food_yield" => F64(self.food_per_cell.clone()),
            "cell.province" => U32(p.of_cell.clone()),
            "province.cells" => U32(p.cells.clone()),
            "province.area_km2" => F64(p.area_km2.clone()),
            "province.center" => Dir(p.center.clone()),
            "province.lat" => F32(p.lat.clone()),
            "province.food_capacity" => U64(p.food_capacity.clone()),
            "province.food_stock" => U64(self.food_stock.clone()),
            "province.starved_this_year" => U64(self.starved_this_year.clone()),
            "pop.province" => U32(self.pops.province.clone()),
            "pop.species" => U16(self.pops.species.clone()),
            "pop.culture" => U16(self.pops.culture.clone()),
            "pop.religion" => U16(self.pops.religion.clone()),
            "pop.class" => U8(self.pops.class.clone()),
            "pop.size" => U64(self.pops.size.clone()),
            "province.rain_shortfall" => U64(self.rain_shortfall.clone()),
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
        };
        if let Some(n) = expect.filter(|&n| n != data.len() && def.name != "cell.drainage_order") {
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
            ("cell.drainage_receiver", U32(v)) => self.drainage.receiver = v,
            ("cell.drainage_order", U32(v)) => self.drainage.order = v,
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
            ("province.food_stock", U64(v)) => self.food_stock = v,
            ("province.starved_this_year", U64(v)) => self.starved_this_year = v,
            ("pop.province", U32(v)) => self.pops.province = v,
            ("pop.species", U16(v)) => self.pops.species = v,
            ("pop.culture", U16(v)) => self.pops.culture = v,
            ("pop.religion", U16(v)) => self.pops.religion = v,
            ("pop.class", U8(v)) => self.pops.class = v,
            ("pop.size", U64(v)) => self.pops.size = v,
            ("province.rain_shortfall", U64(v)) => self.rain_shortfall = v,
            _ => return wrong(),
        }
        Ok(())
    }

    /// Give the province values that change as the world runs an entry for every province, after an
    /// edit added provinces. New provinces start with nothing. (Add new running province fields
    /// here.)
    pub fn grow_province_fields(&mut self) {
        let n = self.provinces.count;
        for v in [&mut self.food_stock, &mut self.starved_this_year, &mut self.rain_shortfall] {
            if v.len() < n {
                v.resize(n, 0);
            }
        }
    }

    /// Check that province and pop fields agree on their lengths (after a load).
    pub fn check_lengths(&self) -> Result<(), String> {
        let n = self.provinces.count;
        let pops = self.pops.size.len();
        for id in FieldId::all() {
            let (def, len) = (id.def(), self.field(id).len());
            let want = match def.tier {
                Tier::Province => n,
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
            let data = match (def.default, self.field(id)) {
                (crate::fields::Default::Zero, FieldData::U64(_)) => FieldData::U64(vec![0; self.provinces.count]),
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
