//! Economics v1, "markets": each province makes goods from its land and people, uses them, and
//! keeps a local price per good.
//!
//! The module owns `province.stock` (the eight goods and the coins), `province.price` and
//! `province.workers`. Once a month, in each province:
//!
//! 1. **Prices** move toward what the stores call for: a good held short of the stores wanted gets
//!    dearer, one held in plenty cheaper (`price = base × (wanted / held) ^ response`, within a
//!    range). Goods people take as one (food) are priced by their stores together.
//! 2. **Work** moves toward what pays: each good is worth its price, less what its inputs cost, for
//!    what one more worker would make of it. Land runs out, so each worker on it adds less than the
//!    one before: work spreads until one more worker is worth the same everywhere. Workers move only
//!    part of the way each month. This is the price response that damps every good's growth: a
//!    good piling up gets cheap and loses its workers.
//! 3. **Making**: each good's output follows its workers and its land; harvests come with the
//!    seasons, rain and the year's luck; tools make workers more productive; tools take ore and
//!    timber to make.
//! 4. **Using**: people use up timber, stone and cloth (less when they are dear). Food is eaten
//!    through Population's change requests, served from all the food kept. Stores rot or wear.
//!
//! Coins (the goods under the money class) are never made, used, spoiled or priced: the province
//! holds the coins it started with until trade moves them. How each good is made, used and priced
//! is data (`data/markets.toml`), saved with the world.

use crate::economy::month_share;
use crate::events::EventKind;
use crate::fields::{known, FieldData, FieldId, Goods, Quantity, Tier};
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::{serve_goods, split_proportional, Cause, Request};
use crate::rng::{draw, purpose, round_stochastic, Module as Rng};
use crate::terrain::fbm;
use crate::world::World;
use serde::{Deserialize, Serialize};
use rayon::prelude::*;
use std::collections::BTreeMap;

/// The parameters shipped in `data/markets.toml`.
pub const STANDARD: &str = include_str!("../data/markets.toml");

/// Prices are kept in thousandths of a coin per unit of the good.
pub const PRICE_SCALE: f64 = 1000.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketParams {
    /// Share of the people who work.
    pub working_share: f64,
    /// Coins each person starts with.
    pub coins_per_person: f64,
    /// How far workers move toward the best-paying work each month.
    pub job_change: f64,
    /// Coins a year that work must be worth for a worker to do it rather than rest.
    pub idle_worth: f64,
    /// price = base × (stores wanted / stores held) ^ this.
    pub price_response: f64,
    /// How far a price moves toward that each month.
    pub price_change: f64,
    /// Prices stay within base / this and base × this.
    pub price_range: f64,
    /// Starting stores, in months of what the province makes.
    pub initial_stock_months: f64,
    /// The class of the goods tree whose goods are coins.
    pub money: String,
    pub harvest_variability: f64,
    pub seasonality: f64,
    pub harvest_peak: f64,
    /// Tools a fully equipped worker holds.
    pub tools_per_worker: f64,
    /// How much more a fully equipped worker makes.
    pub tool_boost: f64,
    /// The good tools are.
    pub tools: String,
    /// Goods priced together, by the node of the goods tree they are under.
    pub groups: BTreeMap<String, GroupParams>,
    pub goods: BTreeMap<String, GoodParams>,
}

