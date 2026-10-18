//! The battle inspector (Oct 18): Warfare's record of the battles going on, which the window reads,
//! follows each battle and agrees with its report; it comes back when going back in history, and
//! after a load from the battles in Warfare's saved state; and the realms' loss counters
//! (`realm.lost_*`) count every soldier killed, dead of camp disease and cold, and deserted, as the
//! flows and events do, year by year in the years' summaries too.

use sim::config::Config;
use sim::diplomacy::WarParams;
use sim::events::Event;
use sim::fields::Quantity;
use sim::grid::NONE;
use sim::logistics::{MassingParams, PayParams, SeasonParams, SiegeParams, SupplyParams};
use sim::markets::MarketParams;
use sim::realms::RealmParams;
use sim::requests::Cause;
use sim::sim::{Problems, Sim, SoldiersLost};
use sim::warfare::{BattleNow, TroopParams};
use sim::world::{World, WorldParams};
use std::collections::BTreeMap;

/// Realms of about 8 provinces (no unrest) under markets, wars by pokes, troops with every part of
/// logistics (camp disease, winter, pay: every way of losing soldiers).
fn config(seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.without_realms();
    c.modules.economics = None;
    c.modules.markets = Some(MarketParams::default());
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy = Some(WarParams::default());
    let mut t = TroopParams::default();
    (t.supply, t.seasons, t.pay, t.sieges) = (Some(SupplyParams::default()), Some(SeasonParams::default()), Some(PayParams::default()), Some(SiegeParams::default()));
    t.massing = Some(MassingParams::default());
    c.modules.warfare = Some(t);
    c
}

fn land_of(w: &World, r: u32) -> usize {
    (0..w.provinces.count).filter(|&p| w.owner[p] == r).count()
}

