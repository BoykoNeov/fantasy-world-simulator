//! Unrest and rebellion (step 7): hunger, occupation and bad successions stir provinces; they rise
//! up at the line, their neighbours join, uprisings die down, and enough of them holding together
//! break away as a realm at war with the one they left. Famines and successions are poked in, as
//! the pops and the rulers send them, so the tests needn't wait for a drought.

use crate::branch::Timelines;
use crate::characters::{RulerParams, Succession};
use crate::config::Config;
use crate::diplomacy::{at_war, WarCause, WarParams, NEVER};
use crate::events::Event;
use crate::grid::NONE;
use crate::module::Slot;
use crate::realms::{RealmParams, UnrestParams, LINE};
use crate::requests::Cause;
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};

fn config(economics: &str, seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.without_realms();
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.characters = Some(RulerParams::default());
    c.modules.diplomacy = Some(WarParams::default());
    c
}

fn world(seed: u64) -> Sim {
    let mut sim = Sim::new(&config("food", seed)).unwrap();
    sim.strict = true;
    sim
}

fn defaults() -> UnrestParams {
    UnrestParams::default()
}

fn land_of(w: &World, r: u32) -> Vec<u32> {
    (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r).collect()
}

/// Run through the next month's first day: today is then its second day.
fn run_month_start(sim: &mut Sim) {
    let start = (sim.day..).find(|&d| sim.calendar.is_month_start(d)).unwrap();
    sim.run_days(start + 1 - sim.day);
}

/// `len` provinces of one realm, none its capital, all with people, that hold together by the
/// province ways: found by walking out from each province in turn.
fn chain(w: &World, len: usize) -> (u32, Vec<u32>) {
    let people = w.province_population();
    let ok = |p: u32, r: u32| w.owner[p as usize] == r && w.realms.capital[r as usize] != p && people[p as usize] > 0;
    for start in 0..w.provinces.count as u32 {
        let r = w.owner[start as usize];
        if r == NONE || !ok(start, r) {
            continue;
        }
        let mut got = vec![start];
        let mut k = 0;
        while k < got.len() && got.len() < len {
            let p = got[k];
            for l in w.routes.of(p) {
                let q = l.other(p);
                if got.len() < len && ok(q, r) && !got.contains(&q) {
                    got.push(q);
                }
            }
            k += 1;
        }
        if got.len() == len {
            return (r, got);
        }
    }
    panic!("no {len} provinces of one realm hold together");
}

fn events(sim: &Sim) -> Vec<Event> {
    sim.event_log.entries.iter().map(|p| p.event.clone()).collect()
}

