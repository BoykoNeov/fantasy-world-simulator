//! The shared fields: every value modules may read, by name, with its tier, unit, bounds and what
//! readers get when no active module owns it.
//!
//! Modules declare fields by name in their manifests. The engine uses this table to validate a
//! configuration, check field contracts, check conservation, fingerprint the world and save it.

use crate::requests::Request;
use crate::taxonomy::{NodeId, Taxonomy};
use crate::world::{Fingerprint, World};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    Cell,
    Province,
    /// One entry per province per good the world keeps (see [`Goods`]): province `p`'s value of
    /// column `c` is entity `p * columns + c`.
    ProvinceGood,
    /// One entry per pop.
    Pop,
    /// One entry per realm (step 7). Realm numbers are never reused: a dissolved realm stays as an
    /// empty entry, as a merged province does.
    Realm,
    /// One entry per realm per good the world keeps, laid out like [`Tier::ProvinceGood`]: realm `r`'s
    /// value of column `c` is entity `r * columns + c`. A realm's own stores: its treasury is the
    /// goods under the money class.
    RealmGood,
    /// One entry per army (step 7); numbers are never reused, a disbanded army stays empty.
    Army,
    /// One entry per character: rulers and heirs (step 7). Numbers are never reused: the dead keep
    /// their entries, with the day they died.
    Character,
    /// One entry per war (step 7); numbers are never reused, an ended war keeps its entry.
    War,
    /// One entry per pair of realms that know each other (step 7, Diplomacy "treaties"): what each
    /// thinks of the other. Numbers are never reused.
    Relation,
    /// One entry per realm per tech of the research tree (Research "tree"), laid out like
    /// [`Tier::RealmGood`]: realm `r`'s value of tech `t` is entity `r * techs + t`, where `techs`
    /// is the length of [`World::techs`].
    RealmTech,
    /// One entry per culture (Culture "assimilation"). Numbers are never reused: a culture with
    /// nobody left keeps its entry.
    Culture,
    /// One entry per faith (Religion "conversion"). Numbers are never reused: a faith with nobody
    /// left keeps its entry.
    Religion,
}

impl Tier {
    /// Tiers whose values are kept per good, in the [`Goods`] columns.
    pub fn per_good(self) -> bool {
        matches!(self, Tier::ProvinceGood | Tier::RealmGood)
    }
}

/// Quantities that are stored as whole numbers and must balance exactly every day.
///
/// Saved as one number, 0 for people and 1 + the class's place for goods, so food (the shipped
/// tree's first class) keeps the number it had before goods had classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "u16", try_from = "u16")]
pub enum Quantity {
    People,
    /// Goods of one top-level class of the goods tree, by its place among the classes. Food is
    /// counted in rations: one feeds one person for one day.
    Goods(u8),
}

impl From<Quantity> for u16 {
    fn from(q: Quantity) -> u16 {
        match q {
            Quantity::People => 0,
            Quantity::Goods(c) => 1 + c as u16,
        }
    }
}

impl TryFrom<u16> for Quantity {
    type Error = String;
    fn try_from(n: u16) -> Result<Quantity, String> {
        match n {
            0 => Ok(Quantity::People),
            n => u8::try_from(n - 1).map(Quantity::Goods).map_err(|_| format!("no quantity {n}")),
        }
    }
}

impl Quantity {
    /// The fields that hold it: people live in pops or serve in armies; goods are stored by
    /// provinces or by realms (a treasury holds coins).
    pub fn fields(self) -> [FieldId; 2] {
        match self {
            Quantity::People => [known("pop.size"), known("army.size")],
            Quantity::Goods(_) => [known("province.stock"), known("realm.stock")],
        }
    }

    /// "people", "food".
    pub fn name(self, goods: &Goods) -> String {
        match self {
            Quantity::People => "people".into(),
            Quantity::Goods(c) => goods.tree.classes().get(c as usize).map_or_else(|| format!("goods class {c}"), |&n| goods.tree.name(n).into()),
        }
    }
}

