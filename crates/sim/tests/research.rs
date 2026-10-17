//! Research "tree" with every other module on: each of its settings at both ends of its range, and
//! the module keeping its state when every live setting is changed.

use sim::config::Config;
use sim::diplomacy::{TreatyParams, WarParams};
use sim::edit::{Edit, SettingChange};
use sim::markets::MarketParams;
use sim::realms::RealmParams;
use sim::research::ResearchParams;
use sim::sim::Sim;
use sim::world::WorldParams;

fn small() -> Config {
    Config::new(WorldParams { n: 24, provinces: 40, seed: 12, land_fraction: 0.4, radius_km: 6371.0 })
}

/// Every runtime module on, Research too, under the economy given.
fn everything(markets: bool) -> Config {
    let mut c = small();
    if markets {
        c.modules.economics = None;
        c.modules.markets = Some(MarketParams::default());
    }
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.characters = Some(Default::default());
    c.modules.warfare = Some(Default::default());
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    c.modules.research = Some(ResearchParams::default());
    c
}

#[test]
fn every_module_keeps_its_state_across_a_change_with_research_on() {
    for markets in [false, true] {
        let c = everything(markets);
        let (mut a, mut b) = (Sim::new(&c).unwrap(), Sim::new(&c).unwrap());
        a.strict = true;
        b.strict = true;
        // The change lands the day after a month's first, with its scholars' wages asked and not
        // yet paid.
        let day = (700..).find(|&d| a.calendar.is_month_start(d)).unwrap() + 1;
        a.run_days(day);
        b.run_days(day);
        let changes: Vec<SettingChange> =
            sim::settings::items(&c).into_iter().filter(|i| i.spec.when == sim::settings::When::Live).map(|i| SettingChange { key: i.key, value: i.value }).collect();
        b.edit(Edit::Settings { changes }).unwrap();
        a.run_days(800);
        b.run_days(800);
        assert_eq!(a.fingerprint(), b.fingerprint(), "markets {markets}");
    }
}

/// Each research setting alone at the ends of its range: the world is made and runs two months
/// (a year for the cost of each tech), or a module refuses it with a reason. Nothing may crash,
/// and no value may break its field's contract.
#[test]
fn every_research_setting_works_at_the_ends_of_its_range() {
    use rayon::prelude::*;
    let mut cases = Vec::new();
    for markets in [false, true] {
        let base = everything(markets);
        for i in sim::settings::items(&base).into_iter().filter(|i| i.key.starts_with("modules.research")) {
            for end in [i.spec.min, i.spec.max] {
                cases.push((i.key.clone(), end, base.clone(), markets));
            }
        }
    }
    assert!(cases.len() >= 2 * 2 * (8 + 24));
    let failures: Vec<String> = cases
        .into_par_iter()
        .filter_map(|(key, end, mut c, markets)| {
            sim::settings::set(&mut c, &key, end).unwrap_or_else(|e| panic!("{key} = {end}: {e}"));
            let run = std::panic::catch_unwind(move || match Sim::new(&c) {
                Err(_) => Ok(()),
                Ok(mut s) => {
                    s.strict = true;
                    s.run_days(400);
                    if s.problems.contract_violations > 0 || s.problems.conservation_breaks > 0 {
                        Err(format!("{:?}", s.problems))
                    } else {
                        Ok(())
                    }
                }
            });
            match run {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(format!("{key} = {end} (markets {markets}): {e}")),
                Err(p) => Some(format!(
                    "{key} = {end} (markets {markets}): crashed: {}",
                    p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
                )),
            }
        })
        .collect();
    assert!(failures.is_empty(), "{} failing:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn research_is_off_by_default_and_on_in_everything() {
    assert!(sim::config::ModuleConfig::default().research.is_none());
    assert!(sim::settings::everything().research.is_some());
}
