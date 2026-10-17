//! The module contract, end to end: choosing versions at world creation, exact conservation across
//! modules and days, saves that resume bit for bit, and loads that refuse what they can't read.

use sim::config::{ClimateVersion, Config, TerrainVersion};
use sim::economy::FoodParams;
use sim::events::{Event, EventKind};
use sim::fields::{quantity_total, Quantity};
use sim::module::Slot;
use sim::save::{SaveFile, FORMAT_VERSION, MAGIC};
use sim::sim::Sim;
use sim::world::WorldParams;

fn params(seed: u64) -> WorldParams {
    WorldParams { n: 24, provinces: 40, seed, land_fraction: 0.4, radius_km: 6371.0 }
}

/// Harsh harvests, so famines happen within a few years.
fn harsh(seed: u64) -> Config {
    let mut c = Config::new(params(seed));
    // No realms: the yearly books below count births and deaths, not soldiers raised and killed.
    c.modules.without_realms();
    c.modules.economics = Some(FoodParams { harvest_variability: 1.2, ..FoodParams::default() });
    c
}

#[test]
fn every_terrain_and_climate_version_makes_a_living_world() {
    for terrain in [TerrainVersion::Null, TerrainVersion::Noise] {
        for climate in [ClimateVersion::Null, ClimateVersion::Latitude] {
            let mut config = harsh(4);
            config.modules.terrain = terrain;
            config.modules.climate = climate;
            let mut sim = Sim::new(&config).unwrap();
            sim.strict = true;
            let what = format!("{terrain:?} terrain, {climate:?} climate");
            let w = &sim.world;
            assert!(w.provinces.count > 0, "{what}: no provinces");
            assert!(w.elevation.iter().any(|&h| h < 0.0) && w.elevation.iter().any(|&h| h >= 0.0), "{what}: no coasts");
            assert!(quantity_total(w, Quantity::People) > 0, "{what}: nobody lives here");
            assert_eq!(sim.manifests().iter().find(|m| m.slot == Slot::Terrain).unwrap().version, match terrain {
                TerrainVersion::Null => "null",
                TerrainVersion::Noise => "noise",
            });

            sim.run_to_year(5);
            assert_eq!(sim.problems, Default::default(), "{what}: {:?}", sim.problems);
            assert_eq!(sim.history.len(), 3, "years 1 to 3 closed; year 4 closes on the first day of year 5");
            assert!(quantity_total(&sim.world, Quantity::People) > 0, "{what}: everyone died");
            // Each year balances exactly, across both modules and across the day between asking
            // for food and learning how much came.
            let mut people = None::<u64>;
            let mut food = None::<u64>;
            for y in &sim.history {
                if let (Some(p0), Some(f0)) = (people, food) {
                    assert_eq!(y.population + y.ledger.deaths, p0 + y.ledger.births, "{what}: people in year {}", y.year);
                    assert_eq!(y.food_stock + y.ledger.spoiled + y.ledger.eaten, f0 + y.ledger.produced, "{what}: food in year {}", y.year);
                }
                people = Some(y.population);
                food = Some(y.food_stock);
            }
        }
    }
    // Null climate really is uniform; the latitude version is not.
    let mut c = Config::new(params(1));
    c.modules.climate = ClimateVersion::Null;
    let flat = Sim::new(&c).unwrap();
    assert!(flat.world.temperature.iter().all(|&t| t == sim::climate::NULL_TEMPERATURE));
    let varied = Sim::new(&Config::new(params(1))).unwrap();
    assert!(varied.world.temperature.iter().any(|&t| t < 0.0) && varied.world.temperature.iter().any(|&t| t > 20.0));
}

#[test]
fn famines_happen_and_are_announced_the_day_after() {
    let mut sim = Sim::new(&harsh(4)).unwrap();
    sim.strict = true;
    sim.run_to_year(4);
    let starved: u64 = sim.history.iter().map(|y| y.ledger.starved).sum();
    assert!(starved > 0, "three harsh years and nobody starved");
    let famines: Vec<_> = sim.event_log.entries.iter().filter(|p| p.event.kind() == EventKind::Famine).collect();
    assert!(!famines.is_empty());
    for p in famines {
        // Food is asked for on a month's first day and granted that evening; people learn on day
        // two, and the famine they announce is delivered on day three.
        assert_eq!(sim.calendar.date(p.day).day_of_month, 2, "famine emitted on {}", sim.calendar.format(p.day));
        assert_eq!(p.emitter, Some(Slot::Population));
    }
}

#[test]
fn turning_off_economics_while_people_still_eat_is_refused() {
    let mut c = Config::new(params(1));
    // Realms need people too: off here, so the refusal is the economy's.
    c.modules.without_realms();
    c.modules.economics = None;
    let e = Sim::new(&c).err().expect("refused");
    assert!(e.0.contains("Population \"basic\" sends change requests to province.stock, but no active module owns it"), "{e}");
    // With both off, the world just sits there: no people, no stores, nothing to break.
    c.modules.population = None;
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    sim.run_to_year(3);
    assert_eq!(quantity_total(&sim.world, Quantity::People), 0);
    assert!(sim.world.stock.iter().all(|&f| f == 0));
}

