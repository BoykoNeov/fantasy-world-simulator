//! Tests of Religion "conversion" (build step 9).

use crate::config::Config;
use crate::culture::CultureParams;
use crate::edit::{Edit, PopValues};
use crate::events::Event;
use crate::grid::NONE;
use crate::realms::RealmParams;
use crate::religion::{foreign_share, main_faiths, mix, names, ReligionParams, SchismParams};
use crate::requests::Cause;
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};

fn config(seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 32, provinces: 120, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.culture = Some(CultureParams { starting: 8, ..CultureParams::default() });
    c.modules.religion = Some(ReligionParams { starting: 5, ..ReligionParams::default() });
    c
}

/// The same world with neither realms nor culture: only the faiths' own, their neighbours' pulls.
fn plain(seed: u64) -> Config {
    let mut c = config(seed);
    let religion = c.modules.religion.take();
    c.modules.without_realms().religion = religion;
    c
}

fn sim(c: &Config) -> Sim {
    let mut s = Sim::new(c).unwrap();
    s.strict = true;
    s
}

fn religion(c: &mut Config) -> &mut ReligionParams {
    c.modules.religion.as_mut().unwrap()
}

/// Followers of a faith in a province.
fn people_of(w: &World, p: u32, faith: u32) -> u64 {
    (0..w.pops.size.len()).filter(|&k| w.pops.province[k] == p && w.pops.religion[k] as u32 == faith).map(|k| w.pops.size[k]).sum()
}

fn faiths_in(w: &World, p: u32) -> usize {
    let mut f: Vec<u16> = (0..w.pops.size.len()).filter(|&k| w.pops.province[k] == p && w.pops.size[k] > 0).map(|k| w.pops.religion[k]).collect();
    f.sort();
    f.dedup();
    f.len()
}

fn settled(w: &World) -> Vec<u32> {
    let people = w.province_population();
    (0..w.provinces.count as u32).filter(|&p| people[p as usize] > 0).collect()
}

/// Runs the world to the day after the next month's start (when that start's shifts are in).
fn past_month_start(s: &mut Sim) {
    s.step_day();
    while !s.calendar.is_month_start(s.day) {
        s.step_day();
    }
    s.step_day();
}

