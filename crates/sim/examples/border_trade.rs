//! Trade across realms' borders: tolls, war stopping trade, trade warming opinions.
//!
//! `long` runs story 1's world (markets, realms with unrest, rulers, armies, Diplomacy "treaties")
//! for the years given, as it ran before trade across borders changed (no tolls, war stops no
//! trade, trade warms no opinion) and as it runs now, and prints, each decade: tolls collected
//! against taxes; the treasuries' share of the coins their land holds; the share of trade (by worth)
//! that crosses a border, and the trade between realms at war (each end as trade saw it: as the
//! day began, occupied land as its occupier's); the capitals' share of the
//! provinces' coins; and, for provinces among the richest 2% that change hands at a peace, their
//! trade in the year before and the year after.
//!
//! `time` takes a large world under markets with realms (no unrest), rulers, armies and
//! "treaties" after a year, and times each month's first day (the day trade runs) for a year, run
//! twice from the same world: as it is, and with tolls and war stopping trade switched off. Run it
//! with `RAYON_NUM_THREADS=1` to time one worker thread.
//!
//! `cold` runs only the world as it runs now with trade warming no opinion, to tell what the
//! warming does from what tolls and war stopping trade do.
//!
//! cargo run --release -p sim --example border_trade -- [seed] [long|cold|time] [years]

use sim::config::Config;
use sim::diplomacy::{TreatyParams, WarParams};
use sim::events::Event;
use sim::grid::NONE;
use sim::markets::Borders;
use sim::module::Slot;
use sim::realms::RealmParams;
use sim::requests::Cause;
use sim::sim::Sim;
use sim::world::{Size, World, WorldParams};
use std::collections::BTreeMap;

fn config(params: WorldParams, unrest: bool, untolled: bool) -> Config {
    let mut c = Config::new(params);
    c.modules.without_realms();
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    c.modules.realms = Some(if unrest { RealmParams::default() } else { RealmParams { unrest: None, ..RealmParams::default() } });
    c.modules.characters = Some(Default::default());
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    c.modules.warfare = Some(Default::default());
    if untolled {
        let t = c.modules.markets.as_mut().unwrap().trade.as_mut().unwrap();
        (t.toll, t.war_stops) = (0.0, 0.0);
        c.modules.diplomacy.as_mut().unwrap().treaties.as_mut().unwrap().trade_opinion = 0.0;
    }
    c
}

fn coins(w: &World, values: &[u64], row: usize) -> u64 {
    let g = &w.goods;
    g.columns_under(g.tree.find("money").unwrap()).unwrap().iter().map(|&c| values[g.index(row, c)]).sum()
}

#[derive(Default)]
struct Decade {
    tolls: u64,
    taxes: u64,
    /// Trade by worth: all of it, across a border, and between realms at war.
    traded: f64,
    across: f64,
    enemies: f64,
    /// Rich provinces handed over at a peace: (trade the year before, the year after), filled in a
    /// year later.
    handed: Vec<(f64, f64)>,
    wars: usize,
    riches: usize,
    at_capitals: usize,
}

