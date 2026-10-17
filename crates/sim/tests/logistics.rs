//! Logistics and support (build step 8), the four parts of Warfare "campaigns": sieges in depth,
//! supply lines and foraging, winter and soldiers' pay. Each part is switched on alone in a world
//! at war and checked day by day; then all four together through saves, going back and settings
//! changes, and every logistics setting at the ends of its range.

use sim::config::Config;
use sim::diplomacy::WarParams;
use sim::edit::{Edit, SettingChange};
use sim::events::Event;
use sim::fields::{known, Quantity};
use sim::grid::NONE;
use sim::logistics::{winter, MassingParams, PayParams, SeasonParams, SiegeParams, SupplyParams};
use sim::markets::MarketParams;
use sim::realms::RealmParams;
use sim::requests::{Cause, Priority};
use sim::sim::{Problems, Sim};
use sim::warfare::{TroopParams, FIELD};
use sim::world::{World, WorldParams};
use std::collections::{BTreeMap, BTreeSet};

/// Realms of about 8 provinces (no unrest), wars by pokes, troops with the parts `parts` sets.
fn config(seed: u64, markets: bool, parts: impl FnOnce(&mut TroopParams)) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.without_realms();
    if markets {
        c.modules.economics = None;
        c.modules.markets = Some(MarketParams::default());
    }
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy = Some(WarParams::default());
    let mut t = TroopParams::default();
    parts(&mut t);
    c.modules.warfare = Some(t);
    c
}

