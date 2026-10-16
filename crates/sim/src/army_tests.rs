//! Armies moving and fighting (step 7, Warfare "troops"): marching along the ways through allowed
//! land, battles going to the stronger side with the defender's edge and breaking at a share lost,
//! sieges by the province's size, occupation only in war, peace with one enemy of two, recruits
//! gathering at home, a rebellion put down at its capital, battle unrest, and armies through saves.

use crate::config::Config;
use crate::diplomacy::{at_war, WarParams};
use crate::events::Event;
use crate::grid::NONE;
use crate::module::Slot;
use crate::realms::{RealmParams, LINE};
use crate::requests::Cause;
use crate::sim::{Problems, Sim};
use crate::taxonomy::Taxonomy;
use crate::warfare::{TroopParams, FIELD, GUARD};
use crate::world::{World, WorldParams};
use std::collections::BTreeMap;

/// Realms (no unrest unless asked), wars and troops.
fn config(seed: u64, unrest: bool) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    let realms = RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() };
    c.modules.realms = Some(if unrest { realms } else { RealmParams { unrest: None, ..realms } });
    c.modules.diplomacy = Some(WarParams::default());
    c.modules.warfare = Some(TroopParams::default());
    c
}

fn strict(c: &Config) -> Sim {
    let mut sim = Sim::new(c).unwrap();
    sim.strict = true;
    sim
}

