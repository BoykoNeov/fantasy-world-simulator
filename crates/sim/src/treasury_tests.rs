//! Treasury and taxes (step 7): under markets, realms tax their provinces' coins into treasuries
//! that the economics module keeps, and spend a share back; under Economics v0 they keep none.

use crate::branch::Timelines;
use crate::config::Config;
use crate::diplomacy::WarParams;
use crate::events::Event;
use crate::fields::{quantity_total, Quantity};
use crate::grid::NONE;
use crate::module::Slot;
use crate::realms::{spending, RealmParams, TaxParams};
use crate::requests::{Cause, Request};
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};

fn config(economics: &str, seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.diplomacy = Some(WarParams::default());
    c
}

fn world(economics: &str, seed: u64) -> Sim {
    let mut sim = Sim::new(&config(economics, seed)).unwrap();
    sim.strict = true;
    sim
}

/// The coins a row of a per-good field holds (a province's store, or a realm's treasury).
fn coins(w: &World, values: &[u64], row: usize) -> u64 {
    let g = &w.goods;
    let money = g.tree.find("money").unwrap();
    g.columns_under(money).unwrap().iter().map(|&c| values[g.index(row, c)]).sum()
}

fn money(w: &World) -> Quantity {
    let t = &w.goods.tree;
    Quantity::Goods(t.class_of(t.find("money").unwrap()).unwrap() as u8)
}

fn land_of(w: &World, r: u32) -> Vec<u32> {
    (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r).collect()
}

/// Run the next month's first day, so `applied_today` holds what was moved that night.
fn run_month_start(sim: &mut Sim) {
    let start = (sim.day..).find(|&d| sim.calendar.is_month_start(d)).unwrap();
    sim.run_days(start + 1 - sim.day);
}

fn moved(sim: &Sim, cause: Cause) -> Vec<(Request, u64)> {
    sim.applied_today.iter().filter(|(r, _)| r.cause == cause).copied().collect()
}

/// A realm with at least `min` provinces, and a realm next to it.
fn neighbours(w: &World, min: usize) -> (u32, u32) {
    for p in 0..w.provinces.count as u32 {
        let b = w.owner[p as usize];
        if b == NONE || land_of(w, b).len() < min {
            continue;
        }
        for l in w.routes.of(p) {
            let a = w.owner[l.other(p) as usize];
            if a != NONE && a != b {
                return (a, b);
            }
        }
    }
    panic!("no realm of {min} provinces has a neighbour");
}

