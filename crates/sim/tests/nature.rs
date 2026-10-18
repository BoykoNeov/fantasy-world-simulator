//! Natural world's settings and its hold on markets, as `tests/settings.rs` checks every other
//! module's settings: changed while the world runs, the module keeps its state; changed on the
//! first day, the same world as one made so; each setting alone at either end of its range makes a
//! world that runs, or is refused. And the markets lines: with nothing taken from the wild, a
//! markets world is the one without the module; half the woodland gone gives timber what half the
//! woodland would; fishers leave a fished-out shore.

use sim::config::Config;
use sim::edit::{Edit, SettingChange};
use sim::nature::{FishParams, ForestParams, NatureParams, WildlifeParams};
use sim::sim::Sim;
use sim::world::{Fingerprint, World, WorldParams};

fn small() -> Config {
    Config::new(WorldParams { n: 24, provinces: 40, seed: 12, land_fraction: 0.4, radius_km: 6371.0 })
}

/// The default modules (realms, rulers, wars, armies, research, culture) under markets, and the
/// natural world with every part.
fn everything() -> Config {
    let mut c = small();
    c.modules.set_choice(sim::module::Slot::Economics, "markets").unwrap();
    c.modules.nature = Some(NatureParams::default());
    c
}

fn sim(c: &Config) -> Sim {
    let mut s = Sim::new(c).unwrap();
    s.strict = true;
    s
}

fn nature_settings(c: &Config) -> Vec<sim::settings::Item> {
    sim::settings::items(c).into_iter().filter(|i| i.key.starts_with("modules.nature")).collect()
}

/// Every field but the natural world's own is the same in both worlds.
fn same_but_the_wild(a: &World, b: &World) {
    for id in sim::fields::FieldId::all() {
        if ["province.woods_lost", "province.fish_lost", "province.game_lost"].contains(&id.def().name) {
            continue;
        }
        let print = |w: &World| {
            let mut h = Fingerprint::default();
            w.field(id).fingerprint(&mut h);
            h.0
        };
        assert_eq!(print(a), print(b), "{}", id.def().name);
    }
}