fn land_of(w: &World, r: u32) -> Vec<u32> {
    (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r).collect()
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

fn tree() -> Taxonomy {
    Taxonomy::named(&TroopParams::default().tree, "troop tree").unwrap()
}

/// What each kind is worth in battle, by node.
fn strengths() -> Vec<f64> {
    let (p, t) = (TroopParams::default(), tree());
    (0..t.nodes.len() as u16).map(|n| p.kinds.get(t.name(n)).map_or(0.0, |k| k.strength)).collect()
}

/// Run the next day, and give the events delivered (emitted the day before).
fn day(sim: &mut Sim) -> Vec<(u64, Event)> {
    let seen = sim.event_log.entries.len();
    sim.run_days(1);
    sim.event_log.entries[seen..].iter().map(|p| (p.day, p.event.clone())).collect()
}

/// A war between rivals, raised, quiet generals (they never attack): the staging for placed
/// battles. Today is the war's second day, before any field company is raised.
fn staged(seed: u64) -> (Sim, u32, u32) {
    let mut c = config(seed, false);
    c.modules.warfare.as_mut().unwrap().outmatch = 1e9;
    let mut sim = strict(&c);
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    (sim, a, b)
}

/// In every province of realm `b` with guards, a company of realm `a`'s levies (guards, so its
/// general leaves them be) with `ratio` times the guards' strength, each from a different home.
/// The world is then saved and loaded, so warfare finds them. Returns (province, company, size).
fn placed(staging: &Sim, a: u32, b: u32, ratio: f64) -> (Sim, Vec<(u32, u32, u64)>) {
    let mut sim = Sim::load(&staging.save()).unwrap();
    let st = strengths();
    let levies = tree().find("levies").unwrap();
    let w = &mut sim.world;
    let homes = land_of(w, a);
    let mut out = Vec::new();
    for (p, home) in land_of(w, b).into_iter().zip(homes) {
        let guards: f64 = (0..w.armies.count()).filter(|&k| w.armies.province[k] == p && w.armies.realm[k] == b).map(|k| w.armies.size[k] as f64 * st[w.armies.kind[k] as usize]).sum();
        if guards < 50.0 {
            continue;
        }
        let size = (guards * ratio).round() as u64;
        let k = w.armies.push(a, home, levies, GUARD, size);
        w.armies.province[k as usize] = p;
        out.push((p, k, size));
    }
    let mut sim = Sim::load(&sim.save()).unwrap();
    sim.strict = true;
    (sim, out)
}

/// Battles by province: winner, loser, killed.
type Fought = BTreeMap<u32, (u32, u32, u64)>;

/// The battles that end within `days`, and the soldiers who came home alive to each province (the
/// broken guards).
fn battles(sim: &mut Sim, days: u64) -> (Fought, BTreeMap<u32, u64>) {
    let mut out = BTreeMap::new();
    let mut home: BTreeMap<u32, u64> = BTreeMap::new();
    for _ in 0..days {
        for (_, e) in day(sim) {
            match e {
                Event::Battle { province, winner, loser, killed } => {
                    out.insert(province, (winner, loser, killed));
                }
                Event::Arrived { province, people, cause: Cause::Returned } => *home.entry(province).or_default() += people,
                _ => {}
            }
        }
    }
    (out, home)
}

#[test]
fn battles_go_to_the_stronger_side_and_a_broken_side_lost_about_three_tenths() {
    let (staging, a, b) = staged(11);
    let edge = 1.0 + TroopParams::default().home_edge;
    for (ratio, attacker_wins) in [(1.5 * edge, true), (0.67 / edge, false)] {
        let (mut sim, placed) = placed(&staging, a, b, ratio);
        assert!(placed.len() >= 5, "{} battles", placed.len());
        let (fought, home) = battles(&mut sim, 25);
        let won = placed.iter().filter(|(p, ..)| fought.get(p).is_some_and(|f| f.0 == a)).count();
        let lost = placed.iter().filter(|(p, ..)| fought.get(p).is_some_and(|f| f.0 == b)).count();
        assert_eq!(won + lost, placed.len(), "every battle ended within 25 days: {fought:?}");
        let share = won as f64 / placed.len() as f64;
        if attacker_wins {
            assert!(share >= 0.9, "1.5 times the defenders' strength wins {won} of {}", placed.len());
        } else {
            assert!(share <= 0.1, "two thirds of it wins {won} of {}", placed.len());
        }
        // A side breaks once it has lost three tenths of what it fought with: the broken attackers'
        // survivors came home (to their home, a province of realm a), between six and seven
        // tenths of them, less the last day's blows.
        let w = &sim.world;
        for &(p, k, size) in &placed {
            if fought[&p].0 == b {
                let alive = home.get(&w.armies.home[k as usize]).copied().unwrap_or(0);
                let share = alive as f64 / size as f64;
                assert!((0.55..=0.7).contains(&share), "province {p}: {alive} of {size} came home");
                assert_eq!(w.armies.size[k as usize], 0, "the broken guards went home");
            }
        }
        assert_eq!(sim.problems, Problems::default());
    }
}

#[test]
fn equal_strength_goes_to_the_side_on_its_own_land() {
    let (staging, a, b) = staged(12);
    let (mut sim, placed) = placed(&staging, a, b, 1.0);
    let (fought, _) = battles(&mut sim, 30);
    let held = placed.iter().filter(|(p, ..)| fought.get(p).is_some_and(|f| f.0 == b)).count();
    assert!(held as f64 >= 0.8 * placed.len() as f64, "the defenders held {held} of {}", placed.len());
    assert_eq!(sim.problems, Problems::default());
}

/// The largest siege days of field companies standing in each province at the day's end.
fn sieges(w: &World) -> BTreeMap<u32, u32> {
    let mut out: BTreeMap<u32, u32> = BTreeMap::new();
    for k in 0..w.armies.count() {
        if w.armies.size[k] > 0 && w.armies.role[k] == FIELD && w.armies.next[k] == NONE {
            let e = out.entry(w.armies.province[k]).or_default();
            *e = (*e).max(w.armies.siege_days[k]);
        }
    }
    out
}

#[test]
fn marching_keeps_to_the_ways_and_allowed_land_sieges_take_the_provinces_time_and_occupation_comes_only_in_war() {
    let mut sim = strict(&config(13, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    let p = TroopParams::default();
    let speeds: Vec<f32> = {
        let t = tree();
        (0..t.nodes.len() as u16).map(|n| p.kinds.get(t.name(n)).map_or(1.0, |k| k.speed as f32)).collect()
    };
    let mut was: BTreeMap<u32, (u32, f32)> = BTreeMap::new();
    // The ends of the last two days: siege days, and people.
    let mut ends = vec![(sieges(&sim.world), sim.world.province_population()); 2];
    let (mut taken, mut marched, mut hops) = (0, 0, 0);
    for _ in 0..2 * 364 {
        let events = day(&mut sim);
        let w = &sim.world;
        // Sent yesterday: the end of the day before it is the first of the two.
        let (yesterday, people) = &ends[0];
        for (emitted, e) in &events {
            if let Event::Occupied { province, by } = *e {
                let pu = province as usize;
                let owner = w.owner[pu];
                // Sent only between realms at war (the peace hasn't come yet in this run).
                assert!(at_war(w, by, owner) || owner == by, "province {province} occupied by {by} out of war");
                let mut middle: Vec<u64> = people.iter().copied().filter(|&x| x > 0).collect();
                middle.sort_unstable();
                let middle = middle[middle.len() / 2] as f64;
                let mut need = p.siege_base_days * (1.0 + people[pu] as f64 / middle);
                if w.realms.capital[owner as usize] == province {
                    need *= p.capital_siege;
                }
                need = need.min(p.siege_max_days);
                if owner == by {
                    need *= p.free_siege;
                }
                // The day before it was sent, the besiegers had one day to go (people change a
                // little from day to day, so give or take a day).
                let had = yesterday.get(&province).copied().unwrap_or(0) as f64;
                assert!((had + 1.0 - need.round().max(1.0)).abs() <= 1.0, "day {emitted}: province {province} after {had} + 1 days, needs {need:.1}");
                taken += 1;
            }
        }
        for k in 0..w.armies.count() {
            let next = w.armies.next[k];
            if w.armies.size[k] == 0 || next == NONE {
                was.remove(&(k as u32));
                continue;
            }
            let (at, realm) = (w.armies.province[k], w.armies.realm[k]);
            let link = w.routes.between(at, next).expect("on a way between neighbours");
            let full = link.land_days.into_iter().chain(link.sea_days).fold(0.0f32, f32::max);
            let left = w.armies.days_left[k];
            assert!(left > 0.0 && left <= full / speeds[w.armies.kind[k] as usize].min(1.0) + 1e-3, "company {k}: {left} days left of a {full}-day way");
            let owner = w.owner[next as usize];
            let allowed = owner == realm || owner == NONE || w.occupier[next as usize] == realm || at_war(w, realm, owner);
            assert!(allowed, "company {k} of realm {realm} goes into {next}, of realm {owner}, not an enemy");
            // On one way, a day takes one day off.
            match was.insert(k as u32, (next, left)) {
                Some((n, l)) if n == next => {
                    assert!((l - 1.0 - left).abs() < 1e-3, "company {k}: {l} then {left} days left");
                    marched += 1;
                }
                _ => hops += 1,
            }
        }
        ends.remove(0);
        ends.push((sieges(w), w.province_population()));
    }
    assert!(taken >= 2 && marched >= 100 && hops >= 50, "{taken} provinces taken or freed, {marched} days marched, {hops} ways begun");

    // After the peace nobody besieges or takes anything between them.
    sim.poke(Event::ForcePeace { a, b, winner: NONE });
    sim.run_days(2);
    for _ in 0..90 {
        for (_, e) in day(&mut sim) {
            assert!(!matches!(e, Event::Occupied { .. } | Event::Battle { .. }), "{e:?} after the peace");
        }
    }
    let w = &sim.world;
    assert!(w.occupier.iter().all(|&o| o == NONE));
    assert!((0..w.armies.count()).all(|k| w.armies.role[k] == GUARD || w.armies.size[k] == 0), "every field company went home");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn recruits_gather_at_home_and_a_company_away_never_grows() {
    let mut sim = strict(&config(14, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    let away = |w: &World, k: usize| w.armies.group[k] != NONE || w.armies.next[k] != NONE || w.armies.province[k] != w.armies.home[k];
    let mut sizes = sim.world.armies.size.clone();
    let mut was: Vec<bool> = (0..sizes.len()).map(|k| away(&sim.world, k)).collect();
    let mut away_days = 0;
    for _ in 0..300 {
        sim.run_days(1);
        let w = &sim.world;
        // Away yesterday and still away today: recruits came to companies at home only.
        for k in 0..sizes.len() {
            if was[k] && away(w, k) && w.armies.size[k] > 0 {
                assert!(w.armies.size[k] <= sizes[k], "company {k}, away, grew from {} to {}", sizes[k], w.armies.size[k]);
                away_days += 1;
            }
        }
        sizes = w.armies.size.clone();
        was = (0..sizes.len()).map(|k| away(w, k)).collect();
    }
    assert!(away_days > 1000, "{away_days}");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_peace_with_one_enemy_brings_soldiers_back_and_the_last_peace_sends_them_home() {
    let mut sim = strict(&config(15, false));
    sim.run_days(2);
    let w = &sim.world;
    // A realm bordering two others.
    let (a, b, c) = (0..w.realms.count() as u32)
        .find_map(|a| {
            let mut near: Vec<u32> = land_of(w, a).iter().flat_map(|&p| w.routes.of(p).filter(|l| l.land_days.is_some()).map(move |l| w.owner[l.other(p) as usize])).filter(|&o| o != NONE && o != a).collect();
            near.sort_unstable();
            near.dedup();
            (near.len() >= 2).then(|| (a, near[0], near[1]))
        })
        .expect("a realm with two neighbours");
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.poke(Event::DeclareWar { attacker: a, defender: c });
    let on_land_of = |w: &World, x: u32| (0..w.armies.count()).filter(|&k| w.armies.realm[k] == a && w.armies.size[k] > 0 && w.armies.next[k] == NONE && w.owner[w.armies.province[k] as usize] == x).count();
    let mut found = false;
    for _ in 0..500 {
        sim.run_days(1);
        if on_land_of(&sim.world, b) > 0 {
            found = true;
            break;
        }
    }
    assert!(found, "realm {a}'s soldiers never stood on realm {b}'s land");
    sim.poke(Event::ForcePeace { a, b, winner: NONE });
    sim.run_days(60);
    let w = &sim.world;
    assert_eq!(on_land_of(w, b), 0, "back from the former enemy's land");
    let field: u64 = (0..w.armies.count()).filter(|&k| w.armies.realm[k] == a && w.armies.role[k] == FIELD).map(|k| w.armies.size[k]).sum();
    assert!(field > 0, "still at war with {c}: the field companies stay");
    assert!(sim.history.iter().all(|y| y.ledger.killed < y.population), "sanity");
    sim.poke(Event::ForcePeace { a, b: c, winner: NONE });
    sim.run_days(3);
    let w = &sim.world;
    let field: u64 = (0..w.armies.count()).filter(|&k| w.armies.realm[k] == a && w.armies.role[k] == FIELD).map(|k| w.armies.size[k]).sum();
    assert_eq!(field, 0, "no war left: every field company went home");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_rebellion_ends_when_the_old_realm_takes_the_rebels_capital_and_battles_stir_unrest_once() {
    let mut sim = strict(&config(16, true));
    // Through a month's first day, then make a chain of three provinces of a big realm rise.
    let month = |sim: &mut Sim| {
        let start = (sim.day..).find(|&d| sim.calendar.is_month_start(d)).unwrap();
        sim.run_days(start + 1 - sim.day);
    };
    month(&mut sim);
    let w = &sim.world;
    let people = w.province_population();
    let big = (0..w.realms.count() as u32).max_by_key(|&r| land_of(w, r).len()).unwrap();
    let cap = w.realms.capital[big as usize];
    let mut chain = vec![];
    for &start in land_of(w, big).iter().rev() {
        chain = vec![start];
        let mut k = 0;
        while k < chain.len() && chain.len() < 3 {
            let p = chain[k];
            for l in w.routes.of(p) {
                let q = l.other(p);
                if chain.len() < 3 && w.owner[q as usize] == big && q != cap && people[q as usize] > 0 && !chain.contains(&q) {
                    chain.push(q);
                }
            }
            k += 1;
        }
        if chain.len() == 3 && !chain.contains(&cap) && chain.iter().all(|&p| people[p as usize] > 0) {
            break;
        }
    }
    assert_eq!(chain.len(), 3);
    for &p in &chain {
        sim.world.unrest[p as usize] = LINE;
    }
    let rebels = sim.world.realms.count() as u32;
    month(&mut sim);
    assert_eq!(sim.world.realms.broke_from.get(rebels as usize), Some(&big), "they broke away");
    let capital = sim.world.realms.capital[rebels as usize];
    let years = 5 * sim.calendar.days_per_year() as u64;
    let began = sim.day;
    let mut ended = None;
    let mut stirred = 0;
    while sim.day < began + years && ended.is_none() {
        let before = sim.world.unrest.clone();
        let month_start = sim.calendar.is_month_start(sim.day);
        let held = sim.world.occupier[capital as usize] == big;
        for (_, e) in day(&mut sim) {
            match e {
                Event::Peace { winner, attacker, rebellion: true, .. } if attacker == rebels => {
                    assert!(held, "the old realm held the rebels' capital when it won");
                    ended = Some(winner);
                }
                Event::Battle { province, killed, .. } if !month_start => {
                    // Delivered today, so today's unrest is yesterday's plus a month of
                    // occupation's worth, once, whatever the battle's length (unless at the line).
                    let pu = province as usize;
                    let add = RealmParams::default().unrest.unwrap().occupied.round() as u64;
                    let now = sim.world.unrest[pu];
                    if now < LINE && sim.world.owner[pu] != NONE {
                        assert!(now >= before[pu] + add && now <= before[pu] + 2 * add, "province {province}: unrest {} then {now}", before[pu]);
                        stirred += 1;
                    }
                    assert!(killed > 0, "a battle in {province} killed nobody");
                }
                _ => {}
            }
        }
    }
    assert_eq!(ended, Some(big), "put down within five years");
    assert!(stirred > 0, "no battle was seen to stir unrest");
    assert!(chain.iter().all(|&p| sim.world.owner[p as usize] == big), "their land went back");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn armies_on_the_march_in_battle_and_besieging_survive_saving_loading_and_going_back() {
    let fresh = || {
        let mut sim = strict(&config(17, false));
        let (a, b) = rivals(&sim.world, 4);
        sim.schedule(3, Event::DeclareWar { attacker: a, defender: b }).unwrap();
        sim
    };
    let mut sim = fresh();
    // Stop on days with companies on the move, battles and sieges going on.
    let mut stops = Vec::new();
    while stops.len() < 4 && sim.day < 600 {
        sim.run_days(1);
        let w = &sim.world;
        let moving = (0..w.armies.count()).any(|k| w.armies.next[k] != NONE && w.armies.size[k] > 0);
        let besieging = (0..w.armies.count()).any(|k| w.armies.siege_days[k] > 3);
        let fighting = sim.event_log.entries.iter().any(|p| matches!(p.event, Event::Battle { .. }) && p.day + 3 >= sim.day);
        if moving && besieging && fighting && stops.last().is_none_or(|&d| sim.day > d + 40) {
            stops.push(sim.day);
        }
    }
    assert!(stops.len() >= 2, "{stops:?}");
    let mut prints = Vec::new();
    for &d in &stops {
        sim.goto(d).unwrap();
        let mut loaded = Sim::load(&sim.save()).unwrap();
        loaded.strict = true;
        let mut twin = fresh();
        twin.run_days(d + 30);
        loaded.run_days(30);
        assert_eq!(loaded.fingerprint(), twin.fingerprint(), "saved on day {d}");
        prints.push(sim.fingerprint());
    }
    let (today, now) = (sim.day, sim.fingerprint());
    for (&d, &print) in stops.iter().zip(&prints).rev() {
        sim.goto(d).unwrap();
        assert_eq!(sim.fingerprint(), print, "back to day {d}");
    }
    sim.goto(today).unwrap();
    assert_eq!(sim.fingerprint(), now, "forward again to day {today}");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn troops_need_a_sea_class_and_sound_battle_numbers() {
    let refused = |f: &dyn Fn(&mut TroopParams)| {
        let mut c = config(1, false);
        f(c.modules.warfare.as_mut().unwrap());
        Sim::new(&c).err().expect("refused").0
    };
    assert!(refused(&|p| p.sea = "navy".into()).contains("sea class navy"));
    assert!(refused(&|p| p.kinds.get_mut("cavalry").unwrap().speed = 0.0).contains("strength and speed"));
    assert!(refused(&|p| p.break_share = 0.0).contains("breaking share"));
    assert!(refused(&|p| p.siege_max_days = 0.0).contains("siege days"));
    // The manifest grows the army fields, runs daily, and sends battles and occupations.
    let sim = Sim::new(&config(1, false)).unwrap();
    let manifests = sim.manifests();
    let m = manifests.iter().find(|m| m.slot == Slot::Warfare).unwrap();
    assert!(m.owns.contains(&"army.siege_days") && m.cadence == crate::module::Cadence::Daily);
}

#[test]
fn soldiers_cross_the_sea_only_with_ships_enough_to_carry_them() {
    let t = tree();
    let p = TroopParams::default();
    let ships = t.leaves_under(t.find(&p.sea).unwrap());
    let carry: Vec<f64> = (0..t.nodes.len() as u16).map(|n| p.kinds.get(t.name(n)).map_or(0.0, |k| k.carry)).collect();
    // Each day: soldiers at sea never outnumber what the ships with them carry. Returns the
    // company-days spent at sea.
    let watch = |sim: &mut Sim, days: u64| -> u64 {
        let mut afloat = 0;
        for _ in 0..days {
            sim.run_days(1);
            let w = &sim.world;
            let mut on: BTreeMap<(u32, u32, u32), (u64, f64)> = BTreeMap::new();
            for k in 0..w.armies.count() {
                let (at, next) = (w.armies.province[k], w.armies.next[k]);
                if w.armies.size[k] == 0 || next == NONE || w.routes.between(at, next).is_none_or(|l| l.land_days.is_some()) {
                    continue;
                }
                let e = on.entry((w.armies.realm[k], at, next)).or_default();
                if ships.contains(&w.armies.kind[k]) {
                    e.1 += w.armies.size[k] as f64 * carry[w.armies.kind[k] as usize];
                } else {
                    e.0 += w.armies.size[k];
                    afloat += 1;
                }
            }
            for ((r, at, next), (soldiers, room)) in on {
                assert!(soldiers as f64 <= room + 1e-6, "day {}: realm {r} has {soldiers} soldiers at sea from {at} to {next} with room for {room}", sim.day);
            }
        }
        afloat
    };
    // Realms whose coasts face each other with no land between them, at war, on worlds in turn
    // until one crosses: a small coast can't gather enough to cross to a big one (a province takes
    // soldiers up to a tenth of its people), so not every such war does.
    let mut tried = Vec::new();
    for seed in 20..45 {
        let mut sim = strict(&config(seed, false));
        sim.run_days(2);
        let w = &sim.world;
        let by_land = |x: u32, y: u32| w.routes.links.iter().any(|l| l.land_days.is_some() && [(x, y), (y, x)].contains(&(w.owner[l.a as usize], w.owner[l.b as usize])));
        let mut pairs: Vec<(u32, u32)> = w
            .routes
            .links
            .iter()
            .filter(|l| l.sea_days.is_some() && l.land_days.is_none())
            .map(|l| (w.owner[l.a as usize], w.owner[l.b as usize]))
            .filter(|&(x, y)| x != NONE && y != NONE && x != y && !by_land(x, y))
            .map(|(x, y)| (x.min(y), x.max(y)))
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        if pairs.is_empty() {
            continue;
        }
        for (a, b) in pairs {
            sim.poke(Event::DeclareWar { attacker: a, defender: b });
        }
        let afloat = watch(&mut sim, 2 * 364);
        assert_eq!(sim.problems, Problems::default());
        tried.push(seed);
        if afloat > 0 {
            return;
        }
    }
    panic!("no crossing on worlds {tried:?}");
}
