//! Reshaping provinces: splits, merges and redrawn borders keep every person and ration, share
//! them by the farmland that moved, keep province numbers, and can be taken back exactly.

use sim::config::Config;
use sim::edit::Edit;
use sim::fields::{quantity_total, Quantity};
use sim::grid::NONE;
use sim::sim::{CommandKind, Sim, Stage};
use sim::world::WorldParams;

fn world(seed: u64) -> Sim {
    let mut s = Sim::new(&Config::new(WorldParams { n: 32, provinces: 60, seed, land_fraction: 0.4, radius_km: 6371.0 })).unwrap();
    s.strict = true;
    s
}

fn people_in(sim: &Sim, p: u32) -> u64 {
    sim.world.province_population()[p as usize]
}

/// The province with the most people, and half its cells (the half with the lower numbers).
fn half_of_biggest(sim: &Sim) -> (u32, Vec<u32>) {
    let people = sim.world.province_population();
    let p = (0..people.len()).max_by_key(|&i| people[i]).unwrap() as u32;
    let cells: Vec<u32> = (0..sim.world.grid.cells() as u32).filter(|&c| sim.world.provinces.of_cell[c as usize] == p).collect();
    (p, cells[..cells.len() / 2].to_vec())
}

fn totals(sim: &Sim) -> (u64, u64) {
    (quantity_total(&sim.world, Quantity::People), quantity_total(&sim.world, Quantity::Food))
}

#[test]
fn a_split_shares_people_and_food_by_farmland_and_keeps_every_one() {
    let mut sim = world(3);
    sim.run_days(400);
    let (p, half) = half_of_biggest(&sim);
    let count = sim.world.provinces.count;
    let (people, food) = (people_in(&sim, p), sim.world.food_stock[p as usize]);
    let capacity = sim.world.provinces.food_capacity[p as usize];
    let before = totals(&sim);

    sim.edit(Edit::Split { cells: half.clone() }).unwrap();
    let new = count as u32;
    assert_eq!(sim.world.provinces.count, count + 1);
    assert!(half.iter().all(|&c| sim.world.provinces.of_cell[c as usize] == new));
    assert_eq!(totals(&sim), before, "a split makes and loses nobody");
    assert_eq!(people_in(&sim, p) + people_in(&sim, new), people);
    assert_eq!(sim.world.food_stock[p as usize] + sim.world.food_stock[new as usize], food);
    // The new province took about its share of the farmland's people (to within rounding).
    let share = sim.world.provinces.food_capacity[new as usize] as f64 / capacity as f64;
    let took = people_in(&sim, new) as f64 / people as f64;
    assert!((took - share).abs() < 0.01, "took {took:.3} of the people for {share:.3} of the farmland");
    // Every running province value has an entry for it; it can be run, saved and loaded.
    assert_eq!(sim.world.food_stock.len(), count + 1);
    assert_eq!(sim.world.rain_shortfall.len(), count + 1);
    sim.run_days(400);
    let loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    assert_eq!(sim.problems, Default::default());
}

#[test]
fn a_merge_leaves_an_empty_placeholder_and_a_border_can_be_redrawn() {
    let mut sim = world(5);
    sim.run_days(200);
    let (p, half) = half_of_biggest(&sim);
    // A neighbour of `p`: the province on the other side of one of its borders.
    let g = &sim.world.grid;
    let of = &sim.world.provinces.of_cell;
    let q = (0..g.cells()).filter(|&c| of[c] == p).flat_map(|c| g.neighbors[c].iter().map(|&m| of[m as usize])).find(|&o| o != p && o != NONE).unwrap();
    let before = totals(&sim);
    let (pp, qp) = (people_in(&sim, p), people_in(&sim, q));
    let count = sim.world.provinces.count;

    sim.edit(Edit::Merge { from: q, into: p }).unwrap();
    assert_eq!(sim.world.provinces.count, count, "numbers don't change");
    assert_eq!(sim.world.provinces.cells[q as usize], 0);
    assert_eq!((people_in(&sim, q), sim.world.food_stock[q as usize]), (0, 0));
    assert_eq!(people_in(&sim, p), pp + qp);
    assert!(sim.world.provinces.center[q as usize].iter().all(|x| x.is_finite()), "the placeholder keeps a centre");
    assert!(sim.edit(Edit::Merge { from: q, into: p }).is_err(), "nothing left to merge");

    // Redraw: half of p's old land goes back to q.
    sim.edit(Edit::MoveCells { cells: half.clone(), to: q }).unwrap();
    assert!(people_in(&sim, q) > 0);
    assert_eq!(totals(&sim), before);
    assert!(sim.edit(Edit::MoveCells { cells: half, to: q }).is_err(), "already there");
    sim.run_days(500);
    assert_eq!(sim.problems, Default::default());
}

#[test]
fn province_edits_can_be_taken_back_replayed_and_explained() {
    let mut plain = world(8);
    let mut edited = world(8);
    let day = plain.calendar.year_start(1) + 90;
    plain.run_days(day);
    edited.run_days(day);
    let (p, half) = half_of_biggest(&edited);
    edited.edit(Edit::Split { cells: half }).unwrap();
    edited.run_days(200);
    let fp = edited.fingerprint();

    // Going back past the split and forward makes the same world.
    edited.rewind(day - 30).unwrap();
    assert_eq!(edited.world.provinces.count, plain.world.provinces.count, "back before the split");
    edited.run_days(230);
    assert_eq!(edited.fingerprint(), fp);

    // Explaining the day names the edit and changes nothing.
    let t = edited.trace(day, p).unwrap();
    assert!(t.steps.iter().any(|s| s.stage == Stage::Edit), "{:#?}", t.steps);
    assert_eq!(edited.fingerprint(), fp);

    // Taking it back gives the world that never had it, map included.
    let split = edited.commands.iter().position(|c| matches!(c.kind, CommandKind::Edit { .. })).unwrap();
    edited.withdraw(split).unwrap();
    plain.run_days(200);
    assert_eq!(edited.fingerprint(), plain.fingerprint());
    assert_eq!(edited.world.provinces.of_cell, plain.world.provinces.of_cell);
    assert_eq!(edited.problems, Default::default());
}
