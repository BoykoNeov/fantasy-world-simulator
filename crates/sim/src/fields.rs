//! The shared fields: every value modules may read, by name, with its tier, unit, bounds and what
//! readers get when no active module owns it.
//!
//! Modules declare fields by name in their manifests. The engine uses this table to validate a
//! configuration, check field contracts, check conservation, fingerprint the world and save it.

use crate::world::{Fingerprint, World};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    Cell,
    Province,
    /// One entry per pop.
    Pop,
}

/// Quantities that are stored as whole numbers and must balance exactly every day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Quantity {
    People,
    /// Rations: one feeds one person for one day.
    Food,
}

pub const QUANTITIES: [Quantity; 2] = [Quantity::People, Quantity::Food];

/// What readers find in a field that no active module owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Default {
    /// Nothing sensible exists: a configuration that reads it without an owner is refused.
    Required,
    /// Zero for every province.
    Zero,
    /// No entries (pop fields: no pops).
    Empty,
}

/// Index into [`FIELDS`]. Stable within a build; saves store field names, not ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FieldId(pub u16);

pub struct FieldDef {
    pub name: &'static str,
    pub tier: Tier,
    pub unit: &'static str,
    /// Field contract: values outside `[min, max]` are clamped and reported.
    pub min: f64,
    pub max: f64,
    pub conserved: Option<Quantity>,
    pub default: Default,
    /// Taxonomy depth the values are stored at: 0 is the top of the tree. Every field is flat for
    /// now; saves record the depth so a later version change can convert between depths.
    pub depth: u8,
}

const fn def(name: &'static str, tier: Tier, unit: &'static str, min: f64, max: f64) -> FieldDef {
    FieldDef { name, tier, unit, min, max, conserved: None, default: Default::Required, depth: 0 }
}

const UNBOUNDED: f64 = f64::INFINITY;
/// Bounds for conserved counts: anything near this is a runaway, not a world.
const COUNT_MAX: f64 = 1e15;

