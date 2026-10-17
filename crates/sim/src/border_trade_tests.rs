//! Trade and realms (step 7): what markets reads as the day began, trade warming opinions under
//! Diplomacy "treaties", and both through saves and going back. How tolls and wars change each
//! deal is tested beside the trade itself, in `markets.rs`.

use crate::config::Config;
use crate::diplomacy::{enemies, TreatyParams, WarParams, NEVER};
use crate::events::Event;
use crate::grid::NONE;
use crate::markets::Borders;
use crate::module::Slot;
use crate::realms::RealmParams;
use crate::sim::{Problems, Sim};
use crate::warfare::TroopParams;
use crate::world::{World, WorldParams};
use std::collections::BTreeMap;

/// Markets with trade, realms (no unrest, so no rebellions), armies, and Diplomacy "treaties"
/// with the settings given (`None`: "wars").
fn config(seed: u64, treaties: Option<TreatyParams>) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy = Some(WarParams { treaties, ..WarParams::default() });
    c.modules.warfare = Some(TroopParams::default());
    c
}

/// No realm declares war of its own accord.
fn calm() -> TreatyParams {
    TreatyParams { war_chance: 0.0, ..TreatyParams::default() }
}

fn strict(c: &Config) -> Sim {
    let mut sim = Sim::new(c).unwrap();
    sim.strict = true;
    sim
}

/// Two realms that border each other by land, and a province of `b`'s next to `a`'s land.
fn neighbours(w: &World) -> (u32, u32, u32) {
    let mut pairs: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for l in w.routes.links.iter().filter(|l| l.land_days.is_some()) {
        let (x, y) = (w.owner[l.a as usize], w.owner[l.b as usize]);
        if x != NONE && y != NONE && x != y {
            pairs.entry((x, y)).or_insert(l.b);
        }
    }
    let (&(a, b), &p) = pairs.iter().next().expect("two realms border each other");
    (a, b, p)
}

/// The first month's first day after `day`.
fn month_after(sim: &Sim, day: u64) -> u64 {
    (day + 1..).find(|&d| sim.calendar.is_month_start(d)).unwrap()
}

