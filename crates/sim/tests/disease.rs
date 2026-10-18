//! Disease "plagues" in whole worlds: story 3's spread and toll (the lighter check; the full one,
//! with the labour shortage after, is `examples/story3.rs`), and every Disease setting at the ends
//! of its range with a plague going on.

use rayon::prelude::*;
use sim::config::Config;
use sim::disease::DiseaseParams;
use sim::events::Event;
use sim::grid::dot;
use sim::module::Slot;
use sim::requests::Cause;
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::collections::BTreeMap;

fn biggest_port(s: &Sim) -> u32 {
    let people = s.world.province_population();
    (0..people.len() as u32).filter(|&p| s.world.routes.of(p).any(|l| l.sea_days.is_some())).max_by_key(|&p| (people[p as usize], std::cmp::Reverse(p))).unwrap()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() { f64::NAN } else { v[v.len() / 2] }
}

/// Story 3 on one small world: a plague poked at the start of year 3 into the most populous port,
/// five years watched. Whether it went by sea at least twice as fast as over land (km a week, the
/// medians of its arrivals in the first 3 years), and whether its dead were 10–50% of the struck
/// provinces' people and at most 35% of the world's.
fn story_3(economics: &str, seed: u64) -> (bool, bool, String) {
    let mut c = Config::new(WorldParams::preset(Size::Small, seed));
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.disease = Some(DiseaseParams { starts: None, ..DiseaseParams::default() });
    let mut s = Sim::new(&c).unwrap();
    s.strict = true;
    let start = s.calendar.date(0).year;
    s.run_to_year(start + 3);
    let port = biggest_port(&s);
    let at_poke = s.world.province_population();
    let poke = s.day;
    let year = s.calendar.days_per_year() as u64;
    s.poke(Event::Plague { province: port });
    let mut reached: BTreeMap<u32, Vec<u64>> = BTreeMap::from([(port, vec![poke])]);
    let mut arrived = Vec::new();
    let mut dead = 0u64;
    while s.day < poke + 5 * year {
        s.step_day();
        for p in s.delivered_today() {
            if let Event::PlagueArrived { province, from, by_sea, .. } = p.event {
                reached.entry(province).or_default().push(p.day);
                arrived.push((province, from, by_sea, p.day));
            }
        }
        dead += s.applied_today.iter().filter(|(r, _)| r.cause == Cause::Plague).map(|x| x.1).sum::<u64>();
    }
    assert_eq!(s.problems, Default::default(), "{economics} seed {seed}");
    let km = |a: u32, b: u32| dot(s.world.provinces.center[a as usize], s.world.provinces.center[b as usize]).clamp(-1.0, 1.0).acos() * s.world.params.radius_km;
    let speed = |sea: bool| {
        median(
            arrived
                .iter()
                .filter(|a| a.2 == sea && a.3 < poke + 3 * year)
                .filter_map(|&(q, from, _, day)| {
                    let d0 = reached[&from].iter().copied().filter(|&d| d < day).max()?;
                    Some(km(from, q) / ((day - d0) as f64 / s.calendar.days_per_week() as f64).max(1.0))
                })
                .collect(),
        )
    };
    let (sea, land) = (speed(true), speed(false));
    let struck: u64 = reached.keys().map(|&p| at_poke[p as usize]).sum();
    let (share, world) = (dead as f64 / struck as f64, dead as f64 / at_poke.iter().sum::<u64>() as f64);
    (sea >= 2.0 * land, (0.10..=0.50).contains(&share) && world <= 0.35, format!("seed {seed}: by sea {sea:.0} km a week, over land {land:.0}; dead {:.1}% of the struck, {:.2}% of the world", 100.0 * share, 100.0 * world))
}

#[test]
fn story_3_plague_lands_in_a_port_and_ships_carry_it_faster_than_roads() {
    // The full story is 6 of 9 seeds (examples/story3.rs); here its first three seeds, 2 of 3.
    let runs: Vec<(&str, u64)> = ["food", "markets"].into_iter().flat_map(|e| [21u64, 7, 1].into_iter().map(move |s| (e, s))).collect();
    let out: Vec<(bool, bool, String)> = runs.par_iter().map(|&(e, s)| story_3(e, s)).collect();
    for e in ["food", "markets"] {
        let mine: Vec<&(bool, bool, String)> = runs.iter().zip(&out).filter(|x| x.0.0 == e).map(|x| x.1).collect();
        let fast = mine.iter().filter(|x| x.0).count();
        let toll = mine.iter().filter(|x| x.1).count();
        let lines: Vec<&str> = mine.iter().map(|x| x.2.as_str()).collect();
        assert!(fast >= 2 && toll >= 2, "{e}: by sea twice as fast on {fast} of 3, its toll in range on {toll} of 3:\n{}", lines.join("\n"));
    }
}

/// Every Disease setting alone at the ends of its range, on a tiny world with realms and armies, a
/// plague poked into its most populous province and its plagues beginning on their own as well: a
/// year runs cleanly (no value out of its bounds, no total broken), or the module refuses it.
#[test]
fn every_disease_setting_works_at_the_ends_of_its_range_with_a_plague_going_on() {
    let mut base = Config::new(WorldParams { n: 24, provinces: 40, seed: 12, land_fraction: 0.4, radius_km: 6371.0 });
    base.modules.disease = Some(DiseaseParams::default());
    let items: Vec<sim::settings::Item> = sim::settings::items(&base).into_iter().filter(|i| i.key.starts_with("modules.disease")).collect();
    assert_eq!(items.len(), 18);
    let mut cases = Vec::new();
    for i in &items {
        for end in [i.spec.min, i.spec.max] {
            let mut c = base.clone();
            sim::settings::set(&mut c, &i.key, if i.whole { end.round() } else { end }).unwrap();
            cases.push((format!("{} = {end}", i.key), c));
        }
    }
    let failures: Vec<String> = cases
        .into_par_iter()
        .filter_map(|(what, c)| {
            let Ok(mut s) = Sim::new(&c) else { return None };
            s.strict = true;
            let people = s.world.province_population();
            let p = (0..people.len()).max_by_key(|&p| people[p]).unwrap() as u32;
            s.run_days(20);
            s.poke(Event::Plague { province: p });
            s.run_days(364);
            let bad = s.problems != Default::default() || s.world.sick.iter().zip(&s.world.immune).any(|(a, b)| a + b > 1_000_000);
            bad.then(|| format!("{what}: {:?}", s.problems))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