#[test]
fn treasuries_fill_with_taxes_and_settle_where_spending_meets_them() {
    let mut sim = world("markets", 1);
    let all = quantity_total(&sim.world, money(&sim.world));
    let start = sim.calendar.date(0).year;
    let mut shares = Vec::new();
    for year in 1..=12 {
        sim.run_to_year(start + year);
        let w = &sim.world;
        // Coins are only moved: none made or lost.
        assert_eq!(quantity_total(w, money(w)), all, "year {year}");
        let alive: Vec<u32> = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect();
        let held: u64 = alive.iter().map(|&r| coins(w, &w.realm_stock, r as usize)).sum();
        let land: u64 = alive.iter().flat_map(|&r| land_of(w, r)).map(|p| coins(w, &w.stock, p as usize)).sum();
        shares.push(held as f64 / land as f64);
    }
    // Taxed 1% a month and spent 10%: the treasuries hold about a tenth of their land's coins.
    assert!(shares[0] > 0.03, "a year's taxes: {shares:?}");
    for (k, s) in shares.iter().enumerate().skip(4) {
        assert!((0.08..0.12).contains(s), "year {}: treasuries hold {s:.3} of their land's coins ({shares:?})", k + 1);
    }
    assert!((shares[11] - shares[5]).abs() < 0.01, "settled: {shares:?}");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn every_province_pays_and_the_spending_goes_by_people_the_capital_counted_thrice() {
    let mut sim = world("markets", 2);
    sim.run_days(100);
    run_month_start(&mut sim);
    let w = &sim.world;
    let taxed = moved(&sim, Cause::Taxes);
    // Every owned province where people live paid its realm once, in full.
    let people = w.province_population();
    for p in 0..w.provinces.count {
        let r = w.owner[p];
        let paid: Vec<_> = taxed.iter().filter(|(q, _)| q.entity == p as u32).collect();
        if r == NONE {
            assert!(paid.is_empty(), "province {p} belongs to nobody");
            continue;
        }
        if people[p] == 0 && paid.is_empty() {
            continue;
        }
        let [(q, granted)] = paid[..] else { panic!("province {p} paid {} times", paid.len()) };
        assert_eq!((q.to.unwrap().entity, *granted), (r, q.take), "province {p}");
    }
    // Each realm's spending, all of it granted, by people, the capital's counted 3 times.
    let spent = moved(&sim, Cause::Spent);
    let mut compared = 0;
    for r in (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)) {
        let mine: Vec<_> = spent.iter().filter(|(q, _)| q.entity == r).collect();
        let total: u64 = mine.iter().map(|(_, g)| g).sum();
        assert!(total > 0, "realm {r} spent nothing");
        assert!(mine.iter().all(|(q, g)| *g == q.take && w.owner[q.to.unwrap().entity as usize] == r));
        let capital = w.realms.capital[r as usize];
        let (mut at_capital, mut elsewhere, mut people_elsewhere) = (0.0, 0.0, 0.0);
        for (q, g) in mine {
            let p = q.to.unwrap().entity as usize;
            if p == capital as usize {
                at_capital = *g as f64 / people[p] as f64;
            } else {
                elsewhere += *g as f64;
                people_elsewhere += people[p] as f64;
            }
        }
        if total > 10_000 && people_elsewhere > 0.0 {
            let ratio = at_capital / (elsewhere / people_elsewhere);
            assert!((2.9..3.1).contains(&ratio), "realm {r}: the capital got {ratio:.2} times as much a person");
            compared += 1;
        }
    }
    assert!(compared > 5, "only {compared} realms compared");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn spending_is_shared_exactly() {
    let people = [100, 0, 300, 600];
    // By people, the capital's (province 2) counted 3 times: 100, 0, 900, 600 of 1,600.
    assert_eq!(spending(&[0, 1, 2, 3], 2, 160, 3.0, &people), vec![(0, 10), (2, 90), (3, 60)]);
    // The capital not among them (occupied): by people alone.
    assert_eq!(spending(&[0, 3], 2, 70, 3.0, &people), vec![(0, 10), (3, 60)]);
    // Nobody lives in any: evenly.
    assert_eq!(spending(&[1], 7, 9, 3.0, &people), vec![(1, 9)]);
    assert_eq!(spending(&[], 2, 100, 3.0, &people), vec![]);
    for spend in [0, 1, 7, 1_000_003] {
        assert_eq!(spending(&[0, 1, 2, 3], 3, spend, 2.37, &people).iter().map(|x| x.1).sum::<u64>(), spend);
    }
}

#[test]
fn an_occupied_province_pays_nobody_and_is_paid_nothing() {
    let mut sim = world("markets", 3);
    let (a, b) = neighbours(&sim.world, 3);
    sim.run_days(100);
    // Just after a month's start, so the war and the occupation come before the next.
    run_month_start(&mut sim);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    // Its capital is taken, and one other province.
    let capital = sim.world.realms.capital[b as usize];
    let other = *land_of(&sim.world, b).iter().find(|&&p| p != capital).unwrap();
    for province in [capital, other] {
        sim.poke(Event::Occupied { province, by: a });
    }
    run_month_start(&mut sim);
    let w = &sim.world;
    assert_eq!((w.occupier[capital as usize], w.occupier[other as usize]), (a, a));
    let taxed = moved(&sim, Cause::Taxes);
    let spent = moved(&sim, Cause::Spent);
    for p in [capital, other] {
        assert!(!taxed.iter().any(|(q, _)| q.entity == p), "{p} paid taxes");
        assert!(!spent.iter().any(|(q, _)| q.to.unwrap().entity == p), "{p} was paid");
    }
    // The rest of b's land still pays, and b's spending goes to it, by people alone.
    let paying: Vec<u32> = land_of(w, b).into_iter().filter(|&p| p != capital && p != other).collect();
    assert!(paying.iter().all(|&p| taxed.iter().any(|(q, _)| q.entity == p && q.to.unwrap().entity == b)));
    assert!(spent.iter().any(|(q, _)| q.entity == b));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_dissolved_realm_leaves_its_coins_to_its_last_capital() {
    let mut c = config("markets", 3);
    c.modules.realms = Some(RealmParams { settled: 0.0, city_states: 1.0, ..RealmParams::default() });
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    let all = quantity_total(&sim.world, money(&sim.world));
    let (a, b) = neighbours(&sim.world, 1);
    sim.run_days(100);
    // Just after a month's start, so the realm falls before the next.
    run_month_start(&mut sim);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    for province in land_of(&sim.world, b) {
        sim.poke(Event::Occupied { province, by: a });
    }
    sim.run_days(2);
    let capital = sim.world.realms.capital[b as usize];
    sim.poke(Event::ForcePeace { a, b, winner: a });
    sim.run_days(3);
    let w = &sim.world;
    assert!(!w.realms.alive(b));
    let left = coins(w, &w.realm_stock, b as usize);
    assert!(left > 0, "b had coins when it fell");

    run_month_start(&mut sim);
    let w = &sim.world;
    let spent: Vec<_> = moved(&sim, Cause::Spent).into_iter().filter(|(q, _)| q.entity == b).collect();
    assert_eq!(spent.len(), 1);
    assert_eq!((spent[0].0.to.unwrap().entity, spent[0].1), (capital, left), "all of it, to its last capital");
    assert_eq!(coins(w, &w.realm_stock, b as usize), 0);
    assert_eq!(quantity_total(w, money(w)), all);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn realms_without_coins_keep_no_treasury() {
    let mut sim = world("food", 4);
    let realms = sim.manifests().into_iter().find(|m| m.slot == Slot::Realms).unwrap();
    assert!(realms.requests.is_empty() && realms.moves.is_empty() && realms.asks.is_empty(), "under v0 the realms ask for no coins");
    sim.run_days(400);
    assert!(sim.world.realm_stock.iter().all(|&x| x == 0));
    assert!(sim.applied_today.iter().all(|(r, _)| r.cause != Cause::Taxes));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn taxes_run_with_claims_off() {
    let mut c = config("markets", 5);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, claim_per_month: 0.0, ..RealmParams::default() });
    let mut sim = Sim::new(&c).unwrap();
    sim.run_days(100);
    assert!(sim.world.realm_stock.iter().any(|&x| x > 0));
}

#[test]
fn bad_tax_settings_are_refused() {
    for taxes in [TaxParams { per_month: 1.5, ..TaxParams::default() }, TaxParams { capital_weight: 0.5, ..TaxParams::default() }] {
        let mut c = config("markets", 1);
        c.modules.realms = Some(RealmParams { taxes, ..RealmParams::default() });
        let e = Sim::new(&c).err().expect("refused").to_string();
        assert!(e.contains("taxes"), "{e}");
    }
}

#[test]
fn treasuries_go_through_saves_going_back_and_scenario_files() {
    let mut sim = world("markets", 6);
    // Mid-month, with a realm's capital remembered.
    sim.run_days(400);
    let print = sim.fingerprint();
    let today = sim.day;
    let mut loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), print);
    let year = sim.calendar.days_per_year() as u64;
    sim.run_days(year);
    loaded.run_days(year);
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    let end = sim.fingerprint();
    sim.goto(today).unwrap();
    assert_eq!(sim.fingerprint(), print);
    sim.goto(today + year).unwrap();
    assert_eq!(sim.fingerprint(), end);

    let text = Timelines::new(sim.fork()).scenario_text();
    assert!(text.contains("spent_per_month"), "the tax settings are written");
    let mut read = Timelines::from_scenario_text(&text).unwrap();
    let back = read.list.swap_remove(read.active).sim;
    assert_eq!((back.day, back.fingerprint()), (sim.day, sim.fingerprint()));
    assert_eq!(back.world.realm_stock, sim.world.realm_stock);
}
