//! How the markets economy settles: each year, people, starved, and per good the world's stores,
//! workers and median price; then the speed.
//!
//! cargo run --release -p sim --example market_stats -- [--n FACE] [--provinces N] [--seed S] [--years Y] [--food]
//!
//! `--food` runs Economics v0 instead, for comparison.

use sim::config::Config;
use sim::fields::quantity_total;
use sim::fields::Quantity;
use sim::markets::PRICE_SCALE;
use sim::module::Slot;
use sim::sim::Sim;
use sim::world::WorldParams;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str, default: u64| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map_or(default, |v| v.parse().expect("a number"));
    let params = WorldParams { n: get("--n", 48) as usize, provinces: get("--provinces", 200) as usize, seed: get("--seed", 1), land_fraction: 0.36, radius_km: 6371.0 };
    let mut config = Config::new(params);
    if !args.iter().any(|a| a == "--food") {
        config.modules.set_choice(Slot::Economics, "markets").unwrap();
    }
    let started = Instant::now();
    let mut sim = Sim::new(&config).unwrap();
    println!("made in {:.2} s: {} provinces", started.elapsed().as_secs_f64(), sim.world.provinces.count);
    let goods = sim.world.goods.clone();
    let names = goods.names();
    let row = |sim: &Sim| {
        let w = &sim.world;
        let k = goods.stride();
        let mut out = String::new();
        for (c, name) in names.iter().enumerate() {
            let stock: u64 = (0..w.provinces.count).map(|p| w.stock[p * k + c]).sum();
            let workers: u64 = (0..w.provinces.count).map(|p| w.workers.get(p * k + c).copied().unwrap_or(0)).sum();
            let mut prices: Vec<u64> = (0..w.provinces.count).filter_map(|p| w.price.get(p * k + c).copied()).filter(|&x| x > 0).collect();
            prices.sort_unstable();
            let median = prices.get(prices.len() / 2).map_or(0.0, |&x| x as f64 / PRICE_SCALE);
            out += &format!(" {name}={stock}/{workers}w/{median:.3}");
        }
        out
    };
    println!("start: people {}{}", quantity_total(&sim.world, Quantity::People), row(&sim));
    let years = get("--years", 30) as i64;
    let first = sim.calendar.def.first_year;
    let started = Instant::now();
    for y in 1..=years {
        sim.run_to_year(first + y);
        let h = sim.year_so_far();
        println!("year {}: people {} starved {} famine provinces {}{}", h.year, h.population, h.ledger.starved, h.famine_provinces, row(&sim));
    }
    let days = sim.day as f64;
    println!("{:.3} ms a day; problems {:?}", started.elapsed().as_secs_f64() * 1000.0 / days, sim.problems);
}