impl Default for MarketParams {
    fn default() -> Self {
        toml::from_str(STANDARD).expect("bundled markets parameters are valid")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupParams {
    /// Wanted a year per person.
    pub need_per_person: f64,
    /// Stores wanted, in months of need.
    pub reserve_months: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Season {
    /// The same every month.
    #[default]
    Steady,
    /// Gathered with the harvest: mostly in late summer far from the equator.
    Harvest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loss {
    #[default]
    Spoilage,
    Wear,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoodParams {
    /// Priced together with the goods of this group.
    #[serde(default)]
    pub group: Option<String>,
    /// Counted as harvest in the yearly table; otherwise as goods made.
    #[serde(default)]
    pub harvested: bool,
    #[serde(default)]
    pub season: Season,
    /// Whether rain and the year's luck change how much is made.
    #[serde(default)]
    pub weather: bool,
    /// What a year can give when fully worked: province values (by field name, or "people") times
    /// a rate.
    pub land: BTreeMap<String, f64>,
    /// What one worker makes in a year while land is plentiful.
    pub per_worker: f64,
    /// Used up a year per person.
    #[serde(default)]
    pub use_per_person: f64,
    /// Stores wanted, in months of use (for goods not in a group).
    #[serde(default)]
    pub reserve_months: f64,
    /// Coins per unit when stores are as wanted.
    pub base_price: f64,
    /// Share of the store lost a year.
    #[serde(default)]
    pub lost_per_year: f64,
    #[serde(default)]
    pub lost_as: Loss,
    /// What one unit takes to make, by good.
    #[serde(default)]
    pub inputs: BTreeMap<String, f64>,
}

/// The province values a good's land can name. Every one is read by the manifest.
const LAND_FIELDS: &[&str] = &[
    "province.area_km2",
    "province.food_capacity",
    "province.grazing_capacity",
    "province.forest_km2",
    "province.coast_km",
    "province.river_km",
    "province.upland_km2",
    "province.ore_km2",
];

/// What "people" means in a good's land: the province's people.
const PEOPLE: &str = "people";

static V1: Manifest = Manifest {
    slot: Slot::Economics,
    version: "markets",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    reads: &[
        "province.area_km2",
        "province.food_capacity",
        "province.grazing_capacity",
        "province.forest_km2",
        "province.coast_km",
        "province.river_km",
        "province.upland_km2",
        "province.ore_km2",
        "province.center",
        "province.lat",
        "province.rain_shortfall",
        "pop.province",
        "pop.size",
    ],
    owns: &["province.stock", "province.price", "province.workers"],
    requests: &[],
    asks: &[],
    // Goods are made by people: a world without them is refused.
    requires: &["pop.size"],
    emits: &[EventKind::Granted],
    handles: &[],
    state_schema: "none",
    // The eight goods and the coins. Which of them are coins is found through the goods tree.
    goods: &["grain", "fish", "livestock", "timber", "stone", "ore", "cloth", "tools", "coin"],
};

/// Where a good's land comes from.
#[derive(Clone, Copy, Debug)]
enum Source {
    Field(FieldId),
    People,
}

/// One good kept, as the module works with it.
#[derive(Clone, Debug)]
struct Good {
    column: usize,
    params: GoodParams,
    land: Vec<(Source, f64)>,
    /// (column, amount per unit made).
    inputs: Vec<(usize, f64)>,
    group: Option<usize>,
}

/// The parameters matched to the goods the world keeps.
#[derive(Clone, Debug)]
struct Resolved {
    /// Goods that are made and priced, inputs before what is made of them.
    goods: Vec<Good>,
    /// Each group's columns and parameters.
    groups: Vec<(Vec<usize>, GroupParams)>,
    /// Columns holding coins.
    coins: Vec<usize>,
    tools: usize,
    /// For each column, its place in `goods` (coins have none: `usize::MAX`).
    index: Vec<usize>,
    /// The columns of `goods`, in order: the tie-break when workers are shared out.
    ids: Vec<u64>,
    /// Whether each group has a good gathered with the harvest.
    group_harvested: Vec<bool>,
}

impl Resolved {
    fn new(p: &MarketParams, kept: &Goods) -> Result<Resolved, String> {
        let tree = &kept.tree;
        let money = tree.find(&p.money).ok_or_else(|| format!("the goods tree has no {} (the money class)", p.money))?;
        let coins = kept.columns_under(money).ok_or_else(|| format!("{} is kept only as part of something bigger", p.money))?;
        let column = |name: &str| kept.column(name).ok_or_else(|| format!("this world keeps no {name} on its own"));
        let mut groups = Vec::new();
        let mut group_names = Vec::new();
        for (name, g) in &p.groups {
            let node = tree.find(name).ok_or_else(|| format!("group {name} is not in the goods tree"))?;
            let cols = kept.columns_under(node).ok_or_else(|| format!("group {name} is kept only as part of something bigger"))?;
            groups.push((cols, g.clone()));
            group_names.push(name.clone());
        }
        let mut goods = Vec::new();
        for c in 0..kept.stride() {
            if coins.contains(&c) {
                continue;
            }
            let name = tree.name(kept.columns[c]);
            let gp = p.goods.get(name).ok_or_else(|| format!("no parameters for {name}: how it is made, used and priced"))?;
            let mut land = Vec::new();
            for (what, &rate) in &gp.land {
                let source = match what.as_str() {
                    PEOPLE => Source::People,
                    f if LAND_FIELDS.contains(&f) => Source::Field(known(f)),
                    f => return Err(format!("{name}'s land names {f}; it may name {} or {PEOPLE}", LAND_FIELDS.join(", "))),
                };
                land.push((source, rate));
            }
            let inputs = gp.inputs.iter().map(|(i, &x)| Ok((column(i)?, x))).collect::<Result<Vec<_>, String>>()?;
            if inputs.iter().any(|&(i, _)| coins.contains(&i) || i == c) {
                return Err(format!("{name} can't be made of coins or of itself"));
            }
            let group = match &gp.group {
                None => None,
                Some(g) => {
                    let k = group_names.iter().position(|n| n == g).ok_or_else(|| format!("{name} is priced with {g}, which has no group parameters"))?;
                    if !groups[k].0.contains(&c) {
                        return Err(format!("{name} is not under {g}"));
                    }
                    Some(k)
                }
            };
            if !(gp.per_worker >= 0.0 && gp.base_price > 0.0 && (0.0..=1.0).contains(&gp.lost_per_year)) {
                return Err(format!("{name}: per_worker must be at least 0, base_price above 0, lost_per_year between 0 and 1"));
            }
            goods.push(Good { column: c, params: gp.clone(), land, inputs, group });
        }
        // Inputs are made before what is made of them (one level of making is enough for now).
        goods.sort_by_key(|g| !g.inputs.is_empty());
        if let Some(g) = goods.iter().find(|g| g.inputs.iter().any(|&(i, _)| goods.iter().any(|h| h.column == i && !h.inputs.is_empty()))) {
            return Err(format!("{} is made of something that is itself made of other goods; not supported yet", tree.name(kept.columns[g.column])));
        }
        let tools = column(&p.tools)?;
        if !goods.iter().any(|g| g.column == tools) {
            return Err(format!("{} (the tools) are coins", p.tools));
        }
        let mut index = vec![usize::MAX; kept.stride()];
        for (i, g) in goods.iter().enumerate() {
            index[g.column] = i;
        }
        let ids = goods.iter().map(|g| g.column as u64).collect();
        let group_harvested = groups.iter().map(|(cols, _)| goods.iter().any(|g| cols.contains(&g.column) && g.params.season == Season::Harvest)).collect();
        Ok(Resolved { goods, groups, coins, tools, index, ids, group_harvested })
    }
}

pub struct Markets {
    pub params: MarketParams,
    resolved: Option<Resolved>,
}

impl Markets {
    pub fn new(params: MarketParams) -> Markets {
        Markets { params, resolved: None }
    }

    fn resolved(&mut self, goods: &Goods) -> Resolved {
        self.resolved.get_or_insert_with(|| Resolved::new(&self.params, goods).expect("checked when the world was made")).clone()
    }
}

/// The province values a month's work needs, read once.
struct Inputs {
    people: Vec<u64>,
    fields: Vec<(FieldId, FieldData)>,
}

impl Inputs {
    fn read(w: &World, r: &Resolved) -> Inputs {
        let mut people = vec![0u64; w.provinces.count];
        for (k, &p) in w.pops.province.iter().enumerate() {
            people[p as usize] += w.pops.size[k];
        }
        let mut fields: Vec<(FieldId, FieldData)> = Vec::new();
        for g in &r.goods {
            for &(s, _) in &g.land {
                if let Source::Field(id) = s
                    && !fields.iter().any(|f| f.0 == id)
                {
                    fields.push((id, w.field(id)));
                }
            }
        }
        Inputs { people, fields }
    }

    /// What good `g` can give in a year in province `p` when fully worked.
    fn land(&self, g: &Good, p: usize) -> f64 {
        let value = |s: Source| match s {
            Source::People => self.people[p] as f64,
            Source::Field(id) => self.fields.iter().find(|f| f.0 == id).map_or(0.0, |f| f.1.value(p)),
        };
        g.land.iter().map(|&(s, rate)| value(s) * rate).sum::<f64>().max(0.0)
    }
}

/// A year's output of a good from `workers` on land that gives at most `land`, each worker making
/// `per_worker` while land is plentiful: every worker adds less than the one before.
fn output(land: f64, per_worker: f64, workers: f64) -> f64 {
    if land <= 0.0 { 0.0 } else { land * (1.0 - (-per_worker * workers / land).exp()) }
}

/// Share up to `total` workers over goods so that one more worker is worth the same on each:
/// `(value per unit, made per worker, land)` each. Goods worth nothing, or with no land, get none.
/// Nobody takes work worth less than `idle_worth` a year: when there is too little worth doing,
/// the rest rest.
pub fn allocate(total: f64, goods: &[(f64, f64, f64)], idle_worth: f64) -> Vec<f64> {
    let mut out = vec![0.0; goods.len()];
    let open: Vec<usize> = (0..goods.len()).filter(|&i| goods[i].0 > 0.0 && goods[i].1 > 0.0 && goods[i].2 > 0.0).collect();
    if total <= 0.0 || open.is_empty() {
        return out;
    }
    // At worth λ for one more worker, good i takes (land / per_worker) × ln(value × per_worker / λ).
    let at = |ln_lambda: f64, out: &mut [f64]| {
        let mut sum = 0.0;
        for &i in &open {
            let (v, a, l) = goods[i];
            out[i] = (l / a * ((v * a).ln() - ln_lambda)).max(0.0);
            sum += out[i];
        }
        sum
    };
    let top = open.iter().map(|&i| (goods[i].0 * goods[i].1).ln()).fold(f64::MIN, f64::max);
    let floor = idle_worth.max(1e-300).ln().max(top - 60.0);
    if floor >= top {
        return out;
    }
    if at(floor, &mut out) <= total {
        return out;
    }
    let (mut lo, mut hi) = (floor, top);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if at(mid, &mut out) > total { lo = mid } else { hi = mid }
    }
    let sum = at(hi, &mut out);
    if sum > 0.0 {
        out.iter_mut().for_each(|x| *x *= total / sum);
    }
    out
}

/// Totals of a month's flows, per class and cause, in a fixed order.
#[derive(Default)]
struct Books(BTreeMap<(u8, Cause), i64>);

impl Books {
    fn add(&mut self, class: u8, cause: Cause, delta: i64) {
        if delta != 0 {
            *self.0.entry((class, cause)).or_default() += delta;
        }
    }
}

impl Module for Markets {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    fn check(&self, goods: &Goods) -> Result<(), String> {
        let p = &self.params;
        let share = |x: f64| (0.0..=1.0).contains(&x);
        if !(share(p.working_share) && share(p.job_change) && share(p.price_change) && p.price_range >= 1.0 && p.price_response >= 0.0) {
            return Err("working_share, job_change and price_change must be between 0 and 1, price_range at least 1".into());
        }
        Resolved::new(p, goods).map(|_| ())
    }

    /// Starting stores, coins, prices and work. Work and prices are settled first, as if the
    /// province had run a few years at steady stores, so the first month doesn't starve anyone.
    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        let r = self.resolved(&w.goods);
        let p = &self.params;
        let (n, k) = (w.provinces.count, w.goods.stride());
        w.stock = vec![0; n * k];
        w.price = vec![0; n * k];
        w.workers = vec![0; n * k];
        let read = Inputs::read(w, &r);
        let boost = 1.0 + p.tool_boost;
        let rows = w.stock.par_chunks_mut(k).zip(w.price.par_chunks_mut(k)).zip(w.workers.par_chunks_mut(k));
        rows.enumerate().for_each(|(prov, ((stock, prices), jobs))| {
            let people = read.people[prov] as f64;
            let workers = (people * p.working_share).round();
            let land: Vec<f64> = r.goods.iter().map(|g| read.land(g, prov)).collect();
            let mut price: Vec<f64> = r.goods.iter().map(|g| g.params.base_price).collect();
            let mut alloc = vec![0.0; r.goods.len()];
            let mut made = vec![0.0; r.goods.len()];
            for _ in 0..40 {
                alloc = allocate(workers, &worth(&r, &price, &land, &made, boost), p.idle_worth);
                for (i, g) in r.goods.iter().enumerate() {
                    made[i] = output(land[i], g.params.per_worker * boost, alloc[i]);
                }
                // A year's need against a year's making, priced as stores would be.
                let need = yearly_need(p, &r, people, workers, &made);
                for (i, g) in r.goods.iter().enumerate() {
                    let (wanted, held) = match g.group {
                        Some(j) => (need.1[j], r.groups[j].0.iter().map(|&c| made[r.index[c]]).sum()),
                        None => (need.0[i], made[i]),
                    };
                    let target = target_price(p, g.params.base_price, wanted, held);
                    price[i] = (price[i] * target).sqrt();
                }
            }
            for (i, g) in r.goods.iter().enumerate() {
                prices[g.column] = (price[i] * PRICE_SCALE).round().max(1.0) as u64;
                stock[g.column] = (made[i] / 12.0 * p.initial_stock_months).round() as u64;
            }
            stock[r.tools] = stock[r.tools].max((workers * p.tools_per_worker).round() as u64);
            let weights: Vec<u64> = alloc.iter().map(|&a| (a * 1000.0).round() as u64).collect();
            for (i, x) in split_proportional(workers as u64, &weights, &r.ids).into_iter().enumerate() {
                jobs[r.goods[i].column] = x;
            }
            if let Some(&c) = r.coins.first() {
                stock[c] = (people * p.coins_per_person).round() as u64;
            }
        });
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let r = self.resolved(&w.goods);
        let k = w.goods.stride();
        // Provinces an edit added start with no prices and no work (the edit gave them empty rows):
        // they are worked out afresh.
        let date = cx.calendar.date(cx.day);
        let month = Month {
            p: &self.params,
            r: &r,
            cal: cx.calendar,
            day: cx.day,
            seed: cx.seed,
            month: date.month,
            month_part: cx.calendar.def.months[date.month].days as f64 / cx.calendar.days_per_year() as f64,
            harvest_key: draw(cx.seed, Rng::Economics, cx.calendar.year_start(date.year), 0, purpose("harvest year")),
            read: Inputs::read(w, &r),
            provinces: &w.provinces,
            rain_shortfall: &w.rain_shortfall,
            goods: &w.goods,
            traced: cx.trace,
        };
        // Provinces don't touch each other's goods this month, so they are worked out side by side;
        // each one's random draws are its own, so the result is the same however they are shared out.
        let rows = w.stock.par_chunks_mut(k).zip(w.price.par_chunks_mut(k)).zip(w.workers.par_chunks_mut(k));
        let done: Vec<(Books, Vec<String>)> = rows.enumerate().map(|(prov, ((stock, price), jobs))| month.province(prov, stock, price, jobs)).collect();
        let mut books = Books::default();
        for (b, notes) in done {
            for ((class, cause), delta) in b.0 {
                books.add(class, cause, delta);
            }
            notes.into_iter().for_each(|n| cx.note(n));
        }
        for ((class, cause), delta) in books.0 {
            cx.flow(Quantity::Goods(class), cause, delta);
        }
    }

    /// Food is asked for as food: it is served from every food good kept, in proportion to each store.
    fn apply_requests(&mut self, w: &mut World, requests: &[Request], cx: &mut Ctx) {
        debug_assert!(requests.iter().all(|r| r.field == known("province.stock")));
        serve_goods(&mut w.stock, requests, cx);
    }
}

/// One month's work, shared by every province.
struct Month<'a> {
    p: &'a MarketParams,
    r: &'a Resolved,
    cal: &'a crate::calendar::Calendar,
    day: u64,
    seed: u64,
    month: usize,
    /// The month's share of the year.
    month_part: f64,
    harvest_key: u64,
    read: Inputs,
    provinces: &'a crate::provinces::Provinces,
    rain_shortfall: &'a [u64],
    goods: &'a Goods,
    traced: Option<u32>,
}

impl Month<'_> {
    /// Province `prov`'s month: its stores, prices and workers are its rows of the per-good
    /// fields, indexed by column. Returns the flows and, for the traced province, what happened.
    fn province(&self, prov: usize, stock: &mut [u64], prices: &mut [u64], jobs: &mut [u64]) -> (Books, Vec<String>) {
        let (p, r, goods) = (self.p, self.r, self.goods);
        let (seed, day) = (self.seed, self.day);
        let mut books = Books::default();
        let mut notes = Vec::new();
        let traced = self.traced == Some(prov as u32);
        let row = (prov * goods.stride()) as u64;
        let people = self.read.people[prov] as f64;
        let workers = (people * p.working_share).round();
        let land: Vec<f64> = r.goods.iter().map(|g| self.read.land(g, prov)).collect();
        let equip = workers * p.tools_per_worker;
        let boost = 1.0 + p.tool_boost * if equip > 0.0 { (stock[r.tools] as f64 / equip).min(1.0) } else { 1.0 };
        let held: Vec<f64> = r.goods.iter().map(|g| stock[g.column] as f64).collect();
        let now: Vec<f64> = r.goods.iter().map(|g| jobs[g.column] as f64).collect();
        let lat = self.provinces.lat[prov] as f64;

        // 1. Prices, from the stores against the stores wanted.
        let to_harvest = months_to_harvest(self.cal, p, self.month, lat);
        let planned: Vec<f64> = r.goods.iter().enumerate().map(|(i, g)| output(land[i], g.params.per_worker * boost, now[i])).collect();
        let need = yearly_need(p, r, people, workers, &planned);
        let mut price = vec![0.0; r.goods.len()];
        for (i, g) in r.goods.iter().enumerate() {
            // Goods gathered with the harvest must last until the next one, on top of the reserve.
            let (wanted, stored) = match g.group {
                Some(j) => {
                    let (cols, gp) = &r.groups[j];
                    let stored = cols.iter().map(|&c| stock[c] as f64).sum();
                    let months = gp.reserve_months + if r.group_harvested[j] { to_harvest } else { 0.0 };
                    (need.1[j] / 12.0 * months, stored)
                }
                None => {
                    let equip = if g.column == r.tools { equip } else { 0.0 };
                    let months = g.params.reserve_months + if g.params.season == Season::Harvest { to_harvest } else { 0.0 };
                    (need.0[i] / 12.0 * months + equip, held[i])
                }
            };
            let target = target_price(p, g.params.base_price, wanted, stored);
            let was = prices[g.column] as f64 / PRICE_SCALE;
            price[i] = if was > 0.0 { was * (target / was).powf(p.price_change) } else { target };
            prices[g.column] = (price[i] * PRICE_SCALE).round().max(1.0) as u64;
        }

        // 2. Work, toward what pays. Goods made of others use what is held and what a year will make.
        let supply: Vec<f64> = (0..r.goods.len()).map(|i| held[i] + planned[i]).collect();
        let target = allocate(workers, &worth(r, &price, &land, &supply, boost), p.idle_worth);
        let fresh = now.iter().sum::<f64>() == 0.0;
        let moved: Vec<f64> = (0..r.goods.len()).map(|i| if fresh { target[i] } else { now[i] + p.job_change * (target[i] - now[i]) }).collect();
        let weights: Vec<u64> = moved.iter().map(|&a| (a.max(0.0) * 1000.0).round() as u64).collect();
        let working = split_proportional(workers as u64, &weights, &r.ids);
        for (i, g) in r.goods.iter().enumerate() {
            jobs[g.column] = working[i];
        }

        // 3. Making: inputs first, then what is made of them.
        let c = self.provinces.center[prov];
        let swing = 2.0 * p.harvest_variability * fbm([c[0] * 2.5, c[1] * 2.5, c[2] * 2.5], 3, self.harvest_key);
        let luck = (1.0 + swing).max(0.0) * (1.0 - self.rain_shortfall[prov] as f64 / 1000.0);
        for (i, g) in r.goods.iter().enumerate() {
            let gp = &g.params;
            let share = match gp.season {
                Season::Steady => self.month_part,
                Season::Harvest => month_share(self.cal, p.seasonality, p.harvest_peak, self.month, lat),
            };
            let weather = if gp.weather { luck } else { 1.0 };
            let expected = output(land[i], gp.per_worker * boost, working[i] as f64) * share * weather;
            let entity = row + g.column as u64;
            let mut made = round_stochastic(expected, seed, Rng::Economics, day, entity, purpose("made"));
            // As much as the inputs allow; each input is used up in whole units.
            for &(c, per) in g.inputs.iter().filter(|x| x.1 > 0.0) {
                made = made.min((stock[c] as f64 / per).floor() as u64);
            }
            for &(c, per) in &g.inputs {
                let used = ((made as f64 * per).ceil() as u64).min(stock[c]);
                stock[c] -= used;
                books.add(goods.class_of_column(c), Cause::MadeInto, -(used as i64));
            }
            stock[g.column] += made;
            books.add(goods.class_of_column(g.column), if gp.harvested { Cause::Harvest } else { Cause::Made }, made as i64);
            if traced {
                notes.push(format!(
                    "{}: {} workers made {made} (a year's land gives at most {:.0}; × {share:.3} of the year{}{}); price {:.3} coins",
                    goods.tree.name(goods.columns[g.column]),
                    working[i],
                    land[i],
                    if gp.weather { format!(" × {weather:.2} for the rain and the year's luck") } else { String::new() },
                    if boost > 1.0 { format!(", tools × {boost:.2}") } else { String::new() },
                    price[i],
                ));
            }
        }

        // 4. Using and losing.
        for (i, g) in r.goods.iter().enumerate() {
            let gp = &g.params;
            let at = g.column;
            let entity = row + at as u64;
            let class = goods.class_of_column(at);
            if gp.use_per_person > 0.0 {
                // Dear goods are used more sparingly.
                let thrift = (gp.base_price / price[i]).sqrt().min(2.0);
                let want = gp.use_per_person * people * self.month_part * thrift;
                let used = round_stochastic(want, seed, Rng::Economics, day, entity, purpose("used")).min(stock[at]);
                stock[at] -= used;
                books.add(class, Cause::Used, -(used as i64));
                if traced && used > 0 {
                    notes.push(format!("{used} {} used by {people} people", goods.tree.name(goods.columns[at])));
                }
            }
            if gp.lost_per_year > 0.0 {
                let lost = round_stochastic(stock[at] as f64 * gp.lost_per_year * self.month_part, seed, Rng::Economics, day, entity, purpose("lost")).min(stock[at]);
                stock[at] -= lost;
                books.add(class, if gp.lost_as == Loss::Wear { Cause::WornOut } else { Cause::Spoilage }, -(lost as i64));
            }
        }
        (books, notes)
    }
}

/// Each good's worth per unit to the one who makes it, what one worker makes and the land, for
/// [`allocate`]: its price less what its inputs cost. A good made of others can be made only as far
/// as `supply` (a year of each good: what is held and what will be made) goes.
fn worth(r: &Resolved, price: &[f64], land: &[f64], supply: &[f64], boost: f64) -> Vec<(f64, f64, f64)> {
    let at = |c: usize| Some(r.index[c]).filter(|&i| i != usize::MAX);
    r.goods
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let cost: f64 = g.inputs.iter().map(|&(c, per)| per * at(c).map_or(0.0, |j| price[j])).sum();
            let reach = g.inputs.iter().filter(|x| x.1 > 0.0).map(|&(c, per)| at(c).map_or(0.0, |j| supply[j]) / per).fold(land[i], f64::min);
            ((price[i] - cost).max(0.0), g.params.per_worker * boost, reach)
        })
        .collect()
}

