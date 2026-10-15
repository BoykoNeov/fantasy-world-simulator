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
//! Then, between provinces:
//!
//! 5. **Trade**: along each way between neighbours (see [`crate::routes`]), goods go to where they
//!    are dearer when the gap in price beats the cost of carrying them, and the buyer pays the
//!    seller in coins. Each way closes part of its gap a month (the damper on trade). Goods priced
//!    together are traded together. Every trade is worked out from the stores as the month's work
//!    left them, then scaled down to what sellers can spare and buyers can pay, so the result
//!    doesn't depend on the order of the ways.
//!
//! Coins (the goods under the money class) are never made, used, spoiled or priced: provinces
//! start with them and trade moves them. How each good is made, used, priced and carried is data
//! (`data/markets.toml`), saved with the world.

use crate::economy::month_share;
use crate::events::EventKind;
use crate::fields::{known, FieldData, FieldId, Goods, Quantity, Tier};
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::{serve_goods, split_proportional, Cause, Request};
use crate::rng::{draw, purpose, round_stochastic, Module as Rng};
use crate::routes::Link;
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
    /// Trade between neighbours; none in a world without it (and in scenario files written
    /// before trade, which keep running as they did).
    #[serde(default)]
    pub trade: Option<TradeParams>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradeParams {
    /// How much of a price gap, beyond the carrying cost, trade closes in a month.
    pub change: f64,
    /// Most of a store a province sells in a month.
    pub most_sold: f64,
    /// Most of its coins a province spends in a month.
    pub most_spent: f64,
    /// A day at sea costs this share of a day overland.
    pub sea_share: f64,
    /// How strongly prices follow the money around: to trade, everything in a province is dearer
    /// by its coins a person against the world's, to this power. 0: not at all.
    #[serde(default)]
    pub money_response: f64,
    /// That stays within 1/this and this times.
    #[serde(default = "no_range")]
    pub money_range: f64,
    /// Coins to carry one unit of each good one day overland.
    pub carriage: BTreeMap<String, f64>,
}

fn no_range() -> f64 {
    1.0
}

/// How dear everything in a province is, to trade, for the money around: its coins a person
/// (`coins`, `people`) against `average`, the world's, to the power `money_response`, within
/// `money_range`. Where coins pile up, goods are dear, so the province buys more and sells less,
/// and the coins flow back out. 1 where nobody lives.
pub fn money_level(t: &TradeParams, coins: f64, people: f64, average: f64) -> f64 {
    if t.money_response == 0.0 || people <= 0.0 || average <= 0.0 {
        return 1.0;
    }
    let x = (coins / people / average).max(1e-9);
    let level = if t.money_response == 1.0 { x } else { x.powf(t.money_response) };
    level.clamp(1.0 / t.money_range, t.money_range)
}

/// Every province's [`money_level`], from its `coins` and `people`, against the coins a person of
/// the provinces where people live. Trade works its prices out with these; the window shows them.
pub fn money_levels(t: &TradeParams, coins: &[u64], people: &[u64]) -> Vec<f64> {
    let n = people.len();
    let (all_coins, all_people) = (0..n).filter(|&q| people[q] > 0).fold((0.0, 0.0), |(c, h), q| (c + coins[q] as f64, h + people[q] as f64));
    let average = if all_people > 0.0 { all_coins / all_people } else { 0.0 };
    (0..n).map(|q| money_level(t, coins[q] as f64, people[q] as f64, average)).collect()
}

