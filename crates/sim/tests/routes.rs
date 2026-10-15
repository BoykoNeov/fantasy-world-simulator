//! Province neighbours stay right however the map changes: through edits, taking them back and
//! giving them again, going back and forward in time, new timelines, and saves.

use sim::config::Config;
use sim::edit::Edit;
use sim::grid::NONE;
use sim::routes::Routes;
use sim::sim::{CommandKind, Sim};
use sim::world::WorldParams;

fn world() -> Sim {
    let mut s = Sim::new(&Config::new(WorldParams { n: 32, provinces: 60, seed: 21, land_fraction: 0.4, radius_km: 6371.0 })).unwrap();
    s.strict = true;
    s
}

/// The neighbours the world has are the ones its map gives now.
#[track_caller]
fn right(sim: &Sim, when: &str) {
    assert_eq!(*sim.world.routes, Routes::build(&sim.world), "{when}");
    assert_eq!(sim.world.routes.seat.len(), sim.world.provinces.count, "{when}");
}

fn cells_of(sim: &Sim, p: u32) -> Vec<u32> {
    (0..sim.world.grid.cells() as u32).filter(|&c| sim.world.provinces.of_cell[c as usize] == p).collect()
}

#[test]
fn neighbours_follow_every_change_of_the_map() {
    let mut sim = world();
    right(&sim, "made");
    let made = sim.world.routes.clone();
    sim.run_days(400);
    assert_eq!(sim.world.routes, made, "running doesn't change the map");
    let start = sim.day;

    // Split the largest province: the new one borders it.
    let w = &sim.world;
    let big = (0..w.provinces.count).max_by_key(|&p| w.provinces.cells[p]).unwrap() as u32;
    let cells = cells_of(&sim, big);
    sim.edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    right(&sim, "split");
    let new = sim.world.provinces.count as u32 - 1;
    assert!(sim.world.routes.between(big, new).is_some_and(|l| l.border_km > 0.0));

    // Merge a neighbour of it away: the placeholder left has no neighbours.
    let gone = sim.world.routes.of(big).find(|l| l.border_km > 0.0 && l.other(big) != new).unwrap().other(big);
    sim.edit(Edit::Merge { from: gone, into: big }).unwrap();
    right(&sim, "merge");
    assert_eq!(sim.world.routes.of(gone).count(), 0);
    assert_eq!(sim.world.routes.seat[gone as usize], NONE);
    sim.run_days(30);

    // Redraw a border with a band of land through a province's seat, which can cut it in two.
    let p = sim.world.routes.of(big).find(|l| l.border_km > 0.0).unwrap().other(big);
    let seat = sim.world.routes.seat[p as usize] as usize;
    let lat = sim.world.grid.lat[seat];
    let band: Vec<u32> = cells_of(&sim, p).into_iter().filter(|&c| (sim.world.grid.lat[c as usize] - lat).abs() < 0.03).collect();
    sim.edit(Edit::MoveCells { cells: band, to: big }).unwrap();
    right(&sim, "redraw");

    // The sea rises: coasts move and some land goes under.
    sim.edit(Edit::SeaLevel { rise_m: 250.0 }).unwrap();
    right(&sim, "sea level");
    sim.run_days(60);
    let edited = sim.world.routes.clone();
    let today = sim.day;

    // Take the last edit back, then give it again.
    let last = sim.commands.iter().rposition(|c| matches!(c.kind, CommandKind::Edit { .. })).unwrap();
    let taken = sim.withdraw(last).unwrap();
    right(&sim, "undo");
    assert_ne!(sim.world.routes, edited);
    sim.reinstate(taken).unwrap();
    right(&sim, "redo");
    assert_eq!(sim.world.routes, edited);

    // Back before every edit (the day before: a day shown has its own edits), and forward again.
    sim.goto(start - 1).unwrap();
    right(&sim, "back");
    assert_eq!(sim.world.routes, made);
    sim.goto(today).unwrap();
    right(&sim, "forward");
    assert_eq!(sim.world.routes, edited);

    // A new timeline from before the edits, and the old one kept.
    let branch = sim.branch(start - 1).unwrap();
    right(&branch, "branch");
    assert_eq!(branch.world.routes, made);
    right(&sim, "after branching");

    // A saved world, loaded.
    let loaded = Sim::load(&sim.save()).unwrap();
    right(&loaded, "load");
    assert_eq!(loaded.world.routes, edited);
}
