//! Realms choosing war and making peace (step 7, Diplomacy "treaties"): opinions, what realms want,
//! whom they may declare war on, the general going for the war's target, peace by treaty (a
//! rebellion's too), battles counted toward their war, and all of it through saves and going back.

use crate::config::Config;
use crate::diplomacy::{pick_for, reach_of, war_weariness, TreatyParams, WarCause, WarParams, HUNGER_MONTHS, NEVER};
use crate::events::Event;
use crate::grid::NONE;
use crate::realms::RealmParams;
use crate::sim::{Problems, Sim};
use crate::warfare::{TroopParams, FIELD};
use crate::world::{World, WorldParams};
use std::collections::BTreeMap;

/// Realms (no unrest, so no rebellions start wars of their own), armies, and Diplomacy "treaties"
/// with the settings given. No rulers: every realm is neither warlike nor greedy.
fn config(seed: u64, t: TreatyParams) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy = Some(WarParams { treaties: Some(t), ..WarParams::default() });
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

fn land_of(w: &World, r: u32) -> Vec<u32> {
    (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r).collect()
}

fn people_of(w: &World, r: u32) -> u64 {
    let people = w.province_population();
    land_of(w, r).iter().map(|&p| people[p as usize]).sum()
}

/// The two realms of at least `min` provinces with the longest land border between them.
fn rivals(w: &World, min: usize) -> (u32, u32) {
    let mut border: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for l in &w.routes.links {
        let (a, b) = (w.owner[l.a as usize], w.owner[l.b as usize]);
        if a != NONE && b != NONE && a != b && l.land_days.is_some() && land_of(w, a).len() >= min && land_of(w, b).len() >= min {
            *border.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    border.into_iter().max_by_key(|&(k, n)| (n, std::cmp::Reverse(k))).expect("two realms border each other").0
}

/// The peaces made, as (war, winner).
fn peaces(sim: &Sim) -> Vec<(u32, u32)> {
    sim.event_log.entries.iter().filter_map(|p| if let Event::Peace { war, winner, .. } = p.event { Some((war, winner)) } else { None }).collect()
}

/// Run to the start of the next month (the day Diplomacy works), and one day on.
fn next_month(sim: &mut Sim) {
    sim.run_days(1);
    while !sim.calendar.is_month_start(sim.day) {
        sim.run_days(1);
    }
    sim.run_days(1);
}

#[test]
fn treaties_need_armies_and_sound_settings() {
    let mut c = config(1, TreatyParams::default());
    c.modules.warfare = None;
    let e = Sim::new(&c).err().expect("treaties without armies are refused").to_string();
    assert!(e.contains("army.size"), "{e}");
    for bad in [TreatyParams { war_chance: -1.0, ..TreatyParams::default() }, TreatyParams { opinion_rebels: -150.0, ..TreatyParams::default() }, TreatyParams { worn: 0.0, ..TreatyParams::default() }] {
        assert!(Sim::new(&config(1, bad)).is_err());
    }
    let sim = Sim::new(&config(1, TreatyParams::default())).unwrap();
    assert!(sim.modules.iter().any(|m| m.manifest().slot == crate::module::Slot::Diplomacy && m.manifest().version == "treaties"));
}

#[test]
fn every_war_cause_comes_back_from_its_number() {
    for c in WarCause::ALL {
        assert_eq!(WarCause::from_u8(c as u8), c);
    }
    assert_ne!(WarCause::Food.describe(), WarCause::Poke.describe());
    assert_ne!(WarCause::Riches.describe(), WarCause::Poke.describe());
}

#[test]
fn opinions_warm_in_peace_cool_between_rivals_and_sour_with_war_and_occupation() {
    let mut sim = strict(&config(3, calm()));
    sim.run_days(45);
    let w = &sim.world;
    // Every pair whose armies can reach each other is known.
    for x in 0..w.realms.count() as u32 {
        for &y in reach_of(w, x).keys() {
            assert!(w.relations.find(x, y).is_some(), "realms {x} and {y} can reach each other");
        }
    }
    let before = sim.world.relations.clone();
    let year = sim.calendar.days_per_year() as u64;
    let months = (sim.day..sim.day + year).filter(|&d| sim.calendar.is_month_start(d)).count() as f32;
    sim.run_days(year);
    let after = &sim.world.relations;
    // A year of peace: each opinion rose 0.5 a month, less 0.3 in months they were rivals.
    let mut warmed = 0;
    for k in 0..before.count() {
        for (b, a) in [(before.a_of_b[k], after.a_of_b[k]), (before.b_of_a[k], after.b_of_a[k])] {
            let rise = a - b;
            assert!((months * 0.2 - 1e-3..=months * 0.5 + 1e-3).contains(&rise), "relation {k}: {b} to {a} in {months} months");
            warmed += (rise > months * 0.5 - 1e-3) as usize;
        }
    }
    assert!(warmed > 0, "some neighbours aren't rivals");

    // Mid-month, so no month's warming in between.
    while (0..5).any(|d| sim.calendar.is_month_start(sim.day + d)) {
        sim.run_days(1);
    }
    let (a, b) = rivals(&sim.world, 3);
    let (ab, ba) = (sim.world.relations.opinion(a, b), sim.world.relations.opinion(b, a));
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    let target = *land_of(&sim.world, b).iter().find(|&&p| sim.world.realms.capital[b as usize] != p).unwrap();
    sim.poke(Event::Occupied { province: target, by: a });
    sim.run_days(2);
    let w = &sim.world;
    assert_eq!(w.relations.opinion(a, b), (ab - 20.0).max(-100.0));
    assert_eq!(w.relations.opinion(b, a), (ba - 60.0 - 10.0).max(-100.0));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_hungry_realm_wants_its_neighbours_food_and_any_realm_their_richest_province() {
    let mut sim = strict(&config(4, calm()));
    sim.run_days(2);
    let w = &sim.world;
    let t = TreatyParams::default();
    let people = w.province_population();
    let value = |p: u32| people[p as usize] as f64 + w.stock_under(p as usize, "food").unwrap() as f64 / 365.0;
    let food = |p: u32| w.stock_under(p as usize, "food").unwrap();
    let (mut rich_seen, mut food_seen) = (0, 0);
    for x in 0..w.realms.count() as u32 {
        let reach = reach_of(w, x);
        if let Some(p) = pick_for(w, &t, &[], sim.day, x) {
            assert_eq!(p.cause, WarCause::Riches);
            let theirs = &reach[&p.realm];
            assert!(theirs.contains(&p.province));
            assert!(theirs.iter().all(|&q| value(q) <= value(p.province)), "the richest of realm {}'s provinces in reach", p.realm);
            rich_seen += 1;
        }
        // A tenth of its people starved this year: it wants food.
        let mut hunger = vec![0u64; w.realms.count() * HUNGER_MONTHS];
        hunger[x as usize * HUNGER_MONTHS + 3] = people_of(w, x) / 10;
        if let Some(p) = pick_for(w, &t, &hunger, sim.day, x) {
            assert_eq!(p.cause, WarCause::Food, "realm {x}");
            let theirs = &reach[&p.realm];
            assert!(theirs.iter().all(|&q| food(q) <= food(p.province)), "the best-stocked of realm {}'s provinces in reach", p.realm);
            food_seen += 1;
        }
    }
    assert!(rich_seen > 0 && food_seen > 0, "{rich_seen} {food_seen}");
}

#[test]
fn no_war_out_of_reach_under_a_truce_on_a_much_stronger_realm_or_a_second_unless_twice_as_strong() {
    let mut sim = strict(&config(5, calm()));
    sim.run_days(2);
    let t = TreatyParams::default();
    let w = &sim.world;
    let mut chooser = None;
    for x in 0..w.realms.count() as u32 {
        let Some(p) = pick_for(w, &t, &[], sim.day, x) else { continue };
        assert!(reach_of(w, x).contains_key(&p.realm), "only realms its armies can reach");
        assert!(people_of(w, x) as f64 >= 0.8 * people_of(w, p.realm) as f64, "never a much stronger realm");
        chooser.get_or_insert((x, p.realm));
    }
    // Nobody is weak enough to attack.
    let proud = TreatyParams { least_strength: 1e9, ..TreatyParams::default() };
    assert!((0..w.realms.count() as u32).all(|x| pick_for(w, &proud, &[], sim.day, x).is_none()));

    // A truce: not that realm again.
    let (x, y) = chooser.expect("some realm wants a war");
    sim.poke(Event::DeclareWar { attacker: x, defender: y });
    sim.run_days(2);
    // At war already: another only when twice as strong as both enemies together.
    let w = &sim.world;
    if let Some(p) = pick_for(w, &t, &[], sim.day, x) {
        assert_ne!(p.realm, y);
        assert!(people_of(w, x) as f64 >= 2.0 * (people_of(w, y) + people_of(w, p.realm)) as f64);
    }
    let one_at_a_time = TreatyParams { second_war: 1e9, ..TreatyParams::default() };
    assert_eq!(pick_for(w, &one_at_a_time, &[], sim.day, x), None);
    sim.poke(Event::ForcePeace { a: x, b: y, winner: NONE });
    sim.run_days(2);
    let w = &sim.world;
    assert!(pick_for(w, &t, &[], sim.day, x).is_none_or(|p| p.realm != y), "a truce is kept");
}

/// Diplomacy's hunger record: (starved per realm and month, the month being filled).
fn hunger(sim: &Sim) -> (Vec<u64>, u8) {
    let m = sim.modules.iter().find(|m| m.manifest().slot == crate::module::Slot::Diplomacy).unwrap();
    postcard::from_bytes(&m.save_state()).unwrap()
}

#[test]
fn famine_counts_against_the_realm_that_owns_the_land_for_twelve_months() {
    // Against the same world without the famine poked in (its own famines count too).
    let mut sim = strict(&config(6, calm()));
    let mut twin = strict(&config(6, calm()));
    // Just after a month's start.
    next_month(&mut sim);
    next_month(&mut twin);
    let x = sim.world.owner.iter().copied().find(|&o| o != NONE).unwrap();
    let land = land_of(&sim.world, x);
    sim.poke(Event::Famine { province: land[0], starved: 700 });
    sim.poke(Event::Famine { province: *land.last().unwrap(), starved: 50 });
    let more = |sim: &mut Sim, twin: &mut Sim| {
        let row = |s: &Sim| {
            let (h, month) = hunger(s);
            let r: Vec<u64> = h.get(x as usize * HUNGER_MONTHS..(x as usize + 1) * HUNGER_MONTHS).map_or(vec![0; HUNGER_MONTHS], |r| r.to_vec());
            (r, month)
        };
        let ((a, m), (b, _)) = (row(sim), row(twin));
        (a.iter().sum::<u64>() - b.iter().sum::<u64>(), a[m as usize] - b[m as usize])
    };
    sim.run_days(2);
    twin.run_days(2);
    assert_eq!(more(&mut sim, &mut twin), (750, 750));
    // Still counted eleven months on, gone after twelve.
    for _ in 0..11 {
        next_month(&mut sim);
        next_month(&mut twin);
    }
    assert_eq!(more(&mut sim, &mut twin).0, 750);
    next_month(&mut sim);
    next_month(&mut twin);
    assert_eq!(more(&mut sim, &mut twin).0, 0);
}

#[test]
fn the_general_goes_first_for_the_province_the_war_was_declared_over() {
    // "wars", so nothing but the poke starts a war; the target set by hand.
    let mut c = config(17, calm());
    c.modules.diplomacy = Some(WarParams::default());
    let run = |target_far: bool| {
        let mut sim = strict(&c);
        sim.run_days(2);
        let (a, b) = rivals(&sim.world, 4);
        sim.poke(Event::DeclareWar { attacker: a, defender: b });
        sim.run_days(2);
        // The province of b's next to a's land furthest from a's capital.
        let w = &sim.world;
        let cap = w.provinces.center[w.realms.capital[a as usize] as usize];
        let far = *reach_of(w, a)[&b]
            .iter()
            .filter(|&&q| w.routes.of(q).any(|l| l.land_days.is_some() && w.owner[l.other(q) as usize] == a))
            .min_by(|&&p, &&q| crate::grid::dot(w.provinces.center[p as usize], cap).total_cmp(&crate::grid::dot(w.provinces.center[q as usize], cap)))
            .unwrap();
        if target_far {
            sim.world.wars.target[0] = far;
        }
        // Raised at the next month's start, then sent.
        next_month(&mut sim);
        next_month(&mut sim);
        let w = &sim.world;
        let gather: Vec<u32> = w.routes.of(far).filter(|l| l.land_days.is_some()).map(|l| l.other(far)).collect();
        // The general's first group: where its companies gather.
        let mine: Vec<usize> = (0..w.armies.count()).filter(|&k| w.armies.realm[k] == a && w.armies.role[k] == FIELD && w.armies.size[k] > 0 && w.armies.group[k] != NONE).collect();
        let first = mine.iter().map(|&k| w.armies.group[k]).min().expect("the general has formed groups");
        let at_target = mine.iter().filter(|&&k| w.armies.group[k] == first).all(|&k| {
            let to = if w.armies.dest[k] != NONE { w.armies.dest[k] } else { w.armies.province[k] };
            gather.contains(&to)
        });
        (at_target, sim.problems)
    };
    let (with, problems) = run(true);
    let (without, _) = run(false);
    assert!(with, "the first group gathers next to the target");
    assert!(!without, "without a target the first group goes elsewhere (nearer)");
    assert_eq!(problems, Problems::default());
}

#[test]
fn weariness_counts_occupation_years_battles_dead_and_unrest() {
    let t = TreatyParams { worn: 1e9, ..calm() };
    let mut sim = strict(&config(7, t.clone()));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 3);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    let cap = sim.world.realms.capital[b as usize];
    let taken: Vec<u32> = land_of(&sim.world, b).into_iter().filter(|&p| p != cap).take(2).collect();
    for &p in &taken {
        sim.poke(Event::Occupied { province: p, by: a });
    }
    sim.run_days(2);
    let w = &mut sim.world;
    let k = 0u32;
    w.wars.defender_dead[0] = 1000;
    w.wars.defender_lost[0] = 2;
    let dpy = sim.calendar.days_per_year() as f64;
    let day = w.wars.began[0] + sim.calendar.days_per_year() as u64 / 2;
    let w = &sim.world;
    let people = w.province_population();
    let held: u64 = taken.iter().map(|&p| people[p as usize]).sum();
    let theirs = people_of(w, b) as f64;
    let (wa, wd) = war_weariness(w, &t, k, day, dpy);
    let unrest = |r: u32| land_of(w, r).iter().map(|&p| w.unrest[p as usize] as f64).sum::<f64>() / land_of(w, r).len() as f64 / crate::realms::LINE as f64;
    let expect_d = 100.0 * held as f64 / theirs + 10.0 * 0.5 + 5.0 * 2.0 + 50.0 * 1000.0 / (0.05 * theirs) + 30.0 * unrest(b);
    assert!((wd - expect_d).abs() < 1e-6, "{wd} against {expect_d}");
    assert!((wa - (10.0 * 0.5 + 30.0 * unrest(a))).abs() < 1e-6, "{wa}");
}

#[test]
fn peace_comes_when_a_side_is_worn_out_or_the_attacker_holds_its_target() {
    // Only occupation wears a side out.
    let only_land = TreatyParams { year_weariness: 0.0, battle_weariness: 0.0, dead_weariness: 0.0, unrest_weariness: 0.0, ..calm() };
    let setup = |t: TreatyParams| {
        let mut sim = strict(&config(8, t));
        sim.run_days(2);
        let (a, b) = rivals(&sim.world, 4);
        sim.poke(Event::DeclareWar { attacker: a, defender: b });
        sim.run_days(2);
        (sim, a, b)
    };
    // Worn out: all of b's land but its capital is occupied, past a line set at half of that.
    let (probe, _, b) = setup(TreatyParams { worn: 1e9, ..only_land.clone() });
    let cap = probe.world.realms.capital[b as usize];
    let taken: Vec<u32> = land_of(&probe.world, b).into_iter().filter(|&p| p != cap).collect();
    let people = probe.world.province_population();
    let share = taken.iter().map(|&p| people[p as usize]).sum::<u64>() as f64 / people_of(&probe.world, b) as f64;
    assert!(share > 0.0);
    let (mut sim, a, _) = setup(TreatyParams { worn: 100.0 * share * 0.5, ..only_land.clone() });
    for &p in &taken {
        sim.poke(Event::Occupied { province: p, by: a });
    }
    next_month(&mut sim);
    sim.run_days(2);
    let w = &sim.world;
    assert_eq!(w.wars.winner[0], a, "the side worn out loses");
    assert!(taken.iter().filter(|&&p| w.owner[p as usize] == a).count() > 0, "and the winner keeps what it holds");

    // The attacker holds the war's target: peace once the defender is half worn out (here, at once).
    let (mut sim, a, b) = setup(TreatyParams { worn: 1e9, held_target_weariness: 0.0, ..only_land.clone() });
    let target = *land_of(&sim.world, b).iter().find(|&&p| p != sim.world.realms.capital[b as usize]).unwrap();
    sim.world.wars.target[0] = target;
    sim.poke(Event::Occupied { province: target, by: a });
    next_month(&mut sim);
    sim.run_days(2);
    assert_eq!(sim.world.wars.winner[0], a);
    assert_eq!(sim.world.owner[target as usize], a);

    // Worn out alike, by the years alone: nobody wins.
    let years = TreatyParams { year_weariness: 100.0, occupied_weariness: 0.0, battle_weariness: 0.0, dead_weariness: 0.0, unrest_weariness: 0.0, ..calm() };
    let (mut sim, _, _) = setup(years);
    let began = sim.world.wars.began[0];
    while sim.world.wars.ongoing(0) && sim.day < began + 2 * sim.calendar.days_per_year() as u64 {
        sim.run_days(1);
    }
    let w = &sim.world;
    assert!(!w.wars.ongoing(0));
    assert_eq!(w.wars.winner[0], NONE);
    let ended = w.wars.ended[0];
    assert!(ended >= began + sim.calendar.days_per_year() as u64 && ended < began + sim.calendar.days_per_year() as u64 + 40, "about a year: {began} to {ended}");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_rebellions_war_ends_by_treaty_either_way() {
    let only_land = TreatyParams { year_weariness: 0.0, battle_weariness: 0.0, dead_weariness: 0.0, unrest_weariness: 0.0, worn: 1.0, ..calm() };
    for rebels_lose in [true, false] {
        let mut sim = strict(&config(9, only_land.clone()));
        sim.run_days(2);
        let (rebels, old) = rivals(&sim.world, 4);
        sim.poke(Event::DeclareWar { attacker: rebels, defender: old });
        sim.run_days(2);
        sim.world.wars.cause[0] = WarCause::Rebellion as u8;
        sim.world.realms.broke_from[rebels as usize] = old;
        let rebel_land = land_of(&sim.world, rebels);
        let (loser, winner) = if rebels_lose { (rebels, old) } else { (old, rebels) };
        let cap = sim.world.realms.capital[loser as usize];
        let taken = *land_of(&sim.world, loser).iter().find(|&&p| p != cap).unwrap();
        sim.poke(Event::Occupied { province: taken, by: winner });
        next_month(&mut sim);
        sim.run_days(2);
        let w = &sim.world;
        assert_eq!((w.wars.winner[0], w.wars.ended[0] != NEVER), (winner, true));
        let peace = sim.event_log.entries.iter().find_map(|p| if let Event::Peace { rebellion, winner, .. } = p.event { Some((rebellion, winner)) } else { None });
        assert_eq!(peace, Some((true, winner)));
        if rebels_lose {
            assert!(rebel_land.iter().all(|&p| w.owner[p as usize] == old), "the realm the rebels left takes all their land back");
        } else {
            assert!(rebel_land.iter().all(|&p| w.owner[p as usize] == rebels), "the rebels go free");
            assert_eq!(w.owner[taken as usize], rebels, "keeping what they hold");
        }
    }
}

#[test]
fn battles_count_toward_their_war_and_sour_opinions() {
    let t = TreatyParams { worn: 1e9, ..calm() };
    let mut sim = strict(&config(17, t));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(sim.calendar.days_per_year() as u64);
    let (mut dead, mut lost, mut battles, mut occupied) = (BTreeMap::<u32, u64>::new(), BTreeMap::<u32, u32>::new(), 0, BTreeMap::<u32, u32>::new());
    let w = &sim.world;
    for p in &sim.event_log.entries {
        match &p.event {
            Event::Battle { loser, sides, .. } => {
                battles += 1;
                for &(x, d) in sides {
                    *dead.entry(x).or_default() += d;
                }
                *lost.entry(*loser).or_default() += 1;
            }
            &Event::Occupied { province, by } if by != w.owner[province as usize] => *occupied.entry(by).or_default() += 1,
            _ => {}
        }
    }
    assert!(battles > 0, "the war is fought");
    assert_eq!((w.wars.attacker_dead[0], w.wars.defender_dead[0]), (dead[&a], dead[&b]));
    assert_eq!((w.wars.attacker_lost[0], w.wars.defender_lost[0]), (lost.get(&a).copied().unwrap_or(0), lost.get(&b).copied().unwrap_or(0)));
    let both = 5.0 * battles as f32;
    let expect_ab = (-20.0 - both - 10.0 * *occupied.get(&b).unwrap_or(&0) as f32).max(-100.0);
    let expect_ba = (-60.0 - both - 10.0 * *occupied.get(&a).unwrap_or(&0) as f32).max(-100.0);
    // Opinions had warmed before the war, for a day or two.
    let start = w.relations.opinion(a, b) - expect_ab;
    assert!((0.0..=1.01).contains(&start) || expect_ab == -100.0, "{} against {expect_ab}", w.relations.opinion(a, b));
    assert!((w.relations.opinion(b, a) - expect_ba).abs() <= 1.01, "{} against {expect_ba}", w.relations.opinion(b, a));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn the_wars_version_leaves_the_new_fields_empty() {
    let mut c = config(17, calm());
    c.modules.diplomacy = Some(WarParams::default());
    let mut sim = strict(&c);
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(200);
    let w = &sim.world;
    assert!(sim.event_log.entries.iter().any(|p| matches!(p.event, Event::Battle { .. })));
    assert_eq!((w.wars.target[0], w.wars.attacker_dead[0], w.wars.defender_dead[0], w.wars.attacker_lost[0], w.wars.defender_lost[0]), (NONE, 0, 0, 0, 0));
    assert_eq!(w.relations.count(), 0);
}

#[test]
fn realms_choose_wars_and_make_peace_on_their_own() {
    let mut sim = strict(&config(21, TreatyParams::default()));
    sim.run_days(30 * sim.calendar.days_per_year() as u64);
    let w = &sim.world;
    let chosen: Vec<usize> = (0..w.wars.count()).filter(|&k| matches!(WarCause::from_u8(w.wars.cause[k]), WarCause::Food | WarCause::Riches)).collect();
    assert!(chosen.len() >= 3, "{} wars chosen in 30 years", chosen.len());
    assert!(chosen.iter().all(|&k| w.wars.target[k] != NONE));
    assert!(chosen.iter().any(|&k| w.wars.ended[k] != NEVER && w.wars.winner[k] != NONE), "some end by treaty with a winner");
    assert!(!sim.event_log.entries.iter().any(|p| matches!(p.event, Event::WarRefused { .. })));
    assert!(peaces(&sim).len() >= 2);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn treaties_survive_saving_loading_and_going_back() {
    let t = TreatyParams { war_chance: 0.05, ..TreatyParams::default() };
    let fresh = || strict(&config(11, t.clone()));
    let mut sim = fresh();
    let stops = [400u64, 900, 1500];
    let mut prints = Vec::new();
    for &d in &stops {
        sim.run_days(d - sim.day);
        let mut loaded = Sim::load(&sim.save()).unwrap();
        loaded.strict = true;
        let mut twin = fresh();
        twin.run_days(d + 60);
        loaded.run_days(60);
        assert_eq!(loaded.fingerprint(), twin.fingerprint(), "saved on day {d}");
        prints.push(sim.fingerprint());
    }
    assert!(sim.world.wars.count() > 0 && sim.world.relations.count() > 0);
    let (today, now) = (sim.day, sim.fingerprint());
    for (&d, &print) in stops.iter().zip(&prints).rev() {
        sim.goto(d).unwrap();
        assert_eq!(sim.fingerprint(), print, "back to day {d}");
    }
    sim.goto(today).unwrap();
    assert_eq!(sim.fingerprint(), now, "forward again");

    // A scenario file carries the settings and makes the same world.
    let text = crate::branch::Timelines::new(sim.fork()).scenario_text();
    assert!(text.contains("treaties") && text.contains("war_chance"), "the settings are written");
    let mut read = crate::branch::Timelines::from_scenario_text(&text).unwrap();
    let back = read.list.swap_remove(read.active).sim;
    assert_eq!((back.day, back.fingerprint()), (sim.day, sim.fingerprint()));
    assert_eq!((&back.world.wars, &back.world.relations), (&sim.world.wars, &sim.world.relations));
    assert_eq!(sim.problems, Problems::default());
}