impl MarketParams {
    /// Every province's money level in `w` as it stands (trade works it out at the start of each
    /// month's trade, so it can differ a little from what last month's trade used). `None` when
    /// the world doesn't trade, or its prices don't follow the money.
    pub fn money_levels_now(&self, w: &World) -> Option<Vec<f64>> {
        let t = self.trade.as_ref().filter(|t| t.money_response != 0.0)?;
        let coin = *w.goods.columns_under(w.goods.tree.find(&self.money)?)?.first()?;
        let k = w.goods.stride();
        let coins: Vec<u64> = (0..w.provinces.count).map(|q| w.stock.get(q * k + coin).copied().unwrap_or(0)).collect();
        Some(money_levels(t, &coins, &w.province_population()))
    }
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
    /// What is priced (and traded) as one: each group, then each good in no group.
    items: Vec<Item>,
    /// For each good (by its place in `goods`), its item.
    item_of: Vec<usize>,
    /// For each column, coins to carry a unit one day overland (0 for coins).
    carriage: Vec<f64>,
}

/// Goods priced as one, by the stores of them all against the stores wanted of them all.
#[derive(Clone, Debug)]
struct Item {
    name: String,
    cols: Vec<usize>,
    kind: ItemKind,
}

#[derive(Clone, Copy, Debug)]
enum ItemKind {
    /// A group, by its place in `groups`.
    Group(usize),
    /// A good in no group, by its place in `goods`.
    Good(usize),
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
        let mut items: Vec<Item> = group_names.iter().zip(&groups).enumerate().map(|(j, (name, (cols, _)))| Item { name: name.clone(), cols: cols.clone(), kind: ItemKind::Group(j) }).collect();
        for (i, g) in goods.iter().enumerate().filter(|(_, g)| g.group.is_none()) {
            items.push(Item { name: tree.name(kept.columns[g.column]).into(), cols: vec![g.column], kind: ItemKind::Good(i) });
        }
        let item_of = goods.iter().enumerate().map(|(i, g)| g.group.unwrap_or_else(|| items.iter().position(|it| matches!(it.kind, ItemKind::Good(x) if x == i)).unwrap())).collect();
        let mut carriage = vec![0.0; kept.stride()];
        if let Some(t) = &p.trade {
            let share = |x: f64| (0.0..=1.0).contains(&x);
            if !(share(t.change) && share(t.most_sold) && share(t.most_spent) && t.sea_share >= 0.0 && t.money_response >= 0.0 && t.money_range >= 1.0) {
                return Err("trade: change, most_sold and most_spent must be between 0 and 1, sea_share and money_response at least 0, money_range at least 1".into());
            }
            for (name, &x) in &t.carriage {
                let c = column(name)?;
                if coins.contains(&c) || x.is_nan() || x < 0.0 {
                    return Err(format!("trade: {name} can't be carried at {x} coins a day; coins aren't carried, and goods cost at least 0"));
                }
                carriage[c] = x;
            }
            if let Some(g) = goods.iter().find(|g| !t.carriage.contains_key(tree.name(kept.columns[g.column]))) {
                return Err(format!("trade: no carrying cost for {}", tree.name(kept.columns[g.column])));
            }
        }
        Ok(Resolved { goods, groups, coins, tools, index, ids, group_harvested, items, item_of, carriage })
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
        let done: Vec<(Books, Vec<f64>, Vec<String>)> = rows.enumerate().map(|(prov, ((stock, price), jobs))| month.province(prov, stock, price, jobs)).collect();
        let people = month.read.people;
        let mut books = Books::default();
        let mut wanted = Vec::with_capacity(done.len() * r.items.len());
        for (b, want, notes) in done {
            for ((class, cause), delta) in b.0 {
                books.add(class, cause, delta);
            }
            wanted.extend(want);
            notes.into_iter().for_each(|n| cx.note(n));
        }
        if let Some(t) = &self.params.trade {
            let routes = w.routes.clone();
            let deals = deals(&r, &self.params, t, &w.stock, &wanted, &people, &routes.links);
            settle(&r, &deals, &mut w.stock, &w.goods, &mut books, cx.trace).into_iter().for_each(|n| cx.note(n));
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
    /// fields, indexed by column. Returns the flows, the stores wanted of each item (for trade)
    /// and, for the traced province, what happened.
    fn province(&self, prov: usize, stock: &mut [u64], prices: &mut [u64], jobs: &mut [u64]) -> (Books, Vec<f64>, Vec<String>) {
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
        // Goods gathered with the harvest must last until the next one, on top of the reserve.
        let wanted: Vec<f64> = r
            .items
            .iter()
            .map(|it| match it.kind {
                ItemKind::Group(j) => {
                    let months = r.groups[j].1.reserve_months + if r.group_harvested[j] { to_harvest } else { 0.0 };
                    need.1[j] / 12.0 * months
                }
                ItemKind::Good(i) => {
                    let g = &r.goods[i];
                    let equip = if g.column == r.tools { equip } else { 0.0 };
                    let months = g.params.reserve_months + if g.params.season == Season::Harvest { to_harvest } else { 0.0 };
                    need.0[i] / 12.0 * months + equip
                }
            })
            .collect();
        let mut price = vec![0.0; r.goods.len()];
        for (i, g) in r.goods.iter().enumerate() {
            let it = r.item_of[i];
            let stored = r.items[it].cols.iter().map(|&c| stock[c] as f64).sum();
            let target = target_price(p, g.params.base_price, wanted[it], stored);
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
        (books, wanted, notes)
    }
}

/// One month's trade along one way: goods of one item go from `from` to `to`, which pays for them.
#[derive(Clone, Debug, PartialEq)]
pub struct Deal {
    pub from: u32,
    pub to: u32,
    /// What is traded, as `(column, units, coins a unit)`: the seller's price and the carrying.
    pub goods: Vec<(usize, u64, f64)>,
    /// What is priced as one (its place among the items), its price at both ends for the seller's
    /// mix of goods (money level included), and the carrying, in coins a unit: for explaining a day.
    item: usize,
    pub prices: (f64, f64),
    pub carrying: f64,
}

/// One province's side of one item, as trade sees it.
#[derive(Clone, Copy, Default)]
struct Side {
    held: f64,
    wanted: f64,
    /// The price against the base price.
    factor: f64,
    /// Base price and carrying a day of a unit of what it holds, in the mix it holds it.
    base: f64,
    carry: f64,
}

/// The month's trades along `links`, from the stores as the month's work left them (`stock`,
/// `wanted` per province and item, `people` per province). Provinces without people don't trade.
///
/// Along each way and for each item, the cheaper end sells to the dearer when the gap in price
/// beats the carrying, moving a share (`change`) of what would close the gap down to the carrying.
/// Where several ways serve one store, each closes only its part. Prices include each end's money
/// level ([`money_level`]), as the month's coins set it. Then each province's
/// sales of an item are scaled down to `most_sold` of its store, and its spending to `most_spent`
/// of its coins. Everything is worked out from the same stores, so the order of the ways doesn't
/// matter.
fn deals(r: &Resolved, p: &MarketParams, t: &TradeParams, stock: &[u64], wanted: &[f64], people: &[u64], links: &[Link]) -> Vec<Deal> {
    let Some(&coin) = r.coins.first() else { return Vec::new() };
    let (n, m, k) = (people.len(), r.items.len(), r.index.len());
    let base = |c: usize| r.goods[r.index[c]].params.base_price;
    let sides: Vec<Side> = (0..n * m)
        .into_par_iter()
        .map(|at| {
            let (prov, item) = (at / m, at % m);
            let cols = &r.items[item].cols;
            let held: f64 = cols.iter().map(|&c| stock[prov * k + c] as f64).sum();
            // A store held empty is priced as if it held equal parts of the item's goods.
            let mix = |f: &dyn Fn(usize) -> f64| {
                if held > 0.0 { cols.iter().map(|&c| f(c) * stock[prov * k + c] as f64).sum::<f64>() / held } else { cols.iter().map(|&c| f(c)).sum::<f64>() / cols.len() as f64 }
            };
            Side { held, wanted: wanted[at], factor: target_price(p, 1.0, wanted[at], held), base: mix(&base), carry: mix(&|c| r.carriage[c]) }
        })
        .collect();
    let coins: Vec<u64> = (0..n).map(|q| stock[q * k + coin]).collect();
    let levels = money_levels(t, &coins, people);

    struct Offer {
        from: usize,
        to: usize,
        item: usize,
        days: f64,
        units: f64,
    }
    // The ways are looked at side by side; the offers come out in the ways' order.
    let mut offers: Vec<Offer> = links.par_iter().flat_map_iter(|l| {
        let mut offers = Vec::new();
        let (a, b) = (l.a as usize, l.b as usize);
        // Days by the cheaper way: a day at sea costs `sea_share` of one by land.
        let days = match (l.land_days, l.sea_days) {
            (Some(x), Some(y)) => (x as f64).min(y as f64 * t.sea_share),
            (Some(x), None) => x as f64,
            (None, Some(y)) => y as f64 * t.sea_share,
            (None, None) => return offers,
        };
        if people[a] == 0 || people[b] == 0 {
            return offers;
        }
        for item in 0..m {
            let (sa, sb) = (sides[a * m + item], sides[b * m + item]);
            let (from, to, s, d) = match (levels[a] * sa.factor).partial_cmp(&(levels[b] * sb.factor)) {
                Some(std::cmp::Ordering::Less) => (a, b, sa, sb),
                Some(std::cmp::Ordering::Greater) => (b, a, sb, sa),
                _ => continue,
            };
            // The gap left after selling q: it shrinks as q grows. Each end's money level stays as
            // the month found it: it follows a province's wealth over months, not one month's
            // buying (a starving province paying for food doesn't want food less).
            let (ls, ld) = (levels[from], levels[to]);
            let gap = |q: f64| s.base * (ld * target_price(p, 1.0, d.wanted, d.held + q) - ls * target_price(p, 1.0, s.wanted, s.held - q)) - s.carry * ls * days;
            if s.held < 1.0 || gap(0.0) <= 0.0 {
                continue;
            }
            // What would close it down to the carrying, found by halving (prices are held within
            // their range, so no formula does).
            let close = if gap(s.held) > 0.0 {
                s.held
            } else {
                let (mut lo, mut hi) = (0.0, s.held);
                for _ in 0..30 {
                    let mid = 0.5 * (lo + hi);
                    if gap(mid) > 0.0 { lo = mid } else { hi = mid }
                }
                lo
            };
            offers.push(Offer { from, to, item, days, units: t.change * close });
        }
        offers
    }).collect();

    // Each way serving a store closes only its part of the gap.
    let mut sellers = vec![0u32; n * m];
    let mut buyers = vec![0u32; n * m];
    for o in &offers {
        sellers[o.from * m + o.item] += 1;
        buyers[o.to * m + o.item] += 1;
    }
    for o in &mut offers {
        o.units /= sellers[o.from * m + o.item].max(buyers[o.to * m + o.item]) as f64;
    }
    let scale = |offers: &mut Vec<Offer>, key: &dyn Fn(&Offer) -> usize, amount: &dyn Fn(&Offer) -> f64, cap: &dyn Fn(usize) -> f64, len: usize| {
        let mut sum = vec![0.0; len];
        for o in offers.iter() {
            sum[key(o)] += amount(o);
        }
        for o in offers.iter_mut() {
            let (s, c) = (sum[key(o)], cap(key(o)));
            if s > c {
                o.units *= c / s;
            }
        }
    };
    // A seller sells at most `most_sold` of a store; a buyer spends at most `most_spent` of its coins.
    scale(&mut offers, &|o| o.from * m + o.item, &|o| o.units, &|at| t.most_sold * sides[at].held, n * m);
    let unit_cost = |o: &Offer| {
        let s = sides[o.from * m + o.item];
        levels[o.from] * (s.base * s.factor + s.carry * o.days)
    };
    scale(&mut offers, &|o| o.to, &|o| o.units * unit_cost(o), &|prov| t.most_spent * stock[prov * k + coin] as f64, n);

    // Whole units, taken from the seller's goods of the item in proportion to what it holds.
    let mut out = Vec::new();
    for o in offers {
        let units = o.units.floor() as u64;
        if units == 0 {
            continue;
        }
        let (s, d) = (sides[o.from * m + o.item], sides[o.to * m + o.item]);
        let cols = &r.items[o.item].cols;
        let (ls, ld) = (levels[o.from], levels[o.to]);
        let unit = |c: usize| base(c) * ls * s.factor + r.carriage[c] * ls * o.days;
        let goods = if let [c] = cols[..] {
            vec![(c, units, unit(c))]
        } else {
            let held: Vec<u64> = cols.iter().map(|&c| stock[o.from * k + c]).collect();
            let ids: Vec<u64> = cols.iter().map(|&c| c as u64).collect();
            let split = split_proportional(units, &held, &ids);
            cols.iter().zip(split).filter(|x| x.1 > 0).map(|(&c, x)| (c, x, unit(c))).collect()
        };
        out.push(Deal {
            from: o.from as u32,
            to: o.to as u32,
            goods,
            item: o.item,
            prices: (s.base * ls * s.factor, s.base * ld * d.factor),
            carrying: s.carry * ls * o.days,
        });
    }
    out
}

/// Carry out `deals` in order: the goods go, and the coins come back. A seller never sends more
/// than it holds nor a buyer pays more than it has (the deals were scaled to leave room for that).
/// Returns what the `traced` province did.
fn settle(r: &Resolved, deals: &[Deal], stock: &mut [u64], goods: &Goods, books: &mut Books, traced: Option<u32>) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(&coin) = r.coins.first() else { return notes };
    let k = goods.stride();
    // What moved, per class: what came in somewhere went out somewhere else.
    let mut moved = vec![0i64; goods.tree.classes().len()];
    for d in deals {
        let (from, to) = (d.from as usize * k, d.to as usize * k);
        let mut owed = 0.0;
        let mut sent = 0;
        for &(c, units, price) in &d.goods {
            let units = units.min(stock[from + c]);
            stock[from + c] -= units;
            stock[to + c] += units;
            moved[goods.class_of_column(c) as usize] += units as i64;
            owed += units as f64 * price;
            sent += units;
        }
        let paid = (owed.round() as u64).min(stock[to + coin]);
        stock[to + coin] -= paid;
        stock[from + coin] += paid;
        moved[goods.class_of_column(coin) as usize] += paid as i64;
        for (who, other, verb) in [(d.from, d.to, "sold"), (d.to, d.from, "bought")] {
            if traced == Some(who) {
                notes.push(format!(
                    "{verb} {sent} {} {} province {other} for {paid} coins ({:.3} a unit where sold, {:.3} where bought, {:.3} to carry)",
                    r.items[d.item].name,
                    if verb == "sold" { "to" } else { "from" },
                    d.prices.0,
                    d.prices.1,
                    d.carrying,
                ));
            }
        }
    }
    for (class, &x) in moved.iter().enumerate() {
        books.add(class as u8, Cause::TradeOut, -x);
        books.add(class as u8, Cause::TradeIn, x);
    }
    notes
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
    let ratio = (wanted + slack) / (held + slack);
    let ratio = if p.price_response == 1.0 { ratio } else { ratio.powf(p.price_response) };
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

    /// The shipped goods and parameters, for trying trade on a few made-up provinces. Columns:
    /// grain, fish, livestock, timber, stone, ore, cloth, tools, coin. Items: food, then timber,
    /// stone, ore, cloth and tools.
    fn shipped() -> (MarketParams, Resolved, Goods) {
        let goods = Goods::new(crate::taxonomy::Taxonomy::standard_goods(), V1.goods).unwrap();
        let p = MarketParams::default();
        let r = Resolved::new(&p, &goods).unwrap();
        assert_eq!(r.items.iter().map(|it| it.name.as_str()).collect::<Vec<_>>(), ["food", "timber", "stone", "ore", "cloth", "tools"]);
        (p, r, goods)
    }

    const K: usize = 9;
    const COIN: usize = 8;
    const ITEMS: usize = 6;

    /// `n` provinces holding nothing and wanting nothing, with a million coins each.
    fn provinces(n: usize) -> (Vec<u64>, Vec<f64>) {
        let mut stock = vec![0; n * K];
        (0..n).for_each(|p| stock[p * K + COIN] = 1_000_000);
        (stock, vec![0.0; n * ITEMS])
    }

    fn way(a: u32, b: u32, land: Option<f32>, sea: Option<f32>) -> Link {
        Link { a, b, border_km: 1.0, land_days: land, sea_days: sea }
    }

    /// A province's price of an item against its base, as trade sees it.
    fn factor(p: &MarketParams, r: &Resolved, stock: &[u64], wanted: &[f64], prov: usize, item: usize) -> f64 {
        let held: u64 = r.items[item].cols.iter().map(|&c| stock[prov * K + c]).sum();
        target_price(p, 1.0, wanted[prov * ITEMS + item], held as f64)
    }

    #[test]
    fn food_goes_where_it_is_dear_and_coins_come_back() {
        let (p, r, goods) = shipped();
        let t = p.trade.clone().unwrap();
        let (mut stock, mut wanted) = provinces(2);
        // Province 0 has plenty of grain and fish, province 1 almost none; both want the same.
        stock[0] = 900_000;
        stock[1] = 100_000;
        stock[K] = 1_000;
        wanted[0] = 500_000.0;
        wanted[ITEMS] = 500_000.0;
        let people = [1000, 1000];
        let deals = deals(&r, &p, &t, &stock, &wanted, &people, &[way(0, 1, Some(2.0), None)]);
        assert_eq!(deals.len(), 1, "{deals:?}");
        let d = &deals[0];
        assert_eq!((d.from, d.to, d.item), (0, 1, 0));
        // Food is taken from the seller's grain and fish as it holds them, nine to one.
        let (grain, fish) = (d.goods[0].1, d.goods[1].1);
        assert_eq!((d.goods[0].0, d.goods[1].0), (0, 1));
        assert!(grain > 0 && (grain as f64 / fish as f64 - 9.0).abs() < 0.1, "{d:?}");
        // At most half the seller's food goes in a month.
        assert!(grain + fish <= 500_000);
        let before = stock.clone();
        let mut books = Books::default();
        let notes = settle(&r, &deals, &mut stock, &goods, &mut books, Some(1));
        // Goods and coins only moved: every class balances.
        for c in 0..K {
            assert_eq!(stock[c] + stock[K + c], before[c] + before[K + c]);
        }
        let paid = before[K + COIN] - stock[K + COIN];
        assert_eq!(stock[COIN] - before[COIN], paid);
        // The buyer paid the seller's price and the carrying, for each good.
        let owed: f64 = d.goods.iter().map(|&(_, u, price)| u as f64 * price).sum();
        assert_eq!(paid, owed.round() as u64);
        assert!(books.0.iter().all(|(&(_, cause), _)| matches!(cause, Cause::TradeIn | Cause::TradeOut)));
        for class in [0u8, 3] {
            assert_eq!(books.0[&(class, Cause::TradeIn)], -books.0[&(class, Cause::TradeOut)]);
        }
        assert!(notes.len() == 1 && notes[0].starts_with(&format!("bought {} food from province 0 for {paid} coins", grain + fish)), "{notes:?}");
        // The gap has narrowed.
        assert!(factor(&p, &r, &stock, &wanted, 1, 0) < factor(&p, &r, &before, &wanted, 1, 0));
    }

    #[test]
    fn nothing_moves_when_carrying_costs_more_than_the_gap() {
        let (p, r, _) = shipped();
        let t = p.trade.clone().unwrap();
        let (mut stock, mut wanted) = provinces(2);
        // Grain a fifth dearer in province 1: 0.02 a ration more, which two days' cart eats up (0.04).
        stock[0] = 500_000;
        stock[K] = 500_000;
        wanted[0] = 500_000.0;
        wanted[ITEMS] = 600_000.0;
        let people = [1000, 1000];
        assert_eq!(deals(&r, &p, &t, &stock, &wanted, &people, &[way(0, 1, Some(2.0), None)]), []);
        // By sea, two days cost a tenth as much: it pays.
        assert_eq!(deals(&r, &p, &t, &stock, &wanted, &people, &[way(0, 1, None, Some(2.0))]).len(), 1);
        // With nobody living at one end, nothing is traded.
        assert_eq!(deals(&r, &p, &t, &stock, &wanted, &[1000, 0], &[way(0, 1, None, Some(2.0))]), []);
    }

    #[test]
    fn a_buyer_spends_at_most_half_its_coins_and_a_seller_sells_at_most_half_its_store() {
        let (p, r, _) = shipped();
        let t = p.trade.clone().unwrap();
        // Province 0 has tools; 1, 2 and 3 have none and need many, and 1 has only 1,000 coins.
        let (mut stock, mut wanted) = provinces(4);
        stock[7] = 10_000;
        stock[K + COIN] = 1_000;
        for q in 1..4 {
            wanted[q * ITEMS + 5] = 1_000_000.0;
        }
        let ways = [way(0, 1, Some(1.0), None), way(0, 2, Some(1.0), None), way(0, 3, Some(1.0), None)];
        let deals = deals(&r, &p, &t, &stock, &wanted, &[10; 4], &ways);
        assert_eq!(deals.len(), 3, "{deals:?}");
        let cost = |d: &Deal| d.goods.iter().map(|&(_, u, price)| u as f64 * price).sum::<f64>();
        assert!(cost(&deals[0]) <= 500.0 && cost(&deals[0]) > 400.0, "{deals:?}");
        let sold: u64 = deals.iter().map(|d| d.goods[0].1).sum();
        assert!(sold <= 5_000 && sold > 3_000, "{sold}");
    }

    #[test]
    fn the_order_of_the_ways_does_not_matter() {
        let (p, r, _) = shipped();
        let t = p.trade.clone().unwrap();
        let (mut stock, mut wanted) = provinces(4);
        for (q, (grain, cloth)) in [(900_000, 10), (10_000, 5_000), (300_000, 0), (50_000, 2_000)].into_iter().enumerate() {
            stock[q * K] = grain;
            stock[q * K + 6] = cloth;
            wanted[q * ITEMS] = 300_000.0;
            wanted[q * ITEMS + 4] = 1_000.0;
        }
        let ways = vec![way(0, 1, Some(1.0), None), way(0, 2, Some(3.0), Some(4.0)), way(1, 3, Some(2.0), None), way(2, 3, None, Some(6.0))];
        let people = [100; 4];
        let mut forward = deals(&r, &p, &t, &stock, &wanted, &people, &ways);
        let mut backward = deals(&r, &p, &t, &stock, &wanted, &people, &ways.iter().rev().copied().collect::<Vec<_>>());
        assert!(forward.len() >= 4);
        let key = |d: &Deal| (d.from, d.to, d.item);
        forward.sort_by_key(key);
        backward.sort_by_key(key);
        assert_eq!(forward, backward);
    }

    #[test]
    fn trade_settles_without_goods_going_back_and_forth() {
        let (p, r, goods) = shipped();
        // Stores alone: the coins the food end earns would make it dearer too.
        let t = TradeParams { money_response: 0.0, ..p.trade.clone().unwrap() };
        // A row of five provinces a day apart, all food at one end.
        let n = 5;
        let (mut stock, mut wanted) = provinces(n);
        stock[0] = 5_000_000;
        for q in 0..n {
            stock[q * K + 1] = 10_000;
            wanted[q * ITEMS] = 1_000_000.0;
        }
        let ways: Vec<Link> = (0..n as u32 - 1).map(|q| way(q, q + 1, Some(1.0), None)).collect();
        let mut seen: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        for _ in 0..60 {
            let deals = deals(&r, &p, &t, &stock, &wanted, &[100; 5], &ways);
            for d in &deals {
                // Food only ever goes one way along each way: down the row.
                *seen.entry((d.from, d.to)).or_default() += 1;
            }
            settle(&r, &deals, &mut stock, &goods, &mut Books::default(), None);
        }
        assert_eq!(seen.keys().copied().collect::<Vec<_>>(), [(0, 1), (1, 2), (2, 3), (3, 4)]);
        // By then the gaps are about what a day's carrying costs, or less, and the far end is fed.
        for q in 0..n - 1 {
            let gap = 0.1 * (factor(&p, &r, &stock, &wanted, q + 1, 0) - factor(&p, &r, &stock, &wanted, q, 0));
            assert!(gap < 0.03, "between {q} and {}: {gap}", q + 1);
        }
        assert!(stock[(n - 1) * K] > 500_000, "{stock:?}");
    }

    #[test]
    fn coins_flow_back_from_where_they_pile_up_without_goods_going_back_and_forth() {
        let (p, r, goods) = shipped();
        let t = p.trade.clone().unwrap();
        assert!(t.money_response > 0.0);
        // Two provinces alike in everything but coins: one has three times the other's.
        let (mut stock, mut wanted) = provinces(2);
        for q in 0..2 {
            for (c, held) in [(0, 400_000), (3, 3_000), (4, 1_000), (5, 300), (6, 2_000), (7, 500)] {
                stock[q * K + c] = held;
            }
            for (item, want) in [(0, 400_000.0), (1, 3_000.0), (2, 1_000.0), (3, 300.0), (4, 2_000.0), (5, 500.0)] {
                wanted[q * ITEMS + item] = want;
            }
        }
        stock[COIN] = 3_000_000;
        let usual = stock.clone();
        let ways = [way(0, 1, Some(1.0), None)];
        let mut seen: BTreeMap<usize, (u32, u32)> = BTreeMap::new();
        let mut gaps = Vec::new();
        for _ in 0..120 {
            // Each month a province makes and uses goods, so its stores drift back toward the usual:
            // what the rich one bought gets used up, and it buys again.
            for q in 0..2 {
                for c in 0..COIN {
                    let at = q * K + c;
                    stock[at] = (0.8 * stock[at] as f64 + 0.2 * usual[at] as f64).round() as u64;
                }
            }
            let deals = deals(&r, &p, &t, &stock, &wanted, &[1000, 1000], &ways);
            for d in &deals {
                // Each good goes only one way: to the province with the coins.
                assert_eq!(*seen.entry(d.item).or_insert((d.from, d.to)), (d.from, d.to), "{} turned back", r.items[d.item].name);
            }
            settle(&r, &deals, &mut stock, &goods, &mut Books::default(), None);
            gaps.push(stock[COIN] as i64 - stock[K + COIN] as i64);
        }
        // The coins even out, never overshooting.
        assert!(gaps.iter().all(|&g| g >= 0) && gaps.windows(2).all(|w| w[1] <= w[0]));
        assert!(*gaps.last().unwrap() < 1_300_000, "{gaps:?}");
        assert!(seen.values().all(|&(from, to)| (from, to) == (1, 0)));
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
