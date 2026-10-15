//! Headless run: generate a world, simulate some years, report speed and outcomes, optionally write
//! PNG maps.
//!
//! simrun [--size small|medium|large|huge] [--n FACE] [--provinces N] [--seed S] [--years Y]
//!        [--terrain null|noise] [--climate null|latitude] [--out DIR] [--width PX]
//!        [--save FILE] [--load FILE] [--scenario FILE] [--write-scenario FILE]
//!        [--realms settled=W,city=W,frontier=W]
//!
//! `--load` continues a saved world for `--years` more years (world options are then ignored);
//! `--save` writes the world at the end. `--scenario` opens a scenario (its played timeline) instead,
//! and `--write-scenario` writes the world at the end as one. `--realms` switches realms on, with
//! the ways to start weighted as given (`--realms settled=1` is the default way).

use sim::branch::Timelines;
use sim::config::Config;
use sim::fields::{quantity_total, Quantity};
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let size = match get("--size").as_deref() {
        Some("small") => Size::Small,
        Some("medium") => Size::Medium,
        None | Some("large") => Size::Large,
        Some("huge") => Size::Huge,
        Some(other) => panic!("unknown size {other}"),
    };
    let seed: u64 = get("--seed").map_or(7, |s| s.parse().expect("--seed"));
    let mut params = WorldParams::preset(size, seed);
    if let Some(n) = get("--n") {
        params.n = n.parse().expect("--n");
    }
    if let Some(p) = get("--provinces") {
        params.provinces = p.parse().expect("--provinces");
    }
    let years: i64 = get("--years").map_or(100, |s| s.parse().expect("--years"));
    let out = get("--out").map(PathBuf::from);
    let width: u32 = get("--width").map_or(2048, |s| s.parse().expect("--width"));
    let mut config = Config::new(params.clone());
    if let Some(v) = get("--terrain") {
        config.modules.terrain = v.parse().unwrap_or_else(|e| panic!("{e}"));
    }
    if let Some(v) = get("--climate") {
        config.modules.climate = v.parse().unwrap_or_else(|e| panic!("{e}"));
    }
    if let Some(v) = get("--realms") {
        let mut r = sim::realms::RealmParams { settled: 0.0, ..Default::default() };
        for part in v.split(',') {
            let (way, weight) = part.split_once('=').unwrap_or_else(|| panic!("--realms: {part}: expected way=weight"));
            let weight: f64 = weight.parse().expect("--realms weight");
            match way {
                "settled" => r.settled = weight,
                "city" => r.city_states = weight,
                "frontier" => r.frontier = weight,
                _ => panic!("--realms: no way {way}; expected settled, city or frontier"),
            }
        }
        config.modules.realms = Some(r);
    }

    let t = Instant::now();
    let mut sim = match (get("--load"), get("--scenario")) {
        (Some(file), _) => {
            let sim = Sim::load_from(file.as_ref()).unwrap_or_else(|e| panic!("{e}"));
            println!("loaded {file} at {}", sim.calendar.format(sim.day));
            sim
        }
        (None, Some(file)) => {
            let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{file}: {e}"));
            let mut t = Timelines::from_scenario_text(&text).unwrap_or_else(|e| panic!("{file}: {e}"));
            let sim = t.list.swap_remove(t.active).sim;
            println!("opened scenario {file} at {}", sim.calendar.format(sim.day));
            sim
        }
        (None, None) => {
            println!("world: {size:?}, face {} ({} cells), target {} provinces, seed {}", params.n, 6 * params.n * params.n, params.provinces, seed);
            Sim::new(&config).unwrap_or_else(|e| panic!("{e}"))
        }
    };
    let gen_s = t.elapsed().as_secs_f64();
    let versions: Vec<String> = sim.manifests().iter().map(|m| format!("{:?} {}", m.slot, m.version)).collect();
    println!("modules: {}", versions.join(", "));
    let w = &sim.world;
    let land = w.elevation.iter().filter(|&&h| h >= 0.0).count();
    let rivers = w.river_flow.iter().zip(&w.elevation).filter(|(f, h)| **f >= sim::world::RIVER_MIN_FLOW && **h >= 0.0).count();
    println!(
        "ready in {gen_s:.2} s: {:.0}% land, {} provinces, {} river cells, {} people",
        100.0 * land as f64 / w.grid.cells() as f64,
        w.provinces.count,
        rivers,
        group(quantity_total(w, Quantity::People))
    );
    print_realms(&sim);
    if let Some(dir) = &out {
        let t = Instant::now();
        let files = sim::export::write_all(&sim, dir, "start_", width).expect("write PNGs");
        println!("wrote {} maps in {:.2} s to {}", files.len(), t.elapsed().as_secs_f64(), dir.display());
    }

    // Time every day, split into days when something ran and idle days.
    let end = sim.calendar.year_start(sim.calendar.date(sim.day).year + years);
    let (mut busy, mut idle) = ((0u64, 0f64), (0u64, 0f64));
    let run_start = Instant::now();
    while sim.day < end {
        let t = Instant::now();
        let ran = sim.step_day();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let slot = if ran { &mut busy } else { &mut idle };
        slot.0 += 1;
        slot.1 += ms;
    }
    let total_s = run_start.elapsed().as_secs_f64();
    let days = busy.0 + idle.0;

    println!("\nyear  population      starved   famine provinces  food stock");
    let mut rows = sim.history.clone();
    rows.push(sim.year_so_far());
    let stride = (rows.len() / 10).max(1);
    for (k, y) in rows.iter().enumerate() {
        if k.is_multiple_of(stride) || k + 1 == rows.len() {
            println!(
                "{:>4}  {:>13}  {:>11}  {:>16}  {:>10}",
                y.year,
                group(y.population),
                group(y.ledger.starved),
                y.famine_provinces,
                group(y.food_stock)
            );
        }
    }
    println!(
        "\nsimulated {years} years ({days} days) in {total_s:.2} s: {:.4} ms per day on average",
        total_s * 1000.0 / days as f64
    );
    println!("  days when something ran:  {:>6}, {:.4} ms each", busy.0, busy.1 / busy.0.max(1) as f64);
    println!("  idle days:                {:>6}, {:.6} ms each", idle.0, idle.1 / idle.0.max(1) as f64);
    println!("  a century at this rate: {:.1} s", total_s * 100.0 / years as f64);
    println!("fingerprint {:016x}", sim.fingerprint());
    let p = sim.problems;
    println!("contract violations {}, conservation breaks {}", p.contract_violations, p.conservation_breaks);
    if let Some(file) = get("--save") {
        let t = Instant::now();
        sim.save_to(file.as_ref()).expect("write save");
        let mb = std::fs::metadata(&file).map_or(0.0, |m| m.len() as f64 / 1e6);
        println!("saved {file} ({mb:.1} MB) in {:.2} s", t.elapsed().as_secs_f64());
    }
    if let Some(file) = get("--write-scenario") {
        std::fs::write(&file, Timelines::new(sim.fork()).scenario_text()).expect("write scenario");
        println!("wrote scenario {file}");
    }

    print_realms(&sim);
    if let Some(dir) = &out {
        let files = sim::export::write_all(&sim, dir, "end_", width).expect("write PNGs");
        println!("wrote {} maps to {}", files.len(), dir.display());
    }
}

