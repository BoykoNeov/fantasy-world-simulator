//! Tests of Lives (followed people, step 9).

use crate::config::Config;
use crate::edit::Edit;
use crate::lives::{LifeParams, People, What, LIVES_READS};
use crate::module::Slot;
use crate::sim::{to_bytes, Sim};
use crate::world::{Fingerprint, WorldParams};

/// A small world with today's defaults (realms, wars, research, faiths, plagues, the wild), under
/// markets if `markets`, with Lives "lives" following `n` people from the start (or off).
fn config(seed: u64, markets: bool, lives: Option<LifeParams>) -> Config {
    let mut c = Config::new(WorldParams { n: 32, provinces: 120, seed, land_fraction: 0.4, radius_km: 6371.0 });
    if markets {
        c.modules.set_choice(Slot::Economics, "markets").unwrap();
    }
    c.modules.lives = lives;
    c
}

fn following(n: u32) -> Option<LifeParams> {
    Some(LifeParams { at_start: n, ..LifeParams::default() })
}

/// Everything about the world but Lives' own state: every field, the day, what is in flight, the
/// year's totals and every other module's state.
fn world_print(s: &Sim) -> u64 {
    let mut h = Fingerprint::default();
    s.world.fingerprint(&mut h);
    h.u64s(&[s.day]);
    h.bytes(&to_bytes(&s.pending));
    h.bytes(&to_bytes(&s.year_flows));
    h.bytes(&to_bytes(&s.history));
    for m in s.modules.iter().filter(|m| m.manifest().slot != Slot::Lives) {
        h.bytes(&m.save_state());
    }
    h.0
}

fn people(s: &Sim) -> &People {
    &s.world.lives
}

/// Every line of every story, in words (so none fails to tell).
fn told(s: &Sim) -> Vec<String> {
    let st = people(s);
    let names = crate::lives::Names::new(&s.world);
    let mut out = Vec::new();
    for k in st.followed() {
        for l in &st.people[k].lines {
            out.push(st.tell(&s.world, &names, k, l, &s.calendar));
        }
        out.push(st.summary(&s.world, &names, k, s.day, s.calendar.days_per_year()));
    }
    out
}

#[test]
fn the_world_runs_the_same_with_lives_on_and_off() {
    for markets in [false, true] {
        for seed in [3, 7] {
            let mut off = Sim::new(&config(seed, markets, None)).unwrap();
            let mut on = Sim::new(&config(seed, markets, following(150))).unwrap();
            assert_eq!(world_print(&off), world_print(&on), "made alike (markets {markets}, seed {seed})");
            for year in 0..30 {
                off.run_days(off.calendar.days_per_year() as u64);
                on.run_days(on.calendar.days_per_year() as u64);
                assert_eq!(world_print(&off), world_print(&on), "year {year} (markets {markets}, seed {seed})");
            }
            assert!(on.world.lives.people.len() >= 150, "people followed");
            assert!(off.world.lives.people.is_empty());
            told(&on);
        }
    }
}

#[test]
fn lives_have_jobs_families_soldiering_and_deaths() {
    let mut s = Sim::new(&config(3, true, following(150))).unwrap();
    s.strict = true;
    s.run_days(40 * s.calendar.days_per_year() as u64);
    let st = people(&s);
    let lines: Vec<&What> = st.people.iter().flat_map(|x| x.lines.iter().map(|l| &l.what)).collect();
    let has = |f: &dyn Fn(&What) -> bool| lines.iter().filter(|w| f(w)).count();
    let jobs = has(&|w| matches!(w, What::Job { .. }));
    let married = has(&|w| matches!(w, What::Married { .. }));
    let children = has(&|w| matches!(w, What::Child { .. }));
    let died = has(&|w| matches!(w, What::Died { .. }));
    let carried = has(&|w| matches!(w, What::CarriedOn { .. }));
    let years = has(&|w| matches!(w, What::Year { .. }));
    let taken = has(&|w| matches!(w, What::Taken { .. }));
    eprintln!("jobs {jobs}, married {married}, children {children}, died {died}, carried on {carried}, years {years}, taken {taken}");
    assert!(jobs >= 150 && married >= 50 && children >= 50 && died >= 30 && carried >= 5 && years >= 100 && taken >= 1);
    let names: std::collections::BTreeSet<&str> = st.people.iter().filter(|x| !x.job.is_empty()).map(|x| x.job.as_str()).collect();
    eprintln!("jobs held: {names:?}");
    assert!(names.contains("farmer") && names.len() >= 4, "{names:?}");
    // Kin have no story; the followed always begin one.
    for x in &st.people {
        assert_eq!(x.followed, !x.lines.is_empty());
    }
    for l in told(&s).iter().take(40) {
        eprintln!("{l}");
    }
}

