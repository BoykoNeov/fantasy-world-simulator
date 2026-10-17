//! Target stories from the design doc, as acceptance tests: each starts from one poke and checks
//! that the story then happens on its own.

use sim::config::Config;
use sim::events::Event;
use sim::module::Slot;
use sim::sim::{Sim, Traced};
use sim::world::WorldParams;

fn world() -> Sim {
    // Story 1's famine half, as it was checked at step 4: no realms (the full story is below).
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed: 21, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.without_realms();
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    sim
}

const START_YEAR: i64 = 3;
const DROUGHT_YEARS: usize = 3;

struct Run {
    sim: Sim,
    /// People starved in the region, by year from the drought's start (the last is the year after).
    starved: Vec<u64>,
    /// The region's smallest food store, summed over its provinces, while the drought lasted.
    lowest_store: u64,
}

/// Run from the start of `START_YEAR` to the end of the year after the drought, with or without it.
fn run(poke: Option<&Event>, region: &[usize]) -> Run {
    let mut sim = world();
    sim.run_to_year(START_YEAR);
    if let Some(e) = poke {
        sim.poke(e.clone());
    }
    let mut starved = vec![0u64; DROUGHT_YEARS + 1];
    let mut lowest_store = u64::MAX;
    let start = sim.day;
    let end = sim.calendar.year_start(START_YEAR + DROUGHT_YEARS as i64 + 1);
    while sim.day < end {
        sim.step_day();
        let year = ((sim.day - 1 - start) / sim.calendar.days_per_year() as u64) as usize;
        for p in sim.delivered_today() {
            if let Event::Famine { province, starved: s } = p.event
                && region.contains(&(province as usize))
            {
                starved[year] += s;
            }
        }
        if year < DROUGHT_YEARS {
            lowest_store = lowest_store.min(region.iter().map(|&i| sim.world.stock[i]).sum());
        }
    }
    Run { sim, starved, lowest_store }
}

/// The harvest the trace of a month's first day reports for `province`.
fn harvest(sim: &mut Sim, day: u64, province: usize) -> u64 {
    let t = sim.trace(day, province as u32).unwrap();
    t.steps
        .iter()
        .find_map(|s| match &s.what {
            Traced::Note(n) if s.module == Some(Slot::Economics) => n.strip_prefix("harvest ")?.split(' ').next()?.parse().ok(),
            _ => None,
        })
        .expect("the harvest explains itself")
}

/// Story 1, first half: a multi-year drought in a breadbasket region cuts harvests, food stores run
/// down and people starve. Only the drought is scripted.
#[test]
fn story_1_a_drought_in_the_breadbasket_brings_famine() {
    let base = world();
    let p = &base.world.provinces;
    // The breadbasket: the province that grows the most food.
    let heart = (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap();
    let (lon, lat) = sim::grid::dir_to_lonlat(p.center[heart]);
    let drought = Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days: DROUGHT_YEARS as u64 * 364 };

    // Where the rain fails: every province the drought reaches.
    let mut probe = world();
    probe.run_to_year(START_YEAR);
    probe.poke(drought.clone());
    probe.run_days(2);
    let region: Vec<usize> = (0..p.count).filter(|&i| probe.world.rain_shortfall[i] > 0).collect();
    assert!(region.len() >= 4 && region.contains(&heart), "{region:?}");

    let mut dry = run(Some(&drought), &region);
    let mut normal = run(None, &region);
    let people: u64 = region.iter().map(|&i| base.world.province_population()[i]).sum();

    // Harvests fail: the same month, the same luck, a third of the crop.
    let cal = &dry.sim.calendar;
    let summer = (cal.year_start(START_YEAR + 1)..).find(|&d| cal.is_month_start(d) && cal.date(d).month == 7).unwrap();
    let (h_dry, h_normal) = (harvest(&mut dry.sim, summer, heart), harvest(&mut normal.sim, summer, heart));
    assert!(h_normal > 0 && h_dry * 10 < h_normal * 4, "harvest {h_dry} in the drought, {h_normal} without");

    // Stores run down to a fraction of a normal year's (seed 21: 18%).
    assert!(dry.lowest_store * 4 < normal.lowest_store, "lowest store {} in the drought, {} without", dry.lowest_store, normal.lowest_store);

    // People starve: in the worst year, at least 5% of the region; without the drought, hardly anyone.
    let worst = *dry.starved.iter().max().unwrap();
    let normal_total: u64 = normal.starved.iter().sum();
    assert!(worst * 20 >= people, "worst year {worst} starved of {people}");
    assert!(normal_total * 10 < worst, "{normal_total} starved without the drought");
    // Once the rain is back, the famine ends.
    assert!(dry.starved[DROUGHT_YEARS] * 10 < worst, "{:?}", dry.starved);

    // Famine stays where the rain failed: elsewhere nothing differs at all, since nothing yet
    // moves food or people between provinces.
    let (a, b) = (dry.sim.world.province_population(), normal.sim.world.province_population());
    for i in (0..p.count).filter(|i| !region.contains(i)) {
        assert_eq!((a[i], dry.sim.world.stock[i]), (b[i], normal.sim.world.stock[i]), "province {i}, outside the drought");
    }
    assert_eq!(dry.sim.problems, Default::default());
}