/// Which goods a world keeps values of, in its [`Tier::ProvinceGood`] fields: one column per node
/// kept, province by province. The owner of those fields chooses the nodes (its manifest's
/// `goods`); when nobody owns them they are the top-level classes. All such fields share the
/// columns. Fixed for the world's life, like its module versions.
#[derive(Clone, Debug, PartialEq)]
pub struct Goods {
    pub tree: Taxonomy,
    pub columns: Vec<NodeId>,
    /// Each column's class, as its place among the tree's classes.
    class: Vec<u8>,
}

impl Goods {
    /// Keep the nodes named `kept`, in that order. Refuses names not in the tree, the root, and a
    /// node kept together with one above or below it (it would be counted twice).
    pub fn new(tree: Taxonomy, kept: &[&str]) -> Result<Goods, String> {
        let mut columns: Vec<NodeId> = Vec::new();
        for name in kept {
            let n = tree.find(name).ok_or_else(|| format!("the goods tree has no {name}"))?;
            if n == 0 {
                return Err(format!("{name} is the whole tree; keep its classes or goods instead"));
            }
            if let Some(&o) = columns.iter().find(|&&o| tree.is_under(n, o) || tree.is_under(o, n)) {
                return Err(format!("{name} and {} overlap: one is part of the other", tree.name(o)));
            }
            columns.push(n);
        }
        if columns.is_empty() {
            return Err("no goods kept".into());
        }
        let class = columns.iter().map(|&n| tree.class_of(n).expect("not the root") as u8).collect();
        Ok(Goods { tree, columns, class })
    }

    /// The top-level classes of `tree`: what readers find when nobody keeps goods.
    pub fn classes(tree: Taxonomy) -> Goods {
        let names: Vec<String> = tree.classes().iter().map(|&n| tree.name(n).to_string()).collect();
        Goods::new(tree, &names.iter().map(String::as_str).collect::<Vec<_>>()).expect("classes never overlap")
    }

    /// Columns per province.
    pub fn stride(&self) -> usize {
        self.columns.len()
    }

    /// The column that keeps the node named `name`, if one does.
    pub fn column(&self, name: &str) -> Option<usize> {
        let n = self.tree.find(name)?;
        self.columns.iter().position(|&c| c == n)
    }

    /// The columns that hold goods of `node`: the columns at or below it, in column order. `None`
    /// when a column above it keeps it only as part of something bigger (grain, when food is kept
    /// whole): then no amount of it alone is known.
    pub fn columns_under(&self, node: NodeId) -> Option<Vec<usize>> {
        if self.columns.iter().any(|&c| c != node && self.tree.is_under(node, c)) {
            return None;
        }
        Some((0..self.columns.len()).filter(|&c| self.tree.is_under(self.columns[c], node)).collect())
    }

    /// How much of `node` one province holds, from its row of a per-good field: the sum of the
    /// columns under it. `None` as for [`Goods::columns_under`].
    pub fn sum_under(&self, row: &[u64], node: NodeId) -> Option<u64> {
        Some(self.columns_under(node)?.into_iter().map(|c| row[c]).sum())
    }

    pub fn class_of_column(&self, c: usize) -> u8 {
        self.class[c]
    }

    /// Entity of province `p`'s value of column `c`.
    pub fn index(&self, p: usize, c: usize) -> usize {
        p * self.columns.len() + c
    }

    /// Province and column of an entity.
    pub fn split(&self, entity: usize) -> (usize, usize) {
        (entity / self.columns.len(), entity % self.columns.len())
    }

    /// The deepest column's depth, as saves record it.
    pub fn depth(&self) -> u8 {
        self.columns.iter().map(|&n| self.tree.depth(n)).max().unwrap_or(0)
    }

    pub fn names(&self) -> Vec<String> {
        self.columns.iter().map(|&n| self.tree.name(n).to_string()).collect()
    }

    /// The conserved quantities goods are counted in: each class with at least one column.
    pub fn quantities(&self) -> Vec<Quantity> {
        let mut q: Vec<u8> = self.class.clone();
        q.sort_unstable();
        q.dedup();
        q.into_iter().map(Quantity::Goods).collect()
    }

    /// The quantity what a request takes is counted in, if its field is conserved.
    pub fn quantity_asked(&self, r: &Request) -> Option<Quantity> {
        if !r.field.def().conserved {
            return None;
        }
        Some(match r.good {
            Some(node) => Quantity::Goods(self.tree.class_of(node).expect("the root is never asked for") as u8),
            None => Quantity::People,
        })
    }