#[test]
fn hunger_stirs_unrest_by_the_share_starved_and_quiet_years_calm_it() {
    let mut sim = world(1);
    run_month_start(&mut sim);
    let (_, ps) = chain(&sim.world, 2);
    let (p, q) = (ps[0], ps[1]);
    let people = sim.world.province_population();
    // A month in which 2% of p's people starve, and one in which 1% of q's do.
    let mut twin = sim.fork();
    for (province, share) in [(p, 0.02), (q, 0.01)] {
        let starved = (people[province as usize] as f64 * share / (1.0 - share)).round() as u64;
        sim.poke(Event::Famine { province, starved });
    }
    run_month_start(&mut sim);
    run_month_start(&mut twin);
    let u = defaults();
    let added = |sim: &Sim, twin: &Sim, p: u32| sim.world.unrest[p as usize] as f64 - twin.world.unrest[p as usize] as f64;
    assert!((added(&sim, &twin, p) - u.famine).abs() <= 1.0, "{} for a full month of famine", added(&sim, &twin, p));
    assert!((added(&sim, &twin, q) - u.famine / 2.0).abs() <= 1.0, "{} for half of one", added(&sim, &twin, q));

    // Two quiet years halve it.
    let before = sim.world.unrest[p as usize] - twin.world.unrest[p as usize];
    for _ in 0..2 * sim.calendar.months_per_year() {
        run_month_start(&mut sim);
        run_month_start(&mut twin);
    }
    let after = sim.world.unrest[p as usize] as f64 - twin.world.unrest[p as usize] as f64;
    assert!((after - before as f64 / 2.0).abs() <= 3.0, "{before} fell to {after} in two years");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_province_at_the_line_rises_up_and_restless_neighbours_join_it_month_by_month() {
    let mut sim = world(2);
    run_month_start(&mut sim);
    let (r, ps) = chain(&sim.world, 3);
    let u = defaults();
    // p reaches the line; q, next to it, is restless enough to join; s, next to q, is not.
    let (p, q, s) = (ps[0] as usize, ps[1] as usize, ps[2] as usize);
    let w = &mut sim.world;
    (w.unrest[p], w.unrest[q], w.unrest[s]) = (LINE, u.join + 100, u.join - 100);
    run_month_start(&mut sim);
    let w = &sim.world;
    assert_eq!((w.uprising[p], w.uprising[q], w.uprising[s]), (p as u32, NONE, NONE), "p rises up; q joins only the month after");
    run_month_start(&mut sim);
    let w = &sim.world;
    assert_eq!((w.uprising[p], w.uprising[q], w.uprising[s]), (p as u32, p as u32, NONE));
    assert!(w.realms.broke_from.iter().all(|&b| b == NONE), "two provinces don't break away");
    assert_eq!(w.owner[p], r);
    sim.run_days(1);
    let told = events(&sim);
    assert!(told.contains(&Event::Uprising { province: p as u32, realm: r, seat: p as u32 }));
    assert!(told.contains(&Event::Uprising { province: q as u32, realm: r, seat: p as u32 }));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn an_uprising_dies_down_once_unrest_falls_below_the_calm() {
    let mut sim = world(3);
    run_month_start(&mut sim);
    let (r, ps) = chain(&sim.world, 1);
    let p = ps[0] as usize;
    sim.world.unrest[p] = LINE;
    run_month_start(&mut sim);
    assert_eq!(sim.world.uprising[p], p as u32);
    // At the calm it goes on; below it, it dies down.
    sim.world.unrest[p] = defaults().calm;
    run_month_start(&mut sim);
    assert_eq!(sim.world.uprising[p], p as u32);
    sim.world.unrest[p] = defaults().calm - 1;
    run_month_start(&mut sim);
    assert_eq!(sim.world.uprising[p], NONE);
    sim.run_days(1);
    assert!(events(&sim).contains(&Event::Calmed { province: p as u32, realm: r }));
}

#[test]
fn capitals_and_occupied_land_never_rise_and_occupation_stirs_unrest() {
    let mut sim = world(4);
    run_month_start(&mut sim);
    let (r, ps) = chain(&sim.world, 1);
    let (capital, p) = (sim.world.realms.capital[r as usize] as usize, ps[0] as usize);
    // An enemy next door occupies p.
    let enemy = (0..sim.world.realms.count() as u32).find(|&e| e != r && sim.world.realms.alive(e)).unwrap();
    sim.poke(Event::DeclareWar { attacker: enemy, defender: r });
    sim.run_days(2);
    sim.poke(Event::Occupied { province: p as u32, by: enemy });
    sim.run_days(2);
    let mut twin = sim.fork();
    twin.poke(Event::Occupied { province: p as u32, by: r });
    run_month_start(&mut sim);
    run_month_start(&mut twin);
    let added = sim.world.unrest[p] as f64 - twin.world.unrest[p] as f64;
    assert!((added - defaults().occupied).abs() <= 1.0, "occupation added {added}");

    (sim.world.unrest[capital], sim.world.unrest[p]) = (LINE, LINE);
    run_month_start(&mut sim);
    assert_eq!((sim.world.uprising[capital], sim.world.uprising[p]), (NONE, NONE));
    // Freed, it keeps its unrest, and at the line rises up.
    sim.poke(Event::Occupied { province: p as u32, by: r });
    sim.run_days(2);
    assert!(sim.world.unrest[p] > LINE * 9 / 10, "{}", sim.world.unrest[p]);
    sim.world.unrest[p] = LINE;
    run_month_start(&mut sim);
    assert_eq!(sim.world.uprising[p], p as u32);
    // Taken again, its uprising is put down.
    sim.poke(Event::Occupied { province: p as u32, by: enemy });
    sim.run_days(2);
    assert_eq!(sim.world.uprising[p], NONE);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_province_in_uprising_pays_no_taxes_and_is_paid_nothing() {
    let mut sim = Sim::new(&config("markets", 5)).unwrap();
    sim.strict = true;
    sim.run_days(100);
    run_month_start(&mut sim);
    let (r, ps) = chain(&sim.world, 1);
    let p = ps[0];
    sim.world.unrest[p as usize] = LINE;
    run_month_start(&mut sim);
    assert_eq!(sim.world.uprising[p as usize], p);
    run_month_start(&mut sim);
    let taxed: Vec<_> = sim.applied_today.iter().filter(|(q, _)| q.cause == Cause::Taxes).collect();
    let spent: Vec<_> = sim.applied_today.iter().filter(|(q, _)| q.cause == Cause::Spent).collect();
    assert!(!taxed.iter().any(|(q, _)| q.entity == p) && !spent.iter().any(|(q, _)| q.to.unwrap().entity == p));
    assert!(taxed.iter().any(|(q, _)| q.to.unwrap().entity == r), "the rest of the realm pays");
    assert_eq!(sim.problems, Problems::default());
}

/// Three provinces of a realm, holding together, all at the line: they rise up and break away the
/// same month. Returns the sim the day after, the old realm, the rebels' realm and its land.
fn breakaway(seed: u64, wars: bool) -> (Sim, u32, u32, Vec<u32>) {
    let mut c = config("food", seed);
    if !wars {
        c.modules.diplomacy = None;
    }
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    run_month_start(&mut sim);
    let (from, ps) = chain(&sim.world, 3);
    for &p in &ps {
        sim.world.unrest[p as usize] = LINE;
    }
    let realm = sim.world.realms.count() as u32;
    run_month_start(&mut sim);
    let w = &sim.world;
    assert_eq!(w.realms.count() as u32, realm + 1, "a new realm");
    assert!(ps.iter().all(|&p| w.owner[p as usize] == realm && w.uprising[p as usize] == NONE && w.unrest[p as usize] == 0));
    assert_eq!(w.realms.broke_from[realm as usize], from);
    let people = w.province_population();
    let capital = *ps.iter().max_by_key(|&&p| (people[p as usize], std::cmp::Reverse(p))).unwrap();
    assert_eq!(w.realms.capital[realm as usize], capital, "its most populous province");
    assert!(w.realms.ruler[realm as usize] != NONE, "the rebels have a ruler the same day");
    assert!(w.realms.broke_from.iter().enumerate().all(|(r, &b)| (r as u32 == realm) == (b != NONE)));
    sim.run_days(1);
    assert!(events(&sim).contains(&Event::Rebellion { realm, from, capital, provinces: 3 }));
    (sim, from, realm, ps)
}

#[test]
fn provinces_in_uprising_that_hold_together_break_away_at_war_with_their_old_realm() {
    let (sim, from, realm, _) = breakaway(6, true);
    let w = &sim.world;
    assert!(at_war(w, realm, from));
    let war = w.wars.current(realm, from).unwrap() as usize;
    assert_eq!((w.wars.attacker[war], w.wars.defender[war], WarCause::from_u8(w.wars.cause[war])), (realm, from, WarCause::Rebellion));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn rebels_put_down_go_back_to_their_old_realm_and_rebels_left_alone_win_after_five_years() {
    // The old realm wins: all the rebels' land goes back, and the rebel realm is no more.
    let (mut sim, from, realm, ps) = breakaway(7, true);
    sim.poke(Event::ForcePeace { a: from, b: realm, winner: from });
    sim.run_days(3);
    let w = &sim.world;
    assert!(ps.iter().all(|&p| w.owner[p as usize] == from));
    assert!(!w.realms.alive(realm));
    assert!(events(&sim).iter().any(|e| matches!(e, Event::Peace { winner, rebellion: true, .. } if *winner == from)));

    // Nobody wins: the rebels stay free.
    let (mut sim, from, realm, ps) = breakaway(7, true);
    sim.poke(Event::ForcePeace { a: from, b: realm, winner: NONE });
    sim.run_days(3);
    assert!(ps.iter().all(|&p| sim.world.owner[p as usize] == realm));

    // Left alone, the rebels have won their freedom after five years; a later war the old realm
    // wins takes only what it occupies.
    let (mut sim, from, realm, ps) = breakaway(7, true);
    let war = sim.world.wars.current(realm, from).unwrap() as usize;
    let began = sim.world.wars.began[war];
    let five = 5 * sim.calendar.days_per_year() as u64;
    sim.run_days(began + five - sim.day);
    assert_eq!(sim.world.wars.ended[war], NEVER, "not before five years");
    run_month_start(&mut sim);
    sim.run_days(1);
    let w = &sim.world;
    assert_eq!(w.wars.winner[war], realm, "the rebels won");
    assert!(ps.iter().all(|&p| w.owner[p as usize] == realm));
    let truce = w.wars.truce_until[war];
    sim.run_days(truce + 1 - sim.day);
    sim.poke(Event::DeclareWar { attacker: from, defender: realm });
    sim.run_days(2);
    let kept = ps[0];
    sim.poke(Event::Occupied { province: kept, by: from });
    sim.run_days(2);
    sim.poke(Event::ForcePeace { a: from, b: realm, winner: from });
    sim.run_days(3);
    let w = &sim.world;
    let mine = land_of(w, realm);
    assert_eq!(w.owner[kept as usize], from);
    assert!(!mine.is_empty() && mine.iter().all(|p| ps.contains(p) && *p != kept), "an ordinary war takes only what it holds");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn without_wars_rebels_are_simply_free() {
    let (sim, _, realm, ps) = breakaway(8, false);
    assert!(sim.world.wars.count() == 0);
    assert!(ps.iter().all(|&p| sim.world.owner[p as usize] == realm));
}

#[test]
fn a_child_heir_or_a_contested_succession_stirs_the_whole_realm() {
    let mut sim = world(9);
    run_month_start(&mut sim);
    let r = (0..sim.world.realms.count() as u32).max_by_key(|&r| land_of(&sim.world, r).len()).unwrap();
    let (capital, ruler) = (sim.world.realms.capital[r as usize], sim.world.realms.ruler[r as usize]);
    let land = land_of(&sim.world, r);
    let before: Vec<u64> = land.iter().map(|&p| sim.world.unrest[p as usize]).collect();
    for how in [Succession::Heir, Succession::Claimant, Succession::Founded] {
        sim.poke(Event::Succession { realm: r, capital, ruler, how });
    }
    sim.run_days(2);
    let after: Vec<u64> = land.iter().map(|&p| sim.world.unrest[p as usize]).collect();
    let add = defaults().claimant as u64;
    assert!(before.iter().zip(&after).all(|(b, a)| *a == (b + add).min(LINE)), "only the contested succession counts: {before:?} {after:?}");
    let other: Vec<u32> = (0..sim.world.provinces.count as u32).filter(|&p| sim.world.owner[p as usize] != r).collect();
    assert!(other.iter().any(|&p| sim.world.unrest[p as usize] < add), "other realms aren't stirred");
}

#[test]
fn unrest_goes_through_saves_going_back_and_scenario_files() {
    let mut sim = world(10);
    sim.run_days(40);
    let (_, ps) = chain(&sim.world, 3);
    // A famine told mid-month, kept until the month's start; then enough to rise up and break away.
    let people = sim.world.province_population();
    for &p in &ps {
        sim.poke(Event::Famine { province: p, starved: people[p as usize] / 10 });
    }
    sim.run_days(3);
    let print = sim.fingerprint();
    let today = sim.day;
    let mut loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), print);
    let year = sim.calendar.days_per_year() as u64;
    sim.run_days(year);
    loaded.run_days(year);
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    assert_eq!(loaded.world.unrest, sim.world.unrest);
    assert!(ps.iter().any(|&p| sim.world.unrest[p as usize] > 0));
    let end = sim.fingerprint();
    sim.goto(today).unwrap();
    assert_eq!(sim.fingerprint(), print);
    sim.goto(today + year).unwrap();
    assert_eq!(sim.fingerprint(), end);

    let text = Timelines::new(sim.fork()).scenario_text();
    assert!(text.contains("half_life_years") && text.contains("rebel_war_years"), "the unrest settings are written");
    let mut read = Timelines::from_scenario_text(&text).unwrap();
    let back = read.list.swap_remove(read.active).sim;
    assert_eq!((back.day, back.fingerprint()), (sim.day, sim.fingerprint()));
}

#[test]
fn a_rebellion_war_is_still_one_after_a_save() {
    let (sim, from, realm, _) = breakaway(11, true);
    let loaded = Sim::load(&sim.save()).unwrap();
    let w = &loaded.world;
    let war = w.wars.current(realm, from).unwrap() as usize;
    assert_eq!(WarCause::from_u8(w.wars.cause[war]), WarCause::Rebellion);
    assert_eq!(w.realms.broke_from[realm as usize], from);
}

#[test]
fn scenario_files_from_before_unrest_have_none() {
    let mut c = config("food", 12);
    let text = toml::to_string(&c).unwrap();
    let start = text.find("[modules.realms.unrest]").expect("written");
    // Drop the unrest table, as files written before it lack it.
    let rest = &text[start + 1..];
    let end = rest.find("\n[").map_or(text.len(), |e| start + 1 + e + 1);
    let old = format!("{}{}", &text[..start], &text[end..]);
    let read: Config = toml::from_str(&old).unwrap();
    assert!(read.modules.realms.as_ref().unwrap().unrest.is_none());
    c.modules.realms.as_mut().unwrap().unrest = None;
    assert_eq!(read, c);
}

#[test]
fn bad_unrest_settings_are_refused() {
    let bad = [
        UnrestParams { half_life_years: 0.0, ..defaults() },
        UnrestParams { famine: -1.0, ..defaults() },
        UnrestParams { famine_share: 0.0, ..defaults() },
        UnrestParams { calm: 600, join: 500, ..defaults() },
        UnrestParams { join: LINE + 1, ..defaults() },
        UnrestParams { breakaway: 0, ..defaults() },
    ];
    for unrest in bad {
        let mut c = config("food", 1);
        c.modules.realms = Some(RealmParams { unrest: Some(unrest.clone()), ..RealmParams::default() });
        let e = Sim::new(&c).err().unwrap_or_else(|| panic!("{unrest:?} refused")).to_string();
        assert!(e.contains("unrest"), "{e}");
    }
    let mut c = config("food", 1);
    c.modules.diplomacy = Some(WarParams { rebel_war_years: -1.0, ..WarParams::default() });
    assert!(Sim::new(&c).is_err());
}


/// Story 1's drought, with realms: within 5 years a province it reaches rises up, and in the same
/// world without it none does (seed 21; on Economics v0). The full story-1 check over three seeds
/// and both economies is step 7's checks piece (`examples/unrest_check.rs` measures it now).
#[test]
fn story_1_s_drought_stirs_its_region_to_rise_up() {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed: 21, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.without_realms();
    c.modules.realms = Some(RealmParams::default());
    c.modules.characters = Some(RulerParams::default());
    c.modules.diplomacy = Some(WarParams::default());
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    let start = sim.calendar.date(0).year;
    sim.run_to_year(start + 3);
    let mut calm = sim.fork();
    let p = &sim.world.provinces;
    let heart = (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap();
    let ((lon, lat), n) = (crate::grid::dir_to_lonlat(p.center[heart]), p.count as u32);
    sim.poke(Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days: 3 * 364 });
    let end = sim.day + 5 * sim.calendar.days_per_year() as u64;
    sim.run_days(2);
    let region: Vec<u32> = (0..n).filter(|&i| sim.world.rain_shortfall[i as usize] > 0).collect();
    sim.run_days(end - sim.day);
    calm.run_days(end - calm.day);
    let rose = |s: &Sim| s.event_log.entries.iter().any(|e| matches!(e.event, Event::Uprising { province, seat, .. } if province == seat && region.contains(&province)));
    assert!(rose(&sim), "the drought region rises up");
    assert!(!rose(&calm), "without the drought it doesn't");
    assert_eq!(sim.problems, Problems::default());
}
