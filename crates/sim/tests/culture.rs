//! Culture's settings, as `tests/settings.rs` checks every other module's: changed while the world
//! runs, the module keeps its state; changed on the first day, the same world as one made so; and
//! each setting alone at either end of its range makes a world that runs, or is refused.

use sim::config::Config;
use sim::culture::CultureParams;
use sim::diplomacy::{TreatyParams, WarParams};
use sim::edit::{Edit, SettingChange};
use sim::realms::RealmParams;
use sim::sim::Sim;
use sim::world::WorldParams;

/// Every runtime module on, culture with every part.
fn everything() -> Config {
    let mut c = Config::new(WorldParams { n: 24, provinces: 40, seed: 12, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.characters = Some(Default::default());
    c.modules.warfare = Some(Default::default());
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    c.modules.culture = Some(CultureParams { starting: 6, ..CultureParams::default() });
    c
}

fn sim(c: &Config) -> Sim {
    let mut s = Sim::new(c).unwrap();
    s.strict = true;
    s
}

fn culture_settings(c: &Config) -> Vec<sim::settings::Item> {
    sim::settings::items(c).into_iter().filter(|i| i.key.starts_with("modules.culture")).collect()
}

#[test]
fn culture_keeps_its_state_across_a_change() {
    let c = everything();
    let (mut a, mut b) = (sim(&c), sim(&c));
    a.run_days(800);
    b.run_days(800);
    let changes: Vec<SettingChange> =
        culture_settings(&c).into_iter().filter(|i| i.spec.when == sim::settings::When::Live).map(|i| SettingChange { key: i.key, value: i.value }).collect();
    assert_eq!(changes.len(), 10);
    b.edit(Edit::Settings { changes }).unwrap();
    a.run_days(5 * 365);
    b.run_days(5 * 365);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn a_culture_change_takes_effect_from_its_day() {
    let mut made = everything();
    made.modules.culture.as_mut().unwrap().per_year = 0.3;
    let mut a = sim(&made);
    let mut b = sim(&everything());
    b.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.culture.per_year".into(), value: 0.3 }] }).unwrap();
    a.run_days(800);
    b.run_days(800);
    assert_eq!(a.fingerprint(), b.fingerprint());
    let mut plain = sim(&everything());
    plain.run_days(800);
    assert_ne!(plain.fingerprint(), b.fingerprint());
    // Starting settings are set when a world is made.
    assert!(b.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.culture.starting".into(), value: 3.0 }] }).unwrap_err().contains("when a world is made"));
}

#[test]
fn splits_switched_off_stay_off_through_a_change() {
    let mut c = everything();
    sim::settings::set_section(&mut c, "modules.culture.splits", false).unwrap();
    sim::settings::set(&mut c, "modules.culture.per_year", 0.1).unwrap();
    assert!(c.modules.culture.as_ref().unwrap().splits.is_none());
    assert!(sim::settings::defaults_for(&c).modules.culture.unwrap().splits.is_none());
    sim::settings::set_section(&mut c, "modules.culture.splits", true).unwrap();
    assert_eq!(c.modules.culture.as_ref().unwrap().splits, CultureParams::default().splits);
}

/// Each culture setting alone at the ends of its range, on a tiny world with every module on: the
/// world runs (a year and a day, so a year start's splits run) or a module refuses it.
#[test]
fn every_culture_setting_works_at_the_ends_of_its_range() {
    let base = everything();
    let mut failures = Vec::new();
    for i in culture_settings(&base) {
        for end in [i.spec.min, i.spec.max] {
            let v = if i.whole { end.round() } else { end };
            let mut c = base.clone();
            sim::settings::set(&mut c, &i.key, v).unwrap_or_else(|e| panic!("{} = {v}: {e}", i.key));
            let key = i.key.clone();
            let run = std::panic::catch_unwind(move || match Sim::new(&c) {
                Err(_) => Ok(()),
                Ok(mut s) => {
                    s.strict = true;
                    s.run_days(400);
                    if s.problems.contract_violations + s.problems.conservation_breaks > 0 { Err(format!("{:?}", s.problems)) } else { Ok(()) }
                }
            });
            match run {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failures.push(format!("{key} = {v}: {e}")),
                Err(_) => failures.push(format!("{key} = {v}: crashed")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
