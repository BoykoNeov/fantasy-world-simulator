//! Per-good values with several goods kept at once. Economics v0 keeps one good (food), so these
//! tests put a stand-in economy in its slot that keeps grain, fish and ore: two classes, three
//! columns per province.

use crate::config::Config;
use crate::edit::Edit;
use crate::events::Event;
use crate::fields::{known, quantity_total, Quantity, Tier};
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::Cause;
use crate::sim::{Problems, Sim, NEVER};
use crate::world::{World, WorldParams};

static PANTRY: Manifest = Manifest {
    slot: Slot::Economics,
    version: "pantry",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    reads: &[],
    owns: &["province.stock"],
    requests: &[],
    requires: &[],
    emits: &[],
    handles: &[],
    state_schema: "none",
    goods: &["grain", "fish", "ore"],
};

const FOOD: Quantity = Quantity::Goods(0);
const RAW: Quantity = Quantity::Goods(1);

/// Each month every province grows 3 grain and digs 2 ore; fish never changes. With `leak`, a unit
/// of province 0's ore turns into grain, and no flow says so.
struct Pantry {
    leak: bool,
}

impl Module for Pantry {
    fn manifest(&self) -> &'static Manifest {
        &PANTRY
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        // Columns in the order kept: grain, fish, ore.
        w.stock = (0..w.provinces.count as u64).flat_map(|p| [100 + p, 50, 10]).collect();
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let g = w.goods.clone();
        let (grain, ore) = (g.column("grain").unwrap(), g.column("ore").unwrap());
        for p in 0..w.provinces.count {
            w.stock[g.index(p, grain)] += 3;
            w.stock[g.index(p, ore)] += 2;
        }
        let n = w.provinces.count as i64;
        cx.flow(FOOD, Cause::Harvest, 3 * n);
        cx.flow(RAW, Cause::Harvest, 2 * n);
        if self.leak {
            w.stock[g.index(0, ore)] -= 1;
            w.stock[g.index(0, grain)] += 1;
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
    assert_eq!(g.names(), ["grain", "fish", "ore"]);
    assert_eq!(sim.world.stock.len(), 3 * n);
    assert_eq!(sim.world.quantities(), [Quantity::People, FOOD, RAW]);
    let total = |sim: &Sim, q| quantity_total(&sim.world, q);
    let (food, raw) = (total(&sim, FOOD), total(&sim, RAW));
    assert_eq!(raw, 10 * n as u64);

    // Two month starts: day 0 and the first day of the second month.
    let month = sim.calendar.def.months[0].days as u64;
    sim.run_days(month + 1);
    assert_eq!((total(&sim, FOOD), total(&sim, RAW)), (food + 6 * n as u64, raw + 4 * n as u64));
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
    assert_eq!(loaded.world.goods.names(), ["grain", "fish", "ore"]);
}

#[test]
fn conservation_is_checked_per_class() {
    // Ore turned into grain leaves the whole store's total as it was, but food and raw goods each
    // changed unexplained.
    let mut sim = world(true);
    sim.run_days(2);
    assert_eq!(sim.problems.conservation_breaks, 2);
    let broken: Vec<(Quantity, i64, Vec<Slot>)> = sim
        .event_log
        .entries
        .iter()
        .filter_map(|p| match &p.event {
            Event::ConservationBroken { quantity, expected, actual, by } => Some((*quantity, *actual as i64 - *expected as i64, by.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(broken, vec![(FOOD, 1, vec![Slot::Economics]), (RAW, -1, vec![Slot::Economics])]);
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
    let before: Vec<u64> = (0..3).map(|c| w.stock[g.index(p, c)]).collect();

    // Half its land becomes a new province, which takes a share of each good.
    sim.edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    let new = sim.world.provinces.count - 1;
    assert_eq!(sim.world.stock.len(), 3 * (new + 1));
    let w = &sim.world;
    for c in 0..3 {
        assert_eq!(w.stock[g.index(p, c)] + w.stock[g.index(new, c)], before[c], "column {c}");
        assert!(w.stock[g.index(new, c)] > 0, "column {c}");
    }
    // Merged back, it all returns.
    sim.edit(Edit::Merge { from: new as u32, into: p as u32 }).unwrap();
    assert_eq!((0..3).map(|c| sim.world.stock[g.index(p, c)]).collect::<Vec<_>>(), before);
    assert_eq!(sim.problems, Problems::default());

    // No food store is kept, so there is none to set.
    assert!(sim.edit(Edit::SetFoodStock { province: 0, rations: 5 }).unwrap_err().contains("nobody keeps a food store"));

    // The sea covers everything: every good is lost, and the year's flows say so.
    let (food, raw) = (quantity_total(&sim.world, FOOD), quantity_total(&sim.world, RAW));
    sim.edit(Edit::SeaLevel { rise_m: 20_000.0 }).unwrap();
    assert!(sim.world.stock.iter().all(|&s| s == 0));
    assert_eq!((sim.year_flows.get(FOOD, Cause::Edit), sim.year_flows.get(RAW, Cause::Edit)), (-(food as i64), -(raw as i64)));
    assert_eq!(sim.problems, Problems::default());
}