/// What a province wants a year: of each good on its own (people's use, inputs for what `made`
/// says will be made, and the wear of tools), and of each group (people's need).
fn yearly_need(p: &MarketParams, r: &Resolved, people: f64, workers: f64, made: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let mut own: Vec<f64> = r.goods.iter().map(|g| g.params.use_per_person * people).collect();
    for (i, g) in r.goods.iter().enumerate() {
        for &(c, per) in &g.inputs {
            own[r.index[c]] += per * made[i];
        }
        if g.column == r.tools {
            own[i] += workers * p.tools_per_worker * g.params.lost_per_year;
        }
    }
    let groups = r.groups.iter().map(|(_, gp)| gp.need_per_person * people).collect();
    (own, groups)
}

/// Months from the middle of `month` to the next harvest's peak at latitude `lat` (radians), as far
/// as the harvest gathers at one time of year there: none at the equator, where it comes evenly.
fn months_to_harvest(cal: &crate::calendar::Calendar, p: &MarketParams, month: usize, lat: f64) -> f64 {
    let strength = (p.seasonality * (lat.abs().to_degrees() / 40.0).min(1.0)).min(1.0);
    let peak = if lat >= 0.0 { p.harvest_peak } else { (p.harvest_peak + 0.5).fract() };
    strength * 12.0 * (peak - cal.month_mid_year_fraction(month)).rem_euclid(1.0)
}

