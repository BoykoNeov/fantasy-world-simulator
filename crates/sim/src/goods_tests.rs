//! Per-good values with several goods kept at once. Economics v0 keeps one good (food), so these
//! tests put a stand-in economy in its slot that keeps grain, fish, ore and coins: three classes,
//! four columns per province. The last tests turn Population on and check it asks for food through
//! the goods tree, whichever goods the economy keeps.

use crate::config::Config;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::fields::{known, quantity_total, Quantity, Tier};
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::{ration, serve_goods, Cause, Request};
use crate::sim::{Problems, Sim, NEVER};
use crate::taxonomy::TreeDef;
use crate::world::{World, WorldParams};

static PANTRY: Manifest = Manifest {
    slot: Slot::Economics,
    version: "pantry",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    reads: &[],
    reads_earlier: &[],
    owns: &["province.stock"],
    requests: &[],
    asks: &[],
    moves: &[],
    requires: &[],
    emits: &[],
    handles: &[],
    state_schema: "none",
    goods: &["grain", "fish", "ore", "coin"],
};

const FOOD: Quantity = Quantity::Goods(0);
const RAW: Quantity = Quantity::Goods(1);
const MONEY: Quantity = Quantity::Goods(3);

/// Each month every province grows 3 grain and digs 2 ore; fish never changes; province 0 pays
/// province 1 five coins, which moves coins without making or losing any. With `leak`, a unit of
/// province 0's ore turns into grain and province 2 finds a coin, and no flow says so.
struct Pantry {
    leak: bool,
}

impl Module for Pantry {
    fn manifest(&self) -> &'static Manifest {
        &PANTRY
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        // Columns in the order kept: grain, fish, ore, coin.
        w.stock = (0..w.provinces.count as u64).flat_map(|p| [100 + p, 50, 10, 1_000]).collect();
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let g = w.goods.clone();
        let (grain, ore, coin) = (g.column("grain").unwrap(), g.column("ore").unwrap(), g.column("coin").unwrap());
        for p in 0..w.provinces.count {
            w.stock[g.index(p, grain)] += 3;
            w.stock[g.index(p, ore)] += 2;
        }
        let n = w.provinces.count as i64;
        cx.flow(FOOD, Cause::Harvest, 3 * n);
        cx.flow(RAW, Cause::Harvest, 2 * n);
        w.stock[g.index(0, coin)] -= 5;
        w.stock[g.index(1, coin)] += 5;
        if self.leak {
            w.stock[g.index(0, ore)] -= 1;
            w.stock[g.index(0, grain)] += 1;
            w.stock[g.index(2, coin)] += 1;
        }
    }
}