/// How many realms, how big, and how much settled land nobody owns.
fn print_realms(sim: &Sim) {
    let w = &sim.world;
    let alive: Vec<u32> = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect();
    if alive.is_empty() {
        return;
    }
    let people = w.province_population();
    let mut size = vec![0usize; w.realms.count()];
    for &r in w.owner.iter().filter(|&&r| r != sim::grid::NONE) {
        size[r as usize] += 1;
    }
    let mut sizes: Vec<usize> = alive.iter().map(|&r| size[r as usize]).collect();
    sizes.sort_unstable();
    let settled = (0..w.provinces.count).filter(|&p| people[p] > 0).count();
    let unowned = (0..w.provinces.count).filter(|&p| people[p] > 0 && w.owner[p] == sim::grid::NONE).count();
    let names = sim::realms::names(w.params.seed, w.realms.count());
    let biggest = alive.iter().max_by_key(|&&r| (size[r as usize], std::cmp::Reverse(r))).copied().expect("not empty");
    println!(
        "realms: {} (provinces each: smallest {}, middle {}, largest {}); settled provinces nobody owns: {unowned} of {settled}; largest: {}, capital province {}",
        alive.len(),
        sizes[0],
        sizes[sizes.len() / 2],
        sizes[sizes.len() - 1],
        names[biggest as usize],
        w.realms.capital[biggest as usize]
    );
}

/// 1234567 -> "1,234,567".
fn group(v: u64) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
