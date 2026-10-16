//! Rulers (step 7, Characters "rulers"): founding rulers and heirs, deaths and successions over a
//! century, and rulers kept through saves, going back, scenario files and edits.

use crate::branch::Timelines;
use crate::characters::{death_per_year, name, title, RulerParams, Succession, ADULT, NEVER};
use crate::config::Config;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::grid::NONE;
use crate::module::Slot;
use crate::realms::RealmParams;
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};
use std::collections::BTreeSet;

fn config(realms: RealmParams, seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..realms });
    c.modules.characters = Some(RulerParams::default());
    c
}

fn world(seed: u64) -> Sim {
    let mut sim = Sim::new(&config(RealmParams::default(), seed)).unwrap();
    sim.strict = true;
    sim
}

fn alive_realms(w: &World) -> Vec<u32> {
    (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect()
}

fn age(sim: &Sim, c: u32) -> f64 {
    sim.world.characters.age(c, sim.day, sim.calendar.days_per_year())
}

/// Every realm alive has a living ruler of its own, crowned; its heir, if any, lives, belongs to it
/// and isn't the ruler; nobody rules or is heir to two realms; dissolved realms have neither.
fn check_rulers(sim: &Sim) {
    let w = &sim.world;
    let ch = &w.characters;
    assert_eq!((w.realms.ruler.len(), w.realms.heir.len()), (w.realms.count(), w.realms.count()));
    let mut seen = BTreeSet::new();
    for r in 0..w.realms.count() as u32 {
        let (ruler, heir) = (w.realms.ruler[r as usize], w.realms.heir[r as usize]);
        if !w.realms.alive(r) {
            assert_eq!((ruler, heir), (NONE, NONE), "dissolved realm {r}");
            continue;
        }
        assert!(ch.alive(ruler), "realm {r}'s ruler {ruler} is dead");
        assert_eq!(ch.realm[ruler as usize], r);
        assert_ne!(ch.crowned[ruler as usize], NEVER);
        assert!(seen.insert(ruler), "character {ruler} holds two places");
        if heir != NONE {
            assert!(ch.alive(heir) && ch.realm[heir as usize] == r && ch.crowned[heir as usize] == NEVER, "realm {r}'s heir {heir}");
            assert!(seen.insert(heir), "character {heir} holds two places");
        }
    }
    for c in 0..ch.count() as u32 {
        assert!(age(sim, c) >= 0.0 && age(sim, c) < 115.0, "character {c} is {} years old", age(sim, c));
        for t in [ch.martial[c as usize], ch.greed[c as usize], ch.ability[c as usize]] {
            assert!(t <= 100);
        }
    }
}

#[test]
fn every_realm_starts_with_a_grown_ruler_and_older_rulers_with_heirs() {
    for seed in [1, 2, 3] {
        let sim = world(seed);
        let w = &sim.world;
        check_rulers(&sim);
        let realms = alive_realms(w);
        assert!(realms.len() > 5);
        let ch = &w.characters;
        let mut with_heir = 0;
        for &r in &realms {
            let ruler = w.realms.ruler[r as usize];
            assert!((18.0..=65.0).contains(&age(&sim, ruler)), "{}", age(&sim, ruler));
            assert_eq!((ch.crowned[ruler as usize], Succession::from_u8(ch.succession[ruler as usize])), (0, Succession::Founded));
            let heir = w.realms.heir[r as usize];
            if heir != NONE {
                with_heir += 1;
                assert_eq!(ch.parent[heir as usize], ruler);
                let gap = age(&sim, ruler) - age(&sim, heir);
                assert!((17.9..=35.1).contains(&gap), "a parent {gap} years older");
            }
        }
        assert!(with_heir > 0 && with_heir < realms.len(), "seed {seed}: {with_heir} of {} rulers have heirs", realms.len());
        assert_eq!(ch.count(), realms.len() + with_heir);
        // The rulers a world starts with are not news.
        assert_eq!(sim.pending.iter().filter(|p| p.event.kind() == EventKind::Succession).count(), 0);
    }
}

#[test]
fn the_same_seed_makes_the_same_rulers() {
    let a = world(4);
    let b = world(4);
    assert_eq!((&a.world.characters, &a.world.realms), (&b.world.characters, &b.world.realms));
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert_ne!(a.world.characters, world(5).world.characters);
    assert_eq!(name(4, 17), name(4, 17));
    let names: BTreeSet<String> = (0..200).map(|k| name(4, k)).collect();
    assert!(names.len() > 190, "names mostly differ: {}", names.len());
    assert!(names.iter().all(|n| n.len() >= 4 && n.chars().next().unwrap().is_uppercase()), "{names:?}");
}

#[test]
fn over_a_century_rulers_die_and_heirs_take_over() {
    // Without unrest: realms that break away would bring founding rulers of their own.
    let mut sim = Sim::new(&config(RealmParams { unrest: None, ..RealmParams::default() }, 2)).unwrap();
    sim.strict = true;
    let start = sim.calendar.date(0).year;
    let mut announced = 0;
    for year in 1..=100 {
        sim.run_to_year(start + year);
        check_rulers(&sim);
        announced += sim.event_log.entries.iter().filter(|p| matches!(p.event, Event::Succession { .. }) && p.day + 1 >= sim.calendar.year_start(start + year - 1)).count();
    }
    assert_eq!(sim.problems, Problems::default());
    let w = &sim.world;
    let ch = &w.characters;
    let year = sim.calendar.days_per_year() as f64;

    // Each realm's rulers follow one another: the next is crowned the day the last died, and an
    // heir taking over was the dead ruler's heir (their child, or a kinsman).
    let mut kinds = [0usize; 5];
    let (mut reigns, mut deaths) = (Vec::new(), Vec::new());
    for r in alive_realms(w) {
        let mut line: Vec<usize> = (0..ch.count()).filter(|&c| ch.realm[c] == r && ch.crowned[c] != NEVER).collect();
        line.sort_by_key(|&c| ch.crowned[c]);
        for pair in line.windows(2) {
            let (old, new) = (pair[0], pair[1]);
            assert_eq!(ch.crowned[new], ch.died[old], "realm {r}: crowned the day the last ruler died");
            let how = Succession::from_u8(ch.succession[new]);
            kinds[how as usize] += 1;
            match how {
                Succession::Heir => assert!((ch.crowned[new] as f64 - ch.born[new]) / year >= ADULT),
                Succession::Child => assert!((ch.crowned[new] as f64 - ch.born[new]) / year < ADULT),
                Succession::Claimant => assert_eq!(ch.parent[new], NONE),
                other => panic!("realm {r}: a ruler after the first {other:?}"),
            }
            reigns.push((ch.died[old] - ch.crowned[old]) as f64 / year);
            deaths.push((ch.died[old] as f64 - ch.born[old]) / year);
        }
    }
    let successions = kinds.iter().sum::<usize>();
    assert_eq!(successions, announced, "every succession is announced");
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (reign, death) = (mean(&reigns), mean(&deaths));
    eprintln!("{successions} successions {kinds:?}; reigns {reign:.1} years on average, rulers die at {death:.1}");
    assert!(successions > 2 * alive_realms(w).len(), "{successions} successions in a century");
    for how in [Succession::Heir, Succession::Child, Succession::Claimant] {
        assert!(kinds[how as usize] > 0, "no {how:?} in a century: {kinds:?}");
    }
    assert!(kinds[Succession::Heir as usize] > kinds[Succession::Claimant as usize], "{kinds:?}");
    // Reigns that ended: shorter than a lifetime; rulers mostly die past middle age.
    assert!((10.0..35.0).contains(&reign), "{reign}");
    assert!((45.0..75.0).contains(&death), "{death}");
}

#[test]
fn rulers_survive_saving_loading_and_going_back() {
    let mut sim = world(3);
    let year = sim.calendar.days_per_year() as u64;
    // Run until a ruler has died, then a little more.
    while !(0..sim.world.characters.count()).any(|c| sim.world.characters.crowned[c] > 0 && sim.world.characters.crowned[c] != NEVER) {
        sim.run_days(year);
    }
    sim.run_days(45);
    let (print, today, rulers) = (sim.fingerprint(), sim.day, sim.world.characters.clone());

    let mut loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), print);
    sim.run_days(10 * year);
    loaded.run_days(10 * year);
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    assert_ne!(sim.world.characters, rulers, "births and deaths in those ten years");

    let end = sim.fingerprint();
    sim.goto(today).unwrap();
    assert_eq!((sim.fingerprint(), &sim.world.characters), (print, &rulers));
    sim.goto(today + 10 * year).unwrap();
    assert_eq!(sim.fingerprint(), end);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn realms_without_rulers_save_and_load() {
    let mut c = config(RealmParams::default(), 1);
    c.modules.characters = None;
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    sim.run_days(400);
    let w = &sim.world;
    assert!(w.realms.count() > 0 && w.characters.count() == 0);
    assert!(w.realms.ruler.iter().chain(&w.realms.heir).all(|&x| x == NONE) && w.realms.ruler.len() == w.realms.count());
    let loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn rulers_need_realms_and_sensible_settings() {
    let mut c = config(RealmParams::default(), 1);
    c.modules.realms = None;
    let e = Sim::new(&c).err().expect("refused without realms").0;
    assert!(e.contains("realm.capital"), "{e}");
    for bad in [RulerParams { births_per_year: 1.5, ..RulerParams::default() }, RulerParams { kin_per_year: -0.1, ..RulerParams::default() }, RulerParams { mortality: f64::NAN, ..RulerParams::default() }] {
        let mut c = config(RealmParams::default(), 1);
        c.modules.characters = Some(bad.clone());
        assert!(Sim::new(&c).is_err(), "{bad:?}");
    }
}

#[test]
fn a_dissolved_realm_loses_its_ruler_at_the_month_start() {
    let mut sim = Sim::new(&config(RealmParams { settled: 0.0, city_states: 1.0, ..RealmParams::default() }, 2)).unwrap();
    sim.strict = true;
    let w = &sim.world;
    let (r, p, q) = alive_realms(w)
        .into_iter()
        .filter_map(|r| {
            let mine: Vec<u32> = (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r).collect();
            match mine[..] {
                [p] => w.routes.of(p).map(|l| l.other(p)).find(|&q| w.owner[q as usize] != NONE).map(|q| (r, p, q)),
                _ => None,
            }
        })
        .next()
        .expect("a one-province city-state with a neighbour");
    let ruler = w.realms.ruler[r as usize];
    sim.edit(Edit::Merge { from: p, into: q }).unwrap();
    assert!(!sim.world.realms.alive(r));
    sim.run_days(40);
    check_rulers(&sim);
    // The ruler lives on, remembered as of that realm.
    assert_eq!(sim.world.characters.realm[ruler as usize], r);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_realm_made_later_gets_a_founding_ruler_at_the_month_start() {
    let mut sim = Sim::new(&config(RealmParams { settled: 0.0, frontier: 1.0, ..RealmParams::default() }, 3)).unwrap();
    sim.strict = true;
    sim.run_days(3);
    // A realm made from outside the modules, as a rebellion will be: a settled province nobody owns.
    let people = sim.world.province_population();
    let p = (0..sim.world.provinces.count).find(|&p| sim.world.owner[p] == NONE && people[p] > 0).expect("unclaimed settled land") as u32;
    let r = sim.world.realms.count() as u32;
    sim.world.realms.capital.push(p);
    sim.world.owner[p as usize] = r;
    sim.world.fit_realm_rows();
    let month = (sim.day..).find(|&d| sim.calendar.is_month_start(d)).unwrap();
    sim.run_days(month + 2 - sim.day);
    check_rulers(&sim);
    let ruler = sim.world.realms.ruler[r as usize];
    let ch = &sim.world.characters;
    assert_eq!((ch.crowned[ruler as usize], Succession::from_u8(ch.succession[ruler as usize])), (month, Succession::Founded));
    let told = sim.event_log.entries.iter().any(|e| e.event == Event::Succession { realm: r, capital: p, ruler, how: Succession::Founded });
    assert!(told, "a new realm's first ruler is announced");
}

#[test]
fn a_scenario_file_keeps_its_rulers() {
    let mut sim = world(3);
    sim.run_days(3 * 364 + 20);
    let text = Timelines::new(sim.fork()).scenario_text();
    assert!(text.contains("births_per_year"), "the rulers' settings are written");
    let mut read = Timelines::from_scenario_text(&text).unwrap();
    let back = read.list.swap_remove(read.active).sim;
    assert_eq!((back.day, back.fingerprint()), (sim.day, sim.fingerprint()));
    assert_eq!(back.world.characters, sim.world.characters);
}

#[test]
fn rulers_run_under_every_economy() {
    for economics in ["markets", "food"] {
        let mut c = config(RealmParams::default(), 2);
        c.modules.set_choice(Slot::Economics, economics).unwrap();
        let mut sim = Sim::new(&c).unwrap();
        sim.strict = true;
        sim.run_to_year(sim.calendar.date(0).year + 5);
        check_rulers(&sim);
        assert_eq!(sim.problems, Problems::default(), "{economics}");
    }
}

#[test]
fn death_comes_more_often_with_age() {
    let ages = [20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 120.0];
    let q: Vec<f64> = ages.iter().map(|&a| death_per_year(a)).collect();
    assert!(q.windows(2).all(|w| w[0] < w[1] || w[1] == 1.0), "{q:?}");
    assert!(death_per_year(2.0) > death_per_year(20.0));
    assert!((0.01..0.03).contains(&death_per_year(40.0)) && (0.1..0.2).contains(&death_per_year(70.0)), "{q:?}");
    assert_eq!(death_per_year(120.0), 1.0);
}

#[test]
fn a_ruler_named_after_an_earlier_one_of_the_realm_gets_a_number() {
    let mut sim = world(1);
    let w = &mut sim.world;
    let r = 0;
    let first = w.realms.ruler[r as usize];
    // Two more rulers of realm 0 with the first one's name, and one of another realm.
    let mut add = |crowned: u64, realm: u32| {
        let c = w.characters.count() as u32;
        let n = w.characters.name[first as usize];
        w.characters.push(realm, NONE, n, -9000.0, [50, 50, 50]);
        w.characters.crowned[c as usize] = crowned;
        c
    };
    let (second, other, third) = (add(500, r), add(600, 1), add(900, r));
    let base = name(w.params.seed, w.characters.name[first as usize]);
    assert_eq!(title(w, first), base);
    assert_eq!(title(w, second), format!("{base} II"));
    assert_eq!(title(w, third), format!("{base} III"));
    assert_eq!(title(w, other), base);
}
