//! The step-7 groundwork: realms with stores of their own, armies of people, and the two ways
//! things move between them and the provinces. No real module does this yet, so stand-ins fill
//! the economics, realms and warfare slots:
//!
//! - an economy that keeps grain and coins for provinces and realms, and serves eating and moves;
//! - realms that tax coins into treasuries, pay some back, and gain a new realm (a rebellion);
//! - an army that raises soldiers from a province and sends some home.

use crate::config::Config;
use crate::events::{Event, EventKind};
use crate::fields::{known, quantity_total, Quantity, Tier};
use crate::grid::NONE;
use crate::module::{plan, Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::{serve_goods_into, Cause, Dest, Priority, Request};
use crate::sim::{Problems, Sim, Traced};
use crate::world::{World, WorldParams};

const MONEY: Quantity = Quantity::Goods(3);

static COFFERS: Manifest = Manifest {
    slot: Slot::Economics,
    version: "coffers",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    reads: &[],
    owns: &["province.stock", "realm.stock"],
    requests: &[],
    asks: &[],
    moves: &[],
    requires: &[],
    emits: &[EventKind::Granted],
    handles: &[],
    state_schema: "none",
    goods: &["grain", "coin"],
};

/// Keeps grain and coins; makes and uses nothing, and serves every request by the shared rules.
struct Coffers;

impl Module for Coffers {
    fn manifest(&self) -> &'static Manifest {
        &COFFERS
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        w.stock = (0..w.provinces.count).flat_map(|_| [1_000_000_000_000, 1_000]).collect();
    }

    fn apply_requests(&mut self, w: &mut World, requests: &[Request], cx: &mut Ctx) {
        let stock = known("province.stock");
        let split = requests.partition_point(|r| r.field == stock);
        let (from_provinces, from_realms) = requests.split_at(split);
        serve_goods_into(&mut w.stock, Some(&mut w.realm_stock), from_provinces, cx);
        serve_goods_into(&mut w.realm_stock, Some(&mut w.stock), from_realms, cx);
    }
}

static CROWNS: Manifest = Manifest {
    slot: Slot::Realms,
    version: "crowns",
    tier: Tier::Realm,
    cadence: Cadence::Monthly,
    reads: &["province.cells"],
    owns: &["province.owner", "realm.capital"],
    requests: &["province.stock", "realm.stock"],
    asks: &["money"],
    moves: &[("province.stock", "realm.stock"), ("realm.stock", "province.stock")],
    requires: &[],
    emits: &[],
    handles: &[EventKind::Granted],
    state_schema: "none",
    goods: &[],
};

/// Two realms share the land, by province number. Each month every province pays 10 coins into
/// its realm's treasury, and realm 1 pays 5 back to its capital. On the second month a province
/// breaks away as realm 2.
struct Crowns;

/// The province that breaks away.
const REBEL: u32 = 3;

impl Module for Crowns {
    fn manifest(&self) -> &'static Manifest {
        &CROWNS
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        let land = |p: usize| w.provinces.cells[p] > 0;
        w.owner = (0..w.provinces.count).map(|p| if land(p) { (p % 2) as u32 } else { NONE }).collect();
        w.realms.capital = (0..2).map(|r| (0..w.provinces.count).find(|&p| land(p) && p % 2 == r).unwrap() as u32).collect();
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        if cx.calendar.date(cx.day).month == 1 {
            w.realms.capital.push(REBEL);
            w.owner[REBEL as usize] = 2;
        }
        let (stock, treasury) = (known("province.stock"), known("realm.stock"));
        let money = cx.goods.tree.find("money").unwrap();
        for p in 0..w.provinces.count as u32 {
            let r = w.owner[p as usize];
            if r != NONE {
                cx.ask_move(stock, p, Some(money), 10, Dest { field: treasury, entity: r }, Priority::Normal, Cause::Taxes, p as u64);
            }
        }
        let capital = w.realms.capital[1];
        cx.ask_move(treasury, 1, Some(money), 5, Dest { field: stock, entity: capital }, Priority::Normal, Cause::Taxes, 0);
    }
}

