//! Economics v1, "markets", end to end: goods made from land and people, used up and priced; prices
//! that answer shortage and plenty; trade between neighbours, paid in coins; coins kept exactly;
//! saves, going back and edits.

use sim::config::Config;
use sim::edit::Edit;
use sim::events::Event;
use sim::fields::{quantity_total, Quantity};
use sim::markets::{MarketParams, PRICE_SCALE};
use sim::module::Slot;
use sim::requests::Cause;
use sim::sim::Sim;
use sim::world::WorldParams;

fn config(seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 32, provinces: 80, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    c
}

fn world(seed: u64) -> Sim {
    made(&config(seed))
}

fn made(c: &Config) -> Sim {
    let mut sim = Sim::new(c).unwrap();
    sim.strict = true;
    sim
}

/// A small planet (1,000 km in radius), so its provinces are a few days' cart apart, as
/// on a large world, rather than weeks.
fn small_planet(seed: u64, trade: bool) -> Sim {
    let mut c = config(seed);
    c.world.radius_km = 1000.0;
    if !trade {
        c.modules.markets.as_mut().unwrap().trade = None;
    }
    made(&c)
}

fn coins(sim: &Sim, p: usize) -> u64 {
    sim.world.stock_of(p, "coin").unwrap()
}


const FOOD: Quantity = Quantity::Goods(0);
const RAW: Quantity = Quantity::Goods(1);
const MADE: Quantity = Quantity::Goods(2);
const MONEY: Quantity = Quantity::Goods(3);

/// A good's price in province `p`, in coins.
fn price(sim: &Sim, p: usize, good: &str) -> f64 {
    let g = &sim.world.goods;
    sim.world.price[g.index(p, g.column(good).unwrap())] as f64 / PRICE_SCALE
}

fn workers(sim: &Sim, p: usize, good: &str) -> u64 {
    let g = &sim.world.goods;
    sim.world.workers[g.index(p, g.column(good).unwrap())]
}

/// The most populous province.
fn biggest(sim: &Sim) -> usize {
    let people = sim.world.province_population();
    (0..people.len()).max_by_key(|&p| (people[p], std::cmp::Reverse(p))).unwrap()
}

#[test]
fn a_markets_world_makes_uses_and_prices_every_good() {
    let mut sim = world(3);
    let goods = sim.world.goods.clone();
    assert_eq!(goods.names(), ["grain", "fish", "livestock", "timber", "stone", "ore", "cloth", "tools", "coin"]);
    assert_eq!(sim.world.quantities(), [Quantity::People, FOOD, RAW, MADE, MONEY]);
    // Coins were handed out with the people: 20 each.
    let people = sim.world.province_population();
    let coins = quantity_total(&sim.world, MONEY);
    let expected: u64 = people.iter().map(|&n| (n as f64 * 20.0).round() as u64).sum();
    assert_eq!(coins, expected);

    sim.run_to_year(4);
    assert_eq!(sim.problems, Default::default());
    // Coins are never made, used or lost: trade only moves them.
    assert_eq!(quantity_total(&sim.world, MONEY), coins);
    assert!(sim.history.iter().all(|y| y.population > 0));

    // Each year's food balances, as with Economics "food".
    for pair in sim.history.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert_eq!(b.food_stock + b.ledger.spoiled + b.ledger.eaten, a.food_stock + b.ledger.produced, "year {}", b.year);
    }
    // The year so far: raw goods were dug and cut, and some went into tools; cloth and timber were
    // used up; tools and cloth wore out.
    let f = &sim.year_flows;
    assert!(f.get(FOOD, Cause::Harvest) > 0 && f.get(FOOD, Cause::Eating) < 0);
    assert!(f.get(RAW, Cause::Made) > 0 && f.get(RAW, Cause::MadeInto) < 0 && f.get(RAW, Cause::Used) < 0);
    assert!(f.get(MADE, Cause::Made) > 0 && f.get(MADE, Cause::Used) < 0 && f.get(MADE, Cause::WornOut) < 0);
    assert_eq!(f.get(MONEY, Cause::Made) + f.get(MONEY, Cause::Used) + f.get(MONEY, Cause::Spoilage), 0);
    // Goods and coins changed hands between provinces; what came in went out somewhere.
    for q in [FOOD, RAW, MADE, MONEY] {
        assert_eq!(f.get(q, Cause::TradeIn), -f.get(q, Cause::TradeOut));
    }
    assert!(f.get(FOOD, Cause::TradeIn) > 0 && f.get(MONEY, Cause::TradeIn) > 0);

    // Every province with people has a price for every good, within its range, and work for about
    // half its people.
    let params = MarketParams::default();
    let people = sim.world.province_population();
    let mut working = 0;
    for p in (0..people.len()).filter(|&p| people[p] > 0) {
        for (good, gp) in &params.goods {
            let x = price(&sim, p, good);
            assert!(x >= gp.base_price / params.price_range * 0.99 && x <= gp.base_price * params.price_range * 1.01, "province {p}: {good} at {x}");
        }
        let jobs: u64 = goods.names().iter().filter(|n| *n != "coin").map(|n| workers(&sim, p, n)).sum();
        assert!(jobs <= people[p], "province {p}: {jobs} working of {}", people[p]);
        working += jobs;
    }
    let all: u64 = people.iter().sum();
    assert!(working * 10 > all * 3 && working * 10 < all * 6, "{working} working of {all}");
    // Asked why a province changed on a month's first day, markets explains each good's making.
    let p = biggest(&sim);
    let day = (0..sim.day).rev().find(|&d| sim.calendar.is_month_start(d)).unwrap();
    let t = sim.trace(day, p as u32).unwrap();
    let notes: Vec<&String> = t.steps.iter().filter(|s| s.module == Some(Slot::Economics)).filter_map(|s| match &s.what {
        sim::sim::Traced::Note(n) => Some(n),
        _ => None,
    }).collect();
    for good in goods.names().iter().filter(|n| *n != "coin") {
        assert!(notes.iter().any(|n| n.starts_with(&format!("{good}: ")) && n.contains("workers made")), "{good}: {notes:?}");
    }
    // Most work is feeding people.
    let farming: u64 = (0..people.len()).map(|p| ["grain", "fish", "livestock"].iter().map(|g| workers(&sim, p, g)).sum::<u64>()).sum();
    assert!(farming * 2 > working, "{farming} of {working} work on food");
}