#[test]
fn trade_reads_owners_occupation_and_wars_as_the_day_began() {
    let mut sim = strict(&config(5, None));
    sim.run_days(2);
    let (a, b, p) = neighbours(&sim.world);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    // Realm a takes the province the day before a month begins, so it is delivered on the month's
    // first day, when trade runs.
    let m = month_after(&sim, sim.day + 2);
    sim.schedule(m - 1, Event::Occupied { province: p, by: a }).unwrap();
    sim.run_days(m + 1 - sim.day);
    let w = &sim.world;
    assert_eq!((w.earlier.day, w.occupier[p as usize]), (m, a));
    assert_eq!(w.earlier.u32s("province.occupier")[p as usize], NONE, "copied before the occupation was delivered");
    let borders = Borders::read(&w.earlier, w.provinces.count);
    assert_eq!(borders.trades_as[p as usize], b, "it traded as its owner's that day");
    assert!(borders.at_war(a, b));

    // A peace delivered on a month's first day: the war is over when trade runs, but trade still
    // reads it as going on, as it stood when the day began.
    let m = month_after(&sim, sim.day + 2);
    sim.schedule(m - 1, Event::ForcePeace { a, b, winner: a }).unwrap();
    sim.run_days(m + 1 - sim.day);
    let w = &sim.world;
    let war = w.wars.current(a, b);
    assert!(war.is_none() && w.wars.ended[0] == m, "the peace was made that day");
    assert_eq!(w.earlier.u64s("war.ended")[0], NEVER);
    assert!(Borders::read(&w.earlier, w.provinces.count).at_war(a, b));
    assert!(!enemies(w)[a as usize].contains(&b));
    // The next day the land is handed over (the peace's event), and the next month's trade sees it.
    sim.run_days(month_after(&sim, sim.day) + 1 - sim.day);
    let w = &sim.world;
    assert_eq!((w.owner[p as usize], w.occupier[p as usize]), (a, NONE));
    let borders = Borders::read(&w.earlier, w.provinces.count);
    assert!(!borders.at_war(a, b) && borders.trades_as[p as usize] == a);
    // On a day no reader of the copy runs, nothing is copied.
    sim.run_days(1);
    assert!(sim.world.earlier.is_empty());
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn trade_between_realms_warms_their_opinions_only_under_treaties() {
    let run = |t: Option<TreatyParams>| {
        let mut sim = strict(&config(9, t));
        // The first month's trade is on day 0; its events are handled on day 1.
        sim.run_days(1);
        let traded: Vec<(u32, u32, u64)> = sim.pending.iter().filter_map(|p| if let Event::Trade { a, b, a_to_b, b_to_a } = p.event { Some((a, b, a_to_b + b_to_a)) } else { None }).collect();
        sim.run_days(1);
        (sim, traded)
    };
    let (on, traded) = run(Some(calm()));
    let (off, same) = run(Some(TreatyParams { trade_opinion: 0.0, ..calm() }));
    assert!(!traded.is_empty() && traded == same, "realms trade across their borders");
    assert!(traded.iter().all(|&(a, b, worth)| a < b && worth > 0));
    // Not kept in the event log.
    assert!(!on.event_log.entries.iter().any(|p| matches!(p.event, Event::Trade { .. })));
    let (r1, r0) = (&on.world.relations, &off.world.relations);
    // Every pair that traded has a relation, made by the trade if their armies can't reach each other.
    for &(a, b, _) in &traded {
        assert!(r1.find(a, b).is_some(), "realms {a} and {b} traded");
    }
    assert!(r1.count() >= r0.count());
    // Each opinion is as warm as without trade's warming, or warmer, by at most 0.5 a month.
    let mut warmer = 0;
    for k in 0..r0.count() {
        assert_eq!((r1.a[k], r1.b[k]), (r0.a[k], r0.b[k]));
        for (x, y) in [(r1.a_of_b[k], r0.a_of_b[k]), (r1.b_of_a[k], r0.b_of_a[k])] {
            assert!(x >= y && x <= y + 0.5 + 1e-6, "relation {k}: {x} against {y}");
            warmer += (x > y) as usize;
        }
    }
    assert!(warmer > 0);
    // The busiest partners get the whole 0.5.
    assert!((0..r0.count()).any(|k| (r1.a_of_b[k] - r0.a_of_b[k] - 0.5).abs() < 1e-6));

    // Under "wars" nobody keeps opinions, and the trade events go unhandled.
    let (wars, traded) = run(None);
    assert!(!traded.is_empty());
    assert_eq!(wars.world.relations.count(), 0);
    for sim in [&on, &off, &wars] {
        assert_eq!(sim.problems, Problems::default());
    }
}

#[test]
fn trade_across_borders_survives_saving_loading_and_going_back_on_its_day() {
    let mut sim = strict(&config(5, Some(calm())));
    sim.run_days(2);
    let (a, b, _) = neighbours(&sim.world);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    // Run until the armies have taken some of the other's land.
    let held = |w: &World| (0..w.provinces.count).any(|q| w.occupier[q] != NONE);
    while !held(&sim.world) {
        assert!(sim.day < 4 * 364, "no land taken in four years of war");
        sim.run_days(30);
    }
    // The peace is made two days before a month begins and its land handed over on the month's
    // first day, when trade runs: owners and occupation change that very day.
    let m = month_after(&sim, sim.day + 3);
    sim.schedule(m - 2, Event::ForcePeace { a, b, winner: a }).unwrap();
    sim.run_days(m - sim.day);
    let saved = sim.save();
    let occupied: Vec<u32> = (0..sim.world.provinces.count as u32).filter(|&q| sim.world.occupier[q as usize] != NONE).collect();
    sim.run_days(1);
    assert!(occupied.iter().any(|&q| sim.world.occupier[q as usize] == NONE), "the peace changed the land on the month's first day");
    sim.run_days(40);
    let (day, print) = (sim.day, sim.fingerprint());

    let mut loaded = Sim::load(&saved).unwrap();
    loaded.strict = true;
    loaded.run_days(41);
    assert_eq!((loaded.day, loaded.fingerprint()), (day, print), "saved the evening before");

    sim.goto(m).unwrap();
    sim.goto(day).unwrap();
    assert_eq!(sim.fingerprint(), print, "gone back and forward again");
    assert_eq!(sim.problems, Problems::default());
}
