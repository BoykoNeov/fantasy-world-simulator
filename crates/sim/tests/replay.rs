//! Going back in time: rewinding to an earlier day replays the same world, a poke made after a
//! rewind gives the same future as one made the first time through, and tracing a past day explains
//! it without changing anything.

use sim::config::Config;
use sim::economy::FoodParams;
use sim::events::{Event, Posted};
use sim::fields::known;
use sim::module::Slot;
use sim::sim::{Sim, Stage, Traced, NEVER};
use sim::world::WorldParams;

fn harsh(seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 24, provinces: 40, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.economics = Some(FoodParams { harvest_variability: 1.2, ..FoodParams::default() });
    c
}

/// A drought over the province with the most farmland.
fn drought(sim: &Sim) -> Event {
    let p = &sim.world.provinces;
    let best = (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap();
    let (lon, lat) = sim::grid::dir_to_lonlat(p.center[best]);
    Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2500.0, shortfall: 800, days: 2 * 364 }
}

fn log(sim: &Sim) -> Vec<Posted> {
    sim.event_log.entries.clone()
}

#[test]
fn rewinding_replays_the_same_days() {
    let mut a = Sim::new(&harsh(5)).unwrap();
    a.strict = true;
    let back = a.calendar.year_start(2) + 47;
    let end = a.calendar.year_start(4) + 100;
    a.run_days(end);
    let (fp, history, events) = (a.fingerprint(), a.history.clone(), log(&a));

    a.rewind(back).unwrap();
    assert_eq!(a.day, back);
    let mut fresh = Sim::new(&harsh(5)).unwrap();
    fresh.run_days(back);
    assert_eq!(a.fingerprint(), fresh.fingerprint(), "rewound state differs from a fresh run to the same day");
    assert_eq!(log(&a), log(&fresh));

    a.run_days(end - back);
    assert_eq!(a.fingerprint(), fp);
    assert_eq!(a.history, history);
    assert_eq!(log(&a), events);
    assert_eq!(a.problems, Default::default());
}

#[test]
fn a_poke_after_a_rewind_is_the_same_as_a_poke_the_first_time() {
    let mut a = Sim::new(&harsh(8)).unwrap();
    let day = a.calendar.year_start(3) + 20;
    a.run_days(day + 300);
    a.rewind(day).unwrap();
    let d = drought(&a);
    a.poke(d.clone());
    a.run_days(500);

    let mut b = Sim::new(&harsh(8)).unwrap();
    b.run_days(day);
    b.poke(d);
    b.run_days(500);
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert_eq!(log(&a), log(&b));
    assert_eq!(a.commands, b.commands);
    // The poke went out on its day and arrived the next.
    let arrived = b.event_log.entries.iter().find(|p| matches!(p.event, Event::Drought { .. })).unwrap();
    assert_eq!((arrived.day, arrived.emitter), (day, None));
    assert!(b.world.rain_shortfall.iter().any(|&s| s > 0) || b.day > day + 2 * 364);
}

#[test]
fn a_save_taken_during_a_drought_resumes_bit_for_bit() {
    let mut a = Sim::new(&harsh(3)).unwrap();
    a.run_to_year(2);
    let d = drought(&a);
    a.poke(d);
    a.run_days(100);
    assert!(a.world.rain_shortfall.iter().any(|&s| s > 0), "the drought should be under way");
    let mut b = Sim::load(&a.save()).unwrap();
    assert_eq!(a.fingerprint(), b.fingerprint());
    a.run_to_year(5);
    b.run_to_year(5);
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert!(a.world.rain_shortfall.iter().all(|&s| s == 0), "the drought should be over");
    // History before the load isn't kept, so going back stops at the load.
    let before = b.calendar.year_start(2);
    assert!(b.rewind(before).unwrap_err().contains("nothing kept"));
}