    /// Exact totals of a conserved field's values, per quantity it holds.
    pub fn totals(&self, id: FieldId, data: &FieldData) -> Vec<(Quantity, u64)> {
        let FieldData::U64(v) = data else { panic!("conserved fields are u64") };
        if id.def().tier.per_good() {
            self.quantities().into_iter().map(|q| (q, self.sum_class(v, q))).collect()
        } else {
            vec![(Quantity::People, v.iter().sum())]
        }
    }

    fn sum_class(&self, v: &[u64], q: Quantity) -> u64 {
        let Quantity::Goods(class) = q else { return 0 };
        if self.class.iter().all(|&c| c == class) {
            return v.iter().sum();
        }
        let cols: Vec<usize> = (0..self.stride()).filter(|&c| self.class[c] == class).collect();
        v.chunks_exact(self.stride()).map(|row| cols.iter().map(|&c| row[c]).sum::<u64>()).sum()
    }
}

impl std::default::Default for Goods {
    fn default() -> Self {
        Goods::classes(Taxonomy::standard_goods())
    }
}

/// What readers find in a field that no active module owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Default {
    /// Nothing sensible exists: a configuration that reads it without an owner is refused.
    Required,
    /// Zero for every province (or realm, for a realm's field).
    Zero,
    /// No entries (pop fields: no pops).
    Empty,
    /// [`crate::grid::NONE`] for every entry: nobody (a province no realm owns, a realm with no
    /// ruler).
    NoOne,
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
    /// Whether the values are a conserved quantity: people, or goods (counted per class).
    pub conserved: bool,
    pub default: Default,
}

