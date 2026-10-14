//! A generated planet: the cell fields and the provinces built on them.
//!
//! Generation order: terrain → climate → biomes → rivers → provinces. Each stage is a plain function
//! over field arrays for now; the module contract is extracted later, from these and the pops.

use crate::biome::{self, Biome};
use crate::grid::Grid;
use crate::hydrology::{self, Drainage};
use crate::provinces::{self, Provinces};
use crate::{climate, terrain};

/// World sizes from the design. Cell count and province count are separate settings; presets are
/// just starting points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
    Huge,
}

#[derive(Clone, Debug, PartialEq)]
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

/// Rivers carrying at least this much water a year (thousand m³; about 60 m³/s) make the land
/// around them more fertile and are drawn on the map.
pub const RIVER_MIN_FLOW: u64 = 2_000_000;

/// How much more food a cell yields when a river runs through it.
const RIVER_FOOD_BONUS: f64 = 1.6;

pub struct World {
    pub params: WorldParams,
    pub grid: Grid,
    /// Metres; below 0 is sea.
    pub elevation: Vec<f32>,
    /// Annual mean, °C.
    pub temperature: Vec<f32>,
    /// Annual total, mm.
    pub rainfall: Vec<f32>,
    pub coast_distance_km: Vec<f32>,
    pub biome: Vec<Biome>,
    pub drainage: Drainage,
    /// Water passing through each cell per year, thousand m³.
    pub river_flow: Vec<u64>,
    /// Food a cell yields in an average year, rations.
    pub food_per_cell: Vec<f64>,
    pub provinces: Provinces,
}

impl World {
    pub fn generate(params: &WorldParams) -> World {
        let p = params;
        let grid = Grid::new(p.n);
        let elevation = terrain::generate(&grid, p.seed, p.land_fraction);
        let c = climate::generate(&grid, &elevation, p.seed, p.radius_km);
        let biome = biome::generate(&elevation, &c.temperature, &c.rainfall);
        let drainage = hydrology::drainage(&grid, &elevation);
        let runoff = hydrology::runoff(&grid, &elevation, &c.temperature, &c.rainfall, p.radius_km);
        let river_flow = hydrology::accumulate(&drainage, &runoff);

        let km2_per_sr = p.radius_km * p.radius_km;
        let food_per_cell: Vec<f64> = (0..grid.cells())
            .map(|i| {
                let river = if river_flow[i] >= RIVER_MIN_FLOW && elevation[i] >= 0.0 { RIVER_FOOD_BONUS } else { 1.0 };
                biome[i].food_per_km2() * grid.area[i] * km2_per_sr * river
            })
            .collect();
        let of_cell = provinces::generate(&grid, &elevation, &food_per_cell, p.provinces, p.seed);
        let provinces = provinces::aggregate(&grid, of_cell, &food_per_cell, p.radius_km);

        World {
            params: p.clone(),
            grid,
            elevation,
            temperature: c.temperature,
            rainfall: c.rainfall,
            coast_distance_km: c.coast_distance_km,
            biome,
            drainage,
            river_flow,
            food_per_cell,
            provinces,
        }
    }

    /// Bit-exact hash of every generated field.
    pub fn fingerprint(&self, h: &mut Fingerprint) {
        h.f32s(&self.elevation);
        h.f32s(&self.temperature);
        h.f32s(&self.rainfall);
        h.f32s(&self.coast_distance_km);
        h.bytes(&self.biome.iter().map(|&b| b as u8).collect::<Vec<_>>());
        h.u32s(&self.drainage.receiver);
        h.u32s(&self.drainage.order);
        h.u64s(&self.river_flow);
        h.f64s(&self.food_per_cell);
        h.u32s(&self.provinces.of_cell);
        h.f64s(&self.provinces.area_km2);
        h.u64s(&self.provinces.food_capacity);
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
