//! Disease's checks.
//!
//! cargo run --release -p sim --example disease_check -- probe [food|markets] [seed] [small|medium|large] [KEY=VALUE ...]
//! cargo run --release -p sim --example disease_check -- check [years] [small|medium] [KEY=VALUE ...]
//! cargo run --release -p sim --example disease_check -- story1 [KEY=VALUE ...]
//! cargo run --release -p sim --example disease_check -- speed [large|medium|small] [KEY=VALUE ...]
//!
//! `probe`: one plague poked at the start of year 3 into the most populous province with a sea way,
//! natural starts off, five years watched. For each arrival, the distance between the provinces'
//! centres over the weeks since the plague reached the province it came from, by sea and over land;
//! the plague's dead against the people of the provinces it struck and of the world.
//!
//! `check`: the 150-year check on seeds 21, 7 and 1 (or `SEEDS=...`) under both economies (or `ECON=...`), with natural starts on,
//! against the same worlds without Disease: people at the end (bar 0.7–1.1 times), outbreaks (2–15),
//! the most of the world one outbreak killed (at most 40%), the most of a province's people one
//! outbreak killed (at most 80%), plague dead a year per 1,000 people (0.2–1.5 at the 5% yearly chance; the plan guessed 1–5, and
//! the user accepted fewer, Oct 18), and any broken
//! totals.
//!
//! `story1`: story 1 in full (the test's three seeds, both economies) with Disease on, its natural
//! starts too.
//!
//! `speed`: the time of a busy week (a plague going on) and of quiet weeks and month starts.

use sim::config::Config;
use sim::disease::DiseaseParams;
use sim::events::Event;
use sim::grid::{dot, NONE};
use sim::module::Slot;
use sim::requests::Cause;
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::collections::BTreeMap;
use std::time::Instant;

fn size(args: &[String], default: Size) -> Size {
    match args.iter().find(|a| ["small", "medium", "large"].contains(&a.as_str())).map(String::as_str) {
        Some("small") => Size::Small,
        Some("medium") => Size::Medium,
        Some("large") => Size::Large,
        _ => default,
    }
}

