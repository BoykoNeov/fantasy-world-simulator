//! Tests of Culture "assimilation" (build step 9), and of the shift requests it makes.

use crate::config::Config;
use crate::culture::{foreign_share, main_cultures, names, CultureParams, SplitParams};
use crate::edit::Edit;
use crate::events::Event;
use crate::grid::NONE;
use crate::realms::RealmParams;
use crate::requests::Cause;
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};

fn config(seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 32, provinces: 120, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.culture = Some(CultureParams { starting: 8, ..CultureParams::default() });
    c
}

fn sim(c: &Config) -> Sim {
    let mut s = Sim::new(c).unwrap();
    s.strict = true;
    s
}

/// People of each culture in each province.
fn people_of(w: &World, p: u32, culture: u32) -> u64 {
    (0..w.pops.size.len()).filter(|&k| w.pops.province[k] == p && w.pops.culture[k] as u32 == culture).map(|k| w.pops.size[k]).sum()
}

fn settled(w: &World) -> Vec<u32> {
    let people = w.province_population();
    (0..w.provinces.count as u32).filter(|&p| people[p as usize] > 0).collect()
}

#[test]
fn cultures_start_by_region_with_mixed_borderlands_and_nobody_made_or_lost() {
    let c = config(7);
    let s = sim(&c);
    let mut off = c.clone();
    off.modules.culture = None;
    let plain = sim(&off);
    // The same people in every province; only their cultures differ.
    assert_eq!(s.world.province_population(), plain.world.province_population());
    assert!(plain.world.cultures.count() == 0 && plain.world.realm_culture.iter().all(|&r| r == NONE));

    let w = &s.world;
    assert_eq!(w.cultures.count(), 8);
    assert!(w.cultures.parent.iter().all(|&p| p == NONE) && w.cultures.born.iter().all(|&b| b == 0) && w.cultures.died.iter().all(|&d| d == u64::MAX));
    let main = main_cultures(w);
    let settled = settled(w);
    assert!(settled.iter().all(|&p| main[p as usize] != NONE));
    // Every culture is the main one somewhere (its hearth, at least), and regions are many
    // provinces, not scattered.
    for c in 0..8u32 {
        let hearth = w.cultures.hearth[c as usize];
        assert_eq!(main[hearth as usize], c, "culture {c}'s hearth");
    }
    let same_as_a_neighbour = settled.iter().filter(|&&p| w.routes.of(p).any(|l| main[l.other(p) as usize] == main[p as usize])).count();
    assert!(same_as_a_neighbour as f64 > 0.8 * settled.len() as f64, "{same_as_a_neighbour} of {}", settled.len());
    // Border provinces hold a share of the neighbouring people; nowhere more than two cultures.
    let mixed: Vec<u32> = settled.iter().copied().filter(|&p| (0..8).filter(|&c| people_of(w, p, c) > 0).count() == 2).collect();
    assert!(!mixed.is_empty());
    for &p in &mixed {
        let total = w.province_population()[p as usize];
        let other: u64 = (0..8).filter(|&c| c != main[p as usize]).map(|c| people_of(w, p, c)).sum();
        assert!((other as f64 / total as f64 - 0.15).abs() < 0.01, "province {p}: {other} of {total}");
    }
    assert!(settled.iter().all(|&p| (0..8).filter(|&c| people_of(w, p, c) > 0).count() <= 2));
    // Realms take their capitals' cultures, and cultures cross realm borders.
    for r in 0..w.realms.count() {
        assert_eq!(w.realm_culture[r], main[w.realms.capital[r] as usize]);
    }
    let crosses = (0..8u32).any(|c| {
        let mut owners: Vec<u32> = settled.iter().filter(|&&p| main[p as usize] == c).map(|&p| w.owner[p as usize]).collect();
        owners.sort();
        owners.dedup();
        owners.len() > 1
    });
    assert!(crosses);
    // Placing them was booked as shifts, before the first day.
    assert!(s.applied_today.iter().all(|(r, _)| r.cause == Cause::Placed));
    // Names: as many as asked, all different, the same from the same seed.
    let n = names(7, 40);
    assert_eq!(n.len(), 40);
    assert_eq!(n, names(7, 40));
    let mut d = n.clone();
    d.sort();
    d.dedup();
    assert_eq!(d.len(), 40);
}

