//! Headless run: generate a world, simulate some years, report speed and outcomes, optionally write
//! PNG maps.
//!
//! simrun [--size small|medium|large|huge] [--n FACE] [--provinces N] [--seed S] [--years Y]
//!        [--out DIR] [--width PX]

use sim::calendar::Calendar;
use sim::pops::PopParams;
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

    println!("world: {size:?}, face {} ({} cells), target {} provinces, seed {}", params.n, 6 * params.n * params.n, params.provinces, seed);
    let t = Instant::now();
    let mut sim = Sim::new(&params, Calendar::standard(), PopParams::default());
    let gen_s = t.elapsed().as_secs_f64();
    let w = &sim.world;
    let land = w.elevation.iter().filter(|&&h| h >= 0.0).count();
    let rivers = w.river_flow.iter().zip(&w.elevation).filter(|(f, h)| **f >= sim::world::RIVER_MIN_FLOW && **h >= 0.0).count();
    println!(
        "generated in {gen_s:.2} s: {:.0}% land, {} provinces, {} river cells, {} people",
        100.0 * land as f64 / w.grid.cells() as f64,
        w.provinces.count,
        rivers,
        group(sim.pops.total_population())
    );
    if let Some(dir) = &out {
        let t = Instant::now();
        let files = sim::export::write_all(&sim, dir, "start_", width).expect("write PNGs");
        println!("wrote {} maps in {:.2} s to {}", files.len(), t.elapsed().as_secs_f64(), dir.display());
    }

    // Time every day, split into days when something ran and idle days.
    let end = sim.calendar.year_start(sim.calendar.def.first_year + years);
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
    println!("  days with a monthly pass: {:>6}, {:.4} ms each", busy.0, busy.1 / busy.0.max(1) as f64);
    println!("  idle days:                {:>6}, {:.6} ms each", idle.0, idle.1 / idle.0.max(1) as f64);
    println!("  a century at this rate: {:.1} s", total_s * 100.0 / years as f64);
    println!("fingerprint {:016x}", sim.fingerprint());

    if let Some(dir) = &out {
        let files = sim::export::write_all(&sim, dir, "end_", width).expect("write PNGs");
        println!("wrote {} maps to {}", files.len(), dir.display());
    }
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
