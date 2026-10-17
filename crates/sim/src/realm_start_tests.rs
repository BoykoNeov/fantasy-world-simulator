//! Realms at world creation (step 7): the three ways to start and their mix, capitals, names,
//! claims over time, and realms put right after edits change the map.

use crate::branch::Timelines;
use crate::config::Config;
use crate::edit::Edit;
use crate::grid::NONE;
use crate::module::Slot;
use crate::realms::{names, RealmParams};
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};
use std::collections::BTreeSet;

fn params(settled: f64, city_states: f64, frontier: f64) -> RealmParams {
    RealmParams { settled, city_states, frontier, provinces_per_realm: 8.0, ..RealmParams::default() }
}

fn config(realms: RealmParams, seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.realms = Some(realms);
    c
}

fn world(realms: RealmParams, seed: u64) -> Sim {
    let mut sim = Sim::new(&config(realms, seed)).unwrap();
    sim.strict = true;
    sim
}

fn settled(w: &World) -> Vec<bool> {
    let people = w.province_population();
    (0..w.provinces.count).map(|p| w.provinces.cells[p] > 0 && people[p] > 0).collect()
}

fn alive(w: &World) -> Vec<u32> {
    (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect()
}

fn provinces_of(w: &World, r: u32) -> Vec<u32> {
    (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r).collect()
}

fn unowned_settled(w: &World) -> usize {
    settled(w).iter().zip(&w.owner).filter(|(s, o)| **s && **o == NONE).count()
}

/// Every realm alive holds its capital, is in one piece along the province ways, and every
/// province with no land belongs to nobody.
fn check_realms(w: &World) {
    check_owners(w);
    for r in alive(w) {
        let mine = provinces_of(w, r);
        let capital = w.realms.capital[r as usize];
        let mut seen = BTreeSet::from([capital]);
        let mut todo = vec![capital];
        while let Some(p) = todo.pop() {
            for l in w.routes.of(p) {
                let q = l.other(p);
                if w.owner[q as usize] == r && seen.insert(q) {
                    todo.push(q);
                }
            }
        }
        assert_eq!(seen.len(), mine.len(), "realm {r} is in pieces: {mine:?}");
    }
}

/// [`check_realms`] without the one piece: after the map changes (a strait opened by a rising
/// sea), a realm can be cut in two.
fn check_owners(w: &World) {
    assert_eq!(w.owner.len(), w.provinces.count);
    for p in 0..w.provinces.count {
        if w.provinces.cells[p] == 0 {
            assert_eq!(w.owner[p], NONE, "province {p} has no land but an owner");
        }
        if w.owner[p] != NONE {
            assert!(w.realms.alive(w.owner[p]), "province {p} belongs to dissolved realm {}", w.owner[p]);
        }
    }
    for r in alive(w) {
        let capital = w.realms.capital[r as usize];
        assert_eq!(w.owner[capital as usize], r, "realm {r} doesn't hold its capital");
    }
}

#[test]
fn realms_over_settled_land_own_all_of_it() {
    for seed in [1, 2, 3] {
        let sim = world(params(1.0, 0.0, 0.0), seed);
        let w = &sim.world;
        check_realms(w);
        assert_eq!(unowned_settled(w), 0, "seed {seed}");
        let settled = settled(w).iter().filter(|&&s| s).count();
        let realms = alive(w).len();
        // About one realm per 8 provinces; islands no realm reaches add a few small ones.
        assert!(realms as f64 >= settled as f64 / 8.0 * 0.7 && realms as f64 <= settled as f64 / 8.0 * 2.0, "seed {seed}: {realms} realms over {settled} settled provinces");
        // The capital is the realm's most populous province.
        let people = w.province_population();
        for r in alive(w) {
            let most = provinces_of(w, r).into_iter().map(|p| people[p as usize]).max().unwrap();
            assert_eq!(people[w.realms.capital[r as usize] as usize], most);
        }
    }
}

#[test]
fn city_states_hold_one_to_three_provinces_all_settled() {
    let sim = world(params(0.0, 1.0, 0.0), 2);
    let w = &sim.world;
    check_realms(w);
    assert_eq!(unowned_settled(w), 0);
    let settled = settled(w);
    for r in alive(w) {
        assert!(provinces_of(w, r).iter().all(|&p| settled[p as usize]), "city-state {r} holds empty land");
    }
    let sizes: Vec<usize> = alive(w).iter().map(|&r| provinces_of(w, r).len()).collect();
    assert!(sizes.iter().all(|&n| (1..=RealmParams::default().city_state_max as usize).contains(&n)), "{sizes:?}");
    assert!(sizes.iter().any(|&n| n > 1), "{sizes:?}");
    assert!(sizes.len() as f64 >= settled.iter().filter(|&&s| s).count() as f64 / RealmParams::default().city_state_max as f64);
}

#[test]
fn the_frontier_leaves_land_between_realms_that_they_claim_over_time() {
    // Without unrest: land changes hands here by claims alone, not by provinces breaking away.
    let mut sim = world(RealmParams { unrest: None, ..params(0.0, 0.0, 1.0) }, 3);
    let realms = alive(&sim.world).len();
    let at_start = unowned_settled(&sim.world);
    check_realms(&sim.world);
    let settled = settled(&sim.world).iter().filter(|&&s| s).count();
    assert!(at_start as f64 > settled as f64 * 0.3, "{at_start} of {settled} settled provinces unclaimed");
    let before = sim.world.owner.clone();
    sim.run_to_year(sim.calendar.date(0).year + 30);
    let w = &sim.world;
    let later = unowned_settled(w);
    assert!(later < at_start / 2, "{at_start} unclaimed at the start, {later} after 30 years");
    // Claims only take land nobody owned, and every realm is still in one piece.
    for p in 0..w.provinces.count {
        assert!(before[p] == NONE || before[p] == w.owner[p], "province {p} changed hands");
    }
    check_realms(w);
    assert_eq!(alive(w).len(), realms, "no realm is made or lost by claiming");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_mix_starts_each_way_in_areas_of_its_own() {
    let sim = world(params(1.0, 1.0, 1.0), 1);
    let w = &sim.world;
    check_realms(w);
    let settled = settled(w);
    let sizes: Vec<usize> = alive(w).iter().map(|&r| provinces_of(w, r).iter().filter(|&&p| settled[p as usize]).count()).collect();
    assert!(sizes.iter().any(|&n| n > RealmParams::default().city_state_max as usize), "a realm bigger than a city-state: {sizes:?}");
    assert!(sizes.iter().filter(|&&n| n <= RealmParams::default().city_state_max as usize).count() >= 5, "city-states: {sizes:?}");
    assert!(unowned_settled(w) > 0, "a frontier");
    // The areas are large: settled neighbours start the same way far more often than the third
    // of the time they would if each province's way were drawn alone. (This world's provinces
    // are big, a few to an area; on a world of thousands an area holds dozens.)
    let way = crate::realms::ways(w, &params(1.0, 1.0, 1.0), w.params.seed, &settled);
    let (mut same, mut all) = (0, 0);
    for l in &w.routes.links {
        if settled[l.a as usize] && settled[l.b as usize] {
            all += 1;
            same += (way[l.a as usize] == way[l.b as usize]) as usize;
        }
    }
    assert!(same as f64 > 0.5 * all as f64, "{same} of {all} neighbours start the same way");
    assert_eq!(BTreeSet::from_iter(settled.iter().zip(&way).filter(|p| *p.0).map(|p| *p.1)).len(), 3);
}

#[test]
fn the_same_seed_makes_the_same_realms_and_names() {
    let a = world(params(1.0, 1.0, 1.0), 4);
    let b = world(params(1.0, 1.0, 1.0), 4);
    let c = world(params(1.0, 1.0, 1.0), 5);
    assert_eq!((&a.world.owner, &a.world.realms), (&b.world.owner, &b.world.realms));
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert_ne!(a.world.owner, c.world.owner);

    let list = names(4, 300);
    assert_eq!(list, names(4, 300));
    assert_eq!(&names(4, 10)[..], &list[..10], "a realm's name doesn't depend on how many there are");
    assert_eq!(BTreeSet::from_iter(&list).len(), list.len(), "no two alike");
    assert!(list.iter().all(|n| n.len() >= 4 && n.chars().next().unwrap().is_uppercase() && n.chars().skip(1).all(|c| c.is_ascii_lowercase())), "{list:?}");
    assert_ne!(names(5, 10), names(4, 10));
}

#[test]
fn realms_need_people_and_sensible_settings() {
    let mut c = config(RealmParams::default(), 1);
    c.modules.population = None;
    let e = Sim::new(&c).err().expect("refused without people").0;
    assert!(e.contains("pop.size"), "{e}");
    for bad in [params(0.0, 0.0, 0.0), params(-1.0, 1.0, 0.0), RealmParams { unclaimed_share: 1.0, ..params(0.0, 0.0, 1.0) }, RealmParams { provinces_per_realm: 0.5, ..params(1.0, 0.0, 0.0) }] {
        assert!(Sim::new(&config(bad.clone(), 1)).is_err(), "{bad:?}");
    }
}

#[test]
fn a_province_merged_away_belongs_to_nobody_and_a_realm_left_without_land_is_dissolved() {
    let mut sim = world(params(0.0, 1.0, 0.0), 2);
    let w = &sim.world;
    // A city-state of one province, next to another realm's province.
    let (r, p, q) = alive(w)
        .into_iter()
        .filter_map(|r| match provinces_of(w, r)[..] {
            [p] => w.routes.of(p).map(|l| l.other(p)).find(|&q| w.owner[q as usize] != NONE).map(|q| (r, p, q)),
            _ => None,
        })
        .next()
        .expect("a one-province city-state with a neighbour");
    let other = sim.world.owner[q as usize];
    sim.edit(Edit::Merge { from: p, into: q }).unwrap();
    let w = &sim.world;
    assert_eq!(w.owner[p as usize], NONE);
    assert!(!w.realms.alive(r), "realm {r} lost its only province");
    assert_eq!(w.owner[q as usize], other);
    check_realms(w);
    sim.run_days(40);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_capital_merged_away_moves_to_the_realms_most_populous_province() {
    let mut sim = world(params(1.0, 0.0, 0.0), 1);
    let w = &sim.world;
    let (r, capital, into) = alive(w)
        .into_iter()
        .find_map(|r| {
            let c = w.realms.capital[r as usize];
            w.routes.of(c).map(|l| l.other(c)).find(|&q| w.owner[q as usize] == r).map(|q| (r, c, q))
        })
        .expect("a capital with a neighbour in its own realm");
    sim.edit(Edit::Merge { from: capital, into }).unwrap();
    let w = &sim.world;
    let people = w.province_population();
    let now = w.realms.capital[r as usize];
    assert_ne!(now, capital);
    assert_eq!(people[now as usize], provinces_of(w, r).iter().map(|&p| people[p as usize]).max().unwrap());
    check_realms(w);
}

#[test]
fn land_sinking_under_the_sea_takes_its_owner_with_it() {
    let mut sim = world(params(1.0, 1.0, 0.0), 3);
    sim.edit(Edit::SeaLevel { rise_m: 400.0 }).unwrap();
    let w = &sim.world;
    assert!((0..w.provinces.count).any(|p| w.provinces.cells[p] == 0), "some provinces sank");
    check_owners(w);
    sim.run_days(40);
    check_owners(&sim.world);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn claims_survive_saving_loading_and_going_back() {
    let mut sim = world(params(0.0, 0.0, 1.0), 3);
    let year = sim.calendar.days_per_year() as u64;
    sim.run_days(3 * year + 40);
    let (print, today) = (sim.fingerprint(), sim.day);
    let owners = sim.world.owner.clone();

    let mut loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), print);
    sim.run_days(2 * year);
    loaded.run_days(2 * year);
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    assert_ne!(sim.world.owner, owners, "claims in those two years");

    let end = sim.fingerprint();
    sim.goto(today).unwrap();
    assert_eq!((sim.fingerprint(), &sim.world.owner), (print, &owners));
    sim.goto(today + 2 * year).unwrap();
    assert_eq!(sim.fingerprint(), end);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn realms_run_under_every_economy() {
    // Not with economics off: realms need people, and people need someone to keep their food.
    for economics in ["markets", "food"] {
        let mut c = config(params(1.0, 1.0, 1.0), 2);
        c.modules.set_choice(Slot::Economics, economics).unwrap();
        let mut sim = Sim::new(&c).unwrap();
        sim.strict = true;
        let at_start = unowned_settled(&sim.world);
        sim.run_to_year(sim.calendar.date(0).year + 3);
        check_realms(&sim.world);
        assert!(unowned_settled(&sim.world) < at_start, "{economics}: claims");
        assert_eq!(sim.problems, Problems::default(), "{economics}");
    }
}

#[test]
fn a_scenario_file_keeps_its_realms() {
    let mut sim = world(params(1.0, 1.0, 1.0), 3);
    sim.run_days(400);
    let text = Timelines::new(sim.fork()).scenario_text();
    assert!(text.contains("city_states"), "the realms settings are written");
    let mut read = Timelines::from_scenario_text(&text).unwrap();
    let back = read.list.swap_remove(read.active).sim;
    assert_eq!((back.day, back.fingerprint()), (sim.day, sim.fingerprint()));
    assert_eq!(back.world.owner, sim.world.owner);
}

#[test]
fn edits_leave_worlds_without_realms_alone() {
    let mut c = config(RealmParams::default(), 1);
    c.modules.realms = None;
    let mut sim = Sim::new(&c).unwrap();
    let p = (0..sim.world.provinces.count as u32).find(|&p| sim.world.provinces.cells[p as usize] > 0).unwrap();
    let q = sim.world.routes.of(p).next().unwrap().other(p);
    sim.edit(Edit::Merge { from: p, into: q }).unwrap();
    sim.edit(Edit::SeaLevel { rise_m: 200.0 }).unwrap();
    assert!(sim.world.realms.count() == 0 && sim.world.owner.iter().all(|&o| o == NONE));
}

#[test]
fn a_number_of_realms_asked_for_starts_whatever_the_province_count() {
    // Land none of the realms asked for reaches (islands) starts realms of its own; one realm
    // asked for shows how many that makes at most.
    let count = |realm_count: u32, provinces: usize| {
        let mut c = config(RealmParams { realm_count, ..RealmParams::default() }, 3);
        c.world.provinces = provinces;
        let sim = Sim::new(&c).unwrap();
        check_realms(&sim.world);
        alive(&sim.world).len()
    };
    for provinces in [120, 400] {
        let (one, six, twenty) = (count(1, provinces), count(6, provinces), count(20, provinces));
        assert!(six >= 5 && six <= one + 6, "{provinces} provinces: {one} realms for 1 asked, {six} for 6");
        assert!(twenty > six + 8, "{provinces} provinces: {six} realms for 6 asked, {twenty} for 20");
    }
}
