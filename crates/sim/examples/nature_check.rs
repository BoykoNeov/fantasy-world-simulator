//! Natural world's checks (step 10).
//!
//! cargo run --release -p sim --example nature_check -- [probe|speed] [food|markets|both] [seeds,comma,separated] [years] [KEY=VALUE ...]
//!
//! `speed` makes the Large world with Natural world and without, runs it `years` (at most 20) and
//! then two more, and prints how long making it took (the years lived in), a day's cost with it and
//! without, and the time Natural world's month start took by the clock: rough while other work
//! runs.
//!
//! **The 150-year check** (the default): the Small world with the default modules, under each
//! economy, for each seed (21, 7 and 1), with Natural world on and without it, side by side.
//! Prints every 25 years the world's woodland standing (of the wild woodland, by area), the fish
//! and game left (by shore and by area), the fisheries collapsed (below "warned below"), people,
//! and under markets the middle timber price. Then the pass bars at the end:
//! - the share cleared follows crowding (correlation over provinces with woodland, at least 0.5);
//! - provinces that lost 30% or more of their people and stayed so for 10 years grew woods back
//!   (passes with at least one such province, 4 in 5 of them regrowing);
//! - the world's woodland standing is 35–85% of the wild;
//! - at most 25% of the fisheries collapsed at the end, and none under 5% of its fish for 30 years
//!   running;
//! - under markets: people within ±10% of the same seed without Natural world, starved a year per
//!   1,000 people at most 20% more, and the middle timber price within 2 times;
//! - year-to-year swings of the fish (boom and bust), and tools a worker against without (timber
//!   makes tools), reported.
//!
//! `probe` (piece 0, worlds unchanged: Natural world off) prints, at years 0, 50 and the last, how
//! crowded the settled provinces are (people a km², by quantile), and under markets how many fishers
//! work a km of shore (coast + 0.67 × river) and how many woodcutters a km² of woodland: what
//! "fully crowded" and "fishers a km of shore" are set against. `years` 0 looks at the start only.

use rayon::prelude::*;
use sim::config::Config;
use sim::module::Slot;
use sim::sim::Sim;
use sim::world::{Size, World, WorldParams};
use std::time::Instant;