const fn def(name: &'static str, tier: Tier, unit: &'static str, min: f64, max: f64) -> FieldDef {
    FieldDef { name, tier, unit, min, max, conserved: false, default: Default::Required }
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
    // Stored goods, per good the world keeps (see `Goods`); food in rations. It took the place of
    // the step-3 food store, which held the same values for food alone.
    FieldDef { conserved: true, default: Default::Zero, ..def("province.stock", Tier::ProvinceGood, "of the good", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Zero, ..def("province.starved_this_year", Tier::Province, "people", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Empty, ..def("pop.province", Tier::Pop, "province id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("pop.species", Tier::Pop, "species id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("pop.culture", Tier::Pop, "culture id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("pop.religion", Tier::Pop, "religion id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("pop.class", Tier::Pop, "class id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { conserved: true, default: Default::Empty, ..def("pop.size", Tier::Pop, "people", 0.0, COUNT_MAX) },
    // Appended after the step-3 fields: saved events refer to fields by position.
    FieldDef { default: Default::Zero, ..def("province.rain_shortfall", Tier::Province, "‰ of normal rain missing", 0.0, 1000.0) },
    // Overrides painted by the editor, which the climate and biome modules respect (step 5).
    def("cell.temperature_set", Tier::Cell, "°C set by an edit, NaN where not set", -UNBOUNDED, UNBOUNDED),
    def("cell.rainfall_set", Tier::Cell, "mm a year set by an edit, NaN where not set", -UNBOUNDED, UNBOUNDED),
    def("cell.biome_set", Tier::Cell, "biome id set by an edit, 255 where not set", -UNBOUNDED, UNBOUNDED),
    def("cell.spring", Tier::Cell, "thousand m³ a year a drawn river's spring adds", 0.0, UNBOUNDED),
    // What each province's land offers, cached from its cells (step 6, for the markets economy).
    def("province.grazing_capacity", Tier::Province, "rations a year, of food_capacity, better from herds", 0.0, UNBOUNDED),
    def("province.forest_km2", Tier::Province, "km² of woodland", 0.0, UNBOUNDED),
    def("province.coast_km", Tier::Province, "km of shore", 0.0, UNBOUNDED),
    def("province.river_km", Tier::Province, "km of big river", 0.0, UNBOUNDED),
    def("province.upland_km2", Tier::Province, "km² of hills and mountains", 0.0, UNBOUNDED),
    def("province.ore_km2", Tier::Province, "km² of land with ore in reach", 0.0, UNBOUNDED),
    // The markets economy's local prices, and who works on what (step 6).
    FieldDef { default: Default::Zero, ..def("province.price", Tier::ProvinceGood, "thousandths of a coin per unit of the good", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Zero, ..def("province.workers", Tier::ProvinceGood, "people working on the good", 0.0, COUNT_MAX) },
    // Realms and armies (step 7).
    FieldDef { default: Default::NoOne, ..def("province.owner", Tier::Province, "realm id, none where no realm owns it", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("realm.capital", Tier::Realm, "province id, none for a dissolved realm", -UNBOUNDED, UNBOUNDED) },
    // A realm's stores per good kept; its coins are its treasury. Kept by the economics module, so
    // coins have one owner. It always has a row per realm: the engine adds rows of nothing as
    // realms appear (`World::fit_realm_rows`).
    FieldDef { conserved: true, default: Default::Zero, ..def("realm.stock", Tier::RealmGood, "of the good", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Empty, ..def("army.realm", Tier::Army, "realm id", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("army.province", Tier::Army, "province id where it stands", -UNBOUNDED, UNBOUNDED) },
    FieldDef { conserved: true, default: Default::Empty, ..def("army.size", Tier::Army, "soldiers", 0.0, COUNT_MAX) },
    // Rulers and heirs (step 7, Characters "rulers"). Rulers are not extra people: none of these
    // is counted in the people total.
    FieldDef { default: Default::NoOne, ..def("realm.ruler", Tier::Realm, "character id, none while the throne is empty", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::NoOne, ..def("realm.heir", Tier::Realm, "character id, none while there is no heir", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("character.realm", Tier::Character, "realm id they rule, are heir to, or last served", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("character.parent", Tier::Character, "character id, none if not known", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("character.name", Tier::Character, "name number (a child may share a forebear's)", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("character.born", Tier::Character, "day born, below 0 before the world's first day", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("character.died", Tier::Character, "day died, none while alive", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("character.crowned", Tier::Character, "day they became ruler, none if never", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("character.succession", Tier::Character, "how they came to the throne (see `Succession`)", 0.0, 5.0) },
    FieldDef { default: Default::Empty, ..def("character.martial", Tier::Character, "0 cautious to 100 warlike", 0.0, 100.0) },
    FieldDef { default: Default::Empty, ..def("character.greed", Tier::Character, "0 open-handed to 100 greedy", 0.0, 100.0) },
    FieldDef { default: Default::Empty, ..def("character.ability", Tier::Character, "0 inept to 100 able", 0.0, 100.0) },
    // Wars and occupation (step 7, Diplomacy "wars"). Occupation is kept by the realms module, which
    // hands occupied land over at the peace.
    FieldDef { default: Default::NoOne, ..def("province.occupier", Tier::Province, "realm id occupying it, none where no enemy does", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.attacker", Tier::War, "realm id that declared it", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.defender", Tier::War, "realm id it was declared on", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.cause", Tier::War, "why it was declared (see `WarCause`)", 0.0, 255.0) },
    FieldDef { default: Default::Empty, ..def("war.began", Tier::War, "day declared", 0.0, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.ended", Tier::War, "day of the peace, none while it goes on", 0.0, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.winner", Tier::War, "realm id that won, none while it goes on or when nobody won", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.truce_until", Tier::War, "first day the two may fight again, none while it goes on", 0.0, UNBOUNDED) },
    // Unrest and rebellion (step 7, kept by the realms module).
    FieldDef { default: Default::Zero, ..def("province.unrest", Tier::Province, "thousandths of the way to an uprising", 0.0, 1000.0) },
    FieldDef { default: Default::NoOne, ..def("province.uprising", Tier::Province, "province id where its uprising began, none where calm", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::NoOne, ..def("realm.broke_from", Tier::Realm, "realm id it broke away from, none if it didn't", -UNBOUNDED, UNBOUNDED) },
    // Troops (step 7, Warfare "troops"): an army entry is a company of one kind from one province.
    FieldDef { default: Default::Empty, ..def("army.kind", Tier::Army, "troop kind: a node of the troop tree", 0.0, u16::MAX as f64) },
    FieldDef { default: Default::Empty, ..def("army.home", Tier::Army, "province id its soldiers came from", -UNBOUNDED, UNBOUNDED) },
    // Armies moving and fighting (step 7, Warfare "troops"): companies march in groups.
    FieldDef { default: Default::Empty, ..def("army.role", Tier::Army, "0 a guard that stays home, 1 a field company that marches", 0.0, 1.0) },
    FieldDef { default: Default::Empty, ..def("army.group", Tier::Army, "the lowest company id of the group it marches in, none if in none", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("army.dest", Tier::Army, "province id it is going to, none when standing", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("army.next", Tier::Army, "province id it is on its way to next, none when standing", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("army.days_left", Tier::Army, "days until it reaches army.next", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("army.siege_days", Tier::Army, "days it has besieged the province it stands in", 0.0, UNBOUNDED) },
    // Realms choosing war and peace (step 7, Diplomacy "treaties"; "wars" keeps the war fields
    // too, leaving them empty).
    FieldDef { default: Default::Empty, ..def("war.target", Tier::War, "province id the war was declared over, none if none", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.attacker_dead", Tier::War, "soldiers the attacker lost in the war's battles", 0.0, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.defender_dead", Tier::War, "soldiers the defender lost in the war's battles", 0.0, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.attacker_lost", Tier::War, "battles the attacker lost", 0.0, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("war.defender_lost", Tier::War, "battles the defender lost", 0.0, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("relation.a", Tier::Relation, "realm id, the lower of the two", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("relation.b", Tier::Relation, "realm id, the higher of the two", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("relation.a_of_b", Tier::Relation, "what realm a thinks of realm b, -100 to 100", -100.0, 100.0) },
    FieldDef { default: Default::Empty, ..def("relation.b_of_a", Tier::Relation, "what realm b thinks of realm a, -100 to 100", -100.0, 100.0) },
    // The first wave of modules built side by side (Oct 17): each module's fields in a block of its
    // own, in this order.
    // Logistics and support (Warfare "campaigns").
    FieldDef { default: Default::NoOne, ..def("province.besieger", Tier::Province, "realm id besieging it, none where nobody does", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("army.supply", Tier::Army, "0 in its own land, 1 fed along a supply line, 2 foraging", 0.0, 2.0) },
    // Research "tree".
    FieldDef { default: Default::Zero, ..def("realm.knowledge", Tier::Realm, "thousandths of a year of study banked toward the tech being studied", 0.0, COUNT_MAX) },
    FieldDef { default: Default::NoOne, ..def("realm.studying", Tier::Realm, "tech being studied (its column), none when nothing is", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::NoOne, ..def("realm.tech", Tier::RealmTech, "day the realm learned the tech, none if it doesn't know it", -UNBOUNDED, UNBOUNDED) },
    // Culture "assimilation".
    FieldDef { default: Default::Empty, ..def("culture.parent", Tier::Culture, "culture id it split from, none for a starting culture", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("culture.born", Tier::Culture, "day it began (0 for a starting culture)", 0.0, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("culture.hearth", Tier::Culture, "province id where it began", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("culture.died", Tier::Culture, "day its last person was gone, none while it lives", 0.0, UNBOUNDED) },
    FieldDef { default: Default::NoOne, ..def("realm.culture", Tier::Realm, "culture id of the realm: its capital's main culture, none if none", -UNBOUNDED, UNBOUNDED) },
    // Warfare: the realm's soldiers lost since it began, by how (the battle inspector, Oct 18).
    FieldDef { default: Default::Zero, ..def("realm.lost_battle", Tier::Realm, "soldiers killed in battle or lost at sea", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Zero, ..def("realm.lost_sickness", Tier::Realm, "soldiers dead of camp disease and cold, away from its land", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Zero, ..def("realm.lost_hunger", Tier::Realm, "soldiers starved for want of the food asked for them", 0.0, COUNT_MAX) },
    FieldDef { default: Default::Zero, ..def("realm.lost_deserted", Tier::Realm, "soldiers who deserted, unfed or unpaid", 0.0, COUNT_MAX) },
    // The second wave of modules built side by side (Oct 18): each module's fields in a block of its
    // own, in this order.
    // Religion "conversion".
    FieldDef { default: Default::Empty, ..def("religion.parent", Tier::Religion, "faith id it broke away from, none for a starting faith", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("religion.born", Tier::Religion, "day it began (0 for a starting faith)", 0.0, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("religion.hearth", Tier::Religion, "province id where it began", -UNBOUNDED, UNBOUNDED) },
    FieldDef { default: Default::Empty, ..def("religion.died", Tier::Religion, "day its last follower was gone, none while it lives", 0.0, UNBOUNDED) },
    FieldDef { default: Default::NoOne, ..def("realm.religion", Tier::Realm, "faith id of the realm: its capital's main faith, none if none", -UNBOUNDED, UNBOUNDED) },
    // Disease "plagues".
    FieldDef { default: Default::Zero, ..def("province.sick", Tier::Province, "millionths of the people ill now", 0.0, 1_000_000.0) },
    FieldDef { default: Default::Zero, ..def("province.immune", Tier::Province, "millionths of the people immune", 0.0, 1_000_000.0) },
    FieldDef { default: Default::Zero, ..def("province.plague_dead", Tier::Province, "people dead of plague since the year began", 0.0, COUNT_MAX) },
    FieldDef { default: Default::NoOne, ..def("province.outbreak", Tier::Province, "outbreak id going on in it, none where there is none", -UNBOUNDED, UNBOUNDED) },
    // Natural world "regrowth": the share of the wild that is gone, so 0 (the default) is untouched.
    FieldDef { default: Default::Zero, ..def("province.woods_lost", Tier::Province, "millionths of its wild woodland cleared", 0.0, 1_000_000.0) },
    FieldDef { default: Default::Zero, ..def("province.fish_lost", Tier::Province, "millionths of its fish gone", 0.0, 1_000_000.0) },
    FieldDef { default: Default::Zero, ..def("province.game_lost", Tier::Province, "millionths of its wild game gone", 0.0, 1_000_000.0) },
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
        // Block by block, skipping blocks that are alike as a whole: a field a daily module keeps
        // changes in a few places (found at step 7's checks: 38,000 army entries compared daily).
        fn go<T, K: PartialEq>(a: &[T], b: &[T], key: impl Fn(&T) -> K) -> Vec<u32> {
            const BLOCK: usize = 64;
            let mut out = Vec::new();
            for start in (0..b.len()).step_by(BLOCK) {
                let end = (start + BLOCK).min(b.len());
                let alike = end <= a.len() && a[start..end].iter().zip(&b[start..end]).all(|(x, y)| key(x) == key(y));
                if !alike {
                    out.extend((start..end).filter(|&i| a.get(i).map(&key) != Some(key(&b[i]))).map(|i| i as u32));
                }
            }
            out
        }
        // Floats compare by bits, so a NaN that stays NaN is not a change.
        match (self, new) {
            (FieldData::F32(a), FieldData::F32(b)) => go(a, b, |x| x.to_bits()),
            (FieldData::F64(a), FieldData::F64(b)) => go(a, b, |x| x.to_bits()),
            (FieldData::U64(a), FieldData::U64(b)) => go(a, b, |x| *x),
            (FieldData::U32(a), FieldData::U32(b)) => go(a, b, |x| *x),
            (FieldData::U16(a), FieldData::U16(b)) => go(a, b, |x| *x),
            (FieldData::U8(a), FieldData::U8(b)) => go(a, b, |x| *x),
            (FieldData::Dir(a), FieldData::Dir(b)) => go(a, b, |x| x.map(f64::to_bits)),
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

}

/// Exact total of a conserved quantity, from the fields that hold it.
pub fn quantity_total(world: &World, q: Quantity) -> u64 {
    match q {
        Quantity::People => world.pops.size.iter().sum::<u64>() + world.armies.size.iter().sum::<u64>(),
        Quantity::Goods(_) => world.goods.sum_class(&world.stock, q) + world.goods.sum_class(&world.realm_stock, q),
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
        // Each conserved field holds a quantity, and each quantity's fields are conserved.
        let mut conserved: Vec<FieldId> = FieldId::all().filter(|id| id.def().conserved).collect();
        let mut holders: Vec<FieldId> = [Quantity::Goods(0), Quantity::People].iter().flat_map(|q| q.fields()).collect();
        conserved.sort();
        holders.sort();
        assert_eq!(conserved, holders);
        // A request takes from the quantity of what it asks for.
        let goods = Goods::default();
        let ask = |field: &str, good: Option<&str>| Request {
            day: 0,
            requester: crate::module::Slot::Population,
            field: known(field),
            entity: 4,
            good: good.map(|g| goods.tree.find(g).unwrap()),
            take: 1,
            priority: crate::requests::Priority::Essential,
            cause: crate::requests::Cause::Eating,
            tag: 0,
            to: None,
            shift: None,
        };
        assert_eq!(goods.quantity_asked(&ask("province.stock", Some("raw"))), Some(Quantity::Goods(1)));
        assert_eq!(goods.quantity_asked(&ask("province.stock", Some("fish"))), Some(Quantity::Goods(0)));
        assert_eq!(goods.quantity_asked(&ask("pop.size", None)), Some(Quantity::People));
        assert_eq!(goods.quantity_asked(&ask("province.starved_this_year", None)), None);
    }

    #[test]
    fn food_keeps_its_saved_number_and_goods_count_per_class() {
        // Before goods had classes, food was saved as 1.
        assert_eq!(postcard::to_stdvec(&Quantity::Goods(0)).unwrap(), postcard::to_stdvec(&1u16).unwrap());
        assert_eq!(postcard::from_bytes::<Quantity>(&[0]).unwrap(), Quantity::People);
        assert!(postcard::from_bytes::<Quantity>(&postcard::to_stdvec(&300u16).unwrap()).is_err());

        let tree = Taxonomy::standard_goods();
        let g = Goods::new(tree.clone(), &["fish", "ore", "grain"]).unwrap();
        assert_eq!((g.stride(), g.column("grain"), g.column("food"), g.depth()), (3, Some(2), None, 2));
        assert_eq!(g.quantities(), vec![Quantity::Goods(0), Quantity::Goods(1)]);
        // Two provinces: fish, ore, grain each.
        let data = FieldData::U64(vec![1, 10, 100, 2, 20, 200]);
        assert_eq!(g.totals(known("province.stock"), &data), vec![(Quantity::Goods(0), 303), (Quantity::Goods(1), 30)]);
        assert_eq!(g.split(4), (1, 1));
        assert_eq!(Quantity::Goods(1).name(&g), "raw");

        // What is under a node: food is grain and fish here, raw is ore, cloth isn't kept.
        let node = |n: &str| tree.find(n).unwrap();
        assert_eq!((g.columns_under(node("food")), g.columns_under(node("raw"))), (Some(vec![0, 2]), Some(vec![1])));
        assert_eq!((g.columns_under(node("cloth")), g.columns_under(0)), (Some(vec![]), Some(vec![0, 1, 2])));
        assert_eq!((g.sum_under(&[1, 10, 100], node("food")), g.sum_under(&[1, 10, 100], node("tools"))), (Some(101), Some(0)));
        // Kept whole, food can't be split back into grain.
        let whole = Goods::new(tree.clone(), &["food", "ore"]).unwrap();
        assert_eq!((whole.columns_under(node("grain")), whole.columns_under(node("food"))), (None, Some(vec![0])));
        assert_eq!(whole.sum_under(&[7, 3], node("grain")), None);

        assert!(Goods::new(tree.clone(), &["food", "fish"]).unwrap_err().contains("overlap"));
        assert!(Goods::new(tree.clone(), &["goods"]).unwrap_err().contains("whole tree"));
        assert!(Goods::new(tree, &["spice"]).unwrap_err().contains("no spice"));
    }

    #[test]
    fn clamping_reports_what_moved() {
        let mut d = FieldData::F32(vec![-5.0, 0.5, 9.0, f32::NAN]);
        let moved = d.clamp(0.0, 1.0);
        assert_eq!(moved.iter().map(|m| m.0).collect::<Vec<_>>(), vec![0, 2, 3]);
        assert_eq!(d, FieldData::F32(vec![0.0, 0.5, 1.0, 0.0]));
    }
}
