//! Settings changed while the world runs: they take effect from their day on, the modules keep
//! their state across the change, and going back, taking the change back, saves and scenario
//! files all give the same world as running straight through.

use sim::branch::Timelines;
use sim::config::Config;
use sim::diplomacy::{TreatyParams, WarParams};
use sim::edit::{Edit, SettingChange};
use sim::markets::MarketParams;
use sim::realms::RealmParams;
use sim::sim::Sim;
use sim::world::WorldParams;

fn small() -> Config {
    Config::new(WorldParams { n: 24, provinces: 40, seed: 12, land_fraction: 0.4, radius_km: 6371.0 })
}

/// Every runtime module on: markets, realms, rulers, armies and diplomacy choosing its wars.
fn everything() -> Config {
    let mut c = small();
    c.modules.economics = None;
    c.modules.markets = Some(MarketParams::default());
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.characters = Some(Default::default());
    c.modules.warfare = Some(Default::default());
    c.modules.research = Some(Default::default());
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    c
}

fn sim(c: &Config) -> Sim {
    let mut s = Sim::new(c).unwrap();
    s.strict = true;
    s
}

fn change(key: &str, value: f64) -> Edit {
    Edit::Settings { changes: vec![SettingChange { key: key.into(), value }] }
}

#[test]
fn every_module_keeps_its_state_across_a_change() {
    // Setting every live setting to the value it has rebuilds every module; the world must run on
    // exactly as if nothing had happened.
    let c = everything();
    let (mut a, mut b) = (sim(&c), sim(&c));
    a.run_days(700);
    b.run_days(700);
    let changes: Vec<SettingChange> = sim::settings::items(&c)
        .into_iter()
        .filter(|i| i.spec.when == sim::settings::When::Live)
        .map(|i| SettingChange { key: i.key, value: i.value })
        .collect();
    assert!(changes.len() > 100, "{}", changes.len());
    b.edit(Edit::Settings { changes }).unwrap();
    a.run_days(800);
    b.run_days(800);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn a_change_takes_effect_from_its_day() {
    // Changed on the first day: the same world as one made with the setting.
    let mut made = small();
    made.modules.population.as_mut().unwrap().births_per_year = 0.05;
    let mut a = sim(&made);
    let mut b = sim(&small());
    b.edit(change("modules.population.births_per_year", 0.05)).unwrap();
    assert_eq!(b.settings.population.as_ref().unwrap().births_per_year, 0.05);
    assert_eq!(b.config.modules.population.as_ref().unwrap().births_per_year, 0.036, "the configuration it was made with stays");
    a.run_days(800);
    b.run_days(800);
    assert_eq!(a.fingerprint(), b.fingerprint());
    // And it changes the world.
    let mut plain = sim(&small());
    plain.run_days(800);
    assert_ne!(plain.fingerprint(), b.fingerprint());
}

#[test]
fn going_back_taking_back_saving_and_scenarios_keep_the_settings_of_each_day() {
    let c = everything();
    let mut s = sim(&c);
    s.run_days(400);
    s.edit(change("modules.markets.trade.carriage.grain", 0.08)).unwrap();
    s.run_days(500);
    let (end, day) = (s.fingerprint(), s.day);

    // A copy (a new timeline starts as one) runs on with the changed settings.
    let mut copy = s.fork();
    let mut ahead = s.fork();
    ahead.goto(day).unwrap();
    copy.run_days(100);
    ahead.run_days(100);
    assert_eq!(copy.fingerprint(), ahead.fingerprint());
    let mut straight = sim(&c);
    straight.run_days(400);
    straight.edit(change("modules.markets.trade.carriage.grain", 0.08)).unwrap();
    straight.run_days(600);
    assert_eq!(copy.fingerprint(), straight.fingerprint());

    // Back before the change: the old settings; forward again: the same world.
    s.goto(200).unwrap();
    assert_eq!(*s.settings, c.modules);
    s.goto(day).unwrap();
    assert_eq!(s.fingerprint(), end);
    assert_eq!(sim::settings::get(&Config { modules: (*s.settings).clone(), ..c.clone() }, "modules.markets.trade.carriage.grain"), Some(0.08));

    // Saved after the change and loaded: it runs on with the changed settings.
    let mut loaded = Sim::load(&s.save()).unwrap();
    assert_eq!(loaded.settings, s.settings);
    s.run_days(300);
    loaded.run_days(300);
    assert_eq!(loaded.fingerprint(), s.fingerprint());

    // A scenario file replays the change on its day.
    let t = Timelines::new(s);
    let again = Timelines::from_scenario_text(&t.scenario_text()).unwrap();
    assert_eq!(again.sim().fingerprint(), t.sim().fingerprint());

    // Taken back: the world that never had it.
    let mut s = t.list.into_iter().next().unwrap().sim;
    let at = s.commands.iter().position(|c| matches!(&c.kind, sim::sim::CommandKind::Edit { edit: Edit::Settings { .. } })).unwrap();
    s.withdraw(at).unwrap();
    let mut never = sim(&c);
    never.run_days(s.day);
    assert_eq!(s.fingerprint(), never.fingerprint());
    assert_eq!(*s.settings, c.modules);
}

#[test]
fn changes_that_dont_fit_are_refused_whole() {
    let c = everything();
    let mut s = sim(&c);
    s.run_days(30);
    let refused = |s: &mut Sim, e: Edit| s.edit(e).unwrap_err();
    assert!(refused(&mut s, change("world.provinces", 10.0)).contains("when a world is made"));
    assert!(refused(&mut s, change("modules.markets.coins_per_person", 5.0)).contains("when a world is made"));
    assert!(refused(&mut s, change("modules.markets.trade.change", 3.0)).contains("outside"));
    assert!(refused(&mut s, change("modules.nothing", 1.0)).contains("not a setting"));
    assert!(refused(&mut s, change("modules.economics.spoilage_per_year", 0.5)).contains("has no"), "food is off: markets runs");
    // The modules' own check: an uprising can't die down above where provinces join it. The good
    // change before it is refused with it.
    let both = Edit::Settings {
        changes: vec![
            SettingChange { key: "modules.realms.taxes.per_month".into(), value: 0.2 },
            SettingChange { key: "modules.realms.unrest.calm".into(), value: 900.0 },
        ],
    };
    assert!(refused(&mut s, both).contains("dies down"));
    assert_eq!(*s.settings, c.modules);
    assert!(s.commands.iter().all(|c| !matches!(c.kind, sim::sim::CommandKind::Edit { .. })));
}

#[test]
fn map_settings_shape_the_world_made() {
    let made = |key: &str, value: f64| {
        let mut c = small();
        sim::settings::set(&mut c, key, value).unwrap();
        Sim::new(&c).unwrap().world
    };
    let plain = Sim::new(&small()).unwrap().world;
    let high = made("modules.terrain_params.mountain_m", 9000.0);
    assert!(high.elevation.iter().cloned().fold(0.0, f32::max) > plain.elevation.iter().cloned().fold(0.0, f32::max) + 3000.0);
    let hot = made("modules.climate_params.equator_c", 40.0);
    assert!(hot.temperature.iter().sum::<f32>() > plain.temperature.iter().sum::<f32>());
    for (key, value) in [("modules.province_params.climb_step", 60.0), ("modules.province_params.roughness", 0.0), ("modules.province_params.river_crossing", 0.0)] {
        assert_ne!(made(key, value).provinces.of_cell, plain.provinces.of_cell, "{key}");
    }
    assert!(made("modules.farmland_params.grassland", 2000.0).food_per_cell.iter().sum::<f64>() > plain.food_per_cell.iter().sum::<f64>());
}

/// Every setting alone at the ends of its range, on a tiny world with every module on: the world is
/// made and runs two months, or a module refuses it with a reason. Nothing may crash, and no value
/// may break its field's contract. (The map detail's top end is left out: it is the world size.)
#[test]
fn every_setting_works_at_the_ends_of_its_range() {
    use rayon::prelude::*;
    let mut base = everything();
    base.modules.economics = Some(Default::default());
    let mut cases = Vec::new();
    for i in sim::settings::items(&base) {
        for end in [i.spec.min, i.spec.max] {
            if i.key == "world.n" && end > 64.0 {
                continue;
            }
            // Food and markets both on is no world; each economy's settings are tried with its own.
            let mut c = base.clone();
            if i.key.starts_with("modules.economics") {
                c.modules.markets = None;
            } else {
                c.modules.economics = None;
            }
            cases.push((i.key.clone(), end, c));
        }
    }
    let failures: Vec<String> = cases
        .into_par_iter()
        .filter_map(|(key, end, mut c)| {
            let v = if sim::settings::items(&c).iter().find(|i| i.key == key)?.whole { end.round() } else { end };
            sim::settings::set(&mut c, &key, v).unwrap_or_else(|e| panic!("{key} = {v}: {e}"));
            let run = std::panic::catch_unwind(move || match Sim::new(&c) {
                Err(_) => Ok(()),
                Ok(mut s) => {
                    s.strict = true;
                    s.run_days(60);
                    if s.problems.contract_violations > 0 { Err(format!("{} contract violations", s.problems.contract_violations)) } else { Ok(()) }
                }
            });
            match run {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(format!("{key} = {v}: {e}")),
                Err(p) => Some(format!("{key} = {v}: crashed: {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
            }
        })
        .collect();
    assert!(failures.is_empty(), "{} failing:\n{}", failures.len(), failures.join("\n"));
}