fn quantiles(mut v: Vec<f64>) -> String {
    if v.is_empty() {
        return "none".into();
    }
    v.sort_by(f64::total_cmp);
    let q = |x: f64| v[((v.len() - 1) as f64 * x).round() as usize];
    format!("n {:4}  10% {:8.3}  25% {:8.3}  50% {:8.3}  75% {:8.3}  90% {:8.3}  max {:9.3}", v.len(), q(0.1), q(0.25), q(0.5), q(0.75), q(0.9), q(1.0))
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn config(economics: &str, seed: u64, nature: bool, set: &[(String, f64)]) -> Config {
    let mut c = Config::new(WorldParams::preset(Size::Small, seed));
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.nature = nature.then(Default::default);
    for (k, v) in set {
        if nature || !k.starts_with("modules.nature") {
            sim::settings::set(&mut c, k, *v).unwrap_or_else(|e| panic!("{k}: {e}"));
        }
    }
    c
}

fn probe_line(w: &World, year: i64) -> Vec<String> {
    let people = w.province_population();
    let p = &w.provinces;
    let settled: Vec<usize> = (0..p.count).filter(|&i| p.cells[i] > 0 && people[i] > 0 && p.area_km2[i] > 0.0).collect();
    let density: Vec<f64> = settled.iter().map(|&i| people[i] as f64 / p.area_km2[i]).collect();
    let mut out = vec![format!("  year {year:3}: people a km²      {}", quantiles(density))];
    let g = &w.goods;
    if let (Some(fish), Some(timber)) = (g.column("fish"), g.column("timber")) {
        let at = |i: usize, c: usize| w.workers.get(g.index(i, c)).copied().unwrap_or(0) as f64;
        let shore: Vec<(usize, f64)> = settled.iter().map(|&i| (i, p.coast_km[i] + 0.67 * p.river_km[i])).filter(|&(_, s)| s > 0.0).collect();
        out.push(format!("            fishers a km shore {}", quantiles(shore.iter().map(|&(i, s)| at(i, fish) / s).collect())));
        let woods: Vec<usize> = settled.iter().copied().filter(|&i| p.forest_km2[i] > 0.0).collect();
        out.push(format!("            woodcutters a km² wood {}", quantiles(woods.iter().map(|&i| at(i, timber) / p.forest_km2[i]).collect())));
        let all_fish: f64 = shore.iter().map(|&(i, _)| at(i, fish)).sum();
        let all_shore: f64 = shore.iter().map(|&(_, s)| s).sum();
        let all_wc: f64 = woods.iter().map(|&i| at(i, timber)).sum();
        let all_wood: f64 = woods.iter().map(|&i| p.forest_km2[i]).sum();
        out.push(format!("            world: {:.2} fishers a km of shore, {:.3} woodcutters a km² of woodland ({:.0} km² in all)", all_fish / all_shore.max(1e-9), all_wc / all_wood.max(1e-9), all_wood));
    }
    let forest: Vec<f64> = settled.iter().map(|&i| p.forest_km2[i] / p.area_km2[i]).collect();
    // Crowding (people a km² ÷ "fully crowded", at most 1), weighted by woodland: what sets the
    // world's woodland standing (about 1 - most cleared × this).
    let wood: f64 = settled.iter().map(|&i| p.forest_km2[i]).sum();
    let weighted = |full: f64| settled.iter().map(|&i| p.forest_km2[i] * (people[i] as f64 / p.area_km2[i] / full).min(1.0)).sum::<f64>() / wood.max(1e-9);
    out.push(format!("            woodland-weighted crowding: {:.3} at 4 a km², {:.3} at 2, {:.3} at 1", weighted(4.0), weighted(2.0), weighted(1.0)));
    if let (Some(fish), Some(grain), Some(herds)) = (g.column("fish"), g.column("grain"), g.column("livestock")) {
        let sum = |c: usize| (0..p.count).map(|i| w.workers.get(g.index(i, c)).copied().unwrap_or(0)).sum::<u64>() as f64;
        out.push(format!("            fishers are {:.1}% of food workers", 100.0 * sum(fish) / (sum(fish) + sum(grain) + sum(herds)).max(1.0)));
    }
    if year == 0 {
        out.push(format!("            woodland share     {}", quantiles(forest)));
    }
    out
}

fn probe(economics: &str, seed: u64, years: i64) -> Vec<String> {
    let mut sim = Sim::new(&config(economics, seed, false, &[])).unwrap();
    let start = sim.calendar.date(0).year;
    let mut lines = vec![format!("{economics} seed {seed}: {} provinces", sim.world.provinces.count)];
    // Year 0 under markets: workers are set by the first month's work, so look a day in.
    sim.run_days(1);
    lines.extend(probe_line(&sim.world, 0));
    for y in [50, years].into_iter().filter(|_| years > 0) {
        sim.run_to_year(start + y);
        lines.extend(probe_line(&sim.world, y));
    }
    lines
}

/// What a world looks like once a year.
#[derive(Clone, Default)]
struct Year {
    people: Vec<u64>,
    woods_lost: Vec<u64>,
    fish_lost: Vec<u64>,
    population: u64,
    starved: u64,
    /// World measures: woodland standing, fish left, game left (shares).
    woods: f64,
    fish: f64,
    game: f64,
    collapsed: f64,
    timber_price: f64,
    tools_per_worker: f64,
}

struct Run {
    years: Vec<Year>,
    /// Shore (km, as the fish see it) and woodland (km²) of each province, and its area.
    shore: Vec<f64>,
    forest: Vec<f64>,
    area: Vec<f64>,
    ms_per_day: f64,
    /// Events told over the run: woods cleared, regrown, fisheries collapsed, recovered.
    told: [u64; 4],
}

fn look(sim: &Sim, shore: &[f64]) -> Year {
    let w = &sim.world;
    let p = &w.provinces;
    let n = p.count;
    let people = w.province_population();
    let left = |v: &[u64], i: usize| 1.0 - v.get(i).copied().unwrap_or(0) as f64 / 1e6;
    let weigh = |v: &[u64], by: &dyn Fn(usize) -> f64| {
        let (mut a, mut b) = (0.0, 0.0);
        for i in 0..n {
            a += by(i) * left(v, i);
            b += by(i);
        }
        if b > 0.0 { a / b } else { 1.0 }
    };
    let fisheries: Vec<usize> = (0..n).filter(|&i| shore[i] > 0.0).collect();
    let low = sim.settings.nature.as_ref().map_or(0.2, |x| x.low);
    let collapsed = fisheries.iter().filter(|&&i| left(&w.fish_lost, i) < low).count() as f64 / fisheries.len().max(1) as f64;
    let g = &w.goods;
    let settled: Vec<usize> = (0..n).filter(|&i| people[i] > 0).collect();
    let timber_price = g.column("timber").map_or(0.0, |c| median(settled.iter().map(|&i| w.price[g.index(i, c)] as f64 / 1000.0).collect()));
    let tools_per_worker = g.column("tools").map_or(0.0, |c| {
        let tools: u64 = (0..n).map(|i| w.stock[g.index(i, c)]).sum();
        let workers: u64 = w.workers.iter().sum();
        tools as f64 / workers.max(1) as f64
    });
    let last = sim.history.last();
    Year {
        woods: weigh(&w.woods_lost, &|i| p.forest_km2[i]),
        fish: weigh(&w.fish_lost, &|i| shore[i]),
        game: weigh(&w.game_lost, &|i| p.area_km2[i]),
        collapsed,
        timber_price,
        tools_per_worker,
        population: people.iter().sum(),
        starved: last.map_or(0, |y| y.ledger.starved),
        people,
        woods_lost: w.woods_lost.clone(),
        fish_lost: w.fish_lost.clone(),
    }
}

fn run(economics: &str, seed: u64, years: i64, nature: bool, set: &[(String, f64)]) -> Run {
    let c = config(economics, seed, nature, set);
    let river_share = c.modules.nature.as_ref().and_then(|x| x.fish.as_ref()).map_or(0.67, |f| f.river_share);
    let mut sim = Sim::new(&c).unwrap();
    let p = &sim.world.provinces;
    let shore: Vec<f64> = (0..p.count).map(|i| p.coast_km[i] + river_share * p.river_km[i]).collect();
    let (forest, area) = (p.forest_km2.clone(), p.area_km2.clone());
    let start = sim.calendar.date(0).year;
    let mut out = vec![look(&sim, &shore)];
    let mut told = [0u64; 4];
    let began = Instant::now();
    for y in 1..=years {
        sim.run_to_year(start + y);
        out.push(look(&sim, &shore));
        for e in sim.event_log.entries.iter().filter(|e| e.day + sim.calendar.days_per_year() as u64 >= sim.day) {
            let k = match e.event {
                sim::events::Event::WoodsCleared { .. } => 0,
                sim::events::Event::WoodsRegrown { .. } => 1,
                sim::events::Event::FisheryCollapsed { .. } => 2,
                sim::events::Event::FisheryRecovered { .. } => 3,
                _ => continue,
            };
            told[k] += 1;
        }
    }
    let ms_per_day = began.elapsed().as_secs_f64() * 1000.0 / sim.day as f64;
    Run { years: out, shore, forest, area, ms_per_day, told }
}

fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let va: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
    let vb: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
    if va > 0.0 && vb > 0.0 { cov / (va * vb).sqrt() } else { 0.0 }
}

