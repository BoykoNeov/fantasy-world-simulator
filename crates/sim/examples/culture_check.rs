//! Culture's 150-year check: how the peoples of a world with realms, rulers, wars and armies change.
//!
//! cargo run --release -p sim --example culture_check -- [food|markets] [seed] [years] [small|medium|large] [still] [KEY=VALUE ...]
//!
//! Each decade it prints: the cultures alive and those above 1% of the world's people (and how
//! many of the starting ones are), the largest culture's share, the share of people not of their
//! province's main culture, pop rows per province (all, and those with people), the share of
//! settled provinces whose main culture is not the one they started with, the people who took up
//! another culture in the decade (by assimilation), and splits so far.
//!
//! `still` turns assimilation off (speed 0, no small groups joining at once, no splits): what then
//! still moves people between cultures is soldiers coming home into each province's biggest pop.
//! Compare its fall in "not of the main culture" with the people assimilated in a normal run.
//! `noarmies` switches armies off (and realms choosing war), to check that soldiers are what moves
//! people then. `off` switches culture off, for the speed of a day without it.

use sim::config::Config;
use sim::culture::CultureParams;
use sim::events::Event;
use sim::grid::NONE;
use sim::module::Slot;
use sim::requests::Cause;
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let economics = args.iter().find(|a| *a == "food" || *a == "markets").cloned().unwrap_or("food".into());
    let mut numbers = args.iter().filter_map(|a| a.parse::<u64>().ok());
    let seed = numbers.next().unwrap_or(21);
    let years = numbers.next().unwrap_or(150) as i64;
    let size = match args.iter().find(|a| ["small", "medium", "large"].contains(&a.as_str())).map(String::as_str) {
        Some("small") => Size::Small,
        Some("large") => Size::Large,
        _ => Size::Medium,
    };
    let still = args.iter().any(|a| a == "still");

    let mut config = Config::new(WorldParams::preset(size, seed));
    config.modules.set_choice(Slot::Economics, &economics).unwrap();
    let mut culture = CultureParams::default();
    if still {
        (culture.per_year, culture.small_people, culture.small_share, culture.splits) = (0.0, 0, 0.0, None);
    }
    config.modules.culture = Some(culture);
    if args.iter().any(|a| a == "noarmies") {
        config.modules.warfare = None;
        config.modules.diplomacy.as_mut().unwrap().treaties = None;
    }
    if args.iter().any(|a| a == "off") {
        config.modules.culture = None;
    }
    for kv in args.iter().filter(|a| a.contains('=')) {
        let (k, v) = kv.split_once('=').unwrap();
        sim::settings::set(&mut config, k, v.parse().unwrap()).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    let made = Instant::now();
    let mut s = Sim::new(&config).unwrap();
    println!(
        "{economics}, seed {seed}, {} provinces, {} realms, {} cultures{} (made in {:.1} s)",
        s.world.provinces.count,
        s.world.realms.count(),
        s.world.cultures.count(),
        if still { ", assimilation off" } else { "" },
        made.elapsed().as_secs_f64()
    );
    let start = sim::culture::main_cultures(&s.world);
    let starting = s.world.cultures.count();
    let shares = |s: &Sim| {
        let mut by = vec![0u64; s.world.cultures.count()];
        for (k, &c) in s.world.pops.culture.iter().enumerate() {
            if let Some(x) = by.get_mut(c as usize) {
                *x += s.world.pops.size[k];
            }
        }
        let all: u64 = by.iter().sum();
        by.into_iter().map(|x| x as f64 / all.max(1) as f64).collect::<Vec<f64>>()
    };
    let above = |sh: &[f64], upto: usize| sh.iter().take(upto).filter(|&&x| x > 0.01).count();
    let first = shares(&s);
    println!("at the start: {} cultures above 1% of the people; largest {:.1}%", above(&first, starting), 100.0 * first.iter().cloned().fold(0.0, f64::max));
    println!("year | alive | >1% (of starting) | largest | not main | rows/prov (with people) | main changed | assimilated | splits | ms per month-start day / the day after");

    let (mut assimilated, mut splits, mut month_ms, mut after_ms, mut month_days) = (0u64, 0usize, 0.0f64, 0.0f64, 0usize);
    let end = s.calendar.year_start(years);
    let y0 = s.calendar.date(s.day).year;
    while s.day < end {
        let month = s.calendar.is_month_start(s.day);
        let after = s.day > 0 && s.calendar.is_month_start(s.day - 1);
        let t = Instant::now();
        s.step_day();
        if month {
            month_ms += t.elapsed().as_secs_f64() * 1000.0;
            month_days += 1;
        }
        if after {
            after_ms += t.elapsed().as_secs_f64() * 1000.0;
        }
        assimilated += s.applied_today.iter().filter(|(r, _)| r.cause == Cause::Assimilated).map(|(_, g)| g).sum::<u64>();
        splits += s.delivered_today().iter().filter(|p| matches!(p.event, Event::CultureBorn { .. })).count();
        let date = s.calendar.date(s.day);
        if s.calendar.is_year_start(s.day) && (date.year - y0) % 10 == 0 {
            let w = &s.world;
            let sh = shares(&s);
            let people = w.province_population();
            let main = sim::culture::main_cultures(w);
            let total: u64 = people.iter().sum();
            // People not of their province's main culture.
            let mut minority = 0u64;
            for (k, &p) in w.pops.province.iter().enumerate() {
                if main[p as usize] != NONE && w.pops.culture[k] as u32 != main[p as usize] {
                    minority += w.pops.size[k];
                }
            }
            let settled: Vec<usize> = (0..w.provinces.count).filter(|&p| people[p] > 0).collect();
            let rows = w.pops.size.len() as f64 / settled.len().max(1) as f64;
            let full = w.pops.size.iter().filter(|&&x| x > 0).count() as f64 / settled.len().max(1) as f64;
            let changed = settled.iter().filter(|&&p| start.get(p).is_some_and(|&c| c != NONE && c != main[p])).count();
            let alive = w.cultures.died.iter().filter(|&&d| d == u64::MAX).count();
            println!(
                "{:>4} | {:>5} | {:>3} ({:>3}) | {:>6.1}% | {:>7.2}% | {:>5.2} ({:>4.2}) | {:>11.1}% | {:>10.2}% | {:>6} | {:.3} / {:.3}",
                date.year,
                alive,
                above(&sh, sh.len()),
                above(&sh, starting),
                100.0 * sh.iter().cloned().fold(0.0, f64::max),
                100.0 * minority as f64 / total.max(1) as f64,
                rows,
                full,
                100.0 * changed as f64 / settled.len().max(1) as f64,
                100.0 * assimilated as f64 / total.max(1) as f64,
                splits,
                month_ms / month_days.max(1) as f64,
                after_ms / month_days.max(1) as f64
            );
            (assimilated, month_ms, after_ms, month_days) = (0, 0.0, 0.0, 0);
        }
    }
    println!("problems: {:?}", s.problems);
}