#[test]
fn more_cultures_asked_than_settled_provinces_gives_one_each() {
    let mut c = config(3);
    c.modules.culture.as_mut().unwrap().starting = 1000;
    let s = sim(&c);
    assert_eq!(s.world.cultures.count(), settled(&s.world).len());
}

#[test]
fn people_keep_exact_totals_over_decades_with_wars_and_both_economies() {
    for economics in ["food", "markets"] {
        let mut c = config(21);
        c.modules.set_choice(crate::module::Slot::Economics, economics).unwrap();
        c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, ..RealmParams::default() });
        let mut s = sim(&c);
        s.run_to_year(31);
        assert_eq!(s.problems, Problems::default(), "{economics}");
        let w = &s.world;
        // Every year balanced to the person (people are only moved between cultures), and the
        // table and the pops agree.
        for y in &s.history {
            assert_eq!(y.ledger.people_clamped, 0);
        }
        assert!(w.pops.culture.iter().all(|&c| (c as usize) < w.cultures.count()));
        assert_eq!(w.realm_culture.len(), w.realms.count());
        w.check_lengths().unwrap();
    }
}

#[test]
fn people_take_up_their_rulers_culture_and_events_tell_of_it() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy.as_mut().unwrap().treaties = None;
    c.modules.culture.as_mut().unwrap().splits = None;
    let mut s = sim(&c);
    let start = main_cultures(&s.world);
    let mut assimilated = 0u64;
    let mut told = 0;
    while s.calendar.date(s.day).year < 61 {
        s.step_day();
        assimilated += s.applied_today.iter().filter(|(r, _)| r.cause == Cause::Assimilated).map(|(_, g)| g).sum::<u64>();
        told += s.delivered_today().iter().filter(|p| matches!(p.event, Event::Assimilated { .. })).count();
    }
    assert_eq!(s.problems, Problems::default());
    let w = &s.world;
    let main = main_cultures(w);
    // Provinces held by a realm of another culture: more of them have taken it up than started so.
    let ruled = |main: &[u32]| settled(w).iter().filter(|&&p| w.owner[p as usize] != NONE && main[p as usize] == w.realm_culture[w.owner[p as usize] as usize]).count();
    assert!(ruled(&main) > ruled(&start), "{} then, {} now", ruled(&start), ruled(&main));
    let changed = settled(w).iter().filter(|&&p| main[p as usize] != start[p as usize]).count();
    assert!(assimilated > 0 && changed > 0 && told >= changed / 2, "{assimilated} people, {changed} provinces, {told} told");
}

/// The user's choice: a ruler slowly wins over even a province all of another people, whose
/// neighbours are all of that people too.
#[test]
fn a_ruler_wins_over_even_a_province_wholly_foreign_and_surrounded_by_its_own_kind() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 1000.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy.as_mut().unwrap().treaties = None;
    c.modules.culture.as_mut().unwrap().splits = None;
    let mut s = sim(&c);
    let w = &s.world;
    // A province of a realm, not next to its capital, all of whose neighbours have people.
    let people = w.province_population();
    let p = settled(w)
        .into_iter()
        .find(|&p| {
            let r = w.owner[p as usize];
            r != NONE
                && w.realms.capital[r as usize] != p
                && w.routes.of(p).all(|l| l.days().is_none() || (people[l.other(p) as usize] > 0 && w.realms.capital[r as usize] != l.other(p) && w.routes.of(l.other(p)).all(|m| m.other(l.other(p)) != w.realms.capital[r as usize])))
                && w.routes.of(p).filter(|l| l.days().is_some()).count() >= 3
        })
        .expect("a province inland of its realm");
    let ruler = s.world.realm_culture[s.world.owner[p as usize] as usize];
    let mut pocket: Vec<u32> = s.world.routes.of(p).filter(|l| l.days().is_some()).map(|l| l.other(p)).collect();
    pocket.push(p);
    s.edit(Edit::NewCulture { provinces: pocket }).unwrap();
    let foreign = (s.world.cultures.count() - 1) as u32;
    assert_eq!(people_of(&s.world, p, ruler), 0);
    assert_eq!(people_of(&s.world, p, foreign), s.world.province_population()[p as usize]);
    s.run_days(5 * 365);
    let now = people_of(&s.world, p, ruler);
    assert!(now > 0, "nobody took up the ruler's culture");
    assert!(people_of(&s.world, p, foreign) > now, "slowly: {now} took it up in five years");
}