/// The two realms of at least `min` provinces with the longest land border between them.
fn rivals(w: &World, min: usize) -> (u32, u32) {
    let mut border: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for l in &w.routes.links {
        let (a, b) = (w.owner[l.a as usize], w.owner[l.b as usize]);
        if a != NONE && b != NONE && a != b && l.land_days.is_some() && land_of(w, a) >= min && land_of(w, b) >= min {
            *border.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    border.into_iter().max_by_key(|&(k, n)| (n, std::cmp::Reverse(k))).expect("two realms border each other").0
}

/// A strict world of seed `seed` whose rivals go to war on its third day.
fn at_war(seed: u64) -> Sim {
    let mut sim = Sim::new(&config(seed)).unwrap();
    sim.strict = true;
    let (a, b) = rivals(&sim.world, 4);
    sim.schedule(3, Event::DeclareWar { attacker: a, defender: b }).unwrap();
    sim
}

#[test]
fn the_battle_record_follows_each_battle_and_agrees_with_its_report() {
    let mut sim = at_war(17);
    // The last record of each battle going on, by province; and of those that left the record
    // today, whose reports come tomorrow.
    let (mut going, mut last): (BTreeMap<u32, BattleNow>, BTreeMap<u32, BattleNow>) = Default::default();
    let (mut reports, mut checked) = (0, 0);
    while sim.day < 500 {
        sim.run_days(1);
        for e in sim.delivered_today() {
            let Event::Battle { province, killed, sides, began, brought, at_sea, .. } = &e.event else { continue };
            reports += 1;
            assert_eq!(sides.len(), brought.len(), "{:?}", e.event);
            for (&(r, dead), &(r2, n)) in sides.iter().zip(brought) {
                assert_eq!(r, r2, "sides and what they brought in one order");
                assert!(dead <= n, "realm {r} lost {dead} of the {n} it brought");
            }
            assert_eq!(*killed, sides.iter().map(|s| s.1).sum::<u64>());
            assert!(*began <= e.day);
            // A land battle was in the record the day before it ended. One at sea is fought out in
            // a day, and may be beside one on land.
            if *at_sea {
                assert_eq!(*began, e.day, "a battle at sea begins the day it ends");
            } else if let Some(b) = last.remove(province) {
                checked += 1;
                assert_eq!(b.began, *began, "province {province}");
                for s in &b.sides {
                    let (_, dead) = sides.iter().find(|x| x.0 == s.realm).expect("every side of the record is in the report");
                    let (_, n) = brought.iter().find(|x| x.0 == s.realm).unwrap();
                    assert!(*dead >= s.killed && *n >= s.brought, "the report counts at least what the record did: {s:?}, {sides:?}, {brought:?}");
                }
            }
        }
        let rec = sim.world.battles.clone();
        for b in &rec.battles {
            assert!(b.began <= rec.day && rec.day < sim.day, "{b:?} on day {}", rec.day);
            assert!(b.sides.len() >= 2, "{b:?}");
            for s in &b.sides {
                assert!(s.killed <= s.brought && s.last_day <= s.killed && s.breaks_at <= s.brought, "{s:?}");
                assert!(s.broke || s.killed < s.breaks_at || s.killed == 0, "a side past its breaking point broke: {s:?}");
            }
        }
        // Every battle that left the record yesterday was reported today.
        assert!(last.is_empty(), "day {}: no report of {last:?}", sim.day);
        let still = |p: u32, b: &BattleNow| rec.battles.iter().any(|n| n.province == p && n.began == b.began);
        last = going.into_iter().filter(|(p, b)| !still(*p, b)).collect();
        going = rec.battles.iter().map(|b| (b.province, b.clone())).collect();
    }
    assert!(reports >= 10 && checked >= 5, "{reports} battles reported, {checked} checked against the record");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn the_battle_record_comes_back_going_back_and_after_a_load() {
    let mut sim = at_war(17);
    // Days with battles going on, at least 30 days apart, and their records.
    let mut stops: Vec<(u64, sim::warfare::BattleRecord)> = Vec::new();
    while stops.len() < 3 && sim.day < 600 {
        sim.run_days(1);
        if !sim.world.battles.battles.is_empty() && stops.last().is_none_or(|s| sim.day > s.0 + 30) {
            stops.push((sim.day, (*sim.world.battles).clone()));
        }
    }
    assert!(stops.len() >= 2, "{:?}", stops.iter().map(|s| s.0).collect::<Vec<_>>());
    let today = sim.day;
    for (d, rec) in stops.iter().rev() {
        sim.goto(*d).unwrap();
        assert_eq!(*sim.world.battles, *rec, "back to day {d}");
        // A world just opened has no record until its next day of battle; then it is the one the
        // world that ran on has.
        let mut loaded = Sim::load(&sim.save()).unwrap();
        assert!(loaded.world.battles.battles.is_empty());
        loaded.run_days(1);
        sim.run_days(1);
        assert_eq!(*loaded.world.battles, *sim.world.battles, "the day after day {d}, loaded");
    }
    sim.goto(today).unwrap();
}

#[test]
fn every_soldier_lost_is_counted_against_their_realm_by_how() {
    let mut sim = at_war(21);
    let lost = |w: &World| SoldiersLost::of_world(w);
    // Deserters reach home the day after they leave: each day's arrivals are the day before's.
    let (mut before, mut deserted_yesterday) = (0u64, 0u64);
    // Battle dead by realm, from the reports.
    let mut reported: BTreeMap<u32, u64> = BTreeMap::new();
    let end = sim.calendar.year_start(sim.calendar.date(sim.day).year + 2) + 40;
    while sim.day < end {
        sim.run_days(1);
        let arrived: u64 = sim.delivered_today().iter().map(|e| if let Event::Arrived { people, cause: Cause::Deserted, .. } = e.event { people } else { 0 }).sum();
        assert_eq!(arrived, deserted_yesterday, "day {}: deserters home against those counted the day before", sim.day);
        let now = lost(&sim.world).deserted;
        (deserted_yesterday, before) = (now - before, now);
        for e in sim.delivered_today() {
            if let Event::Battle { sides, .. } = &e.event {
                for &(r, dead) in sides {
                    *reported.entry(r).or_default() += dead;
                }
            }
        }
    }
    let w = &sim.world;
    let all = lost(w);
    // This year's flows so far.
    let flow = |c: Cause| -> u64 { (-sim.year_flows.get(Quantity::People, c)).max(0) as u64 };
    let killed: u64 = sim.history.iter().map(|y| y.ledger.killed).sum::<u64>() + flow(Cause::Killed);
    assert_eq!(all.battle, killed, "killed in battle or lost at sea, against the ledger");
    // Camp disease and cold aren't kept apart in the years' ledgers: this year's flow is checked,
    // and the summaries' years against the counters.
    let this_year = sim.year_so_far().soldiers;
    assert_eq!(this_year.sickness, flow(Cause::Attrition), "camp disease and cold this year");
    let years = sim.history.iter().fold(SoldiersLost::default(), |sum, y| sum.plus(&y.soldiers));
    assert_eq!(years.plus(&this_year), all, "the years' summaries add up to the counters");
    assert!(sim.history.len() >= 2 && sim.history.iter().all(|y| y.soldiers.battle > 0), "{:?}", sim.history.iter().map(|y| y.soldiers).collect::<Vec<_>>());
    assert!(all.battle > 0 && all.sickness > 0 && all.deserted > 0, "{all:?}");
    // Each realm lost at least the battle dead its reports name (more with soldiers lost at sea).
    for (&r, &dead) in &reported {
        assert!(SoldiersLost::of_realm(w, r).battle >= dead, "realm {r}: {dead} dead in its battles, {:?}", SoldiersLost::of_realm(w, r));
    }
    assert!(reported.values().sum::<u64>() <= all.battle);
    assert_eq!(sim.problems, Problems::default());
}