static MUSTER: Manifest = Manifest {
    slot: Slot::Warfare,
    version: "muster",
    tier: Tier::Army,
    cadence: Cadence::Monthly,
    reads: &["province.owner"],
    owns: &["army.realm", "army.province", "army.size"],
    requests: &["pop.size"],
    asks: &[],
    moves: &[],
    requires: &[],
    emits: &[EventKind::Arrived],
    handles: &[EventKind::Granted],
    state_schema: "none",
    goods: &[],
};

/// Raises 100 soldiers a month from one province into one army, and from the second month sends
/// 30 home again.
struct Muster {
    from: u32,
}

impl Module for Muster {
    fn manifest(&self) -> &'static Manifest {
        &MUSTER
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        cx.request(known("pop.size"), self.from, 100, Priority::Normal, Cause::Raised, 0);
        if let Some(size) = w.armies.size.first_mut().filter(|s| **s >= 30) {
            *size -= 30;
            cx.flow(Quantity::People, Cause::Returned, -30);
            cx.emit(Event::Arrived { province: self.from, people: 30, cause: Cause::Returned });
        }
    }

    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        let Event::Granted { request, granted } = *e else { return };
        if w.armies.count() == 0 {
            w.armies.push(w.owner[request.entity as usize], request.entity, 0);
        }
        w.armies.size[0] += granted;
        cx.flow(Quantity::People, Cause::Raised, granted as i64);
    }
}

fn config() -> Config {
    Config::new(WorldParams { n: 24, provinces: 40, seed: 5, land_fraction: 0.4, radius_km: 6371.0 })
}

/// The province the army is raised from: the first pop's.
fn muster_from(config: &Config) -> u32 {
    Sim::new(config).unwrap().world.pops.province[0]
}

fn modules(config: &Config, from: u32) -> Vec<Box<dyn Module>> {
    let swap = |m: Box<dyn Module>| -> Box<dyn Module> { if m.manifest().slot == Slot::Economics { Box::new(Coffers) } else { m } };
    let mut out: Vec<Box<dyn Module>> = config.modules.build().into_iter().map(swap).collect();
    out.push(Box::new(Crowns));
    out.push(Box::new(Muster { from }));
    out
}

fn world() -> (Sim, u32) {
    let config = config();
    let from = muster_from(&config);
    let mut sim = Sim::new_with(&config, modules(&config, from)).unwrap();
    sim.strict = true;
    (sim, from)
}