#[test]
fn deaths_of_age_give_lives_of_a_believable_length() {
    // Under the food economy, people followed for 60 years: about 2-4% of them die each year.
    let mut s = Sim::new(&config(5, false, following(300))).unwrap();
    s.run_days(60 * s.calendar.days_per_year() as u64);
    let st = people(&s);
    let year = s.calendar.days_per_year();
    let (mut years, mut deaths) = (0.0, 0u64);
    for x in st.people.iter().filter(|x| x.followed) {
        let from = x.lines[0].day;
        let to = x.died.min(s.day);
        years += (to - from) as f64 / year as f64;
        deaths += (!x.alive()) as u64;
    }
    let rate = deaths as f64 / years;
    eprintln!("{deaths} deaths over {years:.0} years followed: {:.1}% a year", rate * 100.0);
    assert!((0.01..0.06).contains(&rate), "{rate}");
}

#[test]
fn a_save_mid_life_carries_on_the_same() {
    let mut a = Sim::new(&config(7, true, following(40))).unwrap();
    a.run_days(5 * 360);
    let mut b = Sim::load(&a.save()).unwrap();
    assert_eq!(*a.world.lives, *b.world.lives, "the same people after loading");
    a.run_days(5 * 360);
    b.run_days(5 * 360);
    assert_eq!(*a.world.lives, *b.world.lives);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn going_back_shows_the_people_of_that_day() {
    let mut s = Sim::new(&config(7, false, following(40))).unwrap();
    s.run_days(3 * 360);
    let day = s.day;
    let then = (*s.world.lives).clone();
    s.run_days(4 * 360);
    assert_ne!(*s.world.lives, then);
    s.goto(day).unwrap();
    assert_eq!(*s.world.lives, then);
}

#[test]
fn following_someone_here() {
    let mut s = Sim::new(&config(7, false, following(0))).unwrap();
    let people = s.world.province_population();
    let p = (0..people.len()).max_by_key(|&p| people[p]).unwrap() as u32;
    s.edit(Edit::Follow { province: p }).unwrap();
    s.run_days(1);
    let st = people_of(&s);
    assert_eq!(st.followed().count(), 1);
    assert_eq!(st.people[0].province, p);
    let empty = (0..people.len()).find(|&q| people[q] == 0 && s.world.provinces.cells[q] > 0);
    if let Some(q) = empty {
        assert!(s.edit(Edit::Follow { province: q as u32 }).is_err(), "nobody lives there");
    }
    let mut off = Sim::new(&config(7, false, None)).unwrap();
    let err = off.edit(Edit::Follow { province: p }).unwrap_err();
    assert!(err.contains("Lives is off"), "{err}");
}

fn people_of(s: &Sim) -> &People {
    &s.world.lives
}

#[test]
fn the_diary_has_no_jobs_or_soldiers() {
    let mut s = Sim::new(&config(3, true, Some(LifeParams { at_start: 60, roles: None, ..LifeParams::default() }))).unwrap();
    assert_eq!(s.modules.last().unwrap().manifest().version, "diary");
    s.run_days(20 * 360);
    let st = people(&s);
    for x in &st.people {
        assert!(x.job.is_empty() && x.army == crate::grid::NONE);
        for l in &x.lines {
            assert!(!matches!(l.what, What::Job { .. } | What::Taken { .. } | What::Battle { .. }), "{:?}", l.what);
        }
    }
    assert!(st.people.iter().flat_map(|x| &x.lines).any(|l| matches!(l.what, What::Year { .. })));
    told(&s);
}

#[test]
fn lives_without_realms_farm() {
    let mut c = config(3, false, following(30));
    c.modules.without_realms();
    let mut s = Sim::new(&c).unwrap();
    s.run_days(10 * 360);
    let st = people(&s);
    assert!(st.people.iter().filter(|x| x.followed && !x.job.is_empty()).all(|x| x.job == "farmer"));
    assert!(st.people.iter().any(|x| x.job == "farmer"));
}

#[test]
fn every_modules_jobs_read_only_what_lives_reads() {
    for m in crate::settings::everything().build() {
        if let Some(j) = m.jobs() {
            for f in j.reads {
                assert!(LIVES_READS.contains(f), "{:?}'s jobs read {f}, which Lives' manifest doesn't list", m.manifest().slot);
            }
        }
    }
}