/// A good's price when `held` is what there is against `wanted`.
fn target_price(p: &MarketParams, base: f64, wanted: f64, held: f64) -> f64 {
    // A little slack, so an empty store with nobody wanting it isn't dear.
    let slack = 1.0;
    let ratio = ((wanted + slack) / (held + slack)).powf(p.price_response);
    base * ratio.clamp(1.0 / p.price_range, p.price_range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_parameters_cover_every_good_kept() {
        let goods = Goods::new(crate::taxonomy::Taxonomy::standard_goods(), V1.goods).unwrap();
        let r = Resolved::new(&MarketParams::default(), &goods).unwrap();
        assert_eq!(r.goods.len(), 8);
        assert_eq!(r.coins, vec![8]);
        // Tools are made last, from ore and timber.
        assert_eq!(r.goods.last().unwrap().column, r.tools);
        assert_eq!(r.goods.last().unwrap().inputs.len(), 2);
        assert_eq!(r.groups.len(), 1);
        assert_eq!(r.groups[0].0, vec![0, 1, 2]);
    }

    #[test]
    fn bad_parameters_are_refused() {
        let goods = Goods::new(crate::taxonomy::Taxonomy::standard_goods(), V1.goods).unwrap();
        let refused = |change: fn(&mut MarketParams)| {
            let mut p = MarketParams::default();
            change(&mut p);
            Resolved::new(&p, &goods).unwrap_err()
        };
        assert!(refused(|p| { p.goods.remove("stone"); }).contains("no parameters for stone"));
        assert!(refused(|p| { p.goods.get_mut("ore").unwrap().land.insert("cell.rainfall".into(), 1.0); }).contains("ore's land names cell.rainfall"));
        assert!(refused(|p| { p.goods.get_mut("tools").unwrap().inputs.insert("coin".into(), 1.0); }).contains("can't be made of coins"));
        assert!(refused(|p| p.money = "gold".into()).contains("no gold"));
        assert!(refused(|p| p.goods.get_mut("timber").unwrap().group = Some("food".into())).contains("timber is not under food"));
        assert!(refused(|p| { p.goods.get_mut("ore").unwrap().inputs.insert("tools".into(), 1.0); }).contains("not supported yet"));
    }

    #[test]
    fn work_goes_where_one_more_worker_is_worth_most() {
        // Two goods alike but for price: the dearer gets more workers, and all workers are placed.
        let a = allocate(100.0, &[(2.0, 10.0, 500.0), (1.0, 10.0, 500.0)], 0.0);
        assert!((a.iter().sum::<f64>() - 100.0).abs() < 1e-6 && a[0] > a[1], "{a:?}");
        // One more worker is worth the same on both.
        let worth = |v: f64, x: f64| v * 10.0 * (-10.0 * x / 500.0).exp();
        assert!((worth(2.0, a[0]) - worth(1.0, a[1])).abs() < 1e-6);
        // Worthless goods and goods with no land get nobody.
        assert_eq!(allocate(10.0, &[(0.0, 10.0, 500.0), (1.0, 10.0, 0.0), (1.0, 5.0, 50.0)], 0.0)[..2], [0.0, 0.0]);
        assert_eq!(allocate(0.0, &[(1.0, 10.0, 500.0)], 0.0), [0.0]);
        // Work worth less than resting isn't done. Here one more worker is worth 10 at first and
        // less with each, so only those worth at least 5 work: 500 / 10 x ln 2 of them.
        let some = allocate(100.0, &[(1.0, 10.0, 500.0)], 5.0);
        assert!((some[0] - 50.0 * 2f64.ln()).abs() < 1e-6, "{some:?}");
        assert_eq!(allocate(100.0, &[(1.0, 10.0, 500.0)], 10.0), [0.0]);
    }

    #[test]
    fn prices_rise_with_shortage_and_fall_with_plenty() {
        let p = MarketParams::default();
        assert!((target_price(&p, 1.0, 100.0, 100.0) - 1.0).abs() < 1e-12);
        assert!(target_price(&p, 1.0, 100.0, 50.0) > 1.9);
        assert!(target_price(&p, 1.0, 100.0, 400.0) < 0.3);
        assert_eq!(target_price(&p, 1.0, 1e9, 0.0), p.price_range);
        assert_eq!(target_price(&p, 1.0, 0.0, 1e9), 1.0 / p.price_range);
    }
}