#[test]
fn coins_move_into_treasuries_and_back_without_being_made_or_lost() {
    let (mut sim, _) = world();
    let coins = quantity_total(&sim.world, MONEY);
    assert_eq!(sim.world.realm_stock.len(), 2 * 2, "two realms, two goods each");
    let owners = sim.world.owner.clone();
    let paying = |r: u32| owners.iter().filter(|&&o| o == r).count() as u64;

    // Day 0 starts a month: taxes and the payment back are asked for and settled that night.
    sim.run_days(1);
    let g = sim.world.goods.clone();
    let coin = g.column("coin").unwrap();
    let treasury = |sim: &Sim, r: usize| sim.world.realm_stock[g.index(r, coin)];
    assert_eq!((treasury(&sim, 0), treasury(&sim, 1)), (10 * paying(0), 10 * paying(1) - 5));
    let capital = sim.world.realms.capital[1] as usize;
    assert_eq!(sim.world.stock[g.index(capital, coin)], 1_000 - 10 + 5);
    // Coins moved; none were made or lost, and no flow claims otherwise.
    assert_eq!(quantity_total(&sim.world, MONEY), coins);
    assert_eq!(sim.year_flows.get(MONEY, Cause::Taxes), 0);
    let taxed: Vec<&(Request, u64)> = sim.applied_today.iter().filter(|(r, _)| r.cause == Cause::Taxes).collect();
    assert_eq!(taxed.len() as u64, paying(0) + paying(1) + 1);

    // A month on, a province has broken away: the new realm gets a row of stores, empty, then
    // taxes of its own.
    let month = sim.calendar.def.months[0].days as u64;
    sim.run_days(month);
    assert_eq!((sim.world.realms.count(), sim.world.realm_stock.len()), (3, 3 * 2));
    assert_eq!(treasury(&sim, 2), 10);
    assert_eq!(quantity_total(&sim.world, MONEY), coins);
    sim.world.check_lengths().unwrap();
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn soldiers_leave_the_pops_for_an_army_and_come_home_again() {
    let (mut sim, from) = world();
    // A second pop in that province: soldiers are taken from both, by their sizes.
    sim.world.pops.push(from, 50_000);
    let pops: Vec<usize> = (0..sim.world.pops.size.len()).filter(|&k| sim.world.pops.province[k] == from).collect();
    let before: Vec<u64> = pops.iter().map(|&k| sim.world.pops.size[k]).collect();

    // Day 0: the request is served that night. The soldiers are on their way for a day: booked out
    // of the pops, not yet into the army.
    sim.run_days(1);
    let raised: Vec<(Request, u64)> = sim.applied_today.iter().filter(|(r, _)| r.cause == Cause::Raised).copied().collect();
    assert_eq!(raised.len(), 1);
    assert_eq!((raised[0].0.entity, raised[0].1), (from, 100));
    assert_eq!(sim.year_flows.get(Quantity::People, Cause::Raised), -100);
    assert_eq!(sim.world.armies.count(), 0);
    // Eating was settled too, so compare the pops' losses against the shares of 100 alone.
    let taken = crate::requests::split_proportional(100, &before, &pops.iter().map(|&k| k as u64).collect::<Vec<_>>());
    assert!(taken.iter().all(|&t| t > 0), "both pops gave soldiers: {taken:?}");

    // Day 1: they join the army, and the people total is whole again.
    sim.run_days(1);
    assert_eq!((sim.world.armies.size.clone(), sim.world.armies.province.clone()), (vec![100], vec![from]));
    assert_eq!(sim.world.armies.realm[0], sim.world.owner[from as usize]);
    assert_eq!(sim.year_flows.get(Quantity::People, Cause::Raised), 0);

    // The next month's first day: 100 more raised, 30 sent home, who arrive the day after and join
    // the province's biggest pop.
    let month = sim.calendar.def.months[0].days as u64;
    sim.run_days(month - 2);
    let biggest = *pops.iter().max_by_key(|&&k| sim.world.pops.size[k]).unwrap();
    let had = sim.world.pops.size[biggest];
    sim.run_days(1);
    assert_eq!(sim.world.armies.size[0], 70);
    assert_eq!(sim.year_flows.get(Quantity::People, Cause::Returned), -30);
    let arrived = sim.pending.iter().any(|p| matches!(p.event, Event::Arrived { province, people: 30, .. } if province == from));
    assert!(arrived, "the 30 are on their way home");
    sim.run_days(1);
    assert_eq!(sim.world.armies.size[0], 170);
    assert_eq!(sim.year_flows.get(Quantity::People, Cause::Returned), 0);
    // Births and deaths are monthly, so the pop changed only by the soldiers it gave and got back.
    let gave = sim.applied_today.iter().filter(|(r, _)| r.cause == Cause::Raised).count();
    assert_eq!(gave, 0, "nothing raised on this day");
    assert!(sim.world.pops.size[biggest] >= had + 30 - 100, "the 30 joined the biggest pop");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn realms_and_armies_go_back_save_and_explain_like_everything_else() {
    let (mut sim, from) = world();
    let month = sim.calendar.def.months[0].days as u64;
    sim.run_days(2 * month + 5);
    let (print, today) = (sim.fingerprint(), sim.day);
    assert!(sim.world.armies.count() == 1 && sim.world.realms.count() == 3);

    // Going back to before the army and the rebel realm existed, and forward again.
    sim.goto(0).unwrap();
    assert_eq!((sim.world.armies.count(), sim.world.realms.count()), (0, 2));
    sim.goto(today).unwrap();
    assert_eq!(sim.fingerprint(), print);

    // Saved and loaded, with the same stand-ins.
    let loaded = Sim::load_with(&sim.save(), |c| modules(c, from)).unwrap();
    assert_eq!(loaded.fingerprint(), print);
    assert_eq!(loaded.world.owner[REBEL as usize], 2);

    // Why did the province change on the first day? The soldiers asked for and taken.
    let trace = sim.trace(0, from).unwrap();
    let asked = trace.steps.iter().any(|s| matches!(&s.what, Traced::Requested(r) if r.cause == Cause::Raised && s.module == Some(Slot::Warfare)));
    let granted = trace.steps.iter().any(|s| matches!(&s.what, Traced::Granted(r, 100) if r.cause == Cause::Raised));
    assert!(asked && granted, "{:#?}", trace.steps);
    assert_eq!(sim.fingerprint(), print);
}

#[test]
fn moves_must_stay_with_one_owner_and_be_listed() {
    let config = config();
    let all = modules(&config, 0);
    // The stand-ins' configuration, with some of them swapped for the manifests given.
    let with = |swap: &[&'static Manifest]| {
        let ms: Vec<&Manifest> = all.iter().map(|m| *swap.iter().find(|s| s.slot == m.manifest().slot).unwrap_or(&m.manifest())).collect();
        plan(&ms)
    };
    assert!(with(&[]).is_ok());
    // Moving coins into a field another module keeps would hand them between owners.
    static ELSEWHERE: Manifest = Manifest { moves: &[("province.stock", "army.size")], ..CROWNS };
    let e = with(&[&ELSEWHERE]).unwrap_err().0;
    assert!(e.contains("one module must keep both"), "{e}");
    // A move must be one of the module's requests.
    static UNLISTED: Manifest = Manifest { requests: &["province.stock"], ..CROWNS };
    let e = with(&[&UNLISTED]).unwrap_err().0;
    assert!(e.contains("doesn't list realm.stock among its requests"), "{e}");
    // Coins can't be moved into a count of people.
    static PEOPLE: Manifest = Manifest { owns: &["province.stock", "realm.stock", "army.size"], ..COFFERS };
    static NO_ARMY: Manifest = Manifest { owns: &["army.realm", "army.province"], ..MUSTER };
    static INTO_PEOPLE: Manifest = Manifest { moves: &[("province.stock", "army.size")], ..CROWNS };
    let e = with(&[&PEOPLE, &NO_ARMY, &INTO_PEOPLE]).unwrap_err().0;
    assert!(e.contains("the same kind of counted quantity"), "{e}");
}

#[test]
fn worlds_without_realms_have_no_owners_and_no_realm_stores() {
    let mut sim = Sim::new(&config()).unwrap();
    sim.strict = true;
    sim.run_days(40);
    let w = &sim.world;
    assert!(w.owner.len() == w.provinces.count && w.owner.iter().all(|&o| o == NONE));
    assert!(w.realms.count() == 0 && w.realm_stock.is_empty() && w.armies.count() == 0);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_province_split_off_keeps_its_owner() {
    let (mut sim, _) = world();
    let w = &sim.world;
    let p = (0..w.provinces.count).filter(|&p| w.owner[p] == 1).max_by_key(|&p| w.provinces.cells[p]).unwrap();
    let cells: Vec<u32> = (0..w.grid.cells() as u32).filter(|&c| w.provinces.of_cell[c as usize] == p as u32).collect();
    sim.edit(crate::edit::Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    assert_eq!(sim.world.owner.len(), sim.world.provinces.count);
    assert_eq!(sim.world.owner[sim.world.provinces.count - 1], 1);
    sim.run_days(40);
    assert_eq!(sim.problems, Problems::default());
}
