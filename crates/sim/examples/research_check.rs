//! Research's 150-year check (step 9): how many techs realms know as the years go by, against the
//! pass bars the user set: at year 150 the middle living realm knows 8 to 14 techs, the leader at
//! most 2.5 times as many, and no realm knows the whole tree before year 120.
//!
//! The Small world (500 provinces) with the default modules (realms, rulers, wars chosen by the
//! realms, armies) and Research, under each economy, for each seed, side by side. Prints, every 30
//! years: living realms, the techs of the middle realm, of the leader and of the least, the leader
//! against the middle, and under markets the scholars' wages as a share of the taxes that year.
//! Then the year the first realm knew every tech, if any, and the time a simulated day took
//! (rough: other work may be running).
//!
//! cargo run --release -p sim --example research_check -- [food|markets|both] [seeds,comma,separated] [years] [without] [serial] [KEY=VALUE ...]
//!
//! `without` runs the same worlds without Research, to compare the time a day takes; `serial` runs
//! them one after another instead of side by side, for steadier times.

use sim::config::Config;
use sim::grid::NONE;
use sim::module::Slot;
use sim::requests::Cause;
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::time::Instant;

struct Report {
    lines: Vec<String>,
    /// (year, median, leader) at the last report.
    end: (i64, f64, usize),
    finished: Option<i64>,
    ms_per_day: f64,
}

fn run(economics: &str, seed: u64, years: i64, without: bool, set: &[(String, f64)]) -> Report {
    let mut c = Config::new(WorldParams::preset(Size::Small, seed));
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.research = if without { None } else { Some(Default::default()) };
    for (k, v) in set {
        sim::settings::set(&mut c, k, *v).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    let mut sim = Sim::new(&c).unwrap();
    let k = sim.world.techs.len();
    let start = sim.calendar.date(0).year;
    let mut lines = Vec::new();
    {
        let w = &sim.world;
        let people = realm_people(w);
        let mut sizes: Vec<u64> = people.iter().copied().filter(|&x| x > 0).collect();
        sizes.sort_unstable();
        let median = sizes.get(sizes.len() / 2).copied().unwrap_or(0).max(1);
        lines.push(format!("{economics} seed {seed}: {} realms; the biggest has {:.1}× the middle realm's people", sizes.len(), *sizes.last().unwrap_or(&0) as f64 / median as f64));
    }
    let (mut wages, mut taxes) = (0u64, 0u64);
    let mut finished = None;
    let mut end = (0, 0.0, 0);
    let t0 = Instant::now();
    for year in 1..=years {
        let target = sim.calendar.year_start(start + year);
        while sim.day < target {
            sim.step_day();
            for (r, granted) in &sim.applied_today {
                match r.cause {
                    Cause::Scholars => wages += granted,
                    Cause::Taxes => taxes += granted,
                    _ => {}
                }
            }
        }
        let w = &sim.world;
        let alive: Vec<u32> = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect();
        let known = |r: u32| (0..k).filter(|&t| w.realm_tech[r as usize * k + t] != NONE).count();
        if k > 0 && finished.is_none() && (0..w.realms.count() as u32).any(|r| known(r) == k) {
            finished = Some(year);
        }
        if year % 30 == 0 || year == years {
            let mut counts: Vec<usize> = alive.iter().map(|&r| known(r)).collect();
            counts.sort_unstable();
            let n = counts.len();
            let median = if n == 0 { 0.0 } else if n % 2 == 1 { counts[n / 2] as f64 } else { (counts[n / 2 - 1] + counts[n / 2]) as f64 / 2.0 };
            let (least, most) = (counts.first().copied().unwrap_or(0), counts.last().copied().unwrap_or(0));
            let pay = if taxes > 0 { format!(", wages {:.1}% of taxes", 100.0 * wages as f64 / taxes as f64) } else { String::new() };
            lines.push(format!("  year {year:>3}: {n:>3} realms; techs: middle {median:>4.1}, leader {most:>2}, least {least:>2}, leader ÷ middle {:.2}{pay}", most as f64 / median.max(0.5)));
            end = (year, median, most);
            if year == years {
                // The realms that know least, and why.
                let people = realm_people(w);
                let mut sizes: Vec<u64> = alive.iter().map(|&r| people[r as usize]).filter(|&x| x > 0).collect();
                sizes.sort_unstable();
                let mid = sizes.get(sizes.len() / 2).copied().unwrap_or(1).max(1) as f64;
                for &r in alive.iter().filter(|&&r| known(r) == least) {
                    let land = w.owner.iter().filter(|&&o| o == r).count();
                    let born = if w.realms.broke_from[r as usize] == NONE { "from the start or the editor" } else { "rebels" };
                    lines.push(format!("    realm {r} ({born}): {land} provinces, {:.3}× the middle realm's people, knows {least}", people[r as usize] as f64 / mid));
                }
            }
            (wages, taxes) = (0, 0);
        }
    }
    let ms_per_day = t0.elapsed().as_secs_f64() * 1000.0 / sim.day as f64;
    Report { lines, end, finished, ms_per_day }
}

fn realm_people(w: &sim::world::World) -> Vec<u64> {
    let pop = w.province_population();
    let mut out = vec![0u64; w.realms.count()];
    for (p, &r) in w.owner.iter().enumerate() {
        if r != NONE && w.realms.alive(r) {
            out[r as usize] += pop[p];
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (without, serial) = (args.iter().any(|a| a == "without"), args.iter().any(|a| a == "serial"));
    let mut rest = args.iter().filter(|a| !a.contains('=') && *a != "without" && *a != "serial");
    let economies: Vec<&str> = match rest.next().map(|s| s.as_str()) {
        Some("food") => vec!["food"],
        Some("markets") => vec!["markets"],
        _ => vec!["food", "markets"],
    };
    let seeds: Vec<u64> = rest.next().map_or(vec![21, 7, 1], |s| s.split(',').map(|x| x.parse().unwrap()).collect());
    let years: i64 = rest.next().map_or(150, |s| s.parse().unwrap());
    let set: Vec<(String, f64)> = args.iter().filter_map(|a| a.split_once('=')).map(|(k, v)| (k.to_string(), v.parse().unwrap())).collect();
    let runs: Vec<(&str, u64)> = economies.iter().flat_map(|&e| seeds.iter().map(move |&s| (e, s))).collect();
    let reports: Vec<Report> = if serial {
        runs.iter().map(|&(e, s)| run(e, s, years, without, &set)).collect()
    } else {
        std::thread::scope(|scope| {
            let set = &set;
            let handles: Vec<_> = runs.iter().map(|&(e, s)| scope.spawn(move || run(e, s, years, without, set))).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        })
    };
    for r in &reports {
        r.lines.iter().for_each(|l| println!("{l}"));
        let fin = r.finished.map_or("nobody knew every tech".to_string(), |y| format!("first realm to know every tech: year {y}"));
        println!("  {fin}; {:.2} ms a simulated day", r.ms_per_day);
    }
    println!("\nPass bars at year {years}: middle 8 to 14, leader at most 2.5 × middle, nobody finished before year 120.");
    for ((e, s), r) in runs.iter().zip(&reports) {
        let (_, median, most) = r.end;
        let ok = (8.0..=14.0).contains(&median) && most as f64 <= 2.5 * median && r.finished.is_none_or(|y| y >= 120);
        println!("  {e} seed {s}: middle {median}, leader {most}, finished {:?}: {}", r.finished, if ok { "pass" } else { "FAIL" });
    }
}