fn report(economics: &str, seed: u64, with: &Run, without: &Run, crowd_full: f64) -> (Vec<String>, Vec<bool>) {
    let mut lines = vec![format!("{economics} seed {seed}: {:.2} ms a day with Natural world, {:.2} without (rough: other work runs)", with.ms_per_day, without.ms_per_day)];
    let last = with.years.len() - 1;
    for y in (0..=last).step_by(25).chain(std::iter::once(last).filter(|l| l % 25 != 0)) {
        let (a, b) = (&with.years[y], &without.years[y]);
        let markets = if a.timber_price > 0.0 { format!(", timber {:.2} against {:.2}, tools a worker {:.2} against {:.2}", a.timber_price, b.timber_price, a.tools_per_worker, b.tools_per_worker) } else { String::new() };
        lines.push(format!(
            "  year {y:3}: woodland {:5.1}% standing, fish {:5.1}% left, game {:5.1}% left, fisheries collapsed {:4.1}%; people {} against {} ({:+.1}%){markets}",
            a.woods * 100.0,
            a.fish * 100.0,
            a.game * 100.0,
            a.collapsed * 100.0,
            a.population,
            b.population,
            (a.population as f64 / b.population.max(1) as f64 - 1.0) * 100.0
        ));
    }
    let end = &with.years[last];
    let n = end.people.len();
    // Cleared share against crowding, over provinces with woodland and people.
    let wooded: Vec<usize> = (0..n).filter(|&i| with.forest[i] > 0.0 && end.people[i] > 0 && with.area[i] > 0.0).collect();
    let crowd: Vec<f64> = wooded.iter().map(|&i| (end.people[i] as f64 / with.area[i] / crowd_full).min(1.0)).collect();
    let cleared: Vec<f64> = wooded.iter().map(|&i| end.woods_lost[i] as f64 / 1e6).collect();
    let r = correlation(&crowd, &cleared);
    // Provinces that lost 30%+ of their people and stayed so for 10 years: did their woods grow back?
    let (mut lost, mut regrew) = (0, 0);
    for i in (0..n).filter(|&i| with.forest[i] > 0.0) {
        for y in 0..last.saturating_sub(10) {
            let base = with.years[y].people[i] as f64;
            if base <= 0.0 || with.years[y].woods_lost[i] < 20_000 {
                continue;
            }
            if (y + 1..=y + 10).all(|z| with.years[z].people[i] as f64 <= 0.7 * base) {
                lost += 1;
                if with.years[y + 10].woods_lost[i] < with.years[y].woods_lost[i] {
                    regrew += 1;
                }
                break;
            }
        }
    }
    // Fisheries under 5% for 30 years running.
    let mut longest = 0;
    for i in (0..n).filter(|&i| with.shore[i] > 0.0) {
        let mut run = 0;
        for y in &with.years {
            if 1.0 - y.fish_lost[i] as f64 / 1e6 < 0.05 {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
    }
    // Boom and bust: year-to-year changes of the world's fish, and of each fishery's.
    let swings: Vec<f64> = with.years.windows(2).skip(10).map(|w| (w[1].fish - w[0].fish).abs()).collect();
    let mut own = Vec::new();
    for i in (0..n).filter(|&i| with.shore[i] > 0.0) {
        let s: Vec<f64> = with.years.windows(2).skip(10).map(|w| (w[1].fish_lost[i] as f64 - w[0].fish_lost[i] as f64).abs() / 1e6).collect();
        own.push(s.iter().sum::<f64>() / s.len().max(1) as f64);
    }
    lines.push(format!(
        "  fish swings a year: world mean {:.4}, largest {:.4}; a fishery's mean {:.4} (the middle), {:.4} (the most)",
        swings.iter().sum::<f64>() / swings.len().max(1) as f64,
        swings.iter().cloned().fold(0.0, f64::max),
        median(own.clone()),
        own.iter().cloned().fold(0.0, f64::max)
    ));
    lines.push(format!("  told: {} woods cleared, {} regrown, {} fisheries collapsed, {} recovered", with.told[0], with.told[1], with.told[2], with.told[3]));
    let mut pass = vec![r >= 0.5, lost > 0 && regrew * 5 >= lost * 4, (0.35..=0.85).contains(&end.woods), end.collapsed <= 0.25 && longest < 30];
    lines.push(format!("  cleared follows crowding: correlation {r:.2} over {} provinces (bar 0.5): {}", wooded.len(), ok(pass[0])));
    lines.push(format!("  people left, woods back: {regrew} of {lost} provinces that lost 30%+ of their people for 10 years grew woods back: {}", ok(pass[1])));
    lines.push(format!("  woodland standing {:.1}% (bar 35–85%): {}", end.woods * 100.0, ok(pass[2])));
    lines.push(format!("  fisheries collapsed {:.1}% (bar 25%), longest under 5% {longest} years (bar under 30): {}", end.collapsed * 100.0, ok(pass[3])));
    let starved = |r: &Run| {
        let (s, p): (u64, u64) = r.years.iter().skip(1).fold((0, 0), |a, y| (a.0 + y.starved, a.1 + y.population));
        s as f64 * 1000.0 / p.max(1) as f64
    };
    let (sw, so) = (starved(with), starved(without));
    let people = end.population as f64 / without.years[last].population.max(1) as f64;
    if economics == "markets" {
        let price = end.timber_price / without.years[last].timber_price.max(1e-9);
        pass.push((0.9..=1.1).contains(&people));
        pass.push(sw <= so * 1.2 + 1e-9);
        pass.push((0.5..=2.0).contains(&price));
        lines.push(format!("  people {:.3}× without (bar ±10%): {}", people, ok(pass[4])));
        lines.push(format!("  starved {sw:.2} a year per 1,000 against {so:.2} (bar +20%): {}", ok(pass[5])));
        lines.push(format!("  middle timber price {:.2}× without (bar 2×): {}", price, ok(pass[6])));
    } else {
        lines.push(format!("  people {:.4}× without, starved {sw:.3} against {so:.3} a year per 1,000 (v0: the same, nothing reads the wild)", people));
    }
    (lines, pass)
}

/// `speed`: on the Large world, how long making the world takes with Natural world (its years
/// lived in) and without, then after `warm` years (the years argument; 150 is too many here, so
/// it is capped at 20) the time Natural world's month start takes, by the clock, and a day's cost
/// with it and without (rough: other work runs on the machine).
fn speed(economics: &str, seed: u64, warm: i64, set: &[(String, f64)]) {
    let warm = warm.min(20);
    let mut out = Vec::new();
    for nature in [true, false] {
        let mut c = config(economics, seed, nature, set);
        c.world = WorldParams::preset(Size::Large, seed);
        let made = Instant::now();
        let mut sim = Sim::new(&c).unwrap();
        let made = made.elapsed().as_secs_f64();
        let start = sim.calendar.date(0).year;
        sim.run_to_year(start + warm);
        sim.timing = Some(Box::default());
        let from = sim.day;
        let began = Instant::now();
        sim.run_to_year(start + warm + 2);
        let days = sim.day - from;
        let ms_day = began.elapsed().as_secs_f64() * 1000.0 / days as f64;
        let months = (from..sim.day).filter(|&d| sim.calendar.is_month_start(d)).count() as f64;
        let t = sim.timing.take().unwrap();
        let mine = t.by_module().into_iter().find(|m| m.0 == Slot::Nature).map_or((0.0, 0.0), |m| (m.1.as_secs_f64() * 1000.0 / months, m.2.as_secs_f64() * 1000.0 / months));
        out.push(format!(
            "{economics} seed {seed}, large world ({} provinces), Natural world {}: made in {made:.2} s; {ms_day:.2} ms a day over years {warm}–{}; Natural world's month start {:.3} ms of its own work and {:.3} ms of the engine's checks around it",
            sim.world.provinces.count,
            if nature { "on" } else { "off" },
            warm + 2,
            mine.0,
            mine.1
        ));
    }
    for l in out {
        println!("{l}");
    }
}

fn ok(b: bool) -> &'static str {
    if b { "pass" } else { "FAIL" }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let economies: Vec<&str> = match args.iter().find(|a| ["food", "markets", "both"].contains(&a.as_str())).map(String::as_str) {
        Some("food") => vec!["food"],
        Some("markets") => vec!["markets"],
        _ => vec!["food", "markets"],
    };
    // Seeds are written with commas ("21,7,1", or "21," for one); a plain number is the years.
    let seeds: Vec<u64> = args.iter().find(|a| a.contains(',')).map(|a| a.split(',').filter(|s| !s.is_empty()).map(|s| s.parse().unwrap()).collect()).unwrap_or(vec![21, 7, 1]);
    let years: i64 = args.iter().find_map(|a| a.parse::<i64>().ok()).unwrap_or(150);
    let set: Vec<(String, f64)> = args.iter().filter_map(|a| a.split_once('=')).map(|(k, v)| (k.to_string(), v.parse().unwrap())).collect();
    let jobs: Vec<(&str, u64)> = economies.iter().flat_map(|&e| seeds.iter().map(move |&s| (e, s))).collect();
    if args.iter().any(|a| a == "probe") {
        let out: Vec<Vec<String>> = jobs.par_iter().map(|&(e, s)| probe(e, s, years)).collect();
        for l in out.into_iter().flatten() {
            println!("{l}");
        }
        return;
    }
    if args.iter().any(|a| a == "speed") {
        for &e in &economies {
            for &s in &seeds {
                speed(e, s, years, &set);
            }
        }
        return;
    }
    let crowd_full = config("food", 1, true, &set).modules.nature.unwrap().crowd_full;
    let runs: Vec<(Run, Run)> = jobs.par_iter().map(|&(e, s)| rayon::join(|| run(e, s, years, true, &set), || run(e, s, years, false, &set))).collect();
    let mut passes: Vec<(String, Vec<bool>)> = Vec::new();
    for (&(e, s), (with, without)) in jobs.iter().zip(&runs) {
        let (lines, pass) = report(e, s, with, without, crowd_full);
        for l in lines {
            println!("{l}");
        }
        passes.push((format!("{e} {s}"), pass));
    }
    println!("summary (per world: crowding, regrowth, woodland, fisheries[, people, starved, timber]):");
    for (name, p) in &passes {
        println!("  {name}: {}", p.iter().map(|&b| ok(b)).collect::<Vec<_>>().join(" "));
    }
    for e in &economies {
        let regrew = passes.iter().filter(|(n, _)| n.starts_with(e)).filter(|(_, p)| p[1]).count();
        println!("  {e}: woods grew back where people left on {regrew} of {} seeds (bar: 2 of 3)", seeds.len());
    }
}
