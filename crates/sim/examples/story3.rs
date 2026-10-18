//! Story 3, "Plague along the trade routes": a plague lands in a port, spreads along trade routes
//! faster than over land, kills a share of the population, and leaves labour shortages and higher
//! wages behind.
//!
//! cargo run --release -p sim --example story3 -- [food|markets] [seeds...] [KEY=VALUE ...]
//!
//! Small worlds, seeds 21, 7, 1, 2, 3, 4, 5, 6 and 8 by default, both economies, plagues beginning
//! by chance switched off. At the start of year 3 a plague is poked into the most populous province
//! with a sea way; the control is the same world without the poke. A seed passes each part:
//!
//! - (a) it lands in a port: so by the setup (the 150-year check counts where plagues begin on
//!   their own);
//! - (b) for each arrival in the first 3 years, the distance between the provinces' centres over
//!   the weeks since the plague reached the province it came from: the median by sea at least twice
//!   the median over land;
//! - (c) the plague's dead within 5 years are 10–50% of the people of the provinces it struck (as
//!   they were at the poke), and at most 35% of the world's;
//! - (d) markets: in the 5 hardest-hit provinces, over the 3 years after the plague left each,
//!   against the control: workers down by at least half the share of their people the plague
//!   killed, and goods made per worker, valued at base prices, at least 5% higher (read from the
//!   month starts' explanations, as story 1 reads harvests). Also shown: goods made per worker at
//!   local prices, and coins per person. Food (Economics v0): in the provinces it struck, over the
//!   3 years after the outbreak ended, food in store per person at or above the control's and no
//!   more starvation.
//!
//! The story passes with at least 6 of 9 seeds on each part, under each economy.

use rayon::prelude::*;
use sim::config::Config;
use sim::disease::DiseaseParams;
use sim::events::Event;
use sim::grid::dot;
use sim::module::Slot;
use sim::requests::Cause;
use sim::sim::{Sim, Traced};
use sim::world::{Size, WorldParams};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const SEEDS: [u64; 9] = [21, 7, 1, 2, 3, 4, 5, 6, 8];

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() { f64::NAN } else { v[v.len() / 2] }
}

fn biggest_port(s: &Sim) -> u32 {
    let people = s.world.province_population();
    (0..people.len() as u32).filter(|&p| s.world.routes.of(p).any(|l| l.sea_days.is_some())).max_by_key(|&p| (people[p as usize], std::cmp::Reverse(p))).unwrap()
}

/// One month start's work in a province, from its explanation: (workers, goods made at base prices,
/// at local prices). `None` where the day can't be traced: the engine's tracing stops on a day a
/// value is added in the province (a new pop row, a new army), so it is traced on a copy and such
/// months are left out (from both worlds).
fn made(sim: &Sim, day: u64, province: u32, base: &BTreeMap<String, f64>) -> Option<(f64, f64, f64)> {
    let mut copy = sim.fork();
    let t = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || copy.trace(day, province))).ok()?.ok()?;
    let (mut workers, mut at_base, mut at_local) = (0.0, 0.0, 0.0);
    for s in &t.steps {
        let Traced::Note(n) = &s.what else { continue };
        if s.module != Some(Slot::Economics) {
            continue;
        }
        let Some((good, rest)) = n.split_once(": ") else { continue };
        let Some((w, rest)) = rest.split_once(" workers made ") else { continue };
        let (Ok(w), Some(Ok(m))) = (w.parse::<f64>(), rest.split(' ').next().map(str::parse::<f64>)) else { continue };
        let price: f64 = rest.rsplit_once("price ").and_then(|x| x.1.strip_suffix(" coins")).and_then(|x| x.parse().ok()).unwrap_or(0.0);
        workers += w;
        at_base += m * base.get(good).copied().unwrap_or(0.0);
        at_local += m * price;
    }
    Some((workers, at_base, at_local))
}

/// What a seed did, part by part.
struct Outcome {
    line: String,
    b: bool,
    c: bool,
    d: bool,
}