/// Every shared field, in a fixed order (fingerprints and saves walk it in this order).
pub const FIELDS: &[FieldDef] = &[
    def("cell.elevation", Tier::Cell, "m, below 0 is sea", -12_000.0, 12_000.0),
    def("cell.temperature", Tier::Cell, "°C, annual mean", -90.0, 70.0),
    def("cell.rainfall", Tier::Cell, "mm a year", 0.0, 20_000.0),
    def("cell.biome", Tier::Cell, "biome id", 0.0, (crate::biome::ALL.len() - 1) as f64),
    def("cell.drainage_receiver", Tier::Cell, "downstream cell", -UNBOUNDED, UNBOUNDED),
    def("cell.river_flow", Tier::Cell, "thousand m³ a year", 0.0, UNBOUNDED),
    def("cell.food_yield", Tier::Cell, "rations a year", 0.0, UNBOUNDED),
    def("cell.province", Tier::Cell, "province id", -UNBOUNDED, UNBOUNDED),
    def("province.cells", Tier::Province, "cells", 0.0, UNBOUNDED),
    def("province.area_km2", Tier::Province, "km²", 0.0, UNBOUNDED),
    def("province.center", Tier::Province, "unit vector", -1.0, 1.0),
    def("province.lat", Tier::Province, "radians", -1.6, 1.6),
    def("province.food_capacity", Tier::Province, "rations a year", 0.0, UNBOUNDED),
    FieldDef { conserved: Some(Quantity::Food), default: Default::Zero, ..def("province.food_stock", Tier::Province, "rations", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Zero, ..def("province.starved_this_year", Tier::Province, "people", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Empty, ..def("pop.province", Tier::Pop, "province id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("pop.species", Tier::Pop, "species id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("pop.culture", Tier::Pop, "culture id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("pop.religion", Tier::Pop, "religion id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("pop.class", Tier::Pop, "class id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { conserved: Some(Quantity::People), default: Default::Empty, ..def("pop.size", Tier::Pop, "people", 0.0, COUNT_MAX) },
    // Appended after the step-3 fields: saved events refer to fields by position.
    FieldDef { default: Default::Zero, ..def("province.rain_shortfall", Tier::Province, "‰ of normal rain missing", 0.0, 1000.0) },
];

pub fn id(name: &str) -> Option<FieldId> {
    FIELDS.iter().position(|f| f.name == name).map(|i| FieldId(i as u16))
}

/// [`id`] for names known at compile time; panics on a typo.
pub fn known(name: &str) -> FieldId {
    id(name).unwrap_or_else(|| panic!("no field named {name}"))
}

impl FieldId {
    pub fn def(self) -> &'static FieldDef {
        &FIELDS[self.0 as usize]
    }
    pub fn all() -> impl Iterator<Item = FieldId> {
        (0..FIELDS.len() as u16).map(FieldId)
    }
}

/// One field's values, owned. Used for saves, contracts and fingerprints, never in hot loops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FieldData {
    F32(Vec<f32>),
    F64(Vec<f64>),
    U64(Vec<u64>),
    U32(Vec<u32>),
    U16(Vec<u16>),
    U8(Vec<u8>),
    Dir(Vec<[f64; 3]>),
}

impl FieldData {
    pub fn len(&self) -> usize {
        match self {
            FieldData::F32(v) => v.len(),
            FieldData::F64(v) => v.len(),
            FieldData::U64(v) => v.len(),
            FieldData::U32(v) => v.len(),
            FieldData::U16(v) => v.len(),
            FieldData::U8(v) => v.len(),
            FieldData::Dir(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn fingerprint(&self, h: &mut Fingerprint) {
        match self {
            FieldData::F32(v) => h.f32s(v),
            FieldData::F64(v) => h.f64s(v),
            FieldData::U64(v) => h.u64s(v),
            FieldData::U32(v) => h.u32s(v),
            FieldData::U16(v) => h.bytes(&v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>()),
            FieldData::U8(v) => h.bytes(v),
            FieldData::Dir(v) => h.f64s(&v.iter().flatten().copied().collect::<Vec<_>>()),
        }
    }

    /// Clamp every value into `[min, max]`. Returns `(entity, value before, value after)` for each
    /// value that moved. Vectors are checked per component.
    pub fn clamp(&mut self, min: f64, max: f64) -> Vec<(u32, f64, f64)> {
        let mut out = Vec::new();
        fn go<T: Copy>(v: &mut [T], min: f64, max: f64, out: &mut Vec<(u32, f64, f64)>, to: impl Fn(T) -> f64, from: impl Fn(f64) -> T) {
            for (i, x) in v.iter_mut().enumerate() {
                let f = to(*x);
                // NaN fails both comparisons; it is reported and replaced by the lower bound.
                if !(f >= min && f <= max) {
                    let c = if f > max { max } else { min };
                    out.push((i as u32, f, c));
                    *x = from(c);
                }
            }
        }
        match self {
            FieldData::F32(v) => go(v, min, max, &mut out, |x| x as f64, |x| x as f32),
            FieldData::F64(v) => go(v, min, max, &mut out, |x| x, |x| x),
            FieldData::U64(v) => go(v, min, max, &mut out, |x| x as f64, |x| x as u64),
            FieldData::U32(v) => go(v, min, max, &mut out, |x| x as f64, |x| x as u32),
            FieldData::U16(v) => go(v, min, max, &mut out, |x| x as f64, |x| x as u16),
            FieldData::U8(v) => go(v, min, max, &mut out, |x| x as f64, |x| x as u8),
            FieldData::Dir(v) => {
                for (i, d) in v.iter_mut().enumerate() {
                    let mut c = *d;
                    go(&mut c, min, max, &mut Vec::new(), |x| x, |x| x);
                    if c != *d {
                        out.push((i as u32, d.iter().map(|x| x.abs()).fold(0.0, f64::max), max));
                        *d = c;
                    }
                }
            }
        }
        out
    }

    /// One value as a number, for display. A direction shows its first component.
    pub fn value(&self, i: usize) -> f64 {
        match self {
            FieldData::F32(v) => v[i] as f64,
            FieldData::F64(v) => v[i],
            FieldData::U64(v) => v[i] as f64,
            FieldData::U32(v) => v[i] as f64,
            FieldData::U16(v) => v[i] as f64,
            FieldData::U8(v) => v[i] as f64,
            FieldData::Dir(v) => v[i][0],
        }
    }

    /// Entities whose value differs from `new`'s, bit for bit. Entities only `new` has count as
    /// changed.
    pub fn differences(&self, new: &FieldData) -> Vec<u32> {
        fn go<T: PartialEq>(a: &[T], b: &[T]) -> Vec<u32> {
            (0..b.len()).filter(|&i| a.get(i) != Some(&b[i])).map(|i| i as u32).collect()
        }
        // Floats compare by bits, so a NaN that stays NaN is not a change.
        let bits32 = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        let bits64 = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        match (self, new) {
            (FieldData::F32(a), FieldData::F32(b)) => go(&bits32(a), &bits32(b)),
            (FieldData::F64(a), FieldData::F64(b)) => go(&bits64(a), &bits64(b)),
            (FieldData::U64(a), FieldData::U64(b)) => go(a, b),
            (FieldData::U32(a), FieldData::U32(b)) => go(a, b),
            (FieldData::U16(a), FieldData::U16(b)) => go(a, b),
            (FieldData::U8(a), FieldData::U8(b)) => go(a, b),
            (FieldData::Dir(a), FieldData::Dir(b)) => go(a, b),
            _ => (0..new.len() as u32).collect(),
        }
    }

    /// The values at entities `at`, in that order.
    pub fn gather(&self, at: &[u32]) -> FieldData {
        fn go<T: Copy>(v: &[T], at: &[u32]) -> Vec<T> {
            at.iter().map(|&i| v[i as usize]).collect()
        }
        match self {
            FieldData::F32(v) => FieldData::F32(go(v, at)),
            FieldData::F64(v) => FieldData::F64(go(v, at)),
            FieldData::U64(v) => FieldData::U64(go(v, at)),
            FieldData::U32(v) => FieldData::U32(go(v, at)),
            FieldData::U16(v) => FieldData::U16(go(v, at)),
            FieldData::U8(v) => FieldData::U8(go(v, at)),
            FieldData::Dir(v) => FieldData::Dir(go(v, at)),
        }
    }

    /// Write `values` (as [`FieldData::gather`] returns them) back at entities `at`.
    pub fn scatter(&mut self, at: &[u32], values: &FieldData) {
        fn go<T: Copy>(v: &mut [T], at: &[u32], x: &[T]) {
            for (&i, &x) in at.iter().zip(x) {
                v[i as usize] = x;
            }
        }
        match (self, values) {
            (FieldData::F32(v), FieldData::F32(x)) => go(v, at, x),
            (FieldData::F64(v), FieldData::F64(x)) => go(v, at, x),
            (FieldData::U64(v), FieldData::U64(x)) => go(v, at, x),
            (FieldData::U32(v), FieldData::U32(x)) => go(v, at, x),
            (FieldData::U16(v), FieldData::U16(x)) => go(v, at, x),
            (FieldData::U8(v), FieldData::U8(x)) => go(v, at, x),
            (FieldData::Dir(v), FieldData::Dir(x)) => go(v, at, x),
            _ => panic!("scatter: the values are of another type"),
        }
    }

    /// Sum of a conserved field. Only whole-number fields can be conserved.
    pub fn total(&self) -> u64 {
        match self {
            FieldData::U64(v) => v.iter().sum(),
            other => panic!("conserved fields are u64, found {:?}", std::mem::discriminant(other)),
        }
    }
}

/// Exact total of a conserved quantity, read from the field that holds it.
pub fn quantity_total(world: &World, q: Quantity) -> u64 {
    match q {
        Quantity::People => world.pops.size.iter().sum(),
        Quantity::Food => world.food_stock.iter().sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_conserved_fields_are_whole_numbers() {
        for (i, f) in FIELDS.iter().enumerate() {
            assert_eq!(id(f.name), Some(FieldId(i as u16)), "{} is listed twice", f.name);
            assert!(f.min <= f.max, "{}", f.name);
        }
        let conserved: Vec<_> = FIELDS.iter().filter_map(|f| f.conserved).collect();
        for q in QUANTITIES {
            assert_eq!(conserved.iter().filter(|&&c| c == q).count(), 1, "{q:?} needs exactly one field");
        }
    }

    #[test]
    fn clamping_reports_what_moved() {
        let mut d = FieldData::F32(vec![-5.0, 0.5, 9.0, f32::NAN]);
        let moved = d.clamp(0.0, 1.0);
        assert_eq!(moved.iter().map(|m| m.0).collect::<Vec<_>>(), vec![0, 2, 3]);
        assert_eq!(d, FieldData::F32(vec![0.0, 0.5, 1.0, 0.0]));
    }
}
