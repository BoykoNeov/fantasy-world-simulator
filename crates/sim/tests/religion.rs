//! Religion's settings, as `tests/settings.rs` checks every other module's: changed while the world
//! runs, the module keeps its state; changed on the first day, the same world as one made so; and
//! each setting alone at either end of its range makes a world that runs, or is refused.

use sim::config::Config;
use sim::culture::CultureParams;
use sim::diplomacy::{TreatyParams, WarParams};
use sim::edit::{Edit, SettingChange};
use sim::realms::RealmParams;
use sim::religion::{ReligionParams, SchismParams};
use sim::sim::Sim;
use sim::world::WorldParams;

/// Every runtime module on, culture and religion with every part.
fn everything() -> Config {
    let mut c = Config::new(WorldParams { n: 24, provinces: 40, seed: 12, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.characters = Some(Default::default());
    c.modules.warfare = Some(Default::default());
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    c.modules.culture = Some(CultureParams { starting: 6, ..CultureParams::default() });
    c.modules.religion = Some(ReligionParams { starting: 4, ..ReligionParams::default() });
    c
}

fn sim(c: &Config) -> Sim {
    let mut s = Sim::new(c).unwrap();
    s.strict = true;
    s
}

fn religion_settings(c: &Config) -> Vec<sim::settings::Item> {
    sim::settings::items(c).into_iter().filter(|i| i.key.starts_with("modules.religion")).collect()
}

#[test]
fn religion_keeps_its_state_across_a_change() {
    let mut c = everything();
    // Schisms soon enough to happen in the run, so the state they keep is used.
    c.modules.religion.as_mut().unwrap().schisms = Some(SchismParams { years: 1.0, provinces: 1, per_year: 0.5 });
    let (mut a, mut b) = (sim(&c), sim(&c));
    a.run_days(800);
    b.run_days(800);
    let changes: Vec<SettingChange> =
        religion_settings(&c).into_iter().filter(|i| i.spec.when == sim::settings::When::Live).map(|i| SettingChange { key: i.key, value: i.value }).collect();
    assert_eq!(changes.len(), 13);
    b.edit(Edit::Settings { changes }).unwrap();
    a.run_days(5 * 365);
    b.run_days(5 * 365);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn a_religion_change_takes_effect_from_its_day() {
    let mut made = everything();
    made.modules.religion.as_mut().unwrap().per_year = 0.3;
    let mut a = sim(&made);
    let mut b = sim(&everything());
    b.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.religion.per_year".into(), value: 0.3 }] }).unwrap();
    a.run_days(800);
    b.run_days(800);
    assert_eq!(a.fingerprint(), b.fingerprint());
    let mut plain = sim(&everything());
    plain.run_days(800);
    assert_ne!(plain.fingerprint(), b.fingerprint());
    // Starting settings are set when a world is made.
    assert!(b.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.religion.starting".into(), value: 3.0 }] }).unwrap_err().contains("when a world is made"));
}

#[test]
fn schisms_switched_off_stay_off_through_a_change() {
    let mut c = everything();
    sim::settings::set_section(&mut c, "modules.religion.schisms", false).unwrap();
    sim::settings::set(&mut c, "modules.religion.per_year", 0.1).unwrap();
    assert!(c.modules.religion.as_ref().unwrap().schisms.is_none());
    assert!(sim::settings::defaults_for(&c).modules.religion.unwrap().schisms.is_none());
    sim::settings::set_section(&mut c, "modules.religion.schisms", true).unwrap();
    assert_eq!(c.modules.religion.as_ref().unwrap().schisms, ReligionParams::default().schisms);
    // A file that doesn't name them has none; one that names the module with nothing else has the
    // other defaults.
    let p: ReligionParams = toml::from_str("per_year = 0.05").unwrap();
    assert_eq!(p, ReligionParams { per_year: 0.05, schisms: None, ..ReligionParams::default() });
}

/// Each religion setting alone at the ends of its range, on a tiny world with every module on: the
/// world runs (a year and a day, so a year start's schisms run) or a module refuses it.
#[test]
fn every_religion_setting_works_at_the_ends_of_its_range() {
    let base = everything();
    let mut failures = Vec::new();
    for i in religion_settings(&base) {
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