fn long(seed: u64, years: i64, cold: bool) {
    // `cold`: only the world as it runs now but with trade warming no opinion, to tell what the
    // warming does from what tolls and war stopping trade do.
    let runs: &[bool] = if cold { &[false] } else { &[true, false] };
    for &untolled in runs {
        let mut c = config(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 }, true, untolled);
        if cold {
            c.modules.diplomacy.as_mut().unwrap().treaties.as_mut().unwrap().trade_opinion = 0.0;
        }
        let mut sim = Sim::new(&c).unwrap();
        println!("{}:", if untolled { "before (no tolls, war stops no trade, trade warms no opinion)" } else if cold { "now, but trade warms no opinion" } else { "now" });
        let start = sim.calendar.date(0).year;
        let n = sim.world.provinces.count;
        // Each province's trade (worth in and out) for each of the last 12 months.
        let mut months: Vec<Vec<f64>> = Vec::new();
        // Rich provinces handed over: (province, trade the year before, years since).
        let mut watching: Vec<(usize, f64, usize)> = Vec::new();
        let mut d = Decade::default();
        let mut last_trade = 0;
        for year in 1..=years {
            let end = sim.calendar.year_start(start + year);
            while sim.day < end {
                let (owners, rich) = {
                    let w = &sim.world;
                    let worth: Vec<u64> = (0..n).map(|p| coins(w, &w.stock, p)).collect();
                    let mut sorted = worth.clone();
                    sorted.sort_unstable();
                    let line = sorted[n - n / 50 - 1];
                    (w.owner.clone(), worth.iter().map(|&v| v > 0 && v >= line).collect::<Vec<bool>>())
                };
                sim.step_day();
                let w = &sim.world;
                d.taxes += sim.applied_today.iter().filter(|(r, _)| r.cause == Cause::Taxes).map(|(_, g)| g).sum::<u64>();
                if w.trade.day != last_trade && !w.trade.shipments.is_empty() {
                    last_trade = w.trade.day;
                    d.tolls += w.trade.tolls.iter().map(|t| t.1).sum::<u64>();
                    // Each end as trade saw it: as the day began, occupied land as its occupier's.
                    let borders = Borders::read(&w.earlier, n);
                    let mut month = vec![0.0; n];
                    for s in &w.trade.shipments {
                        let (a, b) = (borders.trades_as[s.from as usize], borders.trades_as[s.to as usize]);
                        d.traded += s.coins;
                        month[s.from as usize] += s.coins;
                        month[s.to as usize] += s.coins;
                        if a != NONE && b != NONE && a != b {
                            d.across += s.coins;
                            if borders.at_war(a, b) {
                                d.enemies += s.coins;
                            }
                        }
                    }
                    months.push(month);
                    if months.len() > 12 {
                        months.remove(0);
                    }
                }
                for p in sim.delivered_today() {
                    match p.event {
                        Event::WarDeclared { war, defender, cause, .. } => {
                            d.wars += 1;
                            if cause == sim::diplomacy::WarCause::Riches {
                                d.riches += 1;
                                if w.realms.capital.get(defender as usize) == w.wars.target.get(war as usize) {
                                    d.at_capitals += 1;
                                }
                            }
                        }
                        Event::Peace { .. } => {
                            for q in 0..n {
                                if rich[q] && owners[q] != NONE && w.owner[q] != NONE && w.owner[q] != owners[q] {
                                    let before: f64 = months.iter().map(|m| m[q]).sum();
                                    watching.push((q, before, 0));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            // A year after a hand-over, its trade that year against the year before.
            for (q, before, age) in std::mem::take(&mut watching) {
                if age >= 1 {
                    d.handed.push((before, months.iter().map(|m| m[q]).sum()));
                } else {
                    watching.push((q, before, age + 1));
                }
            }
            if year % 10 == 0 {
                let w = &sim.world;
                let land: u64 = (0..n).filter(|&p| w.owner[p] != NONE).map(|p| coins(w, &w.stock, p)).sum();
                let treasuries: u64 = (0..w.realms.count()).map(|r| coins(w, &w.realm_stock, r)).sum();
                let dead: u64 = (0..w.realms.count()).filter(|&r| !w.realms.alive(r as u32)).map(|r| coins(w, &w.realm_stock, r)).sum();
                let all: u64 = (0..n).map(|p| coins(w, &w.stock, p)).sum();
                let capitals: u64 = (0..w.realms.count()).filter(|&r| w.realms.alive(r as u32)).map(|r| coins(w, &w.stock, w.realms.capital[r] as usize)).sum();
                let alive = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).count();
                let decade: Vec<_> = sim.history.iter().filter(|y| y.year > start + year - 11).collect();
                let starved: u64 = decade.iter().map(|y| y.ledger.starved).sum();
                let people = decade.iter().map(|y| y.population as f64).sum::<f64>() / decade.len().max(1) as f64;
                let (b, a): (f64, f64) = d.handed.iter().fold((0.0, 0.0), |s, h| (s.0 + h.0, s.1 + h.1));
                println!(
                    "  year {year:>3}: {alive} realms, {} wars ({} for riches, {} aimed at a capital), starved {:.2} per 1,000 a year; tolls {} against taxes {} ({:.1}%); treasuries {:.1}% of their land's coins (dead realms {}); trade across borders {:.1}%, between enemies {:.2}%; capitals hold {:.1}% of the coins; rich provinces handed over {} (their trade the year after {:.0}% of the year before)",
                    d.wars,
                    d.riches,
                    d.at_capitals,
                    1000.0 * starved as f64 / people.max(1.0) / decade.len().max(1) as f64,
                    d.tolls,
                    d.taxes,
                    100.0 * d.tolls as f64 / d.taxes.max(1) as f64,
                    100.0 * treasuries as f64 / land.max(1) as f64,
                    dead,
                    100.0 * d.across / d.traded.max(1.0),
                    100.0 * d.enemies / d.traded.max(1.0),
                    100.0 * capitals as f64 / all.max(1) as f64,
                    d.handed.len(),
                    if b > 0.0 { 100.0 * a / b } else { 0.0 },
                );
                d = Decade::default();
            }
        }
        let p = sim.problems;
        println!("  problems: {} contract violations, {} conservation breaks", p.contract_violations, p.conservation_breaks);
    }
}

/// The same day run twice from the same world, once as it runs now and once with tolls and war
/// stopping trade switched off (a settings edit): each month's first day for a year, in turns.
fn time(seed: u64) {
    let mut sim = Sim::new(&config(WorldParams::preset(Size::Large, seed), false, false)).unwrap();
    sim.run_days(sim.calendar.days_per_year() as u64);
    let off = sim::edit::Edit::Settings {
        changes: ["modules.markets.trade.toll", "modules.markets.trade.war_stops"].iter().map(|k| sim::edit::SettingChange { key: k.to_string(), value: 0.0 }).collect(),
    };
    let mut ms = [0.0f64; 2];
    let mut tolls = BTreeMap::new();
    let months = 12;
    for m in 0..months {
        while !sim.calendar.is_month_start(sim.day) {
            sim.step_day();
        }
        for k in [m % 2, 1 - m % 2] {
            let mut s = sim.fork();
            if k == 0 {
                s.edit(off.clone()).unwrap();
            }
            let t = std::time::Instant::now();
            s.step_day();
            ms[k] += t.elapsed().as_secs_f64() * 1000.0;
            if k == 1 {
                for &(r, c) in &s.world.trade.tolls {
                    *tolls.entry(r).or_insert(0u64) += c;
                }
            }
        }
        sim.step_day();
    }
    println!(
        "large world, {} provinces: a month's first day takes {:.1} ms without, {:.1} ms with tolls and war stopping trade (the same days, {months} months averaged); {} realms collected tolls",
        sim.world.provinces.count,
        ms[0] / months as f64,
        ms[1] / months as f64,
        tolls.len()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: u64 = args.first().map_or(21, |s| s.parse().expect("seed"));
    match args.get(1).map_or("long", |s| s.as_str()) {
        "time" => time(seed),
        mode => long(seed, args.get(2).map_or(150, |s| s.parse().expect("years")), mode == "cold"),
    }
}