#[test]
fn cut_off_groups_split_off_as_new_cultures() {
    let mut c = config(21);
    c.modules.without_realms();
    c.modules.culture = Some(CultureParams { starting: 4, per_year: 0.0, splits: Some(SplitParams { years: 2.0, provinces: 3, per_year: 1.0 }), ..CultureParams::default() });
    let mut s = sim(&c);
    let w = &s.world;
    // A pocket of culture 0 far from its heartland: three connected provinces of another culture,
    // none of them next to culture 0's land.
    let main = main_cultures(w);
    let pocket = settled(w)
        .into_iter()
        .filter(|&p| main[p as usize] != 0 && w.routes.of(p).all(|l| main[l.other(p) as usize] != 0))
        .find_map(|p| {
            let near: Vec<u32> = w.routes.of(p).filter(|l| l.days().is_some()).map(|l| l.other(p)).filter(|&q| main[q as usize] != 0 && main[q as usize] != NONE && w.routes.of(q).all(|m| main[m.other(q) as usize] != 0)).take(2).collect();
            (near.len() == 2).then(|| vec![p, near[0], near[1]])
        })
        .expect("a pocket");
    s.edit(Edit::SetCulture { provinces: pocket.clone(), culture: 0 }).unwrap();
    s.run_to_year(5);
    let w = &s.world;
    // (Other groups cut off by the map, an island say, may split too.)
    let new = (4..w.cultures.count()).find(|&k| pocket.contains(&w.cultures.hearth[k])).expect("the pocket split off");
    assert_eq!((w.cultures.parent[new], w.cultures.died[new]), (0, u64::MAX));
    assert_eq!(w.cultures.born[new], 2 * s.calendar.days_per_year() as u64, "cut off for two years, then at once");
    for &p in &pocket {
        assert_eq!(people_of(w, p, 0), 0);
        assert_eq!(people_of(w, p, new as u32), w.province_population()[p as usize]);
    }
    assert!(s.event_log.entries.iter().any(|e| matches!(e.event, Event::CultureBorn { culture, parent: 0, provinces: 3, .. } if culture as usize == new)));
    assert_eq!(s.problems, Problems::default());
}

#[test]
fn edits_set_and_make_cultures_and_a_culture_with_nobody_left_is_gone() {
    let mut c = config(7);
    c.modules.without_realms();
    let mut s = sim(&c);
    let w = &s.world;
    let main = main_cultures(w);
    let all_of_1: Vec<u32> = settled(w).into_iter().filter(|&p| (0..w.pops.size.len()).any(|k| w.pops.province[k] == p && w.pops.culture[k] == 1 && w.pops.size[k] > 0)).collect();
    let rows = w.pops.size.len();
    let people = w.province_population();
    // Everyone of culture 1 takes culture 2: nobody is lost, no pop is removed.
    s.edit(Edit::SetCulture { provinces: all_of_1.clone(), culture: 2 }).unwrap();
    assert_eq!(s.world.province_population(), people);
    assert!(s.world.pops.size.len() >= rows);
    assert!(s.world.pops.culture.iter().zip(&s.world.pops.size).all(|(&c, &x)| c != 1 || x == 0));
    for &p in &all_of_1 {
        assert_eq!(people_of(&s.world, p, 2), people[p as usize]);
    }
    // Culture 1 is gone at the next month's start, and told so.
    s.run_days(40);
    assert_ne!(s.world.cultures.died[1], u64::MAX);
    assert!(s.event_log.entries.iter().any(|e| matches!(e.event, Event::CultureGone { culture: 1 })));
    // Given people again, it lives again.
    s.edit(Edit::SetCulture { provinces: vec![all_of_1[0]], culture: 1 }).unwrap();
    assert_eq!(s.world.cultures.died[1], u64::MAX);

    // A new culture, born today in the most populous of the provinces.
    let two: Vec<u32> = settled(&s.world).into_iter().filter(|&p| main[p as usize] == 0).take(2).collect();
    s.edit(Edit::NewCulture { provinces: two.clone() }).unwrap();
    let w = &s.world;
    let new = w.cultures.count() - 1;
    let pp = w.province_population();
    let most = *two.iter().max_by(|&&a, &&b| pp[a as usize].cmp(&pp[b as usize]).then(b.cmp(&a))).unwrap();
    assert_eq!((w.cultures.parent[new], w.cultures.born[new], w.cultures.hearth[new]), (NONE, s.day, most));
    assert!(two.iter().all(|&p| people_of(w, p, new as u32) == pp[p as usize]));
    // Refused: a culture that doesn't exist, provinces with no land.
    assert!(s.edit(Edit::SetCulture { provinces: vec![two[0]], culture: 999 }).unwrap_err().contains("no culture"));
    assert!(s.edit(Edit::NewCulture { provinces: vec![] }).is_err());
    s.run_days(400);
    assert_eq!(s.problems, Problems::default());

    // With culture off there are no cultures to set.
    let mut off = config(7);
    off.modules.culture = None;
    let mut plain = sim(&off);
    assert!(plain.edit(Edit::NewCulture { provinces: vec![two[0]] }).unwrap_err().contains("culture.hearth"));
}