/// Story 1's world with Economics v1, "markets", with or without trade.
fn markets_world(trade: bool) -> Sim {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed: 21, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.without_realms();
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    if !trade {
        c.modules.markets.as_mut().unwrap().trade = None;
    }
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    sim
}

/// A markets run from the start of `START_YEAR` to the end of the year after the drought.
struct MarketRun {
    sim: Sim,
    /// People starved in the region, by year as in [`Run`].
    starved: Vec<u64>,
    /// Food the region bought from provinces outside it, less what it sold them, by year.
    food_in: Vec<i64>,
    /// The region's local food price (grain's: food is priced as one), averaged over its provinces
    /// and the drought's months.
    food_price: f64,
}

fn markets_run(trade: bool, poke: Option<&Event>, region: &[usize]) -> MarketRun {
    let mut sim = markets_world(trade);
    sim.run_to_year(START_YEAR);
    if let Some(e) = poke {
        sim.poke(e.clone());
    }
    let goods = sim.world.goods.clone();
    let food: Vec<u16> = ["grain", "fish", "livestock"].iter().map(|g| goods.column(g).unwrap() as u16).collect();
    let grain = goods.column("grain").unwrap();
    let (mut starved, mut food_in) = (vec![0u64; DROUGHT_YEARS + 1], vec![0i64; DROUGHT_YEARS + 1]);
    let (mut price_sum, mut months) = (0.0, 0);
    let mut seen = sim.world.trade.day;
    let start = sim.day;
    let end = sim.calendar.year_start(START_YEAR + DROUGHT_YEARS as i64 + 1);
    while sim.day < end {
        sim.step_day();
        let year = ((sim.day - 1 - start) / sim.calendar.days_per_year() as u64) as usize;
        for p in sim.delivered_today() {
            if let Event::Famine { province, starved: s } = p.event
                && region.contains(&(province as usize))
            {
                starved[year] += s;
            }
        }
        if sim.world.trade.day != seen {
            seen = sim.world.trade.day;
            for s in sim.world.trade.shipments.iter().filter(|s| food.contains(&s.column)) {
                let (from, to) = (region.contains(&(s.from as usize)), region.contains(&(s.to as usize)));
                if to && !from {
                    food_in[year] += s.units as i64;
                } else if from && !to {
                    food_in[year] -= s.units as i64;
                }
            }
        }
        if year < DROUGHT_YEARS && sim.calendar.is_month_start(sim.day - 1) {
            price_sum += region.iter().map(|&p| sim.world.price[goods.index(p, grain)] as f64).sum::<f64>() / region.len() as f64;
            months += 1;
        }
    }
    MarketRun { sim, starved, food_in, food_price: price_sum / months as f64 / sim::markets::PRICE_SCALE }
}

