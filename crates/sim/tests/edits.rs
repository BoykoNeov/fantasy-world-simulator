//! Edits: they take effect at once, replays and saves repeat them, and taking one back gives
//! exactly the world that never had it.

use sim::config::Config;
use sim::edit::{Edit, PopValues};
use sim::sim::{CommandKind, Sim, Stage, Traced};
use sim::world::WorldParams;

fn world(seed: u64) -> Sim {
    let mut s = Sim::new(&Config::new(WorldParams { n: 24, provinces: 40, seed, land_fraction: 0.4, radius_km: 6371.0 })).unwrap();
    s.strict = true;
    s
}

/// The most populous pop and its province.
fn big_pop(sim: &Sim) -> (u32, u32) {
    let k = (0..sim.world.pops.size.len()).max_by_key(|&k| sim.world.pops.size[k]).unwrap();
    (k as u32, sim.world.pops.province[k])
}

#[test]
fn an_edit_shows_at_once_and_balances_the_year() {
    let mut sim = world(4);
    sim.run_days(500);
    let (k, p) = big_pop(&sim);
    let size = sim.world.pops.size[k as usize];
    sim.edit(Edit::SetPop { pop: k, to: PopValues { culture: 3, ..PopValues::people(size + 12_345) } }).unwrap();
    assert_eq!(sim.world.pops.size[k as usize], size + 12_345);
    assert_eq!(sim.world.pops.culture[k as usize], 3);
    sim.edit(Edit::AddPop { province: p, values: PopValues::people(777) }).unwrap();
    sim.edit(Edit::SetFoodStock { province: p, rations: 5 }).unwrap();
    assert_eq!(*sim.world.pops.size.last().unwrap(), 777);
    assert_eq!(sim.world.stock[p as usize], 5);
    assert!(sim.edit(Edit::SetPop { pop: 99_999, to: PopValues::people(1) }).is_err());
    assert!(sim.edit(Edit::AddPop { province: 99_999, values: PopValues::people(1) }).is_err());
    assert_eq!(sim.commands.iter().filter(|c| matches!(c.kind, CommandKind::Edit { .. })).count(), 3);

    // The year still balances: the edit is a declared source.
    let year = sim.calendar.date(sim.day).year;
    // The year's summary is made on the next year's first day.
    sim.run_to_year(year + 1);
    sim.step_day();
    let y = sim.history.last().unwrap();
    assert_eq!(y.year, year);
    let before = sim.history[sim.history.len() - 2].population;
    let l = &y.ledger;
    assert_eq!(l.people_edited, 12_345 + 777);
    assert_eq!(y.population as i64, before as i64 + l.births as i64 - l.deaths as i64 + l.people_clamped + l.people_edited + l.people_on_the_way);
    assert_eq!(sim.problems, Default::default());
}

#[test]
fn taking_an_edit_back_gives_the_world_that_never_had_it() {
    let mut plain = world(6);
    let mut edited = world(6);
    let day = edited.calendar.year_start(2) + 40;
    plain.run_days(day);
    edited.run_days(day);
    let (k, p) = big_pop(&edited);
    edited.edit(Edit::SetPop { pop: k, to: PopValues::people(3) }).unwrap();
    edited.edit(Edit::AddPop { province: p, values: PopValues::people(50_000) }).unwrap();
    edited.run_days(400);
    plain.run_days(400);
    assert_ne!(edited.fingerprint(), plain.fingerprint());

    // Take both back, the later one first, while the world is far past them.
    let index = |s: &Sim| s.commands.iter().rposition(|c| matches!(c.kind, CommandKind::Edit { .. })).unwrap();
    let second = edited.withdraw(index(&edited)).unwrap();
    let first = edited.withdraw(index(&edited)).unwrap();
    assert_eq!(edited.day, plain.day);
    assert_eq!(edited.fingerprint(), plain.fingerprint());
    assert_eq!(edited.history, plain.history);
    assert_eq!(edited.commands, plain.commands);

    // Giving them again gives the edited world again; going on, it is a world like any other.
    edited.reinstate(first).unwrap();
    edited.reinstate(second).unwrap();
    let mut again = world(6);
    again.run_days(day);
    again.edit(Edit::SetPop { pop: k, to: PopValues::people(3) }).unwrap();
    again.edit(Edit::AddPop { province: p, values: PopValues::people(50_000) }).unwrap();
    again.run_days(400);
    assert_eq!(edited.fingerprint(), again.fingerprint());
    assert!(edited.skipped_edits.is_empty());
    assert_eq!(edited.problems, Default::default());
}

#[test]
fn an_edit_that_relied_on_one_taken_back_is_left_out() {
    let mut sim = world(9);
    sim.run_days(200);
    let (_, p) = big_pop(&sim);
    sim.edit(Edit::AddPop { province: p, values: PopValues::people(1000) }).unwrap();
    let new_pop = sim.world.pops.size.len() as u32 - 1;
    sim.run_days(30);
    sim.edit(Edit::SetPop { pop: new_pop, to: PopValues::people(2000) }).unwrap();
    sim.run_days(30);
    let add = sim.commands.iter().position(|c| matches!(c.kind, CommandKind::Edit { edit: Edit::AddPop { .. } })).unwrap();
    sim.withdraw(add).unwrap();
    assert_eq!(sim.skipped_edits.len(), 1, "{:?}", sim.skipped_edits);
    assert!(sim.skipped_edits[0].1.contains("no pop"), "{:?}", sim.skipped_edits);
    assert_eq!(sim.world.pops.size.len() as u32, new_pop);
}

#[test]
fn rewinds_saves_and_traces_repeat_edits() {
    let mut a = world(3);
    a.run_to_year(2);
    let edit_day = a.day + 17;
    a.run_days(17);
    let (k, p) = big_pop(&a);
    a.edit(Edit::SetPop { pop: k, to: PopValues::people(10) }).unwrap();
    a.run_days(100);
    let fp = a.fingerprint();

    // Going back past the edit and forward again repeats it.
    a.rewind(edit_day - 5).unwrap();
    a.run_days(105);
    assert_eq!(a.fingerprint(), fp);

    // A save keeps it; the loaded world goes on the same.
    let mut b = Sim::load(&a.save()).unwrap();
    assert_eq!(b.fingerprint(), fp);
    a.run_days(300);
    b.run_days(300);
    assert_eq!(a.fingerprint(), b.fingerprint());

    // Explaining the edit's day names it and changes nothing.
    let fp = a.fingerprint();
    let t = a.trace(edit_day, p).unwrap();
    assert_eq!(a.fingerprint(), fp);
    let note = t.steps.iter().find(|s| s.stage == Stage::Edit).expect("the edit is in the trace");
    assert!(matches!(&note.what, Traced::Note(n) if n.contains("set to 10 people")), "{note:?}");
    assert!(t.steps.iter().any(|s| s.stage == Stage::Edit && matches!(s.what, Traced::Changed { entity, .. } if entity == k)));
    assert!(a.last_change[sim::fields::known("pop.size").0 as usize][k as usize] >= edit_day);
}