#[test]
fn saved_loaded_gone_back_and_replayed_worlds_are_the_same() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, ..RealmParams::default() });
    c.modules.culture.as_mut().unwrap().splits = Some(SplitParams { years: 3.0, provinces: 2, per_year: 0.5 });
    let mut s = sim(&c);
    s.run_to_year(8);
    let p = settled(&s.world)[3];
    s.edit(Edit::NewCulture { provinces: vec![p] }).unwrap();
    s.run_days(200);
    let mut loaded = Sim::load(&s.save()).unwrap();
    assert_eq!(loaded.fingerprint(), s.fingerprint());
    let (day, end) = (s.day, {
        s.run_days(3 * 365);
        loaded.run_days(3 * 365);
        s.fingerprint()
    });
    assert_eq!(loaded.fingerprint(), end);
    // Back before the edit and forward again: the same world.
    s.goto(day - 400).unwrap();
    s.goto(day + 3 * 365).unwrap();
    assert_eq!(s.fingerprint(), end);
    // Run again from the start with the same commands: the same world.
    let mut again = sim(&c);
    again.run_to_year(8);
    again.edit(Edit::NewCulture { provinces: vec![p] }).unwrap();
    again.run_days(200 + 3 * 365);
    assert_eq!(again.fingerprint(), end);
}

#[test]
fn without_realms_neighbours_still_pull_and_there_is_no_ruler() {
    let mut c = config(7);
    c.modules.without_realms();
    let mut s = sim(&c);
    assert!(s.world.realm_culture.is_empty());
    let before: u64 = foreign_share(&s.world).iter().sum();
    assert_eq!(before, 0, "no realms, nobody is ruled by foreigners");
    let mut moved = 0u64;
    for _ in 0..20 * 365 {
        s.step_day();
        moved += s.applied_today.iter().filter(|(r, _)| r.cause == Cause::Assimilated).map(|(_, g)| g).sum::<u64>();
    }
    assert!(moved > 0);
    assert_eq!(s.problems, Problems::default());
}

#[test]
fn foreign_share_counts_people_not_of_their_realms_culture() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, ..RealmParams::default() });
    let s = sim(&c);
    let w = &s.world;
    let share = foreign_share(w);
    let main = main_cultures(w);
    let people = w.province_population();
    for p in 0..w.provinces.count {
        let r = w.owner[p];
        if r == NONE || people[p] == 0 {
            assert_eq!(share[p], 0);
            continue;
        }
        let ours = people_of(w, p as u32, w.realm_culture[r as usize]);
        assert_eq!(share[p], ((people[p] - ours) as u128 * 1000 + people[p] as u128 / 2) as u64 / people[p]);
        if w.realms.capital[r as usize] == p as u32 {
            assert!(share[p] < 500, "a capital is mostly of its own culture");
        }
    }
    assert!(share.iter().any(|&x| x > 500) && main.iter().any(|&m| m != NONE));
    // Without culture, everyone is of one culture: nobody is foreign.
    let mut off = c.clone();
    off.modules.culture = None;
    assert!(foreign_share(&sim(&off).world).iter().all(|&x| x == 0));
}