#[test]
fn faiths_start_by_region_with_mixed_borderlands_and_nobody_made_or_lost() {
    let c = config(7);
    let s = sim(&c);
    let mut off = c.clone();
    off.modules.religion = None;
    let none = sim(&off);
    // The same people in every province, of the same cultures; only their faiths differ.
    assert_eq!(s.world.province_population(), none.world.province_population());
    assert_eq!(crate::culture::mix(&s.world), crate::culture::mix(&none.world));
    assert!(none.world.religions.count() == 0 && none.world.realm_religion.iter().all(|&r| r == NONE));

    let w = &s.world;
    assert_eq!(w.religions.count(), 5);
    assert!(w.religions.parent.iter().all(|&p| p == NONE) && w.religions.born.iter().all(|&b| b == 0) && w.religions.died.iter().all(|&d| d == u64::MAX));
    let main = main_faiths(w);
    let settled = settled(w);
    assert!(settled.iter().all(|&p| main[p as usize] != NONE));
    for f in 0..5u32 {
        assert_eq!(main[w.religions.hearth[f as usize] as usize], f, "faith {f}'s hearth");
    }
    let same_as_a_neighbour = settled.iter().filter(|&&p| w.routes.of(p).any(|l| main[l.other(p) as usize] == main[p as usize])).count();
    assert!(same_as_a_neighbour as f64 > 0.8 * settled.len() as f64, "{same_as_a_neighbour} of {}", settled.len());
    // Border provinces hold a share of the neighbouring faith; nowhere more than two faiths.
    let mixed: Vec<u32> = settled.iter().copied().filter(|&p| faiths_in(w, p) == 2).collect();
    assert!(!mixed.is_empty());
    for &p in &mixed {
        let total = w.province_population()[p as usize];
        let other = total - people_of(w, p, main[p as usize]);
        assert!((other as f64 / total as f64 - 0.10).abs() < 0.01, "province {p}: {other} of {total}");
    }
    assert!(settled.iter().all(|&p| faiths_in(w, p) <= 2));
    // Realms take their capitals' faiths, and faiths cross realm and culture borders.
    for r in 0..w.realms.count() {
        assert_eq!(w.realm_religion[r], main[w.realms.capital[r] as usize]);
    }
    let culture = crate::culture::main_cultures(w);
    let crosses = |of: &dyn Fn(u32) -> u32| {
        (0..5u32).any(|f| {
            let mut v: Vec<u32> = settled.iter().filter(|&&p| main[p as usize] == f).map(|&p| of(p)).collect();
            v.sort();
            v.dedup();
            v.len() > 1
        })
    };
    assert!(crosses(&|p| w.owner[p as usize]) && crosses(&|p| culture[p as usize]));
    // Placing them was booked as shifts, before the first day.
    assert!(s.applied_today.iter().all(|(r, _)| r.cause == Cause::Placed));
    assert!(s.applied_today.iter().any(|(r, _)| r.field == crate::fields::known("pop.religion")));
    // The window's mix: the same people, most first.
    let m = mix(w);
    for &p in &settled {
        assert_eq!(m[p as usize].iter().map(|e| e.1).sum::<u64>(), w.province_population()[p as usize]);
        assert_eq!(m[p as usize][0].0, main[p as usize]);
    }
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
fn more_faiths_asked_than_settled_provinces_gives_one_each() {
    let mut c = config(3);
    religion(&mut c).starting = 1000;
    let s = sim(&c);
    assert_eq!(s.world.religions.count(), settled(&s.world).len());
}

#[test]
fn people_keep_exact_totals_over_decades_with_wars_and_both_economies() {
    for economics in ["food", "markets"] {
        let mut c = config(21);
        c.modules.set_choice(crate::module::Slot::Economics, economics).unwrap();
        c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, ..RealmParams::default() });
        religion(&mut c).schisms = Some(SchismParams { years: 5.0, provinces: 2, per_year: 0.5 });
        let mut s = sim(&c);
        let mut converted = 0u64;
        while s.calendar.date(s.day).year < 31 {
            s.step_day();
            converted += s.applied_today.iter().filter(|(r, _)| r.cause == Cause::Converted).map(|(_, g)| g).sum::<u64>();
        }
        assert_eq!(s.problems, Problems::default(), "{economics}");
        let w = &s.world;
        // Every year balanced to the person (people are only moved between faiths), and the
        // table and the pops agree.
        for y in &s.history {
            assert_eq!(y.ledger.people_clamped, 0);
        }
        assert!(converted > 0, "{economics}");
        assert!(w.pops.religion.iter().all(|&f| (f as usize) < w.religions.count()));
        assert!(w.pops.culture.iter().all(|&c| (c as usize) < w.cultures.count()));
        assert_eq!(w.realm_religion.len(), w.realms.count());
        w.check_lengths().unwrap();
        // Every settled province keeps at most "most" faiths, give or take a month's newcomers.
        let worst = settled(w).iter().map(|&p| faiths_in(w, p)).max().unwrap();
        assert!(worst <= 4, "{economics}: a province of {worst} faiths");
    }
}

#[test]
fn people_take_up_their_rulers_faith_and_events_tell_of_it() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy.as_mut().unwrap().treaties = None;
    religion(&mut c).schisms = None;
    let mut s = sim(&c);
    let start = main_faiths(&s.world);
    let mut converted = 0u64;
    let mut told = 0;
    while s.calendar.date(s.day).year < 61 {
        s.step_day();
        converted += s.applied_today.iter().filter(|(r, _)| r.cause == Cause::Converted).map(|(_, g)| g).sum::<u64>();
        told += s.delivered_today().iter().filter(|p| matches!(p.event, Event::Converted { .. })).count();
    }
    assert_eq!(s.problems, Problems::default());
    let w = &s.world;
    let main = main_faiths(w);
    // Provinces held by a realm of another faith: more of them have taken it up than started so.
    let ruled = |main: &[u32]| settled(w).iter().filter(|&&p| w.owner[p as usize] != NONE && main[p as usize] == w.realm_religion[w.owner[p as usize] as usize]).count();
    assert!(ruled(&main) > ruled(&start), "{} then, {} now", ruled(&start), ruled(&main));
    let changed = settled(w).iter().filter(|&&p| main[p as usize] != start[p as usize]).count();
    assert!(converted > 0 && changed > 0 && told >= changed / 2, "{converted} people, {changed} provinces, {told} told");
}

