//! Timelines: a new one leaves the old at a past day without the old one's later commands, the old
//! one is kept exactly, and both can be played, switched between, saved and loaded.

use sim::branch::Timelines;
use sim::config::Config;
use sim::edit::{Edit, PopValues};
use sim::events::Event;
use sim::sim::{CommandKind, Sim};
use sim::world::WorldParams;

fn world() -> Sim {
    let mut s = Sim::new(&Config::new(WorldParams { n: 24, provinces: 40, seed: 12, land_fraction: 0.4, radius_km: 6371.0 })).unwrap();
    s.strict = true;
    s
}

fn drought(sim: &Sim) -> Event {
    let (lon, lat) = sim::grid::dir_to_lonlat(sim.world.provinces.center[0]);
    Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 3000.0, shortfall: 700, days: 400 }
}

fn edits(sim: &Sim) -> usize {
    sim.commands.iter().filter(|c| matches!(c.kind, CommandKind::Edit { .. })).count()
}

#[test]
fn a_copy_is_exact() {
    let mut a = world();
    a.run_days(500);
    a.edit(Edit::SetFoodStock { province: 3, rations: 1 }).unwrap();
    a.run_days(10);
    let mut b = a.fork();
    assert_eq!(a.fingerprint(), b.fingerprint());
    a.run_days(400);
    b.run_days(400);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn a_new_timeline_leaves_the_old_one_without_its_later_commands() {
    let mut t = Timelines::new(world());
    let fork = 400;
    // The old timeline: an edit on the fork day, a poke on it, and more later.
    t.sim_mut().run_days(fork);
    t.sim_mut().edit(Edit::AddPop { province: 2, values: PopValues::people(500) }).unwrap();
    let d = drought(t.sim());
    t.sim_mut().poke(d.clone());
    t.sim_mut().run_days(100);
    t.sim_mut().edit(Edit::SetFoodStock { province: 5, rations: 7 }).unwrap();
    t.sim_mut().poke(d.clone());
    t.sim_mut().run_days(300);
    let (old_fp, old_day, old_commands) = (t.sim().fingerprint(), t.sim().day, t.sim().commands.clone());

    let new = t.branch(fork).unwrap();
    assert_eq!(t.active_id(), new);
    let b = t.sim();
    assert_eq!(b.day, fork);
    assert_eq!(edits(b), 1, "the fork day's edit stays: the day shown has it");
    assert!(b.commands.iter().all(|c| !matches!(c.kind, CommandKind::Poke { .. })), "the fork day's poke and everything later go");

    // The new timeline is the world that only ever had that edit.
    let mut plain = world();
    plain.run_days(fork);
    plain.edit(Edit::AddPop { province: 2, values: PopValues::people(500) }).unwrap();
    assert_eq!(t.sim().fingerprint(), plain.fingerprint());
    t.sim_mut().run_days(400);
    plain.run_days(400);
    assert_eq!(t.sim().fingerprint(), plain.fingerprint());
    // It can go back past where it left, too.
    t.sim_mut().rewind(fork - 100).unwrap();

    // The old timeline is just as it was.
    t.switch(0).unwrap();
    assert_eq!((t.sim().fingerprint(), t.sim().day), (old_fp, old_day));
    assert_eq!(t.sim().commands, old_commands);
    assert_eq!(t.list[1].parent, Some((0, fork)));
}

#[test]
fn every_timeline_is_saved_and_loaded() {
    let mut t = Timelines::new(world());
    t.sim_mut().run_days(300);
    t.sim_mut().edit(Edit::SetFoodStock { province: 1, rations: 3 }).unwrap();
    t.sim_mut().run_days(200);
    t.branch(250).unwrap();
    t.sim_mut().edit(Edit::SetFoodStock { province: 1, rations: 99 }).unwrap();
    t.sim_mut().run_days(50);
    let prints: Vec<u64> = t.list.iter().map(|x| x.sim.fingerprint()).collect();

    let loaded = Timelines::load(&t.save()).unwrap();
    assert_eq!(loaded.list.iter().map(|x| x.sim.fingerprint()).collect::<Vec<_>>(), prints);
    assert_eq!(loaded.active, t.active);
    assert_eq!(loaded.list[1].parent, Some((0, 250)));
    assert!(Timelines::load(b"FMAPTIME").is_err());
    assert!(Timelines::load(&t.save()[..100]).is_err());
}
