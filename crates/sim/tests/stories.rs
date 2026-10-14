//! Target stories from the design doc, as acceptance tests: each starts from one poke and checks
//! that the story then happens on its own.

use sim::config::Config;
use sim::events::Event;
use sim::module::Slot;
use sim::sim::{Sim, Traced};
use sim::world::WorldParams;

fn world() -> Sim {
    let mut sim = Sim::new(&Config::new(WorldParams { n: 48, provinces: 200, seed: 21, land_fraction: 0.36, radius_km: 6371.0 })).unwrap();
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
            lowest_store = lowest_store.min(region.iter().map(|&i| sim.world.food_stock[i]).sum());
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
        assert_eq!((a[i], dry.sim.world.food_stock[i]), (b[i], normal.sim.world.food_stock[i]), "province {i}, outside the drought");
    }
    assert_eq!(dry.sim.problems, Default::default());
}