/// The user's choice: a ruler slowly wins over even a province all of another faith whose
/// neighbours are all of that faith too, where they are days away and its people's faith doesn't
/// hold it (Culture off: no kin pull).
#[test]
fn a_ruler_wins_over_even_a_province_wholly_foreign_and_surrounded_by_its_own_faith() {
    let mut c = config(21);
    c.modules.culture = None;
    c.modules.realms = Some(RealmParams { provinces_per_realm: 1000.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy.as_mut().unwrap().treaties = None;
    religion(&mut c).schisms = None;
    let mut s = sim(&c);
    let w = &s.world;
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
    let ruler = s.world.realm_religion[s.world.owner[p as usize] as usize];
    let mut pocket: Vec<u32> = s.world.routes.of(p).filter(|l| l.days().is_some()).map(|l| l.other(p)).collect();
    pocket.push(p);
    s.edit(Edit::NewReligion { provinces: pocket }).unwrap();
    let foreign = (s.world.religions.count() - 1) as u32;
    assert_eq!(people_of(&s.world, p, ruler), 0);
    assert_eq!(people_of(&s.world, p, foreign), s.world.province_population()[p as usize]);
    s.run_days(5 * 365);
    let now = people_of(&s.world, p, ruler);
    assert!(now > 0, "nobody took up the ruler's faith");
    assert!(people_of(&s.world, p, foreign) > now, "slowly: {now} took it up in five years");
    assert_eq!(s.problems, Problems::default());
}

/// With realms off there is no ruler, so the ruler's pull changes nothing; with Culture off
/// everyone is of one people, so the kin pull changes nothing.
#[test]
fn without_realms_no_rulers_pull_and_without_culture_no_kin_pull() {
    let run = |c: &Config| {
        let mut s = sim(c);
        s.run_to_year(6);
        assert_eq!(s.problems, Problems::default());
        s.fingerprint()
    };
    let mut c = plain(7);
    assert!(sim(&c).world.realm_religion.is_empty());
    let base = run(&c);
    religion(&mut c).ruler = 100.0;
    assert_eq!(run(&c), base, "no realms: the ruler's pull is nothing");
    religion(&mut c).kin = 100.0;
    assert_eq!(run(&c), base, "one culture: the kin pull is nothing");
    // Neighbours still pull without them.
    religion(&mut c).neighbours = 0.0;
    assert_ne!(run(&c), base);

    // With realms and culture, both matter.
    let mut c = config(7);
    let base = run(&c);
    religion(&mut c).ruler = 0.0;
    assert_ne!(run(&c), base, "the ruler's pull");
    let mut c = config(7);
    religion(&mut c).kin = 0.0;
    assert_ne!(run(&c), base, "the kin pull");
    let mut c = config(7);
    c.modules.culture = None;
    let off = run(&c);
    religion(&mut c).kin = 100.0;
    assert_eq!(run(&c), off, "culture off: no kin pull, with realms too");
}

/// The kin faith of a province is the faith most of its main culture's people hold, world-wide;
/// a province of one faith pulled by it and nothing else takes it up.
#[test]
fn the_kin_faith_is_the_main_cultures_world_wide_faith() {
    let mut c = plain(7);
    c.modules.culture = Some(CultureParams { starting: 8, per_year: 0.0, splits: None, ..CultureParams::default() });
    let s = sim(&c);
    let w = &s.world;
    let kin = crate::religion::kin_faiths(&w.pops, w.provinces.count);
    let culture = crate::culture::main_cultures(w);
    assert_eq!(kin.len(), w.provinces.count);
    for p in 0..w.provinces.count {
        if culture[p] == NONE {
            assert_eq!(kin[p], NONE);
            continue;
        }
        let of = |f: u32| -> u64 { (0..w.pops.size.len()).filter(|&k| w.pops.culture[k] as u32 == culture[p] && w.pops.religion[k] as u32 == f).map(|k| w.pops.size[k]).sum() };
        let best = (0..w.religions.count() as u32).max_by(|&a, &b| of(a).cmp(&of(b)).then(b.cmp(&a))).unwrap();
        assert_eq!(kin[p], best, "province {p}");
    }
    // One culture with people: no kin faiths.
    let one = sim(&plain(7));
    assert!(crate::religion::kin_faiths(&one.world.pops, one.world.provinces.count).is_empty());

    // A province whose people's kin faith isn't theirs, with neighbours and its own faith pulling
    // nothing: it takes up the kin faith.
    let mut c2 = c.clone();
    {
        let r = religion(&mut c2);
        (r.own, r.neighbours, r.kin, r.schisms) = (0.0, 0.0, 1.0, None);
    }
    let mut s = sim(&c2);
    let main = main_faiths(&s.world);
    let p = (0..s.world.provinces.count).find(|&p| kin[p] != NONE && main[p] != kin[p]).expect("a province of a faith not its people's") as u32;
    let (f, before) = (kin[p as usize], people_of(&s.world, p, kin[p as usize]));
    past_month_start(&mut s);
    assert!(people_of(&s.world, p, f) > before, "the kin faith gains");
    assert_eq!(s.problems, Problems::default());
}

#[test]
fn a_province_keeps_at_most_the_most_faiths() {
    for most in [3u32, 1] {
        let mut c = plain(7);
        {
            let r = religion(&mut c);
            (r.most, r.schisms) = (most, None);
        }
        let mut s = sim(&c);
        let w = &s.world;
        let main = main_faiths(w);
        let people = w.province_population();
        // An inner province of one faith among neighbours of it.
        let p = settled(w)
            .into_iter()
            .find(|&p| faiths_in(w, p) == 1 && w.routes.of(p).all(|l| l.days().is_none() || main[l.other(p) as usize] == main[p as usize]) && people[p as usize] > 10_000)
            .expect("an inner province");
        let f0 = main[p as usize];
        let others: Vec<u32> = (0..5u32).filter(|&f| f != f0).take(3).collect();
        let x = people[p as usize];
        for (&f, share) in others.iter().zip([0.5, 0.3, 0.2]) {
            s.edit(Edit::AddPop { province: p, values: PopValues { religion: f as u16, ..PopValues::people((x as f64 * share) as u64) } }).unwrap();
        }
        assert_eq!(faiths_in(&s.world, p), 4);
        past_month_start(&mut s);
        let w = &s.world;
        assert_eq!(main_faiths(w)[p as usize], f0);
        assert_eq!(faiths_in(w, p), most as usize, "most {most}");
        // The smallest go first: all of them at once.
        assert_eq!(people_of(w, p, others[2]), 0);
        if most == 3 {
            assert!(people_of(w, p, others[0]) > 0 && people_of(w, p, others[1]) > 0);
        }
        assert_eq!(s.problems, Problems::default());
    }
}

#[test]
fn cut_off_groups_break_away_as_new_faiths() {
    let mut c = plain(21);
    c.modules.religion = Some(ReligionParams { starting: 4, per_year: 0.0, schisms: Some(SchismParams { years: 2.0, provinces: 3, per_year: 1.0 }), ..ReligionParams::default() });
    let mut s = sim(&c);
    let w = &s.world;
    // A pocket of faith 0 far from its heartland: three connected provinces of another faith, none
    // of them next to faith 0's land.
    let main = main_faiths(w);
    let pocket = settled(w)
        .into_iter()
        .filter(|&p| main[p as usize] != 0 && w.routes.of(p).all(|l| main[l.other(p) as usize] != 0))
        .find_map(|p| {
            let near: Vec<u32> = w.routes.of(p).filter(|l| l.days().is_some()).map(|l| l.other(p)).filter(|&q| main[q as usize] != 0 && main[q as usize] != NONE && w.routes.of(q).all(|m| main[m.other(q) as usize] != 0)).take(2).collect();
            (near.len() == 2).then(|| vec![p, near[0], near[1]])
        })
        .expect("a pocket");
    s.edit(Edit::SetReligion { provinces: pocket.clone(), religion: 0 }).unwrap();
    let mut schism = 0u64;
    while s.calendar.date(s.day).year < 5 {
        s.step_day();
        schism += s.applied_today.iter().filter(|(r, _)| r.cause == Cause::Schism).map(|(_, g)| g).sum::<u64>();
    }
    let w = &s.world;
    let new = (4..w.religions.count()).find(|&k| pocket.contains(&w.religions.hearth[k])).expect("the pocket broke away");
    assert_eq!((w.religions.parent[new], w.religions.died[new]), (0, u64::MAX));
    assert_eq!(w.religions.born[new], 2 * s.calendar.days_per_year() as u64, "cut off for two years, then at once");
    for &p in &pocket {
        assert_eq!(people_of(w, p, 0), 0);
        assert_eq!(people_of(w, p, new as u32), w.province_population()[p as usize]);
    }
    assert!(schism > 0);
    assert!(s.event_log.entries.iter().any(|e| matches!(e.event, Event::ReligionBorn { religion, parent: 0, provinces: 3, .. } if religion as usize == new)));
    assert_eq!(s.problems, Problems::default());
}

#[test]
fn edits_set_and_make_faiths_and_a_faith_with_nobody_left_is_gone() {
    let mut s = sim(&plain(7));
    let w = &s.world;
    let main = main_faiths(w);
    let all_of_1: Vec<u32> = settled(w).into_iter().filter(|&p| people_of(w, p, 1) > 0).collect();
    let rows = w.pops.size.len();
    let people = w.province_population();
    // Everyone of faith 1 takes faith 2: nobody is lost, no pop is removed.
    s.edit(Edit::SetReligion { provinces: all_of_1.clone(), religion: 2 }).unwrap();
    assert_eq!(s.world.province_population(), people);
    assert!(s.world.pops.size.len() >= rows);
    assert!(s.world.pops.religion.iter().zip(&s.world.pops.size).all(|(&f, &x)| f != 1 || x == 0));
    for &p in &all_of_1 {
        assert_eq!(people_of(&s.world, p, 2), people[p as usize]);
    }
    // Faith 1 is gone at the next month's start, and told so.
    s.run_days(40);
    assert_ne!(s.world.religions.died[1], u64::MAX);
    assert!(s.event_log.entries.iter().any(|e| matches!(e.event, Event::ReligionGone { religion: 1 })));
    // Given followers again, it lives again.
    s.edit(Edit::SetReligion { provinces: vec![all_of_1[0]], religion: 1 }).unwrap();
    assert_eq!(s.world.religions.died[1], u64::MAX);

    // A new faith, born today in the most populous of the provinces.
    let two: Vec<u32> = settled(&s.world).into_iter().filter(|&p| main[p as usize] == 0).take(2).collect();
    s.edit(Edit::NewReligion { provinces: two.clone() }).unwrap();
    let w = &s.world;
    let new = w.religions.count() - 1;
    let pp = w.province_population();
    let most = *two.iter().max_by(|&&a, &&b| pp[a as usize].cmp(&pp[b as usize]).then(b.cmp(&a))).unwrap();
    assert_eq!((w.religions.parent[new], w.religions.born[new], w.religions.hearth[new]), (NONE, s.day, most));
    assert!(two.iter().all(|&p| people_of(w, p, new as u32) == pp[p as usize]));
    // Refused: a faith that doesn't exist, provinces with no land.
    assert!(s.edit(Edit::SetReligion { provinces: vec![two[0]], religion: 999 }).unwrap_err().contains("no faith"));
    assert!(s.edit(Edit::NewReligion { provinces: vec![] }).is_err());
    s.run_days(400);
    assert_eq!(s.problems, Problems::default());

    // With religion off there are no faiths to set.
    let mut off = plain(7);
    off.modules.religion = None;
    let mut none = sim(&off);
    assert!(none.edit(Edit::NewReligion { provinces: vec![two[0]] }).unwrap_err().contains("religion.hearth"));
}

#[test]
fn saved_loaded_gone_back_and_replayed_worlds_are_the_same() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, ..RealmParams::default() });
    religion(&mut c).schisms = Some(SchismParams { years: 3.0, provinces: 2, per_year: 0.5 });
    let mut s = sim(&c);
    s.run_to_year(8);
    let p = settled(&s.world)[3];
    s.edit(Edit::NewReligion { provinces: vec![p] }).unwrap();
    s.run_days(200);
    let q = settled(&s.world)[7];
    s.edit(Edit::SetReligion { provinces: vec![q], religion: 0 }).unwrap();
    s.run_days(100);
    let mut loaded = Sim::load(&s.save()).unwrap();
    assert_eq!(loaded.fingerprint(), s.fingerprint());
    let (day, end) = (s.day, {
        s.run_days(3 * 365);
        loaded.run_days(3 * 365);
        s.fingerprint()
    });
    assert_eq!(loaded.fingerprint(), end);
    // Back before the edits and forward again: the same world.
    s.goto(day - 400).unwrap();
    s.goto(day + 3 * 365).unwrap();
    assert_eq!(s.fingerprint(), end);
    // Run again from the start with the same commands: the same world.
    let mut again = sim(&c);
    again.run_to_year(8);
    again.edit(Edit::NewReligion { provinces: vec![p] }).unwrap();
    again.run_days(200);
    again.edit(Edit::SetReligion { provinces: vec![q], religion: 0 }).unwrap();
    again.run_days(100 + 3 * 365);
    assert_eq!(again.fingerprint(), end);
}