#[test]
fn prices_answer_shortage_and_plenty_and_work_follows() {
    // Without trade, so only the province itself answers.
    let mut c = config(5);
    c.modules.markets.as_mut().unwrap().trade = None;
    let base = made(&c);
    let p = biggest(&base);
    let month = base.calendar.def.months[0].days as u64;
    let run = |change: &dyn Fn(&mut Sim)| {
        let mut sim = made(&c);
        sim.run_days(3 * month + 1);
        change(&mut sim);
        // The next month's prices follow the stores; work moves over a few months.
        sim.run_days(3 * month);
        sim
    };
    let left = run(&|_| {});
    let short = run(&|sim| {
        for g in ["grain", "fish", "livestock"] {
            sim.edit(Edit::SetStock { province: p as u32, good: g.into(), amount: 0 }).unwrap();
        }
    });
    let glut = run(&|sim| {
        let held = sim.world.stock_of(p, "timber").unwrap();
        sim.edit(Edit::SetStock { province: p as u32, good: "timber".into(), amount: held * 50 + 1_000_000 }).unwrap();
    });
    // No food in store: food is dear, and more people farm.
    assert!(price(&short, p, "grain") > 1.5 * price(&left, p, "grain"), "grain {} against {}", price(&short, p, "grain"), price(&left, p, "grain"));
    let food_work = |sim: &Sim| ["grain", "fish", "livestock"].iter().map(|g| workers(sim, p, g)).sum::<u64>();
    assert!(food_work(&short) > food_work(&left), "{} farm against {}", food_work(&short), food_work(&left));
    // Timber for decades: it is cheap, and woodcutters find other work.
    assert!(price(&glut, p, "timber") * 4.0 < price(&left, p, "timber"), "timber {} against {}", price(&glut, p, "timber"), price(&left, p, "timber"));
    // Workers move a fifth of the way a month: after three months, about half have gone.
    assert!(workers(&glut, p, "timber") * 3 < workers(&left, p, "timber") * 2, "{} cut timber against {}", workers(&glut, p, "timber"), workers(&left, p, "timber"));
    // Other provinces don't notice: without trade nothing moves between provinces.
    let q = (p + 1) % base.world.provinces.count;
    assert_eq!((price(&short, q, "grain"), price(&glut, q, "timber")), (price(&left, q, "grain"), price(&left, q, "timber")));
    assert_eq!((short.problems, glut.problems), (Default::default(), Default::default()));
}