/// Story 1 on Economics v1: the same drought, and now the region buys food in from outside and
/// pays more for it. Only the drought is scripted.
#[test]
fn story_1_on_markets_the_drought_region_buys_food_in() {
    let base = markets_world(true);
    let p = &base.world.provinces;
    let heart = (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap();
    let (lon, lat) = sim::grid::dir_to_lonlat(p.center[heart]);
    let drought = Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days: DROUGHT_YEARS as u64 * 364 };
    let mut probe = markets_world(true);
    probe.run_to_year(START_YEAR);
    probe.poke(drought.clone());
    probe.run_days(2);
    let region: Vec<usize> = (0..p.count).filter(|&i| probe.world.rain_shortfall[i] > 0).collect();
    assert!(region.len() >= 4 && region.contains(&heart), "{region:?}");

    let dry = markets_run(true, Some(&drought), &region);
    let normal = markets_run(true, None, &region);
    let alone = markets_run(false, Some(&drought), &region);
    let people: u64 = region.iter().map(|&i| base.world.province_population()[i]).sum();
    let report = || format!("dry {:?} {:?}, normal {:?} {:?}, no trade {:?}", dry.starved, dry.food_in, normal.starved, normal.food_in, alone.starved);

    // The famine still comes (seed 21: a quarter of the region in the worst year) and ends with the rain.
    let worst = *dry.starved.iter().max().unwrap();
    assert!(worst * 20 >= people, "{}", report());
    assert!(dry.starved[DROUGHT_YEARS] * 10 < worst, "{}", report());

    // The region buys food from outside it every year of the drought, far more than in a normal
    // year (seed 21: 274 million rations against 36 million over the three years).
    let bought = |r: &MarketRun| r.food_in[..DROUGHT_YEARS].iter().sum::<i64>();
    assert!(dry.food_in[..DROUGHT_YEARS].iter().all(|&x| x > 0), "{}", report());
    assert!(bought(&dry) > 4 * bought(&normal).max(0), "{}", report());

    // Food is dear there: the local price, without the money level, which falls as the region
    // pays out its coins (seed 21: 1.09 coins a ration against 0.17).
    assert!(dry.food_price > 3.0 * normal.food_price, "food {:.3} in the drought, {:.3} without", dry.food_price, normal.food_price);

    // Trade helps, a little: food crosses one border a month and carrying it across this world's
    // big provinces is dear, so it covers a few percent of what the region eats (seed 21: 5% fewer
    // starved over the drought than with no trade).
    let starved = |r: &MarketRun| r.starved[..DROUGHT_YEARS].iter().sum::<u64>();
    assert!(starved(&dry) < starved(&alone), "{}", report());
    for r in [&dry, &normal, &alone] {
        assert_eq!(r.sim.problems, Default::default());
    }
}

/// Story 1 in full, on one seed and economy, with the default modules (realms, rulers, realms
/// choosing war and peace, armies): story 1's world, the drought poked in after three years over
/// the province that grows the most food, then five years watched. Whether, with the drought and
/// in the same world without it, a drought province rose up (an uprising begun there) or its realm
/// declared a war for food.
fn story_1_full(economics: &str, seed: u64) -> (bool, bool) {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    let mut sim = Sim::new(&c).unwrap();
    let start = sim.calendar.date(0).year;
    sim.run_to_year(start + 3);
    let mut twin = sim.fork();
    let (count, heart) = {
        let p = &sim.world.provinces;
        (p.count, (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap())
    };
    let (lon, lat) = sim::grid::dir_to_lonlat(sim.world.provinces.center[heart]);
    sim.poke(Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days: DROUGHT_YEARS as u64 * 364 });
    let mut probe = sim.fork();
    probe.run_days(2);
    let region: Vec<u32> = (0..count as u32).filter(|&i| probe.world.rain_shortfall[i as usize] > 0).collect();
    let mut realms: Vec<u32> = region.iter().map(|&q| sim.world.owner[q as usize]).filter(|&r| r != sim::grid::NONE).collect();
    realms.sort_unstable();
    realms.dedup();
    let end = sim.day + 5 * sim.calendar.days_per_year() as u64;
    let watch = |s: &mut Sim| {
        let mut happened = false;
        while s.day < end {
            s.step_day();
            for e in s.delivered_today() {
                happened |= match e.event {
                    Event::Uprising { province, seat, .. } => province == seat && region.contains(&province),
                    Event::WarDeclared { attacker, cause, .. } => cause == sim::diplomacy::WarCause::Food && realms.contains(&attacker),
                    _ => false,
                };
            }
        }
        assert_eq!(s.problems, Default::default(), "{economics}, seed {seed}");
        happened
    };
    (watch(&mut sim), watch(&mut twin))
}

#[test]
fn story_1_in_full_a_drought_brings_an_uprising_or_a_war_for_food() {
    // Passes if on at least 2 of 3 seeds the drought brings an uprising in its region or a war for
    // food by its realm within 5 years, and on at least 2 of 3 the same world without it has
    // neither; under both economies (the pass numbers the user approved, Oct 15).
    for economics in ["food", "markets"] {
        let runs: Vec<(bool, bool)> = [21, 3, 7].into_iter().map(|seed| story_1_full(economics, seed)).collect();
        let with = runs.iter().filter(|r| r.0).count();
        let quiet = runs.iter().filter(|r| !r.1).count();
        assert!(with >= 2 && quiet >= 2, "{economics}: with the drought {with} of 3, quiet without it {quiet} of 3: {runs:?}");
    }
}
