//! Religion's 150-year check: how the faiths of a world with realms, rulers, wars, armies (and
//! culture, or not) change.
//!
//! cargo run --release -p sim --example religion_check -- [food|markets] [seed] [years] [small|medium|large] [noculture] [KEY=VALUE ...]
//! cargo run --release -p sim --example religion_check -- all [years] [threads=4] [small|medium|large] [KEY=VALUE ...]
//!
//! Each decade it prints: the faiths alive and those above 1% of the world's people (and how many
//! of the starting ones are), the largest faith's share, the share of people not of their
//! province's main faith, pop rows per settled province (all, and those with people), the share of
//! settled provinces whose main faith is not the one they started with, the people who took up
//! another faith in the decade (by conversion), schisms so far, and the time of a month-start day.
//!
//! `all` runs the plan's twelve worlds (seeds 21, 7 and 1; both economies; culture on and off),
//! `threads` at a time, and ends with each world's verdict against the pass bars:
//! - the largest faith levels off: its share rises by at most 2 points from year 100 to year 150
//!   (read here as "no steady climb over the last 50 years"), and is below 35%;
//! - at least 4 faiths above 1% of the people (all faiths; the starting ones shown beside);
//! - 10–40% of settled provinces changed main faith since the start;
//! - 0–6 schisms;
//! - no broken totals (no problems, nobody clamped in the yearly tally);
//! - pop rows: with people at most 1.5 a settled province, all rows at most 3.
//!
//! `noculture` switches culture off. `off` switches religion off, for the speed of a day without
//! it. Times are rough: worlds run side by side.

use sim::config::Config;
use sim::events::Event;
use sim::grid::NONE;
use sim::module::Slot;
use sim::religion::ReligionParams;
use sim::requests::Cause;
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::fmt::Write;
use std::time::Instant;

#[derive(Clone)]
struct Run {
    economics: String,
    seed: u64,
    years: i64,
    size: Size,
    culture: bool,
    religion: bool,
    sets: Vec<(String, f64)>,
}

/// What a decade's line says, for the verdict.
#[derive(Clone, Copy, Default)]
struct Decade {
    year: i64,
    largest: f64,
    above: usize,
    above_starting: usize,
    changed: f64,
    rows: f64,
    full: f64,
    schisms: usize,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let size = match args.iter().find(|a| ["small", "medium", "large"].contains(&a.as_str())).map(String::as_str) {
        Some("small") => Size::Small,
        Some("large") => Size::Large,
        _ => Size::Medium,
    };
    let sets: Vec<(String, f64)> = args
        .iter()
        .filter(|a| a.contains('=') && !a.starts_with("threads="))
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap();
            (k.to_string(), v.parse().unwrap())
        })
        .collect();
    let mut numbers = args.iter().filter_map(|a| a.parse::<u64>().ok());
    if args.iter().any(|a| a == "all") {
        let years = numbers.next().unwrap_or(150) as i64;
        let threads: usize = args.iter().find_map(|a| a.strip_prefix("threads=")).map_or(4, |t| t.parse().unwrap());
        let mut runs = Vec::new();
        for culture in [true, false] {
            for economics in ["food", "markets"] {
                for seed in [21, 7, 1] {
                    runs.push(Run { economics: economics.into(), seed, years, size, culture, religion: true, sets: sets.clone() });
                }
            }
        }
        let next = std::sync::atomic::AtomicUsize::new(0);
        let verdicts = std::sync::Mutex::new(vec![String::new(); runs.len()]);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let Some(run) = runs.get(i) else { break };
                        let (report, verdict) = check(run);
                        println!("{report}");
                        verdicts.lock().unwrap()[i] = verdict;
                    }
                });
            }
        });
        println!("verdicts:");
        for v in verdicts.into_inner().unwrap() {
            println!("{v}");
        }
    } else {
        let economics = args.iter().find(|a| *a == "food" || *a == "markets").cloned().unwrap_or("food".into());
        let seed = numbers.next().unwrap_or(21);
        let years = numbers.next().unwrap_or(150) as i64;
        let run = Run { economics, seed, years, size, culture: !args.iter().any(|a| a == "noculture"), religion: !args.iter().any(|a| a == "off"), sets };
        let (report, verdict) = check(&run);
        println!("{report}");
        println!("{verdict}");
    }
}