#[test]
fn foreign_share_counts_people_not_of_their_realms_faith() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, ..RealmParams::default() });
    let mut s = sim(&c);
    s.run_days(40);
    let w = &s.world;
    let share = foreign_share(w);
    let main = main_faiths(w);
    let people = w.province_population();
    for p in 0..w.provinces.count {
        let r = w.owner[p];
        if r == NONE || people[p] == 0 {
            assert_eq!(share[p], 0);
            continue;
        }
        // Worked out from the capital's people, not from the realm's faith field.
        let theirs = main[w.realms.capital[r as usize] as usize];
        let ours = people_of(w, p as u32, theirs);
        assert_eq!(share[p], ((people[p] - ours) as u128 * 1000 + people[p] as u128 / 2) as u64 / people[p]);
        if w.realms.capital[r as usize] == p as u32 {
            assert!(share[p] < 500, "a capital is mostly of its own faith");
        }
    }
    assert!(share.iter().any(|&x| x > 500));
    // Without religion, everyone is of one faith: nobody is foreign.
    let mut off = c.clone();
    off.modules.religion = None;
    assert!(foreign_share(&sim(&off).world).iter().all(|&x| x == 0));
    // Without realms, nobody is ruled.
    assert!(foreign_share(&sim(&plain(21)).world).iter().all(|&x| x == 0));
}