#[test]
fn a_drought_makes_food_dear_where_it_falls() {
    let mut dry = world(7);
    let mut normal = world(7);
    // The tester's drought, wider: this small world's provinces are big.
    let (start, mut drought) = sim::combos::test_drought(&dry).unwrap();
    if let Event::Drought { radius_km, .. } = &mut drought {
        *radius_km = 3500.0;
    }
    for sim in [&mut dry, &mut normal] {
        sim.run_days(start);
    }
    dry.poke(drought);
    dry.run_days(2);
    let region: Vec<usize> = (0..dry.world.provinces.count).filter(|&p| dry.world.rain_shortfall[p] > 0).collect();
    assert!(region.len() >= 3, "{region:?}");
    let year = dry.calendar.days_per_year() as u64;
    dry.run_days(year);
    normal.run_days(year + 2);
    let mean = |sim: &Sim| region.iter().map(|&p| price(sim, p, "grain")).sum::<f64>() / region.len() as f64;
    assert!(mean(&dry) > 1.3 * mean(&normal), "grain {:.3} in the drought, {:.3} without", mean(&dry), mean(&normal));
    // The region bought food in: it paid out coins it would otherwise have kept.
    let held = |sim: &Sim| region.iter().map(|&p| coins(sim, p)).sum::<u64>();
    assert!(held(&dry) < held(&normal), "{} coins in the drought, {} without", held(&dry), held(&normal));
    assert_eq!(dry.problems, Default::default());
}

#[test]
fn a_starving_province_buys_food_from_its_neighbours() {
    let probe = small_planet(13, true);
    let month = probe.calendar.def.months[0].days as u64;
    // The most populous province with neighbours.
    let people = probe.world.province_population();
    let p = (0..people.len()).filter(|&p| probe.world.routes.of(p as u32).next().is_some()).max_by_key(|&p| people[p]).unwrap();
    let run = |trade: bool, starve: bool| {
        let mut sim = small_planet(13, trade);
        sim.run_days(3 * month + 1);
        if starve {
            for g in ["grain", "fish", "livestock"] {
                sim.edit(Edit::SetStock { province: p as u32, good: g.into(), amount: 0 }).unwrap();
            }
        }
        sim.run_days(2 * month);
        assert_eq!(sim.problems, Default::default());
        sim
    };
    let (alone, helped, left) = (run(false, true), run(true, true), run(true, false));
    // Food came in and was eaten: fewer starved. The province paid for it.
    let starved = |sim: &Sim| sim.world.starved_this_year[p];
    assert!(starved(&helped) * 2 < starved(&alone), "{} starved with trade, {} without", starved(&helped), starved(&alone));
    assert!(coins(&helped, p) < coins(&left, p), "{} coins after buying food, {} otherwise", coins(&helped, p), coins(&left, p));
    // Its neighbours got the coins.
    let around: Vec<usize> = helped.world.routes.of(p as u32).map(|l| l.other(p as u32) as usize).collect();
    let earned = |sim: &Sim| around.iter().map(|&q| coins(sim, q)).sum::<u64>();
    assert!(earned(&helped) > earned(&left));
    // Asked why, the month's first day says who it bought from.
    let mut helped = helped;
    let day = (0..helped.day).rev().find(|&d| helped.calendar.is_month_start(d)).unwrap();
    let t = helped.trace(day, p as u32).unwrap();
    let bought = t.steps.iter().any(|s| matches!(&s.what, sim::sim::Traced::Note(n) if n.starts_with("bought") && n.contains(" food from province ")));
    assert!(bought, "{:?}", t.steps);
}

#[test]
fn scenario_files_from_before_trade_run_without_it() {
    // A configuration written before trade has no trade table: it loads, and nothing moves.
    let mut c = config(3);
    c.modules.markets.as_mut().unwrap().trade = None;
    let text = toml::to_string(&c).unwrap();
    assert!(!text.contains("trade"));
    let back: Config = toml::from_str(&text).unwrap();
    assert_eq!(back, c);
    let mut sim = made(&back);
    let start: Vec<u64> = (0..sim.world.provinces.count).map(|p| coins(&sim, p)).collect();
    sim.run_to_year(2);
    assert_eq!((0..sim.world.provinces.count).map(|p| coins(&sim, p)).collect::<Vec<_>>(), start);
    // New worlds trade.
    assert!(config(3).modules.markets.unwrap().trade.is_some());
}