fn set(c: &mut Config, args: &[String]) {
    for kv in args.iter().filter(|a| a.contains('=')) {
        let (k, v) = kv.split_once('=').unwrap();
        sim::settings::set(c, k, v.parse().unwrap()).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() { f64::NAN } else { v[v.len() / 2] }
}

/// What a run of plague saw.
#[derive(Default)]
struct Watch {
    /// Days each province was reached, per outbreak (it can be reached again once free of it).
    reached: BTreeMap<(u32, u32), Vec<u64>>,
    /// (outbreak, province, from, by sea, day)
    arrived: Vec<(u32, u32, u32, bool, u64)>,
    begun: Vec<(u32, u32, u64)>,
    over: Vec<(u32, u64, u32)>,
    /// Dead per (outbreak, province), and people there when it was reached.
    dead: BTreeMap<(u32, u32), u64>,
    people_then: BTreeMap<(u32, u32), u64>,
    /// The world's people when each outbreak began.
    world_then: BTreeMap<u32, u64>,
}

impl Watch {
    fn day(&mut self, s: &Sim) {
        let mut people: Option<Vec<u64>> = None;
        for p in s.delivered_today() {
            match p.event {
                Event::Outbreak { outbreak, province } => {
                    let pp = people.get_or_insert_with(|| s.world.province_population());
                    self.begun.push((outbreak, province, p.day));
                    self.reached.entry((outbreak, province)).or_default().push(p.day);
                    self.people_then.entry((outbreak, province)).or_insert(pp[province as usize]);
                    self.world_then.insert(outbreak, pp.iter().sum());
                }
                Event::PlagueArrived { outbreak, province, from, by_sea } => {
                    let pp = people.get_or_insert_with(|| s.world.province_population());
                    self.arrived.push((outbreak, province, from, by_sea, p.day));
                    self.reached.entry((outbreak, province)).or_default().push(p.day);
                    self.people_then.entry((outbreak, province)).or_insert(pp[province as usize]);
                }
                Event::OutbreakOver { outbreak, dead, provinces } => self.over.push((outbreak, dead, provinces)),
                _ => {}
            }
        }
        for (r, g) in &s.applied_today {
            if r.cause == Cause::Plague {
                *self.dead.entry((r.tag as u32, r.entity)).or_default() += g;
            }
        }
    }

    /// The last day before `day` that outbreak `o` reached province `p`.
    fn since(&self, o: u32, p: u32, day: u64) -> Option<u64> {
        self.reached.get(&(o, p))?.iter().copied().filter(|&d| d < day).max()
    }

    fn km(s: &Sim, a: u32, b: u32) -> f64 {
        let c = &s.world.provinces.center;
        dot(c[a as usize], c[b as usize]).clamp(-1.0, 1.0).acos() * s.world.params.radius_km
    }

    /// km a week for each arrival from `from` reached on day d0 to `to` on day d: (by sea, km a week).
    fn speeds(&self, s: &Sim, before: u64) -> Vec<(bool, f64)> {
        self.arrived
            .iter()
            .filter(|a| a.4 < before)
            .filter_map(|&(o, q, from, sea, day)| {
                let d0 = self.since(o, from, day)?;
                let weeks = ((day - d0) as f64 / 6.0).max(1.0);
                Some((sea, Self::km(s, from, q) / weeks))
            })
            .collect()
    }
}

fn biggest_port(s: &Sim) -> u32 {
    let people = s.world.province_population();
    (0..people.len() as u32).filter(|&p| s.world.routes.of(p).any(|l| l.sea_days.is_some())).max_by_key(|&p| (people[p as usize], std::cmp::Reverse(p))).unwrap()
}

fn probe(args: &[String]) {
    let economics = args.iter().find(|a| *a == "food" || *a == "markets").cloned().unwrap_or("food".into());
    let seed = args.iter().filter_map(|a| a.parse::<u64>().ok()).next().unwrap_or(21);
    let mut c = Config::new(WorldParams::preset(size(args, Size::Small), seed));
    c.modules.set_choice(Slot::Economics, &economics).unwrap();
    c.modules.disease = Some(DiseaseParams { starts: None, ..DiseaseParams::default() });
    set(&mut c, args);
    let mut s = Sim::new(&c).unwrap();
    let start = s.calendar.date(0).year;
    s.run_to_year(start + 3);
    let port = biggest_port(&s);
    let at_poke = s.world.province_population();
    s.poke(Event::Plague { province: port });
    let mut w = Watch::default();
    let end = s.day + 5 * 364;
    let three = s.day + 3 * 364;
    let t = Instant::now();
    while s.day < end {
        s.step_day();
        w.day(&s);
    }
    let sp = w.speeds(&s, three);
    let sea: Vec<f64> = sp.iter().filter(|x| x.0).map(|x| x.1).collect();
    let land: Vec<f64> = sp.iter().filter(|x| !x.0).map(|x| x.1).collect();
    let struck: Vec<u32> = w.reached.keys().map(|k| k.1).collect::<std::collections::BTreeSet<u32>>().into_iter().collect();
    let struck_people: u64 = struck.iter().map(|&p| at_poke[p as usize]).sum();
    let dead: u64 = w.dead.values().sum();
    let world: u64 = at_poke.iter().sum();
    let worst = w.dead.iter().map(|(k, &d)| d as f64 / w.people_then.get(k).copied().unwrap_or(1).max(1) as f64).fold(0.0, f64::max);
    println!(
        "{economics} seed {seed}: plague poked into province {port} ({} people, {} provinces in all); in 5 years {} outbreaks over, {} provinces struck ({} arrivals by sea, {} over land in the first 3 years); km a week: sea median {:.0} ({} arrivals), land median {:.0} ({}), sea/land {:.2}; km a month over land {:.0}",
        at_poke[port as usize],
        at_poke.len(),
        w.over.len(),
        struck.len(),
        sea.len(),
        land.len(),
        median(sea.clone()),
        sea.len(),
        median(land.clone()),
        land.len(),
        median(sea.clone()) / median(land.clone()),
        median(land.clone()) * 30.0 / 6.0
    );
    println!(
        "  dead {dead}: {:.1}% of the struck provinces' people at the poke, {:.1}% of the world's; the worst province lost {:.1}% of its people; ran in {:.1} s; problems {:?}",
        100.0 * dead as f64 / struck_people.max(1) as f64,
        100.0 * dead as f64 / world as f64,
        100.0 * worst,
        t.elapsed().as_secs_f64(),
        s.problems
    );
    // What makes the speeds: each arrival's distance, the days of the way it came by, and the
    // weeks it took.
    let parts = |sea: bool| {
        let v: Vec<(f64, f64, f64)> = w
            .arrived
            .iter()
            .filter(|a| a.4 < three && a.3 == sea)
            .filter_map(|&(o, q, from, _, day)| {
                let d0 = w.since(o, from, day)?;
                let l = s.world.routes.between(from, q)?;
                let days = if sea { l.sea_days? } else { l.land_days? } as f64;
                Some((Watch::km(&s, from, q), days, (day - d0) as f64 / 6.0))
            })
            .collect();
        format!("km {:.0}, way days {:.1}, weeks {:.1}", median(v.iter().map(|x| x.0).collect()), median(v.iter().map(|x| x.1).collect()), median(v.iter().map(|x| x.2).collect()))
    };
    println!("  by sea: {}; over land: {}", parts(true), parts(false));
    let mut first: Vec<(u64, u32, u32, bool)> = w.arrived.iter().map(|a| (a.4, a.1, a.2, a.3)).collect();
    first.sort();
    for (day, q, from, sea) in first.iter().take(if std::env::var("LIST").is_ok() { 12 } else { 0 }) {
        let d0 = w.since(0, *from, *day).unwrap_or(*day);
        println!("  day {day}: {q} from {from} {} {:.0} km in {:.1} weeks", if *sea { "by sea" } else { "over land" }, Watch::km(&s, *from, *q), (day - d0) as f64 / 6.0);
    }
}

/// One 150-year world, with or without Disease: people at the end, and what the plague did.
fn century(economics: &str, seed: u64, years: i64, sz: Size, disease: bool, args: &[String]) -> String {
    let mut c = Config::new(WorldParams::preset(sz, seed));
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.disease = disease.then(DiseaseParams::default);
    // The world without Disease takes every setting but Disease's.
    let keep: Vec<String> = args.iter().filter(|a| disease || !a.starts_with("modules.disease")).cloned().collect();
    set(&mut c, &keep);
    let mut s = Sim::new(&c).unwrap();
    let start = s.calendar.date(0).year;
    let mut w = Watch::default();
    let end = s.calendar.year_start(start + years);
    while s.day < end {
        s.step_day();
        w.day(&s);
    }
    let people: u64 = s.world.province_population().iter().sum();
    if !disease {
        return format!("{people}");
    }
    let mut by_outbreak: BTreeMap<u32, u64> = BTreeMap::new();
    for (&(o, _), &d) in &w.dead {
        *by_outbreak.entry(o).or_default() += d;
    }
    let world_worst = by_outbreak.iter().map(|(o, &d)| d as f64 / w.world_then.get(o).copied().unwrap_or(1).max(1) as f64).fold(0.0, f64::max);
    let prov_worst = w.dead.iter().map(|(k, &d)| d as f64 / w.people_then.get(k).copied().unwrap_or(1).max(1) as f64).fold(0.0, f64::max);
    let plague: u64 = s.history.iter().map(|y| y.ledger.plague).sum::<u64>() + s.year_so_far().ledger.plague;
    let person_years: f64 = s.history.iter().map(|y| y.population as f64).sum();
    let ports = w.begun.iter().filter(|b| s.world.routes.of(b.1).any(|l| l.sea_days.is_some())).count();
    let starts: Vec<String> = w.begun.iter().map(|b| format!("y{}", s.calendar.date(b.2).year)).collect();
    format!(
        "{people} {} outbreaks ({} in ports) [{}], worst outbreak {:.1}% of the world, worst province {:.1}% in one outbreak, plague dead {:.2} a year per 1,000, problems {:?}",
        w.begun.len(),
        ports,
        starts.join(" "),
        100.0 * world_worst,
        100.0 * prov_worst,
        1000.0 * plague as f64 / person_years.max(1.0),
        s.problems
    )
}

fn check(args: &[String]) {
    use rayon::prelude::*;
    let years = args.iter().filter_map(|a| a.parse::<i64>().ok()).next().unwrap_or(150);
    let sz = size(args, Size::Small);
    // SEEDS=21,7,1 (the default) and ECON=food or markets (both by default) to choose.
    let seeds: Vec<u64> = std::env::var("SEEDS").map(|v| v.split(',').map(|x| x.parse().unwrap()).collect()).unwrap_or(vec![21, 7, 1]);
    let economies: Vec<&str> = match std::env::var("ECON").as_deref() {
        Ok("food") => vec!["food"],
        Ok("markets") => vec!["markets"],
        _ => vec!["food", "markets"],
    };
    let runs: Vec<(&str, u64, bool)> = economies.iter().flat_map(|&e| seeds.iter().flat_map(move |&s| [(e, s, true), (e, s, false)])).collect();
    let out: Vec<String> = runs.par_iter().map(|&(e, seed, d)| century(e, seed, years, sz, d, args)).collect();
    for k in (0..runs.len()).step_by(2) {
        let (e, seed, _) = runs[k];
        let with = &out[k];
        let without: f64 = out[k + 1].parse().unwrap();
        let people: f64 = with.split(' ').next().unwrap().parse().unwrap();
        println!("{e} seed {seed}, {years} years: people {:.3} times without Disease; {}", people / without, with.split_once(' ').unwrap().1);
    }
}

fn story1(args: &[String]) {
    use rayon::prelude::*;
    let runs: Vec<(&str, u64)> = ["food", "markets"].iter().flat_map(|&e| [21u64, 3, 7].into_iter().map(move |s| (e, s))).collect();
    let out: Vec<(bool, bool, u32)> = runs
        .par_iter()
        .map(|&(economics, seed)| {
            let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
            c.modules.set_choice(Slot::Economics, economics).unwrap();
            c.modules.disease = Some(DiseaseParams::default());
            set(&mut c, args);
            let mut sim = Sim::new(&c).unwrap();
            let start = sim.calendar.date(0).year;
            sim.run_to_year(start + 3);
            let mut twin = sim.fork();
            let (count, heart) = {
                let p = &sim.world.provinces;
                (p.count, (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap())
            };
            let (lon, lat) = sim::grid::dir_to_lonlat(sim.world.provinces.center[heart]);
            sim.poke(Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days: 3 * 364 });
            let mut probe = sim.fork();
            probe.run_days(2);
            let region: Vec<u32> = (0..count as u32).filter(|&i| probe.world.rain_shortfall[i as usize] > 0).collect();
            let mut realms: Vec<u32> = region.iter().map(|&q| sim.world.owner[q as usize]).filter(|&r| r != NONE).collect();
            realms.sort_unstable();
            realms.dedup();
            let end = sim.day + 5 * sim.calendar.days_per_year() as u64;
            let mut plagues = 0;
            let mut watch = |s: &mut Sim| {
                let mut happened = false;
                while s.day < end {
                    s.step_day();
                    for e in s.delivered_today() {
                        happened |= match e.event {
                            Event::Uprising { province, seat, .. } => province == seat && region.contains(&province),
                            Event::WarDeclared { attacker, cause, .. } => cause == sim::diplomacy::WarCause::Food && realms.contains(&attacker),
                            _ => false,
                        };
                        plagues += matches!(e.event, Event::Outbreak { .. }) as u32;
                    }
                }
                assert_eq!(s.problems, Default::default(), "{economics}, seed {seed}");
                happened
            };
            (watch(&mut sim), watch(&mut twin), plagues)
        })
        .collect();
    for economics in ["food", "markets"] {
        let r: Vec<_> = runs.iter().zip(&out).filter(|x| x.0.0 == economics).map(|x| *x.1).collect();
        let with = r.iter().filter(|x| x.0).count();
        let quiet = r.iter().filter(|x| !x.1).count();
        println!("story 1 in full with Disease on, {economics}: the drought brought an uprising or a war for food on {with} of 3; quiet without it on {quiet} of 3 (bar 2 and 2: {}); outbreaks seen {:?}", if with >= 2 && quiet >= 2 { "pass" } else { "FAIL" }, r.iter().map(|x| x.2).collect::<Vec<_>>());
    }
}

/// Disease's own work and the engine's checks around it, ms a week of the days timed.
fn report(t: &sim::timing::Timing) -> String {
    let weeks = t.days as f64 / 6.0;
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0 / weeks;
    let (work, checks) = t.by_module().into_iter().filter(|m| m.0 == Slot::Disease).map(|m| (m.1, m.2)).next().unwrap_or_default();
    let all = ms(t.total) * weeks / t.days as f64;
    format!("Disease {:.3} ms of work and {:.3} ms of checks a week; the whole day {:.2} ms", ms(work), ms(checks), all)
}

fn speed(args: &[String]) {
    let sz = size(args, Size::Large);
    let mut c = Config::new(WorldParams::preset(sz, 7));
    c.modules.disease = Some(DiseaseParams { starts: None, ..DiseaseParams::default() });
    set(&mut c, args);
    let mut s = Sim::new(&c).unwrap();
    s.run_days(2 * 364);
    // Quiet: nobody ill.
    s.timing = Some(Box::default());
    s.run_days(364);
    let quiet = s.timing.take().unwrap();
    let port = biggest_port(&s);
    s.poke(Event::Plague { province: port });
    s.run_days(120);
    s.timing = Some(Box::default());
    s.run_days(364);
    let busy = s.timing.take().unwrap();
    let sick = s.world.sick.iter().filter(|&&x| x > 0).count();
    println!("{sz:?}: a quiet year: {}", report(&quiet));
    println!("{sz:?}: a year with plague ({sick} provinces ill at its end): {}", report(&busy));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => check(&args[1..]),
        Some("story1") => story1(&args[1..]),
        Some("speed") => speed(&args[1..]),
        _ => probe(&args[1.min(args.len())..]),
    }
}