/// Religion's own cost on a month-start day of a large world (5,000 provinces) with realms, wars
/// and culture, a decade in: the module's tick alone, timed many times on copies of the same day,
/// beside Culture's tick on the same day. Run it by hand:
/// `cargo test --release -p sim religion_speed -- --ignored --nocapture`.
#[test]
#[ignore]
fn religion_speed_on_a_large_world() {
    use crate::module::{Ctx, Module, Outbox, Slot};
    let mut c = Config::new(WorldParams::preset(crate::world::Size::Large, 21));
    c.modules.religion = Some(ReligionParams::default());
    let mut s = Sim::new(&c).unwrap();
    s.run_to_year(11);
    fn time(s: &Sim, slot: Slot, make: &dyn Fn() -> Box<dyn Module>, what: &str) {
        let k = s.modules.iter().position(|m| m.manifest().slot == slot).unwrap();
        let state = s.modules[k].save_state();
        let goods = s.world.goods.clone();
        let (mut times, mut asked) = (Vec::new(), 0);
        for _ in 0..30 {
            let mut w = s.world.clone();
            let mut m = make();
            m.load_state(&state).unwrap();
            let mut out = Outbox::default();
            let mut cx = Ctx { day: s.day, seed: w.params.seed, calendar: &s.calendar, goods: &goods, slot, out: &mut out, trace: None };
            let t = std::time::Instant::now();
            m.tick(&mut w, &mut cx);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            asked = out.requests.len();
        }
        times.sort_by(f64::total_cmp);
        println!("{what}: {:.3} ms (fastest {:.3}), {asked} shift requests", times[times.len() / 2], times[0]);
    }
    let settled = s.world.province_population().iter().filter(|&&x| x > 0).count();
    println!("{} provinces ({settled} settled), {} pop rows ({} with people)", s.world.provinces.count, s.world.pops.size.len(), s.world.pops.size.iter().filter(|&&x| x > 0).count());
    let religion = |p: ReligionParams| move || -> Box<dyn Module> { Box::new(crate::religion::Conversion::new(p.clone())) };
    let culture = || -> Box<dyn Module> { Box::new(crate::culture::Assimilation::new(CultureParams::default())) };
    time(&s, Slot::Religion, &religion(ReligionParams::default()), "religion, a year's start (schisms looked for)");
    time(&s, Slot::Religion, &religion(ReligionParams { per_year: 0.0, schisms: None, ..ReligionParams::default() }), "  of which bookkeeping alone");
    time(&s, Slot::Religion, &religion(ReligionParams { kin: 0.0, ..ReligionParams::default() }), "  with no kin pull");
    time(&s, Slot::Culture, &culture, "culture, the same day");
    s.run_days(31);
    time(&s, Slot::Religion, &religion(ReligionParams::default()), "religion, another month's start");
    time(&s, Slot::Culture, &culture, "culture, the same day");

    // The whole month-start day, with Religion and without (the same seed run as long: a slightly
    // different world, as faiths' pop rows change how the pops' births and deaths round).
    let mut off = c.clone();
    off.modules.religion = None;
    let mut t = Sim::new(&off).unwrap();
    t.run_to_year(11);
    t.run_days(31);
    let day = |s: &Sim| {
        let mut times: Vec<f64> = (0..3)
            .flat_map(|m| {
                let mut f = s.fork();
                f.run_days(m * 31);
                while !f.calendar.is_month_start(f.day) {
                    f.step_day();
                }
                (0..5).map(move |_| {
                    let mut g = f.fork();
                    let t = std::time::Instant::now();
                    g.step_day();
                    t.elapsed().as_secs_f64() * 1000.0
                })
            })
            .collect();
        times.sort_by(f64::total_cmp);
        (times[times.len() / 2], times[0])
    };
    for _ in 0..2 {
        let (on, on_best) = day(&s);
        let (no, no_best) = day(&t);
        println!("a month-start day: {on:.2} ms with Religion (fastest {on_best:.2}), {no:.2} ms without (fastest {no_best:.2})");
    }
}

