//! Painting the map: after a height edit, climate, biomes, rivers and farmland are worked out
//! again, provinces keep their numbers, sunken land's people move, new islands become provinces, and
//! every edit can be taken back to the exact world before it.

use sim::config::Config;
use sim::edit::{Brush, Edit};
use sim::fields::{quantity_total, Quantity};
use sim::grid::{dir_to_lonlat, NONE};
use sim::sim::{CommandKind, Sim};
use sim::world::WorldParams;

fn world(seed: u64) -> Sim {
    let mut s = Sim::new(&Config::new(WorldParams { n: 32, provinces: 60, seed, land_fraction: 0.4, radius_km: 6371.0 })).unwrap();
    s.strict = true;
    s
}

fn totals(sim: &Sim) -> (u64, u64) {
    (quantity_total(&sim.world, Quantity::People), quantity_total(&sim.world, Quantity::Food))
}

/// Longitude and latitude (degrees) of a cell's centre.
fn at(sim: &Sim, c: usize) -> [f64; 2] {
    let (lon, lat) = dir_to_lonlat(sim.world.grid.center[c]);
    [lon.to_degrees(), lat.to_degrees()]
}

/// The province with the most people.
fn biggest(sim: &Sim) -> u32 {
    let people = sim.world.province_population();
    (0..people.len()).max_by_key(|&i| people[i]).unwrap() as u32
}

/// The province's cell nearest its centre.
fn middle(sim: &Sim, p: u32) -> usize {
    let w = &sim.world;
    let c = w.provinces.center[p as usize];
    (0..w.grid.cells()).filter(|&i| w.provinces.of_cell[i] == p).max_by(|&a, &b| sim::grid::dot(w.grid.center[a], c).total_cmp(&sim::grid::dot(w.grid.center[b], c))).unwrap()
}

#[test]
fn a_raised_ridge_changes_the_land_around_it_and_can_be_taken_back() {
    let mut plain = world(2);
    let mut edited = world(2);
    plain.run_days(300);
    edited.run_days(300);
    let p = biggest(&edited);
    let c = middle(&edited, p);
    let (before, height) = (totals(&edited), edited.world.elevation[c]);
    let path = vec![at(&edited, c), at(&edited, edited.world.grid.neighbors[c][0] as usize)];
    edited.edit(Edit::Paint { path, radius_km: 900.0, brush: Brush::Raise(2500.0) }).unwrap();

    assert!(edited.world.elevation[c] > height + 2000.0);
    assert!(edited.world.temperature[c] < plain.world.temperature[c] - 10.0, "higher land is colder");
    assert_ne!(edited.world.biome, plain.world.biome);
    assert_eq!(totals(&edited), before, "painting the land moves nobody when no province is lost");
    assert_eq!(edited.world.provinces.count, plain.world.provinces.count);
    // Farmland follows the new biomes.
    assert_ne!(edited.world.provinces.food_capacity, plain.world.provinces.food_capacity);

    edited.run_days(200);
    plain.run_days(200);
    let edit = edited.commands.iter().position(|c| matches!(c.kind, CommandKind::Edit { .. })).unwrap();
    edited.withdraw(edit).unwrap();
    assert_eq!(edited.fingerprint(), plain.fingerprint(), "the map and everything since is as if never painted");
    assert_eq!(edited.problems, Default::default());
}

#[test]
fn a_sunken_province_sends_its_people_to_the_nearest_land() {
    let mut sim = world(4);
    sim.run_days(100);
    let p = biggest(&sim);
    let before = totals(&sim);
    let cells: Vec<usize> = (0..sim.world.grid.cells()).filter(|&c| sim.world.provinces.of_cell[c] == p).collect();
    // A stroke over every cell of the province, wide enough to drown it all.
    let path: Vec<[f64; 2]> = cells.iter().map(|&c| at(&sim, c)).collect();
    sim.edit(Edit::Paint { path, radius_km: 400.0, brush: Brush::Flatten(-200.0) }).unwrap();

    assert!(cells.iter().all(|&c| sim.world.elevation[c] < 0.0 && sim.world.provinces.of_cell[c] == NONE));
    assert_eq!(sim.world.provinces.cells[p as usize], 0, "the province is an empty placeholder now");
    assert_eq!(sim.world.province_population()[p as usize], 0);
    assert_eq!(totals(&sim), before, "nobody drowned: there was land to go to");
    sim.run_days(400);
    assert_eq!(sim.problems, Default::default());
}

#[test]
fn new_land_joins_a_neighbour_or_becomes_a_province_of_its_own() {
    let mut sim = world(6);
    sim.run_days(50);
    let count = sim.world.provinces.count;
    // The sea cell farthest from any land: an island raised there touches no province.
    let w = &sim.world;
    let land: Vec<usize> = (0..w.grid.cells()).filter(|&c| w.elevation[c] >= 0.0).collect();
    let far = (0..w.grid.cells())
        .filter(|&c| w.elevation[c] < 0.0)
        .min_by(|&a, &b| {
            let near = |c: usize| land.iter().map(|&l| sim::grid::dot(w.grid.center[c], w.grid.center[l])).fold(-1.0, f64::max);
            near(a).total_cmp(&near(b))
        })
        .unwrap();
    sim.edit(Edit::Paint { path: vec![at(&sim, far)], radius_km: 700.0, brush: Brush::Flatten(300.0) }).unwrap();
    assert!(sim.world.elevation[far] > 0.0);
    assert_eq!(sim.world.provinces.of_cell[far], count as u32, "an island of its own");
    assert_eq!(sim.world.provinces.count, count + 1);
    assert_eq!(sim.world.food_stock.len(), count + 1);
    assert_eq!(sim.world.province_population()[count], 0);

    // Raising a coast joins the new land to the province next to it.
    let coast = (0..sim.world.grid.cells())
        .find(|&c| sim.world.elevation[c] < 0.0 && sim.world.grid.neighbors[c].iter().any(|&m| sim.world.provinces.of_cell[m as usize] != NONE && (m as usize) != far))
        .unwrap();
    let neighbours: Vec<u32> = sim.world.grid.neighbors[coast].iter().map(|&m| sim.world.provinces.of_cell[m as usize]).filter(|&p| p != NONE).collect();
    sim.edit(Edit::Paint { path: vec![at(&sim, coast)], radius_km: 60.0, brush: Brush::Flatten(50.0) }).unwrap();
    assert!(neighbours.contains(&sim.world.provinces.of_cell[coast]));

    sim.run_days(300);
    let loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    assert_eq!(sim.problems, Default::default());
}

#[test]
fn a_rising_sea_drowns_coasts_and_replays_the_same() {
    let mut sim = world(8);
    sim.run_days(200);
    let land = |s: &Sim| s.world.elevation.iter().filter(|&&h| h >= 0.0).count();
    let (before, dry) = (totals(&sim), land(&sim));
    sim.edit(Edit::SeaLevel { rise_m: 300.0 }).unwrap();
    assert!(land(&sim) < dry * 9 / 10, "{} of {dry} cells still dry", land(&sim));
    assert_eq!(totals(&sim), before);
    let rivers = sim.world.downstream.clone();
    sim.run_days(400);
    let fp = sim.fingerprint();

    // Back before the rise and forward again: the same world, the map change made again.
    sim.rewind(150).unwrap();
    assert_eq!(land(&sim), dry);
    sim.run_days(450);
    assert_eq!(sim.fingerprint(), fp);
    assert_eq!(sim.world.downstream, rivers);
    assert_eq!(sim.problems, Default::default());
}