#[test]
fn nature_keeps_its_state_across_a_change() {
    let c = everything();
    let (mut a, mut b) = (sim(&c), sim(&c));
    a.run_days(800);
    b.run_days(800);
    let changes: Vec<SettingChange> =
        nature_settings(&c).into_iter().filter(|i| i.spec.when == sim::settings::When::Live).map(|i| SettingChange { key: i.key, value: i.value }).collect();
    assert_eq!(changes.len(), 17);
    b.edit(Edit::Settings { changes }).unwrap();
    a.run_days(3 * 365);
    b.run_days(3 * 365);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn a_nature_change_takes_effect_from_its_day() {
    let mut made = everything();
    made.modules.nature.as_mut().unwrap().fish.as_mut().unwrap().catch = 1.5;
    let mut a = sim(&made);
    let mut b = sim(&everything());
    b.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.nature.fish.catch".into(), value: 1.5 }] }).unwrap();
    a.run_days(800);
    b.run_days(800);
    // The world made with it lived in its 200 years with that catch; the changed one didn't, so
    // only the fishing from the first day on is the same: compare a world that wasn't lived in.
    let mut made0 = made.clone();
    made0.modules.nature.as_mut().unwrap().lived_in_years = 0.0;
    let mut plain0 = everything();
    plain0.modules.nature.as_mut().unwrap().lived_in_years = 0.0;
    let mut a0 = sim(&made0);
    let mut b0 = sim(&plain0);
    b0.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.nature.fish.catch".into(), value: 1.5 }] }).unwrap();
    a0.run_days(800);
    b0.run_days(800);
    assert_eq!(a0.fingerprint(), b0.fingerprint());
    // And it changes the world: more fish gone.
    let mut plain = sim(&everything());
    plain.run_days(800);
    assert!(b.world.fish_lost.iter().sum::<u64>() > plain.world.fish_lost.iter().sum::<u64>());
    assert_ne!(a.world.fish_lost, plain.world.fish_lost);
    // Years lived in are set when a world is made.
    assert!(b.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.nature.lived_in_years".into(), value: 3.0 }] }).unwrap_err().contains("when a world is made"));
    // Warned below above back above: refused, the module's own check.
    assert!(b.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.nature.low".into(), value: 0.7 }] }).unwrap_err().contains("back above"));
}

#[test]
fn parts_switched_off_stay_off_through_a_change() {
    let mut c = everything();
    for part in ["forests", "fish", "wildlife"] {
        let key = format!("modules.nature.{part}");
        assert!(sim::settings::section_on(&c, &key));
        sim::settings::set_section(&mut c, &key, false).unwrap();
    }
    sim::settings::set(&mut c, "modules.nature.crowd_full", 2.0).unwrap();
    let n = c.modules.nature.as_ref().unwrap();
    assert!(n.forests.is_none() && n.fish.is_none() && n.wildlife.is_none());
    let d = sim::settings::defaults_for(&c).modules.nature.unwrap();
    assert!(d.forests.is_none() && d.fish.is_none() && d.wildlife.is_none());
    sim::settings::set_section(&mut c, "modules.nature.fish", true).unwrap();
    assert_eq!(c.modules.nature.as_ref().unwrap().fish, Some(FishParams::default()));
    // With every part off, nothing is touched.
    let mut off = everything();
    let n = off.modules.nature.as_mut().unwrap();
    (n.forests, n.fish, n.wildlife) = (None, None, None);
    let mut s = sim(&off);
    s.run_days(400);
    assert!(s.world.woods_lost.iter().chain(&s.world.fish_lost).chain(&s.world.game_lost).all(|&x| x == 0));
}

/// Each natural-world setting alone at the ends of its range, on a tiny world with every module on,
/// under each economy: the world runs a year and a month, or a module refuses it.
#[test]
fn every_nature_setting_works_at_the_ends_of_its_range() {
    use rayon::prelude::*;
    let mut cases = Vec::new();
    for economics in ["food", "markets"] {
        let mut base = everything();
        base.modules.set_choice(sim::module::Slot::Economics, economics).unwrap();
        for i in nature_settings(&base) {
            for end in [i.spec.min, i.spec.max] {
                cases.push((economics, i.key.clone(), if i.whole { end.round() } else { end }, base.clone()));
            }
        }
    }
    assert!(cases.len() >= 2 * 2 * 18);
    let failures: Vec<String> = cases
        .into_par_iter()
        .filter_map(|(economics, key, v, mut c)| {
            sim::settings::set(&mut c, &key, v).unwrap_or_else(|e| panic!("{key} = {v}: {e}"));
            let run = std::panic::catch_unwind(move || match Sim::new(&c) {
                Err(_) => Ok(()),
                Ok(mut s) => {
                    s.strict = true;
                    s.run_days(400);
                    if s.problems.contract_violations + s.problems.conservation_breaks > 0 { Err(format!("{:?}", s.problems)) } else { Ok(()) }
                }
            });
            match run {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(format!("{economics} {key} = {v}: {e}")),
                Err(_) => Some(format!("{economics} {key} = {v}: crashed")),
            }
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn with_nothing_taken_from_the_wild_a_markets_world_is_the_one_without_it() {
    // No clearing, felling, fishing or hunting: every measure stays untouched (0), markets skips
    // its lines, and the world (default modules: realms, wars, armies, research, culture) is the
    // world without the module, value for value.
    let mut c = everything();
    c.modules.nature = Some(NatureParams {
        forests: Some(ForestParams { most_cleared: 0.0, felled_km2: 0.0, ..ForestParams::default() }),
        fish: Some(FishParams { catch: 0.0, ..FishParams::default() }),
        wildlife: Some(WildlifeParams { hunting: 0.0, ..WildlifeParams::default() }),
        ..NatureParams::default()
    });
    let mut with = sim(&c);
    c.modules.nature = None;
    let mut without = sim(&c);
    with.run_days(3 * 365);
    without.run_days(3 * 365);
    assert!(with.world.woods_lost.iter().chain(&with.world.fish_lost).chain(&with.world.game_lost).all(|&x| x == 0));
    same_but_the_wild(&with.world, &without.world);
    assert_eq!(with.history, without.history);
    assert!(with.world.realms.count() > 1, "realms and their wars ran");
}

/// A markets world without realms (quicker), the natural world with only its woodland part.
fn woods_world(most_cleared: f64, timber_forest_rate: f64) -> Sim {
    let mut c = small();
    c.modules.set_choice(sim::module::Slot::Economics, "markets").unwrap();
    c.modules.without_realms();
    // Every settled province fully crowded (at a billionth of a person a km²), and cleared at once
    // to the aim: the share cleared is exactly "most cleared" wherever people live, every month.
    c.modules.nature = Some(NatureParams {
        crowd_full: 1e-9,
        forests: Some(ForestParams { most_cleared, clearing: 1.0, felled_km2: 0.0, ..ForestParams::default() }),
        fish: None,
        wildlife: None,
        ..NatureParams::default()
    });
    c.modules.markets.as_mut().unwrap().goods.get_mut("timber").unwrap().land.insert("province.forest_km2".into(), timber_forest_rate);
    sim(&c)
}

#[test]
fn half_the_woodland_gone_gives_timber_what_half_the_woodland_would() {
    // Half the woodland cleared everywhere people live, against the woodland's rate in timber's land
    // formula halved: the same markets, value for value (open land's timber stays in both).
    let mut half = woods_world(0.5, 20.0);
    let mut rate = woods_world(0.0, 10.0);
    let w = &half.world;
    let people = w.province_population();
    let wooded = (0..w.provinces.count).filter(|&i| w.provinces.forest_km2[i] > 0.0 && people[i] > 0).count();
    assert!(wooded > 5);
    assert!((0..w.provinces.count).all(|i| w.woods_lost[i] == if w.provinces.forest_km2[i] > 0.0 && people[i] > 0 { 500_000 } else { 0 }));
    half.run_days(2 * 365);
    rate.run_days(2 * 365);
    same_but_the_wild(&half.world, &rate.world);
    // And it is less timber than with the woods whole.
    let mut whole = woods_world(0.0, 20.0);
    whole.run_days(2 * 365);
    let timber = |s: &Sim| {
        let g = &s.world.goods;
        let t = g.column("timber").unwrap();
        (0..s.world.provinces.count).map(|i| s.world.workers[g.index(i, t)]).sum::<u64>()
    };
    assert_ne!(timber(&half), timber(&whole));
    assert_eq!(half.problems, Default::default());
}

#[test]
fn fishers_leave_a_fished_out_shore() {
    let mut c = small();
    c.modules.set_choice(sim::module::Slot::Economics, "markets").unwrap();
    c.modules.without_realms();
    // Fish that neither breed, swim in nor are caught: they stay where the edit sets them.
    c.modules.nature = Some(NatureParams { forests: None, wildlife: None, fish: Some(FishParams { breeding: 0.0, inflow: 0.0, catch: 0.0, ..FishParams::default() }), ..NatureParams::default() });
    let mut out = sim(&c);
    let mut full = sim(&c);
    let w = &out.world;
    let shores: Vec<u32> = (0..w.provinces.count as u32).filter(|&i| w.provinces.cells[i as usize] > 0 && w.provinces.coast_km[i as usize] + w.provinces.river_km[i as usize] > 0.0).collect();
    out.edit(Edit::SetNature { provinces: shores.clone(), woods: None, fish: Some(0.05), game: None }).unwrap();
    out.run_days(365);
    full.run_days(365);
    assert!(shores.iter().all(|&i| out.world.fish_lost[i as usize] == 950_000));
    let workers = |s: &Sim, good: &str| {
        let g = &s.world.goods;
        let c = g.column(good).unwrap();
        shores.iter().map(|&i| s.world.workers[g.index(i as usize, c)]).sum::<u64>() as f64
    };
    let (fishers, before) = (workers(&out, "fish"), workers(&full, "fish"));
    assert!(fishers < 0.3 * before, "fishers on the fished-out shores {fishers} against {before}");
    assert!(workers(&out, "grain") + workers(&out, "livestock") > workers(&full, "grain") + workers(&full, "livestock"), "they went to other food");
    assert_eq!(out.problems, Default::default());
}
