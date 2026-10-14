//! Province shape statistics for large worlds: how evenly area and farmland are shared out, and
//! how big provinces get. `cargo run --release -p sim --example province_stats`
use sim::world::{Size, World, WorldParams};
fn cv(v: &[f64]) -> f64 { let m = v.iter().sum::<f64>() / v.len() as f64; (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt() / m }
fn main() {
    for seed in [7u64, 8] {
        let t = std::time::Instant::now();
        let w = World::generate(&WorldParams::preset(Size::Large, seed));
        let s = t.elapsed().as_secs_f64();
        let p = &w.provinces;
        let area: Vec<f64> = p.area_km2.clone();
        let food: Vec<f64> = p.food_capacity.iter().map(|&f| f as f64).collect();
        let mut cells: Vec<u32> = p.cells.clone(); cells.sort();
        println!("seed {seed}: {} provinces in {s:.2} s; spread (std/mean) of area {:.2}, of farmland {:.2}; cells per province min {} median {} max {}",
            p.count, cv(&area), cv(&food), cells[0], cells[cells.len()/2], cells[cells.len()-1]);
    }
}