/// Run to the end of the first month-start day on which some pop was granted less food than it
/// asked for: grants and famines are then in flight.
fn run_into_a_shortfall(sim: &mut Sim) {
    for _ in 0..20 * 364 {
        sim.step_day();
        let short = sim.applied_today.iter().any(|(r, g)| *g < r.take);
        if short {
            return;
        }
    }
    panic!("no food shortfall in 20 years");
}

#[test]
fn a_save_taken_mid_month_resumes_bit_for_bit() {
    let mut a = Sim::new(&harsh(6)).unwrap();
    run_into_a_shortfall(&mut a);
    assert!(a.pending_count() > 0, "grants should be waiting for tomorrow");
    let bytes = a.save();
    let mut b = Sim::load(&bytes).unwrap();
    assert_eq!(a.fingerprint(), b.fingerprint(), "loaded state differs");
    assert_eq!(b.save(), bytes, "saving is not stable");

    // Several months on, including the day the waiting grants are settled.
    for _ in 0..200 {
        a.step_day();
        b.step_day();
    }
    a.run_to_year(a.calendar.date(a.day).year + 2);
    b.run_to_year(b.calendar.date(b.day).year + 2);
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert_eq!(a.history, b.history);
    assert_eq!(a.problems, Default::default());
    assert_eq!(b.commands, a.commands);
}

#[test]
fn loads_refuse_what_they_cannot_read() {
    let mut c = harsh(2);
    c.modules.terrain = TerrainVersion::Null;
    let sim = Sim::new(&c).unwrap();
    let bytes = sim.save();

    let refused = |b: &[u8]| Sim::load(b).err().expect("should be refused").0;
    assert!(refused(b"not a save").contains("not a save file"));

    let mut newer = bytes.clone();
    newer[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert!(refused(&newer).contains(&format!("save format {}; this build reads format {FORMAT_VERSION}", FORMAT_VERSION + 1)));

    let edit = |f: &dyn Fn(&mut SaveFile)| {
        let mut file: SaveFile = postcard::from_bytes(&bytes[12..]).unwrap();
        f(&mut file);
        let mut out = Vec::from(&MAGIC[..]);
        out.extend(FORMAT_VERSION.to_le_bytes());
        out.extend(postcard::to_stdvec(&file).unwrap());
        out
    };
    let other_version = edit(&|f| f.modules[0].version = "plates".into());
    assert!(refused(&other_version).contains("the save's modules are [Terrain plates (none)"));
    let other_schema = edit(&|f| f.snapshots[0].module_state[7].state_schema = "pops-2".into());
    assert!(refused(&other_schema).contains("Population state is schema pops-2"));
    let food = |f: &SaveFile| f.snapshots[0].fields.iter().position(|b| b.name == "province.stock").unwrap();
    let deeper = edit(&|f| {
        let k = food(f);
        f.snapshots[0].fields[k].depth = 2
    });
    assert!(refused(&deeper).contains("province.stock is stored at taxonomy depth 2"));
    let other_goods = edit(&|f| {
        let k = food(f);
        f.snapshots[0].fields[k].goods = vec!["grain".into()];
    });
    assert!(refused(&other_goods).contains("province.stock holds goods [grain], this world keeps [food]"));
    let missing = edit(&|f| {
        f.snapshots[0].fields.remove(0);
    });
    assert!(refused(&missing).contains("missing field cell.elevation"));
    let short = edit(&|f| {
        let k = food(f);
        if let sim::fields::FieldData::U64(v) = &mut f.snapshots[0].fields[k].data {
            v.pop();
        }
    });
    assert!(refused(&short).contains("province.stock"));
    assert!(refused(&bytes[..bytes.len() / 2]).contains("damaged save"));
    // And the untouched bytes still load.
    assert!(Sim::load(&bytes).is_ok());
}

#[test]
fn out_of_bounds_values_are_clamped_reported_and_still_balance() {
    let mut sim = Sim::new(&Config::new(params(3))).unwrap();
    sim.run_days(5);
    // A value far past the food stock's contract, as a buggy edit might leave it.
    sim.world.stock[0] = 5_000_000_000_000_000;
    let month = sim.calendar.def.months[0].days as u64;
    sim.run_days(month - 5 + 2);
    assert_eq!(sim.problems.conservation_breaks, 0);
    assert_eq!(sim.problems.contract_violations, 1);
    let v: Vec<_> = sim.event_log.entries.iter().filter_map(|p| match &p.event {
        Event::ContractViolated { field, entity, clamped_to, by, .. } => Some((field.def().name, *entity, *clamped_to, *by)),
        _ => None,
    }).collect();
    assert_eq!(v, vec![("province.stock", 0, 1e15, Some(Slot::Economics))]);
    let year = sim.year_so_far();
    assert!(year.food_stock < 2_000_000_000_000_000);
    // The clamp is in the year's totals, so the year still balances.
    assert!(year.ledger.food_clamped < -3_000_000_000_000_000, "{}", year.ledger.food_clamped);
}