fn one(economics: &str, seed: u64, args: &[String]) -> Outcome {
    let mut c = Config::new(WorldParams::preset(Size::Small, seed));
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.disease = Some(DiseaseParams { starts: None, ..DiseaseParams::default() });
    for kv in args.iter().filter(|a| a.contains('=')) {
        let (k, v) = kv.split_once('=').unwrap();
        sim::settings::set(&mut c, k, v.parse().unwrap()).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    let mut base = Sim::new(&c).unwrap();
    let start = base.calendar.date(0).year;
    base.run_to_year(start + 3);
    let year = base.calendar.days_per_year() as u64;
    let port = biggest_port(&base);
    let at_poke = base.world.province_population();
    let poke = base.day;

    // Pass 1: the plague's five years.
    let mut plague = base.fork();
    plague.poke(Event::Plague { province: port });
    let mut reached: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    reached.entry(port).or_default().push(poke);
    let mut arrived: Vec<(u32, u32, bool, u64)> = Vec::new();
    let mut dead: BTreeMap<u32, u64> = BTreeMap::new();
    let mut last_dead: BTreeMap<u32, u64> = BTreeMap::new();
    let mut over = None;
    while plague.day < poke + 5 * year {
        plague.step_day();
        for p in plague.delivered_today() {
            match p.event {
                Event::PlagueArrived { province, from, by_sea, .. } => {
                    reached.entry(province).or_default().push(p.day);
                    arrived.push((province, from, by_sea, p.day));
                }
                Event::OutbreakOver { .. } => over = over.or(Some(p.day)),
                _ => {}
            }
        }
        for (r, g) in &plague.applied_today {
            if r.cause == Cause::Plague {
                *dead.entry(r.entity).or_default() += g;
                last_dead.insert(r.entity, r.day);
            }
        }
    }
    let problems = plague.problems;
    // (b) km a week, sea and land, in the first 3 years.
    let km = |a: u32, b: u32| {
        let ctr = &plague.world.provinces.center;
        dot(ctr[a as usize], ctr[b as usize]).clamp(-1.0, 1.0).acos() * plague.world.params.radius_km
    };
    let speed = |sea: bool| -> Vec<f64> {
        arrived
            .iter()
            .filter(|a| a.2 == sea && a.3 < poke + 3 * year)
            .filter_map(|&(q, from, _, day)| {
                let d0 = reached.get(&from)?.iter().copied().filter(|&d| d < day).max()?;
                Some(km(from, q) / ((day - d0) as f64 / plague.calendar.days_per_week() as f64).max(1.0))
            })
            .collect()
    };
    let (sea, land) = (speed(true), speed(false));
    let (ms, ml) = (median(sea.clone()), median(land.clone()));
    let b = ms >= 2.0 * ml;
    // (c) the dead within 5 years.
    let struck: BTreeSet<u32> = reached.keys().copied().collect();
    let struck_people: u64 = struck.iter().map(|&p| at_poke[p as usize]).sum();
    let all_dead: u64 = dead.values().sum();
    let world: u64 = at_poke.iter().sum();
    let (cs, cw) = (all_dead as f64 / struck_people.max(1) as f64, all_dead as f64 / world as f64);
    let c_pass = (0.10..=0.50).contains(&cs) && cw <= 0.35;

    // (d) After it left: the 5 hardest-hit provinces (by the share of their people it killed), over
    // the 3 years after each was last struck; for the food economy, every province it struck over
    // the 3 years after the outbreak ended.
    let mut hardest: Vec<(u32, f64)> = dead.iter().map(|(&p, &d)| (p, d as f64 / at_poke[p as usize].max(1) as f64)).collect();
    hardest.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    hardest.truncate(5);
    let markets = economics == "markets";
    let cal = plague.calendar.clone();
    let month_starts = |from: u64, to: u64| (from + 1..=to).filter(|&d| cal.is_month_start(d)).collect::<Vec<u64>>();
    let mut need: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    if markets {
        for &(p, _) in &hardest {
            let left = last_dead[&p];
            for d in month_starts(left, left + 3 * year) {
                need.entry(d).or_default().push(p);
            }
        }
    }
    let ended = over.unwrap_or(poke + 5 * year);
    let food_days = month_starts(ended, ended + 3 * year);
    let end = need.keys().last().copied().unwrap_or(0).max(*food_days.last().unwrap_or(&0)) + 1;
    let base_price: BTreeMap<String, f64> = c.modules.markets.as_ref().map(|m| m.goods.iter().map(|(k, g)| (k.clone(), g.base_price)).collect()).unwrap_or_default();
    // Pass 2: the plague again, and the control, to the end of the windows.
    let watch = |mut s: Sim, poked: bool| {
        if poked {
            s.poke(Event::Plague { province: port });
        }
        let goods = s.world.goods.clone();
        let food = goods.columns_under(goods.tree.find("food").unwrap()).unwrap_or_default();
        let coins = goods.columns_under(goods.tree.find("money").unwrap()).unwrap_or_default();
        // (workers, at base prices, at local prices, coins, people) for each month and province.
        let mut work: BTreeMap<(u64, u32), [f64; 5]> = BTreeMap::new();
        let (mut stored, mut fed, mut starved) = (0.0, 0.0, 0u64);
        while s.day < end {
            let d = s.day;
            let traced = need.get(&d);
            let mine = traced.is_some() && !s.calendar.is_year_start(d);
            if mine {
                s.checkpoints.push(Arc::new(s.checkpoint()));
            }
            s.step_day();
            if let Some(ps) = traced {
                let people = s.world.province_population();
                for &p in ps {
                    if let Some((w, at_base, at_local)) = made(&s, d, p, &base_price) {
                        let k = goods.stride();
                        let coin: u64 = coins.iter().map(|&c| s.world.stock[p as usize * k + c]).sum();
                        work.insert((d, p), [w, at_base, at_local, coin as f64, people[p as usize] as f64]);
                    }
                }
            }
            if mine {
                s.checkpoints.retain(|c| c.day != d);
            }
            if d > ended && d <= ended + 3 * year {
                if s.calendar.is_month_start(d) {
                    let people = s.world.province_population();
                    let k = goods.stride();
                    for &p in &struck {
                        stored += food.iter().map(|&c| s.world.stock[p as usize * k + c] as f64).sum::<f64>();
                        fed += people[p as usize] as f64;
                    }
                }
                for e in s.delivered_today() {
                    if let Event::Famine { province, starved: x } = e.event
                        && struck.contains(&province)
                    {
                        starved += x;
                    }
                }
            }
        }
        (work, stored / fed.max(1.0), starved)
    };
    let (a, b2, watch) = (base.fork(), base.fork(), &watch);
    let ((wp, food_p, starved_p), (wc, food_c, starved_c)) = rayon::join(move || watch(a, true), move || watch(b2, false));
    // Months traced in both worlds, summed.
    let both: Vec<&(u64, u32)> = wp.keys().filter(|k| wc.contains_key(k)).collect();
    let skipped = need.values().map(Vec::len).sum::<usize>() - both.len();
    let sum = |m: &BTreeMap<(u64, u32), [f64; 5]>| both.iter().fold([0.0f64; 5], |mut a, k| {
        a.iter_mut().zip(m[*k]).for_each(|(x, v)| *x += v);
        a
    });
    let (wp, wc) = (sum(&wp), sum(&wc));
    let hard_dead: u64 = hardest.iter().map(|h| dead[&h.0]).sum();
    let hard_people: u64 = hardest.iter().map(|h| at_poke[h.0 as usize]).sum();
    let death_share = hard_dead as f64 / hard_people.max(1) as f64;
    let workers_down = 1.0 - wp[0] / wc[0].max(1.0);
    let per_worker = (wp[1] / wp[0].max(1.0)) / (wc[1] / wc[0].max(1.0)).max(1e-9);
    let per_worker_local = (wp[2] / wp[0].max(1.0)) / (wc[2] / wc[0].max(1.0)).max(1e-9);
    let coins_pp = (wp[3] / wp[4].max(1.0)) / (wc[3] / wc[4].max(1.0)).max(1e-9);
    let d = if markets { workers_down >= 0.5 * death_share && per_worker >= 1.05 } else { food_p >= food_c && starved_p <= starved_c };
    let line = format!(
        "{economics} seed {seed}: port {port}; (b) km a week by sea {ms:.0} ({} arrivals), over land {ml:.0} ({}), {:.2}x {}; (c) dead {:.1}% of the {} struck provinces' people, {:.2}% of the world's {}; (d) {} {}; problems {problems:?}",
        sea.len(),
        land.len(),
        ms / ml,
        if b { "pass" } else { "FAIL" },
        100.0 * cs,
        struck.len(),
        100.0 * cw,
        if c_pass { "pass" } else { "FAIL" },
        if markets {
            format!(
                "5 hardest hit (lost {:.1}% of their people): workers down {:.1}%, made per worker at base prices {per_worker:.3}x, at local prices {per_worker_local:.3}x, coins per person {coins_pp:.3}x ({} months traced, {skipped} not)",
                100.0 * death_share,
                100.0 * workers_down,
                both.len()
            )
        } else {
            String::new()
        },
        if markets { if d { "pass" } else { "FAIL" } } else { "" },
    ) + &format!(
        "; food in store per person in the struck provinces {:.3}x the control's, starved {starved_p} against {starved_c}{}",
        food_p / food_c.max(1e-9),
        if markets { String::new() } else if d { " pass".into() } else { " FAIL".into() }
    );
    Outcome { line, b, c: c_pass, d }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let economies: Vec<&str> = match args.iter().find(|a| *a == "food" || *a == "markets") {
        Some(e) => vec![e.as_str()],
        None => vec!["food", "markets"],
    };
    let mut seeds: Vec<u64> = args.iter().filter_map(|a| a.parse().ok()).collect();
    if seeds.is_empty() {
        seeds = SEEDS.to_vec();
    }
    let runs: Vec<(&str, u64)> = economies.iter().flat_map(|&e| seeds.iter().map(move |&s| (e, s))).collect();
    // The engine's tracing can stop on some days (see `made`): quietly, as those months are skipped.
    let shown = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !info.location().is_some_and(|l| l.file().ends_with("fields.rs")) {
            shown(info);
        }
    }));
    let out: Vec<Outcome> = runs.par_iter().map(|&(e, s)| one(e, s, &args)).collect();
    let _ = std::panic::take_hook();
    for e in &economies {
        let mine: Vec<&Outcome> = runs.iter().zip(&out).filter(|x| x.0.0 == *e).map(|x| x.1).collect();
        for o in &mine {
            println!("{}", o.line);
        }
        let n = mine.len();
        let (b, c, d) = (mine.iter().filter(|o| o.b).count(), mine.iter().filter(|o| o.c).count(), mine.iter().filter(|o| o.d).count());
        let bar = (2 * n).div_ceil(3);
        println!(
            "{e}: (a) {n} of {n} (by the setup); (b) {b} of {n}; (c) {c} of {n}; (d) {d} of {n}; bar {bar} of {n} each: {}",
            if b >= bar && c >= bar && d >= bar { "story 3 passes" } else { "story 3 FAILS" }
        );
    }
}
