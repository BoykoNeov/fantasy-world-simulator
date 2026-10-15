//! Scenario files: every timeline written as readable text and made again exactly from it, and
//! hand edits to the text take effect.

use sim::branch::Timelines;
use sim::config::Config;
use sim::edit::{Brush, Edit, Override, PopValues};
use sim::events::Event;
use sim::grid::dir_to_lonlat;
use sim::sim::Sim;
use sim::world::WorldParams;

fn world() -> Sim {
    Sim::new(&Config::new(WorldParams { n: 24, provinces: 40, seed: 21, land_fraction: 0.4, radius_km: 6371.0 })).unwrap()
}

/// A world with an edit of every kind, a poke, a poke scheduled for later, and a second timeline.
fn busy() -> Timelines {
    let mut t = Timelines::new(world());
    let s = t.sim_mut();
    let (lon, lat) = dir_to_lonlat(s.world.provinces.center[0]);
    let here = [lon.to_degrees(), lat.to_degrees()];
    s.edit(Edit::SetFoodStock { province: 0, rations: 7 }).unwrap();
    s.run_days(100);
    s.edit(Edit::SetPop { pop: 1, to: PopValues::people(4321) }).unwrap();
    s.edit(Edit::AddPop { province: 2, values: PopValues::people(99) }).unwrap();
    let cells: Vec<u32> = (0..s.world.grid.cells() as u32).filter(|&c| s.world.provinces.of_cell[c as usize] == 3).collect();
    s.edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    s.edit(Edit::Merge { from: 4, into: 5 }).unwrap();
    s.edit(Edit::Paint { path: vec![here, [here[0] + 3.0, here[1]]], radius_km: 600.0, brush: Brush::Roughen(900.0) }).unwrap();
    s.edit(Edit::Override { path: vec![here], radius_km: 500.0, set: Override::Rainfall(2500.0) }).unwrap();
    s.run_days(50);
    s.edit(Edit::SeaLevel { rise_m: -20.0 }).unwrap();
    s.edit(Edit::River { path: vec![here, [here[0] + 10.0, here[1] + 5.0]], spring: 30_000_000 }).unwrap();
    let drought = Event::Drought { lon: here[0], lat: here[1], radius_km: 2000.0, shortfall: 600, days: 300 };
    s.poke(drought.clone());
    s.schedule(700, drought).unwrap();
    s.run_days(400);
    t.branch(120).unwrap();
    t.sim_mut().edit(Edit::SetFoodStock { province: 6, rations: 12345 }).unwrap();
    t.sim_mut().run_days(30);
    t
}

#[test]
fn every_timeline_is_made_again_exactly_from_its_scenario() {
    let t = busy();
    let text = t.scenario_text();
    assert!(text.contains("Roughen") && text.contains("Drought") && text.contains("seed = 21"), "{text}");
    let back = Timelines::from_scenario_text(&text).unwrap();
    assert_eq!(back.list.len(), 2);
    for (a, b) in t.list.iter().zip(&back.list) {
        assert_eq!((a.id, a.parent, a.sim.day), (b.id, b.parent, b.sim.day));
        assert_eq!(a.sim.fingerprint(), b.sim.fingerprint(), "timeline {}", a.id);
        assert_eq!(a.sim.commands, b.sim.commands);
    }
    assert_eq!(back.active_id(), t.active_id());
    // Written again, it is the same text.
    assert_eq!(back.scenario_text(), text);
}

#[test]
fn hand_edits_to_a_scenario_take_effect_and_mistakes_are_refused() {
    let t = Timelines::new(world());
    let mut s = t.to_scenario();
    s.timeline[0].day = 30;
    let text = toml::to_string(&s).unwrap();
    // Add an edit by hand: a food store set on day 10.
    assert!(text.contains("command = []"), "{text}");
    let by_hand = text.replace("command = []", "command = [{ day = 10, edit = { SetFoodStock = { province = 3, rations = 555 } } }]");
    let loaded = Timelines::from_scenario_text(&by_hand).unwrap();
    let mut expect = world();
    expect.run_days(10);
    expect.edit(Edit::SetFoodStock { province: 3, rations: 555 }).unwrap();
    expect.run_days(20);
    assert_eq!(loaded.sim().fingerprint(), expect.fingerprint());

    assert!(Timelines::from_scenario_text("format = 99").err().unwrap().contains("cannot read"));
    let mut wrong = t.to_scenario();
    wrong.format = 99;
    assert!(Timelines::from_scenario(&wrong).err().unwrap().contains("format 99"));
    let neither = text.replace("command = []", "command = [{ day = 10 }]");
    assert!(Timelines::from_scenario_text(&neither).err().unwrap().contains("needs one edit or one poke"));
    let mut lost = t.to_scenario();
    lost.active = 7;
    assert!(Timelines::from_scenario(&lost).err().unwrap().contains("no timeline 7"));
}

#[test]
fn expectations_hold_fail_with_the_value_and_are_written_again() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scenarios/drought_famine.toml")).unwrap();
    let mut t = Timelines::from_scenario_text(&text).unwrap();
    assert_eq!(t.list[1].expect.len(), 4);
    // Written again, the expectations are kept.
    let again = Timelines::from_scenario_text(&t.scenario_text()).unwrap();
    assert_eq!(again.list.iter().map(|t| t.expect.clone()).collect::<Vec<_>>(), t.list.iter().map(|t| t.expect.clone()).collect::<Vec<_>>());
    assert_eq!(t.check_expectations(), Vec::new());

    // The drought timeline's year-4 famine, asked to be far bigger than it was.
    let wrong = text.replace("min = 2_500_000", "min = 25_000_000");
    let failed = Timelines::from_scenario_text(&wrong).unwrap().check_expectations();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].0, 1);
    assert!(failed[0].1.starts_with("year 4: Starved was ") && failed[0].1.ends_with("expected at least 25000000"), "{}", failed[0].1);
    // A year that is never closed can't be checked.
    let mut never = Timelines::from_scenario_text(&text).unwrap();
    never.list[0].expect[0].year = 0;
    assert!(never.check_expectations()[0].1.contains("year 0 was never reached"));
}
