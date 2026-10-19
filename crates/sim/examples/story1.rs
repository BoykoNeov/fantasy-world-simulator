//! Story 1 in full, as `tests/stories.rs` checks it, with modules switched on beyond the defaults:
//! story 1's world (200 provinces), the drought poked in after three years over the province that
//! grows the most food, then five years watched, on seeds 21, 3 and 7 under both economies. It
//! passes when on at least 2 of 3 seeds the drought brings an uprising in its region or a war for
//! food by its realm, and on at least 2 of 3 the same world without it has neither.
//!
//! cargo run --release -p sim --example story1 -- [religion] [disease] [nature] [KEY=VALUE ...]
//!
//! `religion`, `disease` and `nature` switch those modules on (the second wave; all three on by
//! default since Oct 19, so naming them changes nothing);
//! `KEY=VALUE` changes a setting.

use sim::config::Config;
use sim::events::Event;
use sim::module::Slot;
use sim::sim::Sim;
use sim::world::WorldParams;

const DROUGHT_YEARS: u64 = 3;

fn config(economics: &str, seed: u64, args: &[String]) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    let on = |m: &str| args.iter().any(|a| a == m);
    if on("religion") {
        c.modules.religion = Some(Default::default());
    }
    if on("disease") {
        c.modules.disease = Some(Default::default());
    }
    if on("nature") {
        c.modules.nature = Some(Default::default());
    }
    for (k, v) in args.iter().filter_map(|a| a.split_once('=')) {
        sim::settings::set(&mut c, k, v.parse().expect("a number")).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    c
}

/// Whether, with the drought and in the same world without it, a drought province rose up or its
/// realm declared a war for food within five years.
fn story(c: &Config) -> (bool, bool) {
    let mut sim = Sim::new(c).unwrap();
    let start = sim.calendar.date(0).year;
    sim.run_to_year(start + 3);
    let mut twin = sim.fork();
    let (count, heart) = {
        let p = &sim.world.provinces;
        (p.count, (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap())
    };
    let (lon, lat) = sim::grid::dir_to_lonlat(sim.world.provinces.center[heart]);
    sim.poke(Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days: DROUGHT_YEARS * 364 });
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
        assert_eq!(s.problems, Default::default());
        happened
    };
    (watch(&mut sim), watch(&mut twin))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut all = true;
    for economics in ["food", "markets"] {
        let runs: Vec<(bool, bool)> = [21, 3, 7].into_iter().map(|seed| story(&config(economics, seed, &args))).collect();
        let with = runs.iter().filter(|r| r.0).count();
        let quiet = runs.iter().filter(|r| !r.1).count();
        let pass = with >= 2 && quiet >= 2;
        all &= pass;
        println!("{economics}: with the drought {with} of 3, quiet without it {quiet} of 3 {runs:?}: {}", if pass { "pass" } else { "FAIL" });
    }
    println!("story 1 {}", if all { "passes" } else { "FAILS" });
}
