//! Logistics' checks (build step 8): Warfare with all five parts of logistics on (supply lines,
//! winter, pay, sieges in depth and armies acting together) against plain "troops".
//!
//! `long` runs story 1's world (200 provinces, every module on, realms choosing their wars) for the
//! years given, once with each, and prints each decade: realms alive, wars begun, provinces that
//! changed hands at a peace, battles and soldiers killed, people starved, the share under arms,
//! soldiers lost to camp disease and winter and deserted, wages paid, coins plundered, food taken
//! by foragers and besiegers, the days provinces were besieged and occupied, the treasuries' share
//! of the coins; and the processor time a day.
//!
//! `story1` runs story 1 in full (as `tests/stories.rs` does) with the five parts on: a drought
//! over the province that grows the most food after three years, then five years watched, with and
//! without the drought, on seeds 21, 3 and 7 and both economies.
//!
//! `parts` runs today's default world (sieges in depth, Research and Culture on) under markets with
//! the parts named on too, and prints one line of totals over the years: wars, provinces handed
//! over, soldiers killed in battle, lost to camp disease and cold and deserted per 1,000
//! soldier-years spent away from their realm's land, wages against taxes, and the processor time a
//! day. `KEY=VALUE` changes a setting.
//!
//! cargo run --release -p sim --example campaigns_check -- long [years] [seed] [food|markets|both]
//! cargo run --release -p sim --example campaigns_check -- parts [years] [seed] [sieges|supply|seasons|pay|massing|all ...] [KEY=VALUE ...]
//! cargo run --release -p sim --example campaigns_check -- story1

use sim::config::Config;
use sim::events::Event;
use sim::fields::{known, Quantity};
use sim::grid::NONE;
use sim::logistics::{PayParams, SeasonParams, SiegeParams, SupplyParams};
use sim::module::Slot;
use sim::requests::Cause;
use sim::sim::Sim;
use sim::world::WorldParams;

fn config(economics: &str, seed: u64, campaigns: bool) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.set_choice(Slot::Warfare, "troops").unwrap();
    if campaigns {
        let t = c.modules.warfare.as_mut().expect("armies are on by default");
        (t.supply, t.seasons, t.pay, t.sieges) = (Some(SupplyParams::default()), Some(SeasonParams::default()), Some(PayParams::default()), Some(SiegeParams::default()));
        t.massing = Some(Default::default());
    }
    c
}

/// Processor time this process has used, all its threads, in ms.
#[cfg(windows)]
fn cpu_ms() -> f64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(h: isize, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: the four pointers are to locals that outlive the call.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    (k + u) as f64 / 10_000.0
}

#[cfg(not(windows))]
fn cpu_ms() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[derive(Default, Clone)]
struct Decade {
    wars: u64,
    handed: u64,
    battles: u64,
    killed: u64,
    starved: u64,
    attrition: u64,
    deserted: u64,
    wages: u64,
    plunder: u64,
    foraged: u64,
    besieged_days: u64,
    occupied_days: u64,
}

fn long(economics: &str, seed: u64, years: i64, campaigns: bool) {
    let mut sim = Sim::new(&config(economics, seed, campaigns)).unwrap();
    let start = sim.calendar.date(0).year;
    let n = sim.world.provinces.count;
    let money = sim.world.goods.tree.find("money");
    let coin_cols = money.and_then(|m| sim.world.goods.columns_under(m)).unwrap_or_default();
    let stock = known("province.stock");
    let mut d = Decade::default();
    let t = cpu_ms();
    println!("{economics}, seed {seed}, {}:", if campaigns { "all five parts" } else { "troops" });
    println!("  year realms wars handed battles killed starved under_arms% attrition deserted wages plunder foraged besieged_days occupied_days treasury%");
    let mut last_attrition = 0i64;
    for year in 1..=years {
        let end = sim.calendar.year_start(start + year);
        while sim.day < end {
            let owners = sim.world.owner.clone();
            sim.step_day();
            let w = &sim.world;
            for e in sim.delivered_today() {
                match &e.event {
                    Event::WarDeclared { .. } => d.wars += 1,
                    Event::Battle { killed, .. } => {
                        d.battles += 1;
                        d.killed += killed;
                    }
                    Event::Famine { starved, .. } => d.starved += starved,
                    Event::Arrived { people, cause: Cause::Deserted, .. } => d.deserted += people,
                    Event::Peace { .. } => {}
                    _ => {}
                }
            }
            if sim.delivered_today().iter().any(|e| matches!(e.event, Event::Peace { .. })) {
                d.handed += (0..n).filter(|&p| owners[p] != NONE && w.owner[p] != NONE && w.owner[p] != owners[p]).count() as u64;
            }
            for (r, granted) in &sim.applied_today {
                match r.cause {
                    Cause::Wages => d.wages += granted,
                    Cause::Plunder => d.plunder += granted,
                    Cause::Foraging if r.field == stock => d.foraged += granted,
                    _ => {}
                }
            }
            d.besieged_days += w.besieger.iter().filter(|&&b| b != NONE).count() as u64;
            d.occupied_days += w.occupier.iter().filter(|&&o| o != NONE).count() as u64;
            let attrition = sim.year_flows.get(Quantity::People, Cause::Attrition);
            if attrition < last_attrition {
                d.attrition += (last_attrition - attrition) as u64;
            }
            last_attrition = attrition;
        }
        last_attrition = 0;
        if year % 10 == 0 || year == years {
            let w = &sim.world;
            let people: u64 = w.province_population().iter().sum();
            let soldiers: u64 = w.armies.size.iter().sum();
            let k = w.goods.stride();
            let coins = |v: &[u64], rows: usize| -> u64 { (0..rows).map(|r| coin_cols.iter().map(|&c| v[r * k + c]).sum::<u64>()).sum() };
            let (held, treasury) = (coins(&w.stock, n), coins(&w.realm_stock, w.realms.count()));
            let alive = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).count();
            println!(
                "  {year:>4} {alive:>6} {:>4} {:>6} {:>7} {:>6} {:>7} {:>10.2} {:>9} {:>8} {:>5} {:>7} {:>7} {:>13} {:>13} {:>9.1}",
                d.wars,
                d.handed,
                d.battles,
                d.killed,
                d.starved,
                100.0 * soldiers as f64 / (people + soldiers).max(1) as f64,
                d.attrition,
                d.deserted,
                d.wages,
                d.plunder,
                d.foraged,
                d.besieged_days,
                d.occupied_days,
                if held + treasury > 0 { 100.0 * treasury as f64 / (held + treasury) as f64 } else { 0.0 },
            );
            d = Decade::default();
        }
    }
    let days = sim.day as f64;
    println!("  {:.3} ms of processor time a day; problems {:?}", (cpu_ms() - t) / days, sim.problems);
}