#[test]
fn markets_needs_people() {
    let mut c = config(1);
    c.modules.population = None;
    let refused = Sim::new(&c).err().unwrap().0;
    assert!(refused.contains("Economics \"markets\" requires pop.size"), "{refused}");
}

#[test]
fn bad_parameters_are_refused_when_the_world_is_made() {
    let mut c = config(1);
    c.modules.markets.as_mut().unwrap().goods.remove("stone");
    let refused = Sim::new(&c).err().unwrap().0;
    assert_eq!(refused, "Economics \"markets\": no parameters for stone: how it is made, used and priced");
}

#[test]
fn saves_and_going_back_give_the_same_world() {
    let mut sim = world(9);
    sim.run_days(400);
    let (print, day) = (sim.fingerprint(), sim.day);
    let mut loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), print);
    sim.goto(37).unwrap();
    sim.goto(day).unwrap();
    assert_eq!(sim.fingerprint(), print);
    sim.run_days(200);
    loaded.run_days(200);
    assert_eq!(sim.fingerprint(), loaded.fingerprint());
    assert_eq!(sim.problems, Default::default());
}

#[test]
fn last_months_trade_is_kept_for_showing() {
    let mut sim = small_planet(5, true);
    sim.run_days(400);
    let record = sim.world.trade.clone();
    let coin = sim.world.goods.column("coin").unwrap() as u16;
    assert!(!record.shipments.is_empty(), "provinces a few days apart trade");
    assert!(record.day < sim.day && sim.day - record.day <= 31, "last month's");
    assert!(record.shipments.iter().all(|s| s.from != s.to && s.units > 0 && s.coins > 0.0 && s.column != coin));
    // Going back shows the trade of that time; coming back, today's again.
    let day = sim.day;
    sim.goto(200).unwrap();
    assert!(sim.world.trade.day < 200 && !sim.world.trade.shipments.is_empty());
    sim.goto(day).unwrap();
    assert_eq!(*sim.world.trade, *record);
    // Not saved and not in the fingerprint: a world just opened has none, and is the same world.
    let loaded = Sim::load(&sim.save()).unwrap();
    assert!(loaded.world.trade.shipments.is_empty());
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
}

#[test]
fn a_province_made_by_an_edit_gets_prices_and_work() {
    let mut sim = world(11);
    sim.run_days(10);
    let p = biggest(&sim);
    let cells: Vec<u32> = (0..sim.world.grid.cells() as u32).filter(|&c| sim.world.provinces.of_cell[c as usize] == p as u32).collect();
    sim.edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    let new = sim.world.provinces.count - 1;
    assert_eq!(price(&sim, new, "grain"), 0.0, "nothing is priced until the month's work");
    sim.run_days(sim.calendar.def.months[0].days as u64 * 2);
    assert!(sim.world.province_population()[new] > 0);
    assert!(price(&sim, new, "grain") > 0.0 && workers(&sim, new, "grain") > 0);
    assert_eq!(sim.problems, Default::default());
}

#[test]
fn one_economics_version_at_a_time() {
    let mut c = Config::new(WorldParams { n: 16, provinces: 10, seed: 1, land_fraction: 0.4, radius_km: 6371.0 });
    assert_eq!(c.modules.choice(Slot::Economics), Some("food"));
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    assert!(c.modules.economics.is_none() && c.modules.markets.is_some());
    assert_eq!(c.modules.choice(Slot::Economics), Some("markets"));
    c.modules.set_choice(Slot::Economics, "off").unwrap();
    assert!(c.modules.economics.is_none() && c.modules.markets.is_none());
    // Both set at once fills the slot twice.
    c.modules.economics = Some(Default::default());
    c.modules.markets = Some(MarketParams::default());
    assert!(Sim::new(&c).err().unwrap().0.contains("two modules fill the Economics slot"));
    // The parameters go into scenario files and come back the same.
    let c = config(2);
    let text = toml::to_string(&c).unwrap();
    assert_eq!(toml::from_str::<Config>(&text).unwrap(), c);
}