fn config() -> Config {
    let mut c = Config::new(WorldParams { n: 24, provinces: 40, seed: 5, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.population = None;
    c
}

fn modules(config: &Config, leak: bool) -> Vec<Box<dyn Module>> {
    let swap = |m: Box<dyn Module>| -> Box<dyn Module> { if m.manifest().slot == Slot::Economics { Box::new(Pantry { leak }) } else { m } };
    config.modules.build().into_iter().map(swap).collect()
}

fn world(leak: bool) -> Sim {
    let config = config();
    let mut sim = Sim::new_with(&config, modules(&config, leak)).unwrap();
    sim.strict = true;
    sim
}

#[test]
fn several_goods_are_kept_counted_stamped_and_saved() {
    let mut sim = world(false);
    let n = sim.world.provinces.count;
    let g = sim.world.goods.clone();
    assert_eq!(g.names(), ["grain", "fish", "ore", "coin"]);
    assert_eq!(sim.world.stock.len(), 4 * n);
    assert_eq!(sim.world.quantities(), [Quantity::People, FOOD, RAW, MONEY]);
    assert_eq!(MONEY.name(&g), "money");
    let total = |sim: &Sim, q| quantity_total(&sim.world, q);
    let (food, raw) = (total(&sim, FOOD), total(&sim, RAW));
    assert_eq!((raw, total(&sim, MONEY)), (10 * n as u64, 1_000 * n as u64));

    // Two month starts: day 0 and the first day of the second month. Coins changed hands twice,
    // but none were made or lost.
    let month = sim.calendar.def.months[0].days as u64;
    sim.run_days(month + 1);
    assert_eq!((total(&sim, FOOD), total(&sim, RAW)), (food + 6 * n as u64, raw + 4 * n as u64));
    assert_eq!(total(&sim, MONEY), 1_000 * n as u64);
    assert_eq!((sim.world.stock_of(0, "coin"), sim.world.stock_of(1, "coin"), sim.world.stock_under(1, "money")), (Some(990), Some(1_010), Some(1_010)));
    assert_eq!(sim.problems, Problems::default());
    // The year's ledger counts food only.
    let year = sim.year_so_far();
    assert_eq!((year.ledger.produced, year.food_stock), (6 * n as u64, total(&sim, FOOD)));

    // Grain and ore last changed at the second month start; fish never did.
    let stamps = &sim.last_change[known("province.stock").0 as usize];
    let at = |c: &str| stamps[g.index(1, g.column(c).unwrap())];
    assert_eq!((at("grain"), at("ore"), at("fish")), (month, month, NEVER));

    // Going back and forth gives the same world, and so does saving and loading it.
    let (print, today) = (sim.fingerprint(), sim.day);
    sim.goto(3).unwrap();
    assert_eq!(sim.world.stock[g.index(1, 0)], 101 + 3);
    sim.goto(today).unwrap();
    assert_eq!(sim.fingerprint(), print);
    let loaded = Sim::load_with(&sim.save(), |c| modules(c, false)).unwrap();
    assert_eq!(loaded.fingerprint(), print);
    assert_eq!(loaded.world.goods.names(), ["grain", "fish", "ore", "coin"]);
}

#[test]
fn conservation_is_checked_per_class() {
    // Ore turned into grain leaves the whole store's total as it was, but food and raw goods each
    // changed unexplained; so did money, by the coin found.
    let mut sim = world(true);
    sim.run_days(2);
    assert_eq!(sim.problems.conservation_breaks, 3);
    let broken: Vec<(Quantity, i64, Vec<Slot>)> = sim
        .event_log
        .entries
        .iter()
        .filter_map(|p| match &p.event {
            Event::ConservationBroken { quantity, expected, actual, by } => Some((*quantity, *actual as i64 - *expected as i64, by.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(broken, vec![(FOOD, 1, vec![Slot::Economics]), (RAW, -1, vec![Slot::Economics]), (MONEY, 1, vec![Slot::Economics])]);
}

#[test]
fn clamping_books_each_class_on_its_own() {
    let mut sim = world(false);
    sim.run_days(1);
    let g = sim.world.goods.clone();
    let ore = g.index(2, g.column("ore").unwrap());
    // Far past the store's bound, as a buggy edit might leave it; the next month's work clamps it.
    sim.world.stock[ore] = 5_000_000_000_000_000;
    sim.run_days(sim.calendar.def.months[0].days as u64 + 1);
    assert_eq!((sim.problems.contract_violations, sim.problems.conservation_breaks), (1, 0));
    assert_eq!(sim.year_flows.get(RAW, Cause::ContractClamp), 1_000_000_000_000_000 - 5_000_000_000_000_002);
    assert_eq!(sim.year_flows.get(FOOD, Cause::ContractClamp), 0);
    let clamped = sim.event_log.entries.iter().find_map(|p| match &p.event {
        Event::ContractViolated { field, entity, .. } => Some((field.def().name, *entity as usize)),
        _ => None,
    });
    assert_eq!(clamped, Some(("province.stock", ore)));
}

#[test]
fn edits_move_every_good() {
    let mut sim = world(false);
    let g = sim.world.goods.clone();
    let w = &sim.world;
    let p = (0..w.provinces.count).max_by_key(|&p| w.provinces.cells[p]).unwrap();
    let cells: Vec<u32> = (0..w.grid.cells() as u32).filter(|&c| w.provinces.of_cell[c as usize] == p as u32).collect();
    let before: Vec<u64> = (0..4).map(|c| w.stock[g.index(p, c)]).collect();

    // Half its land becomes a new province, which takes a share of each good and of the coins.
    sim.edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    let new = sim.world.provinces.count - 1;
    assert_eq!(sim.world.stock.len(), 4 * (new + 1));
    let w = &sim.world;
    for c in 0..4 {
        assert_eq!(w.stock[g.index(p, c)] + w.stock[g.index(new, c)], before[c], "column {c}");
        assert!(w.stock[g.index(new, c)] > 0, "column {c}");
    }
    // Merged back, it all returns.
    sim.edit(Edit::Merge { from: new as u32, into: p as u32 }).unwrap();
    assert_eq!((0..4).map(|c| sim.world.stock[g.index(p, c)]).collect::<Vec<_>>(), before);
    assert_eq!(sim.problems, Problems::default());

    // Any good kept on its own can be set; the change is the edit's doing.
    let set = |good: &str, amount| Edit::SetStock { province: 2, good: good.into(), amount };
    let (grain, ore) = (g.index(2, g.column("grain").unwrap()), g.index(2, g.column("ore").unwrap()));
    let (had_grain, had_ore) = (sim.world.stock[grain], sim.world.stock[ore]);
    sim.edit(set("grain", 7)).unwrap();
    sim.edit(set("ore", had_ore + 40)).unwrap();
    assert_eq!((sim.world.stock[grain], sim.world.stock[ore]), (7, had_ore + 40));
    assert_eq!((sim.year_flows.get(FOOD, Cause::Edit), sim.year_flows.get(RAW, Cause::Edit)), (7 - had_grain as i64, 40));
    // Coins are set the same way, and the money they add is the edit's doing too.
    let coin = g.index(2, g.column("coin").unwrap());
    let had_coins = sim.world.stock[coin];
    sim.edit(set("coin", had_coins + 250)).unwrap();
    assert_eq!((sim.world.stock[coin], sim.year_flows.get(MONEY, Cause::Edit)), (had_coins + 250, 250));
    assert_eq!(sim.problems, Problems::default());
    // Food is kept as grain and fish here, so it can't be set as one store; nor can what isn't kept.
    let food_whole = "food is kept as grain and fish; set each of those";
    assert_eq!(sim.edit(Edit::SetFoodStock { province: 0, rations: 5 }).unwrap_err(), food_whole);
    assert_eq!(sim.edit(set("food", 5)).unwrap_err(), food_whole);
    assert_eq!(sim.edit(set("cloth", 5)).unwrap_err(), "this world keeps no cloth");
    assert_eq!(sim.edit(set("spice", 5)).unwrap_err(), "there is no good called spice");
    assert!(sim.edit(set("grain", 2_000_000_000_000_000)).unwrap_err().contains("more than province.stock can hold"));
    // Refused edits leave nothing in the log.
    let logged = sim.commands.iter().filter(|c| matches!(&c.kind, crate::sim::CommandKind::Edit { edit: Edit::SetStock { .. } })).count();
    assert_eq!(logged, 3);

    // The sea covers everything: every good and coin is lost, and the year's flows say so.
    let had = [FOOD, RAW, MONEY].map(|q| (quantity_total(&sim.world, q), sim.year_flows.get(q, Cause::Edit)));
    sim.edit(Edit::SeaLevel { rise_m: 20_000.0 }).unwrap();
    assert!(sim.world.stock.iter().all(|&s| s == 0));
    let lost = [FOOD, RAW, MONEY].map(|q| sim.year_flows.get(q, Cause::Edit));
    assert_eq!(lost, had.map(|(total, set)| set - total as i64));
    assert_eq!(sim.problems, Problems::default());
}

/// Keeps the goods its manifest names, each province starting with `fill(good)` of each. Nothing is
/// ever made: it only serves what is asked, by the shared rule.
struct Larder {
    manifest: &'static Manifest,
    fill: fn(&str) -> u64,
}

impl Module for Larder {
    fn manifest(&self) -> &'static Manifest {
        self.manifest
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        let row: Vec<u64> = w.goods.names().iter().map(|n| (self.fill)(n)).collect();
        w.stock = (0..w.provinces.count).flat_map(|_| row.clone()).collect();
    }

    fn apply_requests(&mut self, w: &mut World, requests: &[Request], cx: &mut Ctx) {
        serve_goods(&mut w.stock, requests, cx);
    }
}

const fn larder(goods: &'static [&'static str]) -> Manifest {
    Manifest {
        slot: Slot::Economics,
        version: "larder",
        tier: Tier::Province,
        cadence: Cadence::Monthly,
        reads: &[],
        reads_earlier: &[],
        owns: &["province.stock"],
        requests: &[],
        asks: &[],
        moves: &[],
        requires: &[],
        emits: &[EventKind::Granted],
        handles: &[],
        state_schema: "none",
        goods,
    }
}

static GRAIN_FISH_LIVESTOCK: Manifest = larder(&["grain", "fish", "livestock", "ore"]);

fn eating_config(tree: Option<&str>) -> Config {
    let mut config = Config::new(WorldParams { n: 24, provinces: 40, seed: 5, land_fraction: 0.4, radius_km: 6371.0 });
    if let Some(t) = tree {
        config.goods = TreeDef::from_toml(t).unwrap();
    }
    config
}

fn with_larder(config: &Config, manifest: &'static Manifest, fill: fn(&str) -> u64) -> Result<Sim, crate::module::ConfigError> {
    let swap = |m: Box<dyn Module>| -> Box<dyn Module> { if m.manifest().slot == Slot::Economics { Box::new(Larder { manifest, fill }) } else { m } };
    Sim::new_with(config, config.modules.build().into_iter().map(swap).collect())
}

fn eating_world(manifest: &'static Manifest, tree: Option<&str>, fill: fn(&str) -> u64) -> Sim {
    let mut sim = with_larder(&eating_config(tree), manifest, fill).map_err(|e| e.0).unwrap();
    sim.strict = true;
    sim
}

/// Day 0 starts a month: every pop asks for its month of food, served tonight. Returns the requests
/// served, with what each got.
fn first_meal(sim: &mut Sim) -> Vec<(Request, u64)> {
    sim.run_days(1);
    sim.applied_today.clone()
}

/// How much of the good named `c` province `p` lost between `before` and now.
fn taken(sim: &Sim, before: &[u64], p: usize, c: &str) -> u64 {
    let g = &sim.world.goods;
    let at = g.index(p, g.column(c).unwrap());
    before[at] - sim.world.stock[at]
}

#[test]
fn population_asks_for_food_and_gets_it_from_every_food_good() {
    let mut sim = eating_world(&GRAIN_FISH_LIVESTOCK, None, |_| 1_000_000_000);
    let food = sim.world.goods.tree.find("food").unwrap();
    let before = sim.world.stock.clone();
    let served = first_meal(&mut sim);

    // One request per pop, for food in its province, and one grant each: plenty for all.
    let pops = &sim.world.pops;
    assert_eq!(served.len(), pops.size.len());
    for (r, granted) in &served {
        assert_eq!((r.good, r.entity, *granted), (Some(food), pops.province[r.tag as usize], r.take));
    }
    // Each province's meal came out of grain, fish and livestock alike (they held the same), and
    // no ore was touched.
    for p in 0..sim.world.provinces.count {
        let mine = || served.iter().filter(|(r, _)| r.entity == p as u32);
        let (eaten, meals) = (mine().map(|(_, g)| g).sum::<u64>(), mine().count() as u64);
        let food_taken = ["grain", "fish", "livestock"].map(|c| taken(&sim, &before, p, c));
        assert_eq!(food_taken.iter().sum::<u64>(), eaten);
        assert!(food_taken.iter().all(|t| t.abs_diff(eaten / 3) <= meals), "province {p}: {food_taken:?} of {eaten}");
        assert_eq!(taken(&sim, &before, p, "ore"), 0);
    }
    let eaten: u64 = served.iter().map(|(_, g)| g).sum();
    assert_eq!((sim.year_flows.get(FOOD, Cause::Eating), sim.year_flows.get(RAW, Cause::Eating)), (-(eaten as i64), 0));
    let p0: u64 = served.iter().filter(|(r, _)| r.entity == 0).map(|(_, g)| g).sum();
    assert_eq!(sim.world.stock_under(0, "food"), Some(3_000_000_000 - p0));
    // The grants are delivered and settled; everything balanced.
    sim.run_days(1);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_food_shortage_is_shared_out_from_all_the_food_there_is() {
    // 30 grain, 20 fish and no livestock per province: far less than a month of eating.
    let mut sim = eating_world(&GRAIN_FISH_LIVESTOCK, None, |good| match good {
        "grain" => 30,
        "fish" => 20,
        "livestock" => 0,
        _ => 5,
    });
    let served = first_meal(&mut sim);
    let g = sim.world.goods.clone();
    let eating: Vec<u32> = (0..sim.world.provinces.count as u32).filter(|&p| served.iter().any(|(r, _)| r.entity == p)).collect();
    assert!(!eating.is_empty());
    for p in eating {
        let mine: Vec<&(Request, u64)> = served.iter().filter(|(r, _)| r.entity == p).collect();
        let wants: Vec<_> = mine.iter().map(|(r, _)| (r.priority, r.take, r.tag)).collect();
        // The province's 50 rations are shared by the one rule, as a single store's would be.
        assert_eq!(mine.iter().map(|(_, g)| *g).collect::<Vec<_>>(), ration(50, &wants));
        let left = ["grain", "fish", "livestock", "ore"].map(|c| sim.world.stock[g.index(p as usize, g.column(c).unwrap())]);
        assert_eq!(left, [0, 0, 0, 5]);
    }
    sim.run_days(1);
    assert!(sim.year_so_far().ledger.starved > 0);
    assert_eq!(sim.problems, Problems::default());
}

/// A deeper tree: cereals is a class of food with goods of its own.
const DEEP: &str = "root = 'goods'
[children]
goods = ['food', 'raw']
food = ['cereals', 'fish']
cereals = ['wheat', 'barley']
raw = ['ore']
[units]
food = 'rations'
";

static CEREALS_WHOLE: Manifest = larder(&["cereals", "fish", "ore"]);
static CEREALS_SPLIT: Manifest = larder(&["wheat", "barley", "fish"]);

#[test]
fn food_is_found_however_deep_the_tree_and_whichever_level_is_kept() {
    for (manifest, food_goods) in [(&CEREALS_WHOLE, &["cereals", "fish"][..]), (&CEREALS_SPLIT, &["wheat", "barley", "fish"][..])] {
        let mut sim = eating_world(manifest, Some(DEEP), |_| 1_000_000_000);
        let names = sim.world.goods.names();
        let before = sim.world.stock.clone();
        let served = first_meal(&mut sim);
        assert!(!served.is_empty() && served.iter().all(|(r, granted)| *granted == r.take), "{names:?}");
        // Every food good kept gave its share, in every province that ate.
        for p in 0..sim.world.provinces.count {
            let eaten: u64 = served.iter().filter(|(r, _)| r.entity == p as u32).map(|(_, g)| g).sum();
            let food_taken: Vec<u64> = food_goods.iter().map(|c| taken(&sim, &before, p, c)).collect();
            assert_eq!(food_taken.iter().sum::<u64>(), eaten, "{names:?}");
            assert!(eaten == 0 || food_taken.iter().all(|&t| t > 0), "{names:?}: {food_taken:?}");
        }
        // Reading a class sums its goods; a good kept only inside a bigger whole can't be read.
        let (w, g) = (&sim.world, &sim.world.goods);
        let held = |c: &str| w.stock[g.index(0, g.column(c).unwrap())];
        let food: u64 = food_goods.iter().map(|c| held(c)).sum();
        assert_eq!((w.stock_under(0, "food"), w.stock_under(0, "cereals")), (Some(food), Some(food - held("fish"))));
        assert_eq!(w.stock_under(0, "wheat").is_some(), g.column("wheat").is_some());
        assert_eq!(w.stock_under(0, "spice"), None);
        sim.run_days(40);
        assert_eq!(sim.problems, Problems::default(), "{names:?}");
    }
}

#[test]
fn a_good_kept_only_inside_a_bigger_whole_cannot_be_set_alone() {
    let mut sim = eating_world(&CEREALS_WHOLE, Some(DEEP), |_| 1_000);
    let wheat = Edit::SetStock { province: 0, good: "wheat".into(), amount: 5 };
    assert_eq!(sim.edit(wheat).unwrap_err(), "wheat is kept only as part of cereals; set the cereals store instead");
    sim.edit(Edit::SetStock { province: 0, good: "cereals".into(), amount: 5 }).unwrap();
    assert_eq!(sim.world.stock_under(0, "cereals"), Some(5));
}

#[test]
fn edits_to_what_no_module_keeps_are_refused() {
    let mut config = Config::new(WorldParams { n: 24, provinces: 40, seed: 5, land_fraction: 0.4, radius_km: 6371.0 });
    (config.modules.economics, config.modules.population) = (None, None);
    let mut sim = Sim::new(&config).unwrap();
    sim.strict = true;
    let nothing = |what: &str| format!("no module keeps {what} in this world, so there are none to set");
    assert_eq!(sim.edit(Edit::SetFoodStock { province: 0, rations: 5 }).unwrap_err(), nothing("stored goods"));
    assert_eq!(sim.edit(Edit::SetStock { province: 0, good: "food".into(), amount: 5 }).unwrap_err(), nothing("stored goods"));
    let people = crate::edit::PopValues::people(10);
    assert_eq!(sim.edit(Edit::AddPop { province: 0, values: people }).unwrap_err(), nothing("people"));
    assert!(sim.world.pops.size.is_empty() && sim.world.stock.iter().all(|&s| s == 0));

    // The map can still be edited: there is just nothing to move.
    let w = &sim.world;
    let p = (0..w.provinces.count as u32).max_by_key(|&p| w.provinces.cells[p as usize]).unwrap();
    let cells: Vec<u32> = (0..w.grid.cells() as u32).filter(|&c| w.provinces.of_cell[c as usize] == p).collect();
    sim.edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    sim.run_days(40);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_world_whose_economy_keeps_no_food_is_refused_when_population_eats() {
    static ORE_ONLY: Manifest = larder(&["ore"]);
    let refused = with_larder(&eating_config(None), &ORE_ONLY, |_| 0).err().unwrap().0;
    assert!(refused.contains("Population \"basic\" asks for food, but Economics \"larder\" keeps none"), "{refused}");
    // A goods tree without food is refused as well.
    let refused = with_larder(&eating_config(Some("root = 'goods'\n[children]\ngoods = ['raw']\nraw = ['ore']")), &ORE_ONLY, |_| 0).err().unwrap().0;
    assert!(refused.contains("asks for food, which is not a good"), "{refused}");
}
