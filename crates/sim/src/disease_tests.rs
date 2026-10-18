//! Tests of Disease "plagues" (build step 10).

use crate::config::Config;
use crate::disease::{DiseaseParams, StartParams, IMMUNE_MOST};
use crate::edit::{Edit, SettingChange};
use crate::events::Event;
use crate::grid::NONE;
use crate::requests::{Cause, Priority};
use crate::sim::{Problems, Sim};
use crate::world::WorldParams;
use std::sync::Arc;

/// A small world without realms, with Disease and no plagues beginning by chance.
fn config(seed: u64, params: DiseaseParams) -> Config {
    let mut c = Config::new(WorldParams { n: 32, provinces: 120, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.without_realms();
    c.modules.disease = Some(DiseaseParams { starts: None, ..params });
    c
}

fn sim(c: &Config) -> Sim {
    let mut s = Sim::new(c).unwrap();
    s.strict = true;
    s
}

/// Plague kept to one province: crowding, hunger, weakening and immunity's halving switched off.
fn plain() -> DiseaseParams {
    DiseaseParams { crowding: 0.0, hunger: 0.0, fade_years: 1000.0, immunity_years: 1000.0, ..DiseaseParams::default() }
}

/// Cut every way into and out of the provinces `ps`, except between `keep` pairs.
fn cut(s: &mut Sim, ps: &[u32], keep: &[(u32, u32)]) {
    let r = Arc::make_mut(&mut s.world.routes);
    for l in r.links.iter_mut() {
        let kept = keep.iter().any(|&(a, b)| (l.a, l.b) == (a.min(b), a.max(b)));
        if !kept && (ps.contains(&l.a) || ps.contains(&l.b)) {
            (l.land_days, l.sea_days) = (None, None);
        }
    }
}

fn most_populous(s: &Sim) -> u32 {
    let people = s.world.province_population();
    (0..people.len()).max_by_key(|&p| (people[p], std::cmp::Reverse(p))).unwrap() as u32
}

/// What a run saw: outbreaks begun (number, province), arrivals (outbreak, province, from, by sea,
/// day), outbreaks over (number, dead, provinces), dead of plague granted, and the plague requests.
#[derive(Default)]
struct Seen {
    begun: Vec<(u32, u32)>,
    arrived: Vec<(u32, u32, u32, bool, u64)>,
    over: Vec<(u32, u64, u32)>,
    granted: u64,
    refused: Vec<String>,
}

fn run(s: &mut Sim, days: u64, seen: &mut Seen) {
    for _ in 0..days {
        s.step_day();
        for p in s.delivered_today() {
            match &p.event {
                Event::Outbreak { outbreak, province } => seen.begun.push((*outbreak, *province)),
                Event::PlagueArrived { outbreak, province, from, by_sea } => seen.arrived.push((*outbreak, *province, *from, *by_sea, p.day)),
                Event::OutbreakOver { outbreak, dead, provinces } => seen.over.push((*outbreak, *dead, *provinces)),
                Event::Refused { why } => seen.refused.push(why.clone()),
                _ => {}
            }
        }
        for (r, g) in &s.applied_today {
            if r.cause == Cause::Plague {
                assert_eq!(r.priority, Priority::Critical, "plague deaths come before everything");
                assert_eq!(*g, r.take, "plague deaths are always granted in full");
                seen.granted += g;
            }
        }
    }
}

/// Run until every outbreak begun is over (at most `days`).
fn run_out(s: &mut Sim, days: u64, seen: &mut Seen) {
    let mut left = days;
    while left > 0 && (seen.begun.is_empty() || seen.over.len() < seen.begun.len()) {
        run(s, 7, seen);
        left = left.saturating_sub(7);
    }
}

fn final_size(r0: f64) -> f64 {
    // z = 1 - exp(-r0 z), by repeated substitution.
    let mut z = 0.9;
    for _ in 0..200 {
        z = 1.0 - (-r0 * z).exp();
    }
    z
}

#[test]
fn a_plague_in_one_province_takes_its_textbook_share_and_kills_its_deadliness_of_them() {
    // With nobody dying of it, the share ever ill is the textbook final size; with deaths a little
    // more fall ill, as the dead no longer meet anyone and the sick are a larger share of the living.
    for (seed, catching, deadliness) in [(7, 2.0, 0.0), (3, 3.0, 0.0), (11, 1.5, 0.0), (7, 2.0, 0.3), (3, 3.0, 0.3)] {
        let mut s = sim(&config(seed, DiseaseParams { catching, deadliness, ..plain() }));
        s.run_days(30);
        let p = most_populous(&s);
        cut(&mut s, &[p], &[]);
        let before = s.world.province_population()[p as usize];
        let year_dead = |s: &Sim| s.history.iter().map(|y| y.ledger.plague).sum::<u64>() + s.year_so_far().ledger.plague;
        let already = year_dead(&s);
        s.poke(Event::Plague { province: p });
        let mut seen = Seen::default();
        run_out(&mut s, 3 * 364, &mut seen);
        assert_eq!(seen.begun, vec![(0, p)]);
        assert!(seen.arrived.is_empty(), "the province is cut off");
        assert_eq!(seen.over.len(), 1, "seed {seed}: the outbreak ends");
        let (_, dead, provinces) = seen.over[0];
        assert_eq!(provinces, 1);
        // Every count of the dead agrees, to the person.
        assert_eq!(dead, seen.granted);
        assert_eq!(year_dead(&s) - already, dead);
        assert_eq!(s.problems, Problems::default());
        // Those ever ill: the dead and the immune survivors (immunity doesn't fade here; births
        // since thin it a little, so the survivors are counted as people now × the immune share).
        let now = s.world.province_population()[p as usize];
        let survivors = now as f64 * s.world.immune[p as usize] as f64 / 1e6;
        let ill = dead as f64 + survivors;
        let z = final_size(catching);
        let share_ill = ill / before as f64;
        if deadliness == 0.0 {
            assert_eq!(dead, 0);
            assert!((share_ill - z).abs() < 0.02, "seed {seed}, catching {catching}: {share_ill:.3} ill, the textbook {z:.3}");
        } else {
            assert!(share_ill > z - 0.01 && share_ill < z + 0.1, "seed {seed}, catching {catching}: {share_ill:.3} ill, the textbook {z:.3}");
            assert!((dead as f64 / ill - deadliness).abs() < 0.01, "seed {seed}: {dead} dead of {ill:.0} ill");
        }
        println!("seed {seed}, catching {catching}, deadliness {deadliness}: {share_ill:.3} of the people ill (textbook {z:.3}), {:.3} dead", dead as f64 / before as f64);
        assert_eq!((s.world.sick[p as usize], s.world.outbreak[p as usize]), (0, NONE));
        // The year's dead in the province's field (the outbreak ran within a year or two).
        assert!(s.world.plague_dead[p as usize] <= dead);
    }
}

#[test]
fn a_plague_poked_where_nobody_lives_is_refused_and_one_that_cannot_catch_ends() {
    let mut s = sim(&config(5, plain()));
    let people = s.world.province_population();
    let empty = (0..people.len()).find(|&p| people[p] == 0).unwrap_or(people.len()) as u32;
    s.poke(Event::Plague { province: empty });
    let mut seen = Seen::default();
    run(&mut s, 3, &mut seen);
    assert!(seen.begun.is_empty());
    assert!(seen.refused.iter().any(|w| w.contains("nobody lives")), "{:?}", seen.refused);
    // Nobody catches it: the first cases recover (or die) and it is over within weeks.
    let mut s = sim(&config(5, DiseaseParams { catching: 0.0, ..plain() }));
    let p = most_populous(&s);
    s.poke(Event::Plague { province: p });
    let mut seen = Seen::default();
    run_out(&mut s, 200, &mut seen);
    assert_eq!(seen.over.len(), 1);
    let first = (people[p as usize] as f64 * 0.0005).max(10.0);
    assert!((seen.over[0].1 as f64) < first * 0.5, "{} dead of about {first} first cases", seen.over[0].1);
}

#[test]
fn plague_deaths_come_before_recruits_in_a_world_at_war() {
    // Realms, armies and wars on: every plague death asked for is granted in full, first.
    let mut c = Config::new(WorldParams { n: 32, provinces: 120, seed: 21, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.disease = Some(DiseaseParams { starts: None, ..DiseaseParams::default() });
    let mut s = sim(&c);
    s.run_days(60);
    let p = most_populous(&s);
    s.poke(Event::Plague { province: p });
    let mut seen = Seen::default();
    run(&mut s, 2 * 364, &mut seen);
    assert!(seen.granted > 0);
    assert_eq!(s.problems, Problems::default());
}

#[test]
fn ships_carry_it_sooner_than_roads_of_the_same_days() {
    // A province with one neighbour 3 days away by road and another 3 days away by sea, cut off
    // from everything else; poked on many days, the port is reached first most of the time.
    let params = DiseaseParams { towns: 0.0, ..plain() };
    let mut base = sim(&config(9, params));
    base.run_days(14);
    let p = most_populous(&base);
    let people = base.world.province_population();
    let mut near: Vec<u32> = base.world.routes.of(p).map(|l| l.other(p)).filter(|&q| people[q as usize] > 0).collect();
    near.sort();
    assert!(near.len() >= 2, "province {p} has {} neighbours with people", near.len());
    let (road, port) = (near[0], near[1]);
    cut(&mut base, &[p, road, port], &[(p, road), (p, port)]);
    {
        let r = Arc::make_mut(&mut base.world.routes);
        for l in r.links.iter_mut() {
            if (l.a, l.b) == (p.min(road), p.max(road)) {
                (l.land_days, l.sea_days) = (Some(3.0), None);
            }
            if (l.a, l.b) == (p.min(port), p.max(port)) {
                (l.land_days, l.sea_days) = (None, Some(3.0));
            }
        }
    }
    let (mut by_sea, mut by_land, mut trials) = (0, 0, 0);
    for k in 0..40 {
        let mut s = base.fork();
        s.run_days(k * 3);
        s.poke(Event::Plague { province: p });
        let mut seen = Seen::default();
        run_out(&mut s, 2 * 364, &mut seen);
        trials += 1;
        if let Some(&(_, q, from, sea, _)) = seen.arrived.first() {
            assert_eq!(from, p);
            assert_eq!(sea, q == port, "the way to the port is by sea, the other by road");
            if q == port { by_sea += 1 } else { by_land += 1 }
        }
    }
    assert!(by_sea + by_land > trials / 2, "it rarely left the province: {by_sea} + {by_land} of {trials}");
    assert!(by_sea as f64 >= 0.6 * (by_sea + by_land) as f64, "by sea first {by_sea}, by land first {by_land}");
}

#[test]
fn survivors_are_immune_so_a_second_plague_soon_after_fizzles_and_one_a_generation_later_does_not() {
    let params = DiseaseParams { crowding: 0.0, hunger: 0.0, ..DiseaseParams::default() };
    let mut s = sim(&config(7, params));
    s.run_days(30);
    let p = most_populous(&s);
    cut(&mut s, &[p], &[]);
    s.poke(Event::Plague { province: p });
    let mut seen = Seen::default();
    run_out(&mut s, 3 * 364, &mut seen);
    let first = seen.over[0].1;
    assert!(s.world.immune[p as usize] > 600_000, "{}", s.world.immune[p as usize]);
    assert!(s.world.immune[p as usize] <= IMMUNE_MOST);
    let mut later = s.fork();
    s.poke(Event::Plague { province: p });
    let mut soon = Seen::default();
    run_out(&mut s, 3 * 364, &mut soon);
    assert_eq!(soon.over.len(), 1);
    assert!((soon.over[0].1 as f64) < 0.02 * first as f64, "soon after: {} dead against {first}", soon.over[0].1);
    // Thirty years on, immunity has halved twice and births have thinned it.
    later.run_days(30 * 364);
    assert!(later.world.immune[p as usize] < 250_000, "{}", later.world.immune[p as usize]);
    later.poke(Event::Plague { province: p });
    let mut again = Seen::default();
    run_out(&mut later, 3 * 364, &mut again);
    assert!((again.over[0].1 as f64) > 0.3 * first as f64, "a generation later: {} dead against {first}", again.over[0].1);
    assert_eq!(later.problems, Problems::default());
}

#[test]
fn outbreaks_weaken_with_age_and_hunger_makes_it_worse() {
    // Weakening: with outbreaks halving every few weeks, fewer fall ill than at full strength.
    let dead = |params: DiseaseParams, starve: bool| {
        let mut s = sim(&config(7, params));
        s.run_days(30);
        let p = most_populous(&s);
        cut(&mut s, &[p], &[]);
        if starve {
            // No food in store and no rain for two years.
            s.edit(Edit::SetFoodStock { province: p, rations: 0 }).unwrap();
            let (lon, lat) = crate::grid::dir_to_lonlat(s.world.provinces.center[p as usize]);
            s.poke(Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 300.0, shortfall: 1000, days: 2 * 364 });
        }
        s.poke(Event::Plague { province: p });
        let mut seen = Seen::default();
        run_out(&mut s, 10 * 364, &mut seen);
        assert_eq!(seen.over.len(), 1, "the outbreak is over within 10 years");
        let starved = s.world.starved_this_year[p as usize] + s.history.iter().map(|y| y.ledger.starved).sum::<u64>();
        (seen.over[0].1, starved)
    };
    let full = dead(plain(), false).0;
    let weak = dead(DiseaseParams { fade_years: 0.1, ..plain() }, false).0;
    assert!((weak as f64) < 0.7 * full as f64, "weakening: {weak} against {full}");
    // Hunger: the same province with its food store taken away starves, and the plague takes more.
    let (fed, _) = dead(DiseaseParams { hunger: 10.0, ..plain() }, false);
    let (hungry, starved) = dead(DiseaseParams { hunger: 10.0, catching: 1.5, ..plain() }, true);
    let (hungry_plain, _) = dead(DiseaseParams { hunger: 0.0, catching: 1.5, ..plain() }, true);
    assert!(starved > 0, "the province starved");
    assert!(hungry > hungry_plain, "with hunger {hungry}, without {hungry_plain}");
    assert!(fed > 0);
}

#[test]
fn plagues_beginning_by_chance_begin_mostly_in_ports() {
    let ports = |weight: f64| {
        // Chance 1 a year, checked monthly: one each month. Nobody else catches it, so each is
        // over within weeks.
        let starts = StartParams { per_year: 1.0, port_weight: weight, city_power: 1.0, smallest: 0 };
        let mut c = config(13, DiseaseParams { catching: 0.0, ..plain() });
        c.modules.disease.as_mut().unwrap().starts = Some(starts);
        let mut s = sim(&c);
        let mut seen = Seen::default();
        run(&mut s, 15 * 364, &mut seen);
        let port = |p: u32| s.world.routes.of(p).any(|l| l.sea_days.is_some());
        let n = seen.begun.len();
        (seen.begun.iter().filter(|b| port(b.1)).count() as f64 / n as f64, n)
    };
    let (weighted, n) = ports(10.0);
    let (even, m) = ports(1.0);
    // Chance 1 a year, checked monthly: one each month (unless every big place has one already).
    assert!(n > 150 && m > 150, "{n} and {m} plagues began");
    assert!(weighted >= 0.6, "{:.0}% in ports", 100.0 * weighted);
    assert!(weighted > even + 0.1, "ports {weighted:.2} at weight 10, {even:.2} at 1");
}

#[test]
fn a_world_with_disease_and_no_plague_is_the_world_without_it() {
    for economics in ["food", "markets"] {
        let mut c = config(21, plain());
        c.modules.set_choice(crate::module::Slot::Economics, economics).unwrap();
        let mut off = c.clone();
        off.modules.disease = None;
        let (mut a, mut b) = (sim(&c), sim(&off));
        a.run_days(3 * 364);
        b.run_days(3 * 364);
        assert_eq!(a.world.pops, b.world.pops, "{economics}");
        assert_eq!(a.world.stock, b.world.stock, "{economics}");
    }
}

#[test]
fn outbreaks_go_on_through_saves_live_settings_and_map_edits() {
    let mut c = config(7, DiseaseParams::default());
    c.modules.disease.as_mut().unwrap().starts = Some(StartParams::default());
    let mut s = sim(&c);
    s.run_days(30);
    let p = most_populous(&s);
    s.poke(Event::Plague { province: p });
    s.run_days(70);
    assert!(s.world.sick.iter().any(|&x| x > 0));
    // Saved and loaded mid-outbreak: it runs on the same.
    let mut loaded = Sim::load(&s.save()).unwrap();
    let mut straight = s.fork();
    // Every live setting set to its value: the module is made again and keeps its state.
    let changes: Vec<SettingChange> = crate::settings::items(&c)
        .into_iter()
        .filter(|i| i.key.starts_with("modules.disease"))
        .map(|i| SettingChange { key: i.key, value: i.value })
        .collect();
    assert_eq!(changes.len(), 18);
    s.edit(Edit::Settings { changes }).unwrap();
    for x in [&mut s, &mut loaded, &mut straight] {
        x.run_days(200);
    }
    assert_eq!(s.fingerprint(), straight.fingerprint());
    assert_eq!(loaded.fingerprint(), straight.fingerprint());
    // A live change acts at once: no more deaths with deadliness 0.
    let mut harmless = straight.fork();
    harmless.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.disease.deadliness".into(), value: 0.0 }] }).unwrap();
    let mut seen = Seen::default();
    run(&mut harmless, 100, &mut seen);
    assert_eq!(seen.granted, 0, "nobody dies of it");

    // A province split off a sick province takes its shares; merged, the shares mix by people.
    let mut e = sim(&config(7, plain()));
    e.run_days(30);
    let p = most_populous(&e);
    e.poke(Event::Plague { province: p });
    e.run_days(50);
    let (sick, immune, o) = (e.world.sick[p as usize], e.world.immune[p as usize], e.world.outbreak[p as usize]);
    assert!(sick > 0 && o != NONE);
    let cells: Vec<u32> = (0..e.world.provinces.of_cell.len() as u32).filter(|&c| e.world.provinces.of_cell[c as usize] == p).take(3).collect();
    e.edit(Edit::Split { cells }).unwrap();
    let new = e.world.provinces.count - 1;
    assert_eq!((e.world.sick[new], e.world.immune[new], e.world.outbreak[new]), (sick, immune, o));
    e.edit(Edit::Merge { from: new as u32, into: p }).unwrap();
    assert_eq!(e.world.outbreak[new], NONE);
    assert_eq!(e.world.sick[new], 0);
    assert!(e.world.sick[p as usize].abs_diff(sick) <= 1);
    let mut seen = Seen::default();
    run_out(&mut e, 3 * 364, &mut seen);
    assert_eq!(e.problems, Problems::default());
    e.world.check_lengths().unwrap();
}

#[test]
fn the_years_plague_dead_start_again_each_year() {
    let mut s = sim(&config(7, plain()));
    s.run_days(300);
    let p = most_populous(&s);
    s.poke(Event::Plague { province: p });
    let mut seen = Seen::default();
    run_out(&mut s, 3 * 364, &mut seen);
    let year = s.calendar.date(s.day).year;
    s.run_to_year(year + 2);
    s.run_days(10);
    assert!(s.world.plague_dead.iter().all(|&d| d == 0), "a year with no plague has no plague dead");
    let ledger: u64 = s.history.iter().map(|y| y.ledger.plague).sum();
    assert_eq!(ledger, seen.granted);
    // The count since the world began keeps them all, in the province where they died.
    assert_eq!(s.world.plague_dead_total.iter().sum::<u64>(), seen.granted);
    assert!(s.world.plague_dead_total[p as usize] > 0);
}

#[test]
fn plague_names_differ_and_come_round_with_a_number() {
    let names: Vec<String> = (0..192).map(|o| crate::disease::name(7, o)).collect();
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 192, "{names:?}");
    assert!(names.iter().all(|n| n.starts_with("the ") && n.split(' ').count() == 3), "{names:?}");
    assert_eq!(crate::disease::name(7, 192), format!("{} II", names[0]));
    // Another world names them in another order.
    assert_ne!((0..5).map(|o| crate::disease::name(8, o)).collect::<Vec<_>>(), names[..5]);
}