/// Today's default world (Warfare "campaigns": troops with sieges in depth; Research and Culture
/// on) under markets, with the named parts of logistics on too ("supply", "seasons", "pay",
/// "massing"; "all" for the four), and settings changed (`set`, keys as `simrun --settings` lists
/// them).
fn with_parts(seed: u64, parts: &[&str], set: &[(String, f64)]) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    let t = c.modules.warfare.as_mut().expect("armies are on by default");
    assert!(t.sieges.is_some(), "sieges in depth are on by default");
    let on = |p: &str| parts.iter().any(|&x| x == p || x == "all");
    if on("supply") {
        t.supply = Some(Default::default());
    }
    if on("seasons") {
        t.seasons = Some(Default::default());
    }
    if on("pay") {
        t.pay = Some(Default::default());
    }
    if on("massing") {
        t.massing = Some(Default::default());
    }
    for (k, v) in set {
        sim::settings::set(&mut c, k, *v).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    c
}

/// What each part costs and does over `years`, against today's default (sieges only): one line of
/// totals. Losses are counted against the soldier-years spent away from their realm's own land
/// (standing or marching in a province their realm doesn't own), where camp disease, cold and
/// foraging bite; wages against the taxes realms took in.
fn parts(seed: u64, years: i64, parts: &[&str], set: &[(String, f64)]) {
    let mut sim = Sim::new(&with_parts(seed, parts, set)).unwrap();
    let start = sim.calendar.date(0).year;
    let n = sim.world.provinces.count;
    let stock = known("province.stock");
    let (mut wars, mut handed, mut killed, mut starved, mut attrition, mut deserted, mut wages, mut taxes, mut foraged) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut away_days, mut soldier_days, mut besieged_days) = (0u64, 0u64, 0u64);
    let mut last_attrition = 0i64;
    let mut year_seen = start;
    let end = sim.calendar.year_start(start + years);
    let t = cpu_ms();
    while sim.day < end {
        let owners = sim.world.owner.clone();
        sim.step_day();
        let w = &sim.world;
        for e in sim.delivered_today() {
            match &e.event {
                Event::WarDeclared { .. } => wars += 1,
                Event::Battle { killed: k, .. } => killed += k,
                Event::Famine { starved: s, .. } => starved += s,
                Event::Arrived { people, cause: Cause::Deserted, .. } => deserted += people,
                _ => {}
            }
        }
        if sim.delivered_today().iter().any(|e| matches!(e.event, Event::Peace { .. })) {
            handed += (0..n).filter(|&p| owners[p] != NONE && w.owner[p] != NONE && w.owner[p] != owners[p]).count() as u64;
        }
        for (r, granted) in &sim.applied_today {
            match r.cause {
                Cause::Wages => wages += granted,
                // Taxed from provinces into treasuries, not spent back.
                Cause::Taxes if r.field == stock => taxes += granted,
                Cause::Foraging if r.field == stock => foraged += granted,
                _ => {}
            }
        }
        let arm = &w.armies;
        for a in 0..arm.count() {
            soldier_days += arm.size[a];
            if w.owner.get(arm.province[a] as usize).copied() != Some(arm.realm[a]) {
                away_days += arm.size[a];
            }
        }
        besieged_days += w.besieger.iter().filter(|&&b| b != NONE).count() as u64;
        // The year's flows start again each year.
        let year = sim.calendar.date(sim.day).year;
        if year != year_seen {
            (last_attrition, year_seen) = (0, year);
        }
        let now = sim.year_flows.get(Quantity::People, Cause::Attrition);
        if now < last_attrition {
            attrition += (last_attrition - now) as u64;
        }
        last_attrition = now;
    }
    let dpy = sim.calendar.days_per_year() as f64;
    let (away_years, soldier_years) = (away_days as f64 / dpy, soldier_days as f64 / dpy);
    let per_1000 = |x: u64| 1000.0 * x as f64 / away_years.max(1.0);
    println!(
        "{seed:>3} {:<8} {:?} wars {wars:>4} handed {handed:>4} besieged_days {besieged_days:>6} | soldier-years {soldier_years:>9.0} away {:>4.1}% | per 1,000 soldier-years away: killed in battle {:>6.1}, disease+cold {:>6.1}, deserted {:>6.1} | wages {:>5.1}% of taxes | starved {starved:>9} foraged {foraged:>12} | {:.3} ms a day",
        parts.join("+"),
        set,
        100.0 * away_years / soldier_years.max(1.0),
        per_1000(killed),
        per_1000(attrition),
        per_1000(deserted),
        100.0 * wages as f64 / taxes.max(1) as f64,
        (cpu_ms() - t) / sim.day as f64,
    );
    assert_eq!(sim.problems, Default::default());
}

