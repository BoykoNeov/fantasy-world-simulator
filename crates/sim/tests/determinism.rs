//! Same seed, config and (empty) command log give the same world, bit for bit, however many
//! threads do the work.

use sim::config::Config;
use sim::fields::{quantity_total, Quantity};
use sim::sim::Sim;
use sim::world::WorldParams;

fn config(seed: u64) -> Config {
    Config::new(WorldParams { n: 24, provinces: 40, seed, land_fraction: 0.4, radius_km: 6371.0 })
}

fn run(seed: u64, years: i64) -> (u64, Sim) {
    let mut sim = Sim::new(&config(seed)).unwrap();
    sim.run_to_year(1 + years);
    (sim.fingerprint(), sim)
}

fn in_pool<T: Send>(threads: usize, f: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap().install(f)
}

#[test]
fn same_seed_gives_the_same_world() {
    let (a, sim_a) = run(3, 4);
    let (b, sim_b) = run(3, 4);
    assert_eq!(a, b);
    assert_eq!(sim_a.history, sim_b.history);
    assert_eq!(sim_a.history.len(), 3, "a summary for each finished year");
}

#[test]
fn thread_count_does_not_change_the_world() {
    let one = in_pool(1, || run(5, 3).0);
    let many = in_pool(8, || run(5, 3).0);
    assert_eq!(one, many);
}

#[test]
fn different_seeds_give_different_worlds() {
    assert_ne!(run(1, 1).0, run(2, 1).0);
}

#[test]
fn the_world_changes_as_time_passes() {
    let mut sim = Sim::new(&config(9)).unwrap();
    let start = sim.fingerprint();
    let people = quantity_total(&sim.world, Quantity::People);
    sim.run_to_year(6);
    assert_ne!(sim.fingerprint(), start);
    assert_ne!(quantity_total(&sim.world, Quantity::People), people);
    assert_eq!(sim.day, 5 * sim.calendar.days_per_year() as u64);
}