/// One world's run: its decade table, and its verdict against the bars.
fn check(run: &Run) -> (String, String) {
    let mut config = Config::new(WorldParams::preset(run.size, run.seed));
    config.modules.set_choice(Slot::Economics, &run.economics).unwrap();
    config.modules.religion = run.religion.then(ReligionParams::default);
    if !run.culture {
        config.modules.culture = None;
    }
    for (k, v) in &run.sets {
        sim::settings::set(&mut config, k, *v).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    let what = format!("{}, seed {}, culture {}", run.economics, run.seed, if run.culture { "on" } else { "off" });
    let mut out = String::new();
    let made = Instant::now();
    let mut s = Sim::new(&config).unwrap();
    writeln!(
        out,
        "== {what}: {} provinces, {} realms, {} cultures, {} faiths (made in {:.1} s)",
        s.world.provinces.count,
        s.world.realms.count(),
        s.world.cultures.count(),
        s.world.religions.count(),
        made.elapsed().as_secs_f64()
    )
    .unwrap();
    let start = sim::religion::main_faiths(&s.world);
    let starting = s.world.religions.count();
    let shares = |s: &Sim| {
        let mut by = vec![0u64; s.world.religions.count()];
        for (k, &f) in s.world.pops.religion.iter().enumerate() {
            if let Some(x) = by.get_mut(f as usize) {
                *x += s.world.pops.size[k];
            }
        }
        let all: u64 = by.iter().sum();
        by.into_iter().map(|x| x as f64 / all.max(1) as f64).collect::<Vec<f64>>()
    };
    let above = |sh: &[f64], upto: usize| sh.iter().take(upto).filter(|&&x| x > 0.01).count();
    let first = shares(&s);
    writeln!(out, "at the start: {} faiths above 1% of the people; largest {:.1}%", above(&first, starting), 100.0 * first.iter().cloned().fold(0.0, f64::max)).unwrap();
    writeln!(out, "year | alive | >1% (of starting) | largest | not main | rows/prov (with people) | main changed | ruled by another faith | converted | schisms | ms per month-start day").unwrap();

    let (mut converted, mut schisms, mut month_ms, mut month_days) = (0u64, 0usize, 0.0f64, 0usize);
    let mut decades: Vec<Decade> = Vec::new();
    let y0 = s.calendar.date(s.day).year;
    let end = s.calendar.year_start(y0 + run.years);
    while s.day < end {
        let month = s.calendar.is_month_start(s.day);
        let t = Instant::now();
        s.step_day();
        if month {
            month_ms += t.elapsed().as_secs_f64() * 1000.0;
            month_days += 1;
        }
        converted += s.applied_today.iter().filter(|(r, _)| r.cause == Cause::Converted).map(|(_, g)| g).sum::<u64>();
        schisms += s.delivered_today().iter().filter(|p| matches!(p.event, Event::ReligionBorn { .. })).count();
        let date = s.calendar.date(s.day);
        if s.calendar.is_year_start(s.day) && (date.year - y0) % 10 == 0 {
            let w = &s.world;
            let sh = shares(&s);
            let people = w.province_population();
            let main = sim::religion::main_faiths(w);
            let total: u64 = people.iter().sum();
            // People not of their province's main faith.
            let mut minority = 0u64;
            for (k, &p) in w.pops.province.iter().enumerate() {
                if main[p as usize] != NONE && w.pops.religion[k] as u32 != main[p as usize] {
                    minority += w.pops.size[k];
                }
            }
            let settled: Vec<usize> = (0..w.provinces.count).filter(|&p| people[p] > 0).collect();
            let rows = w.pops.size.len() as f64 / settled.len().max(1) as f64;
            let full = w.pops.size.iter().filter(|&&x| x > 0).count() as f64 / settled.len().max(1) as f64;
            let changed = settled.iter().filter(|&&p| start.get(p).is_some_and(|&f| f != NONE && f != main[p])).count() as f64 / settled.len().max(1) as f64;
            let alive = w.religions.died.iter().filter(|&&d| d == u64::MAX).count();
            // Settled provinces held by a living realm whose faith is not their main faith.
            let ruled = settled
                .iter()
                .filter(|&&p| {
                    let r = w.owner[p];
                    r != NONE && w.realms.alive(r) && w.realm_religion.get(r as usize).is_some_and(|&f| f != NONE && f != main[p])
                })
                .count() as f64
                / settled.len().max(1) as f64;
            let d = Decade {
                year: date.year,
                largest: sh.iter().cloned().fold(0.0, f64::max),
                above: above(&sh, sh.len()),
                above_starting: above(&sh, starting),
                changed,
                rows,
                full,
                schisms,
            };
            writeln!(
                out,
                "{:>4} | {:>5} | {:>3} ({:>3}) | {:>6.1}% | {:>7.2}% | {:>5.2} ({:>4.2}) | {:>11.1}% | {:>21.1}% | {:>8.2}% | {:>7} | {:.3}",
                d.year,
                alive,
                d.above,
                d.above_starting,
                100.0 * d.largest,
                100.0 * minority as f64 / total.max(1) as f64,
                rows,
                full,
                100.0 * changed,
                100.0 * ruled,
                100.0 * converted as f64 / total.max(1) as f64,
                schisms,
                month_ms / month_days.max(1) as f64,
            )
            .unwrap();
            decades.push(d);
            (converted, month_ms, month_days) = (0, 0.0, 0);
        }
    }
    let clamped: i64 = s.history.iter().map(|y| y.ledger.people_clamped.abs()).sum();
    writeln!(out, "problems: {:?}; people clamped in the yearly tally: {clamped}", s.problems).unwrap();

    // The verdict, on the last decade's line and the line 50 years before it.
    let last = decades.last().copied().unwrap_or_default();
    let before = decades.iter().rev().find(|d| d.year <= last.year - 50).copied().unwrap_or_default();
    let worst_full = decades.iter().map(|d| d.full).fold(0.0, f64::max);
    let worst_rows = decades.iter().map(|d| d.rows).fold(0.0, f64::max);
    let mut fails = Vec::new();
    if last.largest - before.largest > 0.02 {
        fails.push(format!("largest climbs {:.1} to {:.1}%", 100.0 * before.largest, 100.0 * last.largest));
    }
    if last.largest >= 0.35 {
        fails.push("largest 35% or more".into());
    }
    if last.above < 4 {
        fails.push("fewer than 4 above 1%".into());
    }
    if !(0.10..=0.40).contains(&last.changed) {
        fails.push("main faith changed outside 10-40%".into());
    }
    if last.schisms > 6 {
        fails.push("more than 6 schisms".into());
    }
    if s.problems != Default::default() || clamped > 0 {
        fails.push("broken totals".into());
    }
    if worst_full > 1.5 || worst_rows > 3.0 {
        fails.push(format!("pop rows (worst {worst_rows:.2}, {worst_full:.2} with people)"));
    }
    let verdict = format!(
        "{what}: year {} largest {:.1}% (year {} {:.1}%), {} above 1% ({} of {starting} starting), {:.1}% changed main faith, {} schisms, rows worst {worst_rows:.2} ({worst_full:.2} with people): {}",
        last.year,
        100.0 * last.largest,
        before.year,
        100.0 * before.largest,
        last.above,
        last.above_starting,
        100.0 * last.changed,
        last.schisms,
        if fails.is_empty() { "PASS".to_string() } else { format!("FAIL ({})", fails.join("; ")) }
    );
    (out, verdict)
}