const DROUGHT_YEARS: u64 = 3;

/// Story 1 in full, as `tests/stories.rs` checks it, with the four parts on: whether, with the
/// drought and in the same world without it, a drought province rose up or its realm declared a
/// war for food.
fn story1(economics: &str, seed: u64) -> (bool, bool) {
    let mut sim = Sim::new(&config(economics, seed, true)).unwrap();
    let start = sim.calendar.date(0).year;
    sim.run_to_year(start + 3);
    let mut twin = sim.fork();
    let (count, heart) = {
        let p = &sim.world.provinces;
        (p.count, (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap())
    };
    let (lon, lat) = sim::grid::dir_to_lonlat(sim.world.provinces.center[heart]);
    sim.poke(Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days: DROUGHT_YEARS * 364 });
    let mut probe = sim.fork();
    probe.run_days(2);
    let region: Vec<u32> = (0..count as u32).filter(|&i| probe.world.rain_shortfall[i as usize] > 0).collect();
    let mut realms: Vec<u32> = region.iter().map(|&q| sim.world.owner[q as usize]).filter(|&r| r != NONE).collect();
    realms.sort_unstable();
    realms.dedup();
    let end = sim.day + 5 * sim.calendar.days_per_year() as u64;
    let watch = |s: &mut Sim| {
        let mut happened = false;
        while s.day < end {
            s.step_day();
            for e in s.delivered_today() {
                happened |= match e.event {
                    Event::Uprising { province, seat, .. } => province == seat && region.contains(&province),
                    Event::WarDeclared { attacker, cause, .. } => cause == sim::diplomacy::WarCause::Food && realms.contains(&attacker),
                    _ => false,
                };
            }
        }
        assert_eq!(s.problems, Default::default(), "{economics}, seed {seed}");
        happened
    };
    (watch(&mut sim), watch(&mut twin))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("parts") => {
            let years: i64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(150);
            let seed: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(21);
            let named: Vec<&str> = args.iter().skip(3).filter(|a| !a.contains('=')).map(|s| s.as_str()).collect();
            let set: Vec<(String, f64)> = args.iter().skip(3).filter_map(|a| a.split_once('=')).map(|(k, v)| (k.to_string(), v.parse().expect("a number"))).collect();
            parts(seed, years, if named.is_empty() { &["sieges"] } else { &named }, &set);
        }
        Some("story1") => {
            for economics in ["food", "markets"] {
                let runs: Vec<(bool, bool)> = [21, 3, 7].into_iter().map(|seed| story1(economics, seed)).collect();
                let with = runs.iter().filter(|r| r.0).count();
                let quiet = runs.iter().filter(|r| !r.1).count();
                println!("story 1 in full, {economics}, all five parts: with the drought {with} of 3, quiet without it {quiet} of 3 ({}): {runs:?}", if with >= 2 && quiet >= 2 { "passes" } else { "fails" });
            }
        }
        _ => {
            let years: i64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(150);
            let seed: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(21);
            let economies: Vec<&str> = match args.get(3).map(|s| s.as_str()) {
                Some("food") => vec!["food"],
                Some("markets") => vec!["markets"],
                _ => vec!["food", "markets"],
            };
            for e in economies {
                for campaigns in [false, true] {
                    long(e, seed, years, campaigns);
                }
            }
        }
    }
}