#[test]
fn tracing_a_past_day_explains_it_and_changes_nothing() {
    let mut traced = Sim::new(&harsh(6)).unwrap();
    traced.strict = true;
    traced.run_to_year(2);
    let d = drought(&traced);
    traced.poke(d);
    traced.run_days(150);
    let mut plain = Sim::new(&harsh(6)).unwrap();
    plain.run_to_year(2);
    plain.poke(drought(&plain));
    plain.run_days(150);

    let (fp, events) = (traced.fingerprint(), log(&traced));
    // The first month start after the drought arrived: rain, harvest, spoilage, food asked for.
    let cal = &traced.calendar;
    let day = (cal.year_start(2) + 2..).find(|&d| cal.is_month_start(d)).unwrap();
    let province = (0..traced.world.provinces.count).max_by_key(|&i| traced.world.rain_shortfall[i]).unwrap() as u32;
    let t = traced.trace(day, province).unwrap();
    assert_eq!(traced.fingerprint(), fp, "tracing changed the world");
    assert_eq!(log(&traced), events, "tracing changed the event log");

    let notes: Vec<(Slot, &str)> = t.steps.iter().filter_map(|s| if let Traced::Note(n) = &s.what { Some((s.module.unwrap(), n.as_str())) } else { None }).collect();
    assert!(notes.iter().any(|(m, n)| *m == Slot::Weather && n.contains("drought")), "{notes:?}");
    assert!(notes.iter().any(|(m, n)| *m == Slot::Economics && n.starts_with("harvest") && n.contains("for the rain")), "{notes:?}");
    let stock = known("province.food_stock");
    let harvested = t.steps.iter().any(|s| s.stage == Stage::Tick && matches!(s.what, Traced::Changed { field, entity, .. } if field == stock && entity == province));
    assert!(harvested, "{:#?}", t.steps);
    assert!(t.steps.iter().any(|s| matches!(&s.what, Traced::Requested(r) if r.requester == Slot::Population)));
    assert!(t.steps.iter().any(|s| s.stage == Stage::Apply && matches!(&s.what, Traced::Granted(..))));

    // The next day the grant arrives, and the pops say how they fared.
    let t2 = traced.trace(day + 1, province).unwrap();
    assert!(t2.steps.iter().any(|s| s.stage == Stage::Deliver && s.module == Some(Slot::Population) && matches!(&s.what, Traced::Note(n) if n.contains("of the month's food"))));
    assert_eq!(traced.fingerprint(), fp);

    // A day that hasn't run can't be traced; and tracing on or off, the future is the same.
    assert!(traced.trace(traced.day, province).is_err());
    traced.run_to_year(5);
    plain.run_to_year(5);
    assert_eq!(traced.fingerprint(), plain.fingerprint());
}

#[test]
fn last_changed_days_follow_the_running_world() {
    let mut sim = Sim::new(&harsh(2)).unwrap();
    let stock = known("province.food_stock").0 as usize;
    let elevation = known("cell.elevation").0 as usize;
    assert!(sim.last_change[stock].iter().all(|&d| d == NEVER));
    let month = sim.calendar.def.months[0].days as u64;
    sim.run_days(month + 3);
    // Harvests and eating changed every farming province's store on the second month's first day.
    let changed = &sim.last_change[stock];
    assert!(changed.iter().filter(|&&d| d == month).count() > sim.world.provinces.count / 2, "{changed:?}");
    assert!(changed.iter().all(|&d| d == NEVER || d <= month + 2));
    // Generation's work never changes once the world runs.
    assert!(sim.last_change[elevation].is_empty());
}

#[test]
fn going_back_years_and_forward_again_gives_the_same_world_and_log() {
    use sim::edit::{Brush, Edit};
    let mut a = Sim::new(&harsh(4)).unwrap();
    a.strict = true;
    a.run_to_year(1);
    let (lon, lat) = sim::grid::dir_to_lonlat(a.world.provinces.center[0]);
    a.edit(Edit::Paint { path: vec![[lon.to_degrees(), lat.to_degrees()]], radius_km: 800.0, brush: Brush::Raise(600.0) }).unwrap();
    a.run_days(100);
    let d = drought(&a);
    a.poke(d);
    let end = a.calendar.year_start(5) + 200;
    a.run_days(end - a.day);
    let (fp, events) = (a.fingerprint(), log(&a));

    // Back four years (past the edit) and forward again: the forward jump starts from a kept
    // checkpoint more than a year ahead.
    a.goto(a.calendar.year_start(1) + 50).unwrap();
    a.goto(end).unwrap();
    assert_eq!(a.fingerprint(), fp);
    // The year before the day is in the log again, as the famine layer needs it.
    let year = a.calendar.days_per_year() as u64;
    let recent = |l: &[Posted]| l.iter().filter(|p| p.day + year >= end).cloned().collect::<Vec<_>>();
    assert_eq!(recent(&log(&a)), recent(&events));
    assert_eq!(a.problems, Default::default());
}