/// Culture's own cost on a month-start day of a large world (5,000 provinces) with realms and
/// wars, a decade in: the module's tick alone, timed many times on copies of the same day. Run it
/// by hand: `cargo test --release -p sim culture_speed -- --ignored --nocapture`.
#[test]
#[ignore]
fn culture_speed_on_a_large_world() {
    use crate::module::{Ctx, Module, Outbox, Slot};
    let mut c = Config::new(WorldParams::preset(crate::world::Size::Large, 21));
    c.modules.culture = Some(CultureParams::default());
    let mut s = Sim::new(&c).unwrap();
    s.run_to_year(11);
    let time = |s: &Sim, params: CultureParams, what: &str| {
        let k = s.modules.iter().position(|m| m.manifest().slot == Slot::Culture).unwrap();
        let state = s.modules[k].save_state();
        let goods = s.world.goods.clone();
        let (mut times, mut asked) = (Vec::new(), 0);
        for _ in 0..30 {
            let mut w = s.world.clone();
            let mut m = crate::culture::Assimilation::new(params.clone());
            m.load_state(&state).unwrap();
            let mut out = Outbox::default();
            let mut cx = Ctx { day: s.day, seed: w.params.seed, calendar: &s.calendar, goods: &goods, slot: Slot::Culture, out: &mut out, trace: None };
            let t = std::time::Instant::now();
            m.tick(&mut w, &mut cx);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            asked = out.requests.len();
        }
        times.sort_by(f64::total_cmp);
        println!("{what}: {:.3} ms (fastest {:.3}), {asked} shift requests", times[times.len() / 2], times[0]);
    };
    let settled = s.world.province_population().iter().filter(|&&x| x > 0).count();
    println!("{} provinces ({settled} settled), {} pop rows", s.world.provinces.count, s.world.pops.size.len());
    time(&s, CultureParams::default(), "a year's start (splits looked for)");
    time(&s, CultureParams { per_year: 0.0, splits: None, ..CultureParams::default() }, "  of which bookkeeping alone");
    s.run_days(31);
    time(&s, CultureParams::default(), "another month's start");
}

/// People coming home (soldiers) to a province of several cultures are shared out over its pops by
/// size, not all given to the biggest (the user's choice, Oct 17): they don't swell the majority.
#[test]
fn people_coming_home_to_a_mixed_province_join_every_culture_by_size() {
    let mut c = config(21);
    c.modules.without_realms();
    let mut s = sim(&c);
    s.run_days(10);
    let w = &s.world;
    let mixed = settled(w).into_iter().find(|&p| {
        let of: Vec<usize> = (0..w.pops.size.len()).filter(|&k| w.pops.province[k] == p && w.pops.size[k] > 0).collect();
        of.iter().any(|&k| w.pops.culture[k] != w.pops.culture[of[0]])
    });
    let p = mixed.expect("a borderland province of two cultures");
    let lived: Vec<usize> = (0..w.pops.size.len()).filter(|&k| w.pops.province[k] == p && w.pops.size[k] > 0).collect();
    let before: Vec<u64> = lived.iter().map(|&k| w.pops.size[k]).collect();
    let ids: Vec<u64> = lived.iter().map(|&k| k as u64).collect();
    let parts = crate::requests::split_proportional(10_000, &before, &ids);
    s.poke(Event::Arrived { province: p, people: 10_000, cause: Cause::Returned });
    // Sent today, delivered tomorrow; neither is a month's start, when the pops eat and grow.
    s.run_days(2);
    let after: Vec<u64> = lived.iter().map(|&k| s.world.pops.size[k]).collect();
    let grew: Vec<u64> = after.iter().zip(&before).map(|(a, b)| a - b).collect();
    assert_eq!(grew, parts);
    assert!(grew.iter().filter(|&&g| g > 0).count() >= 2, "more than one pop took them in");
    assert_eq!(s.problems, Problems::default());
}

/// Realms: unrest from foreign rule rises where a province's people are not of its realm's culture,
/// as much as the setting says, and nowhere with the setting at 0.
#[test]
fn foreign_rule_stirs_unrest_by_the_share_of_people_not_of_the_realms_culture() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, ..RealmParams::default() });
    let run = |foreign: f64| {
        let mut c = c.clone();
        c.modules.realms.as_mut().unwrap().unrest.as_mut().unwrap().foreign = foreign;
        let mut s = sim(&c);
        s.run_days(95);
        let share = foreign_share(&s.world);
        let unrest = s.world.unrest.clone();
        (share, unrest)
    };
    let (share, stirred) = run(5.0);
    let (_, calm) = run(0.0);
    let foreign: Vec<usize> = (0..share.len()).filter(|&p| share[p] > 500).collect();
    assert!(!foreign.is_empty());
    let sum = |u: &[u64]| foreign.iter().map(|&p| u[p]).sum::<u64>();
    assert!(sum(&stirred) > sum(&calm) + 5 * foreign.len() as u64, "foreign provinces grow restless: {} against {}", sum(&stirred), sum(&calm));
}