/// Realms: unrest from rule by a foreign faith rises where a province's people are not of its
/// realm's faith, as much as the setting says, and nowhere with the setting at 0.
#[test]
fn a_foreign_faith_stirs_unrest_by_the_share_of_people_not_of_the_realms_faith() {
    let mut c = config(21);
    c.modules.realms = Some(RealmParams { provinces_per_realm: 6.0, ..RealmParams::default() });
    let run = |faith: f64| {
        let mut c = c.clone();
        let u = c.modules.realms.as_mut().unwrap().unrest.as_mut().unwrap();
        // Culture's own unrest off, so only the faith's differs between the two runs.
        (u.foreign, u.foreign_faith) = (0.0, faith);
        let mut s = sim(&c);
        s.run_days(365);
        (foreign_share(&s.world), s.world.unrest.clone())
    };
    let (share, stirred) = run(1.5);
    let (_, calm) = run(0.0);
    let foreign: Vec<usize> = (0..share.len()).filter(|&p| share[p] > 500).collect();
    assert!(!foreign.is_empty(), "some provinces are ruled by a realm of another faith");
    let sum = |u: &[u64]| foreign.iter().map(|&p| u[p]).sum::<u64>();
    assert!(sum(&stirred) > sum(&calm) + 5 * foreign.len() as u64, "foreign-faith provinces grow restless: {} against {}", sum(&stirred), sum(&calm));
}