fn all_parts(t: &mut TroopParams) {
    (t.supply, t.seasons, t.pay, t.sieges) = (Some(SupplyParams::default()), Some(SeasonParams::default()), Some(PayParams::default()), Some(SiegeParams::default()));
    t.massing = Some(MassingParams::default());
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

/// A world of `c` at war: the rivals' war is poked on its second day, and a second pair's too.
fn at_war(c: &Config) -> (Sim, u32, u32) {
    let mut sim = Sim::new(c).unwrap();
    sim.strict = true;
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    (sim, a, b)
}

/// The general's groups in a world's Warfare state, by number.
fn groups(sim: &Sim) -> BTreeMap<u32, sim::warfare::GroupView> {
    let bytes = sim.save();
    let file: sim::save::SaveFile = postcard::from_bytes(&bytes[12..]).unwrap();
    let st = file.snapshots[0].module_state.iter().find(|m| m.slot == sim::module::Slot::Warfare).unwrap();
    sim::warfare::groups_in_state(&st.bytes).unwrap().into_iter().map(|g| (g.number, g)).collect()
}

#[test]
fn massed_groups_set_off_without_stragglers_and_give_up_a_slow_gathering() {
    let join_days = 20;
    let c = config(17, false, |t| t.massing = Some(MassingParams { join_days, ..MassingParams::default() }));
    let (mut sim, ..) = at_war(&c);
    let mut seen: BTreeMap<u32, u64> = BTreeMap::new();
    let mut before = groups(&sim);
    let (mut launched, mut without_stragglers) = (0, 0);
    for _ in 0..2 * 364 {
        sim.run_days(1);
        let now = groups(&sim);
        for (k, g) in &now {
            let first = *seen.entry(*k).or_insert(sim.day);
            // Nobody waits to gather much longer than join_days (the general looks at least monthly).
            assert!(g.launched || sim.day - first <= join_days as u64 + 31, "group {k} gathering since day {first}, now {}", sim.day);
            if let Some(was) = before.get(k)
                && g.launched
                && !was.launched
            {
                launched += 1;
                if g.members.len() < was.members.len() {
                    without_stragglers += 1;
                }
            }
        }
        before = now;
    }
    assert_eq!(sim.problems, Problems::default());
    assert!(launched > 5 && without_stragglers > 0, "{launched} groups set off, {without_stragglers} without their stragglers");
}

fn year_part(sim: &Sim, day: u64) -> f64 {
    let cal = &sim.calendar;
    (day - cal.year_start(cal.date(day).year)) as f64 / cal.days_per_year() as f64
}

#[test]
fn winter_comes_far_from_the_equator_and_half_a_year_apart_north_and_south() {
    let p = SeasonParams::default();
    assert_eq!(winter(&p, 0.0, p.deepest), 0.0, "none at the equator");
    assert_eq!(winter(&p, 25.0, p.deepest), 0.0, "none where it begins");
    assert!((winter(&p, 55.0, p.deepest) - 1.0).abs() < 1e-9, "full, at its deepest, 30 degrees further");
    assert!((winter(&p, 40.0, p.deepest) - 0.5).abs() < 1e-9, "half way");
    assert_eq!(winter(&p, 60.0, p.deepest + 0.5), 0.0, "no winter in the north's summer");
    assert!((winter(&p, -60.0, p.deepest + 0.5) - 1.0).abs() < 1e-9, "the south's deepest winter, half a year later");
    assert_eq!(winter(&p, -60.0, p.deepest), 0.0);
    // Over half the year, deepening and fading smoothly.
    let days: Vec<f64> = (0..365).map(|d| winter(&p, 70.0, d as f64 / 365.0)).collect();
    let winter_days = days.iter().filter(|&&x| x > 0.0).count();
    assert!((175..=190).contains(&winter_days), "{winter_days} days of winter");
}

#[test]
fn sieges_in_depth_stop_raising_strip_stores_hasten_starving_sieges_cut_trade_and_plunder() {
    // Every besieged province counts as starving, so each siege day counts three.
    let c = config(11, true, |t| t.sieges = Some(SiegeParams { starving_days: 1e9, ..SiegeParams::default() }));
    let (mut sim, ..) = at_war(&c);
    let pop_size = known("pop.size");
    let stock = known("province.stock");
    let (mut besieged_months, mut stripped, mut fast, mut slow, mut plundered, mut cut) = (0, 0, 0, 0, 0u64, 0);
    let mut siege_days: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    for _ in 0..3 * 364 {
        let before: BTreeSet<u32> = (0..sim.world.provinces.count as u32).filter(|&p| sim.world.besieger[p as usize] != NONE).collect();
        let month_start = sim.calendar.is_month_start(sim.day);
        sim.run_days(1);
        let w = &sim.world;
        // Whoever besieges a province has field soldiers standing in it.
        for p in 0..w.provinces.count {
            let by = w.besieger[p];
            if by != NONE {
                let there = (0..w.armies.count()).any(|a| w.armies.realm[a] == by && w.armies.province[a] == p as u32 && w.armies.size[a] > 0 && w.armies.role[a] == FIELD && w.armies.next[a] == NONE);
                assert!(there, "province {p} besieged by {by}, who has no field soldiers in it");
            }
        }
        if month_start && !before.is_empty() {
            besieged_months += 1;
            for (r, granted) in &sim.applied_today {
                if r.field == pop_size && r.cause == Cause::Raised {
                    assert!(!before.contains(&r.entity), "besieged province {} raised {granted}", r.entity);
                }
                if r.field == stock && r.cause == Cause::Foraging && before.contains(&r.entity) {
                    assert_eq!(r.priority, Priority::Critical);
                    stripped += 1;
                }
            }
            // No trade into or out of a province besieged when the day began.
            if w.trade.day + 1 == sim.day {
                cut += 1;
                for s in &w.trade.shipments {
                    assert!(!before.contains(&s.from) && !before.contains(&s.to), "trade with a besieged province: {s:?}");
                }
            }
        }
        for (r, granted) in &sim.applied_today {
            if r.cause == Cause::Plunder {
                assert_eq!(*granted, r.take, "the coins were there");
                plundered += granted;
            }
        }
        // A starving siege counts three days a day.
        let mut now: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
        for a in 0..w.armies.count() {
            if w.armies.size[a] > 0 && w.armies.role[a] == FIELD && w.armies.next[a] == NONE && w.armies.siege_days[a] > 0 {
                now.insert(a as u32, (w.armies.province[a], w.armies.siege_days[a]));
            }
        }
        for (a, &(p, d)) in &now {
            if let Some(&(q, e)) = siege_days.get(a)
                && p == q
                && d != e
            {
                if d == e + 3 {
                    fast += 1;
                } else {
                    slow += 1;
                }
            }
        }
        siege_days = now;
    }
    assert_eq!(sim.problems, Problems::default());
    assert!(besieged_months >= 3, "{besieged_months} months began with a siege");
    assert!(stripped > 0 && cut > 0, "stores stripped {stripped} times, trade cut {cut} months");
    assert!(fast > 20 && slow == 0, "siege days went up three at a time {fast} times, otherwise {slow}");
    assert!(plundered > 0, "no coins plundered");
}

#[test]
fn supply_lines_feed_companies_from_home_and_foragers_take_first_on_enemy_land() {
    for reach in [0, 2] {
        let c = config(12, false, |t| t.supply = Some(SupplyParams { reach_ways: reach, ..SupplyParams::default() }));
        let (mut sim, ..) = at_war(&c);
        let (mut lined, mut foraging, mut first, mut attrition) = (0, 0, 0, false);
        for _ in 0..2 * 364 {
            let month_start = sim.calendar.is_month_start(sim.day);
            // Where each company stood as the day began, when it was told how it is fed.
            let stood: Vec<u32> = sim.world.armies.province.clone();
            sim.run_days(1);
            attrition |= sim.year_flows.get(Quantity::People, Cause::Attrition) < 0;
            if !month_start {
                continue;
            }
            let w = &sim.world;
            let enemies = sim::diplomacy::enemies(w);
            // How each company is fed: at home on its own land, else along a line or foraging.
            for (a, &at) in stood.iter().enumerate().take(w.armies.count()) {
                if w.armies.size[a] == 0 {
                    continue;
                }
                let at = at as usize;
                let home_land = w.owner[at] == w.armies.realm[a] && w.occupier[at] == NONE;
                match w.armies.supply[a] {
                    0 => assert!(home_land, "company {a} fed as at home away from it"),
                    1 => {
                        assert!(!home_land && reach > 0);
                        lined += 1;
                    }
                    _ => {
                        assert!(!home_land);
                        foraging += 1;
                    }
                }
            }
            for (r, _) in &sim.applied_today {
                if r.cause == Cause::Eating && r.priority == Priority::Critical {
                    // Only on an enemy's land, where a foraging enemy of its owner stands.
                    let p = r.entity as usize;
                    let foragers = (0..w.armies.count().min(stood.len())).any(|a| {
                        stood[a] as usize == p && w.armies.supply[a] == 2 && enemies[w.armies.realm[a] as usize].binary_search(&w.owner[p]).is_ok()
                    });
                    assert!(foragers, "food taken first in province {p} with no foraging enemy there");
                    first += 1;
                }
            }
        }
        assert_eq!(sim.problems, Problems::default());
        if reach == 0 {
            assert!(foraging > 0 && first > 0 && lined == 0, "reach {reach}: {foraging} foraging, {first} taken first, {lined} along a line");
        } else {
            assert!(lined > 0, "nobody fed along a line");
        }
        assert!(attrition, "camp disease killed nobody");
    }
}

#[test]
fn winter_slows_marches_and_no_new_attack_begins_in_deep_winter() {
    // Winter almost everywhere, deep: marches take up to three times as long.
    let seasons = SeasonParams { start_lat: 10.0, full_span: 20.0, road_slow: 3.0, ..SeasonParams::default() };
    let c = config(13, false, |t| t.seasons = Some(seasons.clone()));
    let (mut sim, ..) = at_war(&c);
    let (mut checked, mut launched, mut slowed) = (0, 0, 0);
    let mut was: Vec<(u32, u32, f32, u32)> = Vec::new();
    for _ in 0..2 * 364 {
        let day = sim.day;
        sim.run_days(1);
        let w = &sim.world;
        let part = year_part(&sim, day);
        let arm = &w.armies;
        let enemies = sim::diplomacy::enemies(w);
        for a in 0..arm.count() {
            let Some(&(next, dest, left, at)) = was.get(a) else { continue };
            if arm.size[a] == 0 || arm.next[a] == NONE {
                continue;
            }
            // Still on the same way: a day covered 1 / (1 + 2 × winter) of a day's march.
            if next != NONE && arm.next[a] == next && arm.province[a] == at {
                let want = 1.0 / (1.0 + 2.0 * winter(&seasons, w.provinces.lat[at as usize].to_degrees(), part));
                assert!(((left - arm.days_left[a]) as f64 - want).abs() < 1e-3, "company {a}: {left} → {}, want {want}", arm.days_left[a]);
                checked += 1;
                slowed += (want < 0.6) as u32;
            }
            // Newly set off, in a group, for an enemy's province it doesn't occupy: an attack.
            let d = arm.dest[a] as usize;
            let realm = arm.realm[a];
            if dest != arm.dest[a] && arm.group[a] != NONE && w.owner[d] != realm && w.occupier[d] != realm && enemies[realm as usize].binary_search(&w.owner[d]).is_ok() {
                let deep = winter(&seasons, w.provinces.lat[d].to_degrees(), part);
                assert!(deep <= seasons.no_attack, "company {a} set off for province {d} in a winter of {deep:.2}");
                launched += 1;
            }
        }
        was = (0..arm.count()).map(|a| (arm.next[a], arm.dest[a], arm.days_left[a], arm.province[a])).collect();
    }
    assert_eq!(sim.problems, Problems::default());
    assert!(checked > 100 && slowed > 10 && launched > 0, "{checked} march days checked, {slowed} slowed, {launched} attacks set off");
}

#[test]
fn soldiers_are_paid_from_the_treasury_into_their_homes_and_the_unpaid_desert() {
    let wages = known("realm.stock");
    for wage in [1.0, 1e4] {
        let c = config(14, true, |t| t.pay = Some(PayParams { wage, ..PayParams::default() }));
        let (mut sim, ..) = at_war(&c);
        let (mut months, mut deserted) = (0, 0u64);
        for _ in 0..364 {
            let month_start = sim.calendar.is_month_start(sim.day);
            let seen = sim.event_log.entries.len();
            sim.run_days(1);
            deserted += sim.event_log.entries[seen..].iter().map(|e| match e.event {
                Event::Arrived { people, cause: Cause::Deserted, .. } => people,
                _ => 0,
            }).sum::<u64>();
            if !month_start {
                continue;
            }
            let w = &sim.world;
            let soldiers: u64 = (0..w.armies.count()).filter(|&a| w.realms.alive(w.armies.realm[a])).map(|a| w.armies.size[a]).sum();
            let (asked, paid): (u64, u64) = sim.applied_today.iter().filter(|(r, _)| r.field == wages && r.cause == Cause::Wages).fold((0, 0), |(a, p), (r, g)| (a + r.take, p + g));
            assert!(asked > 0, "no wages asked");
            // Asked for this month's soldiers (a few died or came home since).
            let want = wage * soldiers as f64;
            assert!((asked as f64 - want).abs() <= 0.05 * want + 1.0, "asked {asked} for {soldiers} soldiers");
            if wage == 1.0 {
                assert_eq!(asked, paid, "a realm pays its soldiers at a coin a month");
            } else {
                assert!(paid < asked, "nobody pays {wage} a month");
            }
            months += 1;
        }
        assert_eq!(sim.problems, Problems::default());
        assert!(months >= 12);
        if wage > 1.0 {
            assert!(deserted > 0, "unpaid soldiers stayed");
        }
    }
    // Under Economics v0 there are no coins: pay does nothing.
    let c = config(14, false, |t| t.pay = Some(PayParams::default()));
    let (mut sim, ..) = at_war(&c);
    for _ in 0..100 {
        sim.run_days(1);
        assert!(sim.applied_today.iter().all(|(r, _)| r.cause != Cause::Wages));
    }
}

#[test]
fn campaigns_survive_saving_loading_going_back_and_settings_changes() {
    let c = config(15, true, all_parts);
    let (mut sim, ..) = at_war(&c);
    sim.run_days(500);
    // Saving and loading.
    let mut loaded = Sim::load(&sim.save()).unwrap();
    let mut same = Sim::load(&sim.save()).unwrap();
    sim.run_days(200);
    loaded.run_days(200);
    assert_eq!(sim.fingerprint(), loaded.fingerprint(), "a loaded world runs on the same");
    // Going back and running the same days again.
    let end = sim.day;
    let print = sim.fingerprint();
    sim.goto(end - 150).unwrap();
    sim.run_days(150);
    assert_eq!(sim.fingerprint(), print, "going back and on again");
    // Every logistics setting set to the value it has: the module is made again, and runs on the
    // same.
    let changes: Vec<SettingChange> = sim::settings::items(&c)
        .into_iter()
        .filter(|i| ["supply", "seasons", "pay", "sieges", "massing"].iter().any(|p| i.key.starts_with(&format!("modules.warfare.{p}"))) || i.key == "modules.markets.trade.siege_stops")
        .map(|i| SettingChange { key: i.key, value: i.value })
        .collect();
    assert_eq!(changes.len(), 4 + 7 + 2 + 5 + 1 + 2);
    same.edit(Edit::Settings { changes }).unwrap();
    same.run_days(200);
    assert_eq!(same.fingerprint(), print, "settings changed to what they were");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn every_logistics_setting_works_at_the_ends_of_its_range() {
    use rayon::prelude::*;
    let mut cases = Vec::new();
    for markets in [false, true] {
        let mut base = config(16, markets, all_parts);
        base.world = WorldParams { n: 24, provinces: 40, seed: 16, land_fraction: 0.4, radius_km: 6371.0 };
        base.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, unrest: None, ..RealmParams::default() });
        for i in sim::settings::items(&base) {
            let ours = ["modules.warfare.supply", "modules.warfare.seasons", "modules.warfare.pay", "modules.warfare.sieges", "modules.warfare.massing", "modules.markets.trade.siege_stops"];
            if ours.iter().any(|k| i.key.starts_with(k)) {
                for end in [i.spec.min, i.spec.max] {
                    cases.push((i.key.clone(), end, base.clone()));
                }
            }
        }
    }
    assert!(cases.len() >= 2 * 2 * 18, "{} cases", cases.len());
    let failures: Vec<String> = cases
        .into_par_iter()
        .filter_map(|(key, end, mut c)| {
            sim::settings::set(&mut c, &key, end).unwrap_or_else(|e| panic!("{key} = {end}: {e}"));
            let run = std::panic::catch_unwind(move || match Sim::new(&c) {
                Err(_) => Ok(()),
                Ok(mut s) => {
                    s.strict = true;
                    s.run_days(2);
                    let (a, b) = rivals(&s.world, 2);
                    s.poke(Event::DeclareWar { attacker: a, defender: b });
                    s.run_days(240);
                    if s.problems != Problems::default() { Err(format!("{:?}", s.problems)) } else { Ok(()) }
                }
            });
            match run {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(format!("{key} = {end}: {e}")),
                Err(p) => Some(format!("{key} = {end}: crashed: {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
            }
        })
        .collect();
    assert!(failures.is_empty(), "{} failing:\n{}", failures.len(), failures.join("\n"));
}
