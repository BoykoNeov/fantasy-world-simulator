//! Tests of Research "tree" (build step 9): the tree and its check, monthly study, the discount
//! from neighbours and trade partners, new and gone realms, scholars' wages under coins, the
//! SetTech edit, and saves, going back and settings changed while the world runs.

use crate::branch::Timelines;
use crate::config::Config;
use crate::diplomacy::WarParams;
use crate::edit::{Edit, SettingChange};
use crate::events::{Event, EventKind};
use crate::fields::{quantity_total, Quantity};
use crate::grid::NONE;
use crate::module::{Module, Slot};
use crate::realms::RealmParams;
use crate::requests::Cause;
use crate::research::{tech_names, Research, ResearchParams, Survey};
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};

fn config(economics: &str, seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.without_realms();
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.characters = Some(Default::default());
    c.modules.diplomacy = Some(WarParams::default());
    c.modules.research = Some(ResearchParams::default());
    c
}

fn world(economics: &str, seed: u64) -> Sim {
    let mut sim = Sim::new(&config(economics, seed)).unwrap();
    sim.strict = true;
    sim
}

fn k(w: &World) -> usize {
    w.techs.len()
}

fn knows(w: &World, r: u32, t: usize) -> bool {
    w.realm_tech[r as usize * k(w) + t] != NONE
}

fn known_count(w: &World, r: u32) -> usize {
    (0..k(w)).filter(|&t| knows(w, r, t)).count()
}

fn col(w: &World, name: &str) -> usize {
    w.techs.iter().position(|t| t == name).unwrap()
}

fn alive(w: &World) -> Vec<u32> {
    (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect()
}

fn land_of(w: &World, r: u32) -> Vec<u32> {
    (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r).collect()
}

fn coins(w: &World, values: &[u64], row: usize) -> u64 {
    let g = &w.goods;
    let money = g.tree.find("money").unwrap();
    g.columns_under(money).unwrap().iter().map(|&c| values[g.index(row, c)]).sum()
}

fn money(w: &World) -> Quantity {
    let t = &w.goods.tree;
    Quantity::Goods(t.class_of(t.find("money").unwrap()).unwrap() as u8)
}

/// Run up to the start of the next month's first day (it hasn't run yet).
fn to_month_start(sim: &mut Sim) {
    let start = (sim.day + 1..).find(|&d| sim.calendar.is_month_start(d)).unwrap();
    sim.run_days(start - sim.day);
}

#[test]
fn the_shipped_tree_is_a_first_draft_of_24_techs_in_six_fields() {
    let p = ResearchParams::default();
    assert_eq!(p.techs.len(), 24);
    assert_eq!(p.fields.len(), 6);
    for f in &p.fields {
        assert_eq!(p.techs.values().filter(|t| &t.field == f).count(), 4, "{f}");
    }
    assert!(p.techs.values().all(|t| (5.0..=25.0).contains(&t.years)));
    let total: f64 = p.techs.values().map(|t| t.years).sum();
    assert!((280.0..=330.0).contains(&total), "{total}");
    let needs = p.resolve().unwrap();
    assert!(needs.iter().any(|n| n.is_empty()), "something can be studied first");
    // Columns go by name.
    let mut c = Config::new(WorldParams::preset(crate::world::Size::Small, 1));
    assert!(tech_names(&c.modules).is_empty(), "off by default");
    c.modules.research = Some(p);
    let names = tech_names(&c.modules);
    assert!(names.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn a_tree_that_doesnt_fit_together_is_refused() {
    let refused = |f: &dyn Fn(&mut ResearchParams)| {
        let mut p = ResearchParams::default();
        f(&mut p);
        p.resolve().unwrap_err()
    };
    assert!(refused(&|p| p.techs.get_mut("writing").unwrap().needs.push("magic".into())).contains("magic, which is not a tech"));
    assert!(refused(&|p| p.techs.get_mut("writing").unwrap().needs.push("philosophy".into())).contains("needs itself"));
    assert!(refused(&|p| p.techs.get_mut("writing").unwrap().field = "magic".into()).contains("not one of the fields"));
    assert!(refused(&|p| p.techs.get_mut("writing").unwrap().years = 0.0).contains("costs from"));
    assert!(refused(&|p| p.neighbour_discount = 1.0).contains("always costs something"));
    assert!(refused(&|p| p.ruler_effect = -0.1).contains("ruler"));
    assert!(refused(&|p| p.size_cap = 0.5).contains("big realm"));
    assert!(refused(&|p| p.speed = f64::NAN).contains("speed"));
    // The engine refuses such a world.
    let mut c = config("food", 1);
    c.modules.research.as_mut().unwrap().techs.get_mut("writing").unwrap().needs.push("magic".into());
    assert!(Sim::new(&c).err().unwrap().0.contains("Research \"tree\": writing needs magic"));
}

#[test]
fn research_needs_realms() {
    let mut c = config("food", 1);
    c.modules.without_realms();
    let e = Sim::new(&c).err().expect("refused");
    assert!(e.0.contains("Research \"tree\""), "{e}");
}

#[test]
fn realms_study_learn_and_choose_what_their_needs_allow() {
    let mut sim = world("food", 1);
    let start = sim.calendar.date(0).year;
    let mut discoveries = 0;
    for year in 1..=30 {
        sim.run_to_year(start + year);
        discoveries += sim.event_log.entries.iter().filter(|p| p.event.kind() == EventKind::Discovery && p.day + 364 >= sim.day).count();
    }
    let w = &sim.world;
    let p = ResearchParams::default();
    let needs = p.resolve().unwrap();
    for r in alive(w) {
        let row = &w.realm_tech[r as usize * k(w)..(r as usize + 1) * k(w)];
        // Nothing is known before what it needs (realms made by the editor aside, there are none).
        for (t, &day) in row.iter().enumerate() {
            if day != NONE {
                assert!(needs[t].iter().all(|&n| row[n] != NONE && row[n] <= day), "realm {r}: {} before its needs", w.techs[t]);
            }
        }
        let s = w.realm_studying[r as usize];
        if s != NONE {
            assert_eq!(row[s as usize], NONE, "realm {r} studies what it knows");
            assert!(needs[s as usize].iter().all(|&n| row[n] != NONE));
        }
    }
    let counts: Vec<usize> = alive(w).iter().map(|&r| known_count(w, r)).collect();
    assert!(counts.iter().sum::<usize>() > 0, "{counts:?}");
    assert!(discoveries > 0);
    assert_eq!(sim.problems, Problems::default());
}

/// A survey made by hand: realm 0 with neighbours 1 and 2, and trade partner 3; who knows tech 0
/// as given.
fn survey(known_by: &[u32], partners: bool) -> Survey {
    let k = ResearchParams::default().techs.len();
    let mut known = vec![NONE; 4 * k];
    for &r in known_by {
        known[r as usize * k] = 1;
    }
    Survey {
        people: vec![100; 4],
        paid_in: vec![Vec::new(); 4],
        median: 100.0,
        neighbours: vec![vec![1, 2], vec![0], vec![0], vec![]],
        partners: vec![if partners { vec![3] } else { vec![] }, vec![], vec![], vec![0]],
        known,
    }
}

#[test]
fn a_tech_neighbours_and_trade_partners_know_costs_less() {
    let r = Research::new(ResearchParams::default(), false);
    let full = (r.params.techs.values().next().unwrap().years * 1000.0).round() as u64;
    assert_eq!(r.cost(&survey(&[], false), 0, 0), full);
    // Every neighbour knows it: half off.
    assert_eq!(r.cost(&survey(&[1, 2], false), 0, 0), full / 2);
    // One of two: a quarter off.
    assert_eq!(r.cost(&survey(&[1], false), 0, 0), full * 3 / 4);
    // A trade partner counts half a neighbour: (0 + 0.5) / 2.5 of the way.
    assert_eq!(r.cost(&survey(&[3], true), 0, 0), (full as f64 * (1.0 - 0.5 * 0.2)).round() as u64);
    assert_eq!(r.cost(&survey(&[1, 2, 3], true), 0, 0), full / 2);
    // Trade alone (no neighbours) counts fully.
    assert_eq!(r.cost(&survey(&[0], true), 3, 0), full / 2);
    // Trade weighted 0 passes on nothing.
    let none = Research::new(ResearchParams { trade_weight: 0.0, ..ResearchParams::default() }, false);
    assert_eq!(none.cost(&survey(&[3], true), 0, 0), full);
}

#[test]
fn trade_partners_come_from_last_months_trade_and_only_with_trade() {
    let mut r = Research::new(ResearchParams::default(), true);
    let mut sim = world("markets", 1);
    sim.run_days(40);
    let mut out = crate::module::Outbox::default();
    let goods = sim.world.goods.clone();
    let mut cx = crate::module::Ctx { day: 40, seed: 1, calendar: &sim.calendar, goods: &goods, slot: Slot::Research, out: &mut out, trace: None };
    r.handle(&mut sim.world, &Event::Trade { a: 3, b: 1, a_to_b: 10, b_to_a: 0 }, &mut cx);
    assert!(r.partners().contains(&(1, 3)));
    // Kept with the trade weighted 0 too (the weight is a live setting; costs apply it).
    let mut none = Research::new(ResearchParams { trade_weight: 0.0, ..ResearchParams::default() }, true);
    none.handle(&mut sim.world, &Event::Trade { a: 1, b: 3, a_to_b: 10, b_to_a: 0 }, &mut cx);
    assert!(none.partners().contains(&(1, 3)));

    // In a running world with trade, realms trade and the module hears of it.
    let mut sim = world("markets", 1);
    to_month_start(&mut sim);
    sim.run_days(3);
    let m = sim.modules.iter().find(|m| m.manifest().slot == Slot::Research).unwrap();
    let mut st = Research::new(ResearchParams::default(), true);
    st.load_state(&m.save_state()).unwrap();
    assert!(!st.partners().is_empty(), "realms trade across their borders");
}

#[test]
fn bigger_realms_and_able_rulers_study_faster() {
    let mut sim = world("food", 3);
    sim.run_days(40);
    let r = Research::new(ResearchParams::default(), false);
    let w = &mut sim.world;
    let s = r.survey(w);
    let living = alive(w);
    let big = *living.iter().max_by_key(|&&x| s.people[x as usize]).unwrap();
    let small = *living.iter().min_by_key(|&&x| s.people[x as usize]).unwrap();
    // The same ruler for both.
    for x in [big, small] {
        let ruler = w.realms.ruler[x as usize];
        w.characters.ability[ruler as usize] = 50;
    }
    let (rb, rs) = (r.rate(w, &s, big as usize), r.rate(w, &s, small as usize));
    assert!(rb > rs, "{rb} {rs}");
    let expected = ((s.people[big as usize] as f64 / s.median).powf(0.5)).min(2.0) * r.params.speed;
    assert!((rb - expected).abs() < 1e-9);
    let ruler = w.realms.ruler[big as usize];
    w.characters.ability[ruler as usize] = 100;
    assert!((r.rate(w, &s, big as usize) - rb * 1.25).abs() < 1e-9);
    w.characters.ability[ruler as usize] = 0;
    assert!((r.rate(w, &s, big as usize) - rb * 0.75).abs() < 1e-9);
}

#[test]
fn rebels_inherit_what_their_realm_knew_and_conquest_passes_nothing() {
    let mut sim = world("food", 1);
    sim.run_days(365 * 15);
    let w = &sim.world;
    let old = *alive(w).iter().filter(|&&r| land_of(w, r).len() >= 4 && known_count(w, r) > 0).max_by_key(|&&r| land_of(w, r).len()).expect("a big realm that knows something");
    let capital = w.realms.capital[old as usize];
    let away: Vec<u32> = land_of(w, old).into_iter().filter(|&p| p != capital).take(3).collect();
    let row: Vec<u32> = w.realm_tech[old as usize * k(w)..(old as usize + 1) * k(w)].to_vec();
    let before = w.realms.count();
    sim.poke(Event::Breakaway { provinces: away });
    sim.run_days(3);
    let w = &sim.world;
    assert_eq!(w.realms.count(), before + 1, "they broke away");
    let rebels = before as u32;
    assert_eq!(w.realms.broke_from[rebels as usize], old);
    let theirs = &w.realm_tech[rebels as usize * k(w)..(rebels as usize + 1) * k(w)];
    assert_eq!(theirs, &row[..], "rebels know what their realm knew, from the day it learned it");

    // The old realm takes their land back by hand: it learns nothing from them it didn't know.
    let mut sim2 = world("food", 1);
    sim2.run_days(365 * 15);
    let w = &sim2.world;
    let (a, b) = neighbours_differing(w);
    let a_row: Vec<u32> = w.realm_tech[a as usize * k(w)..(a as usize + 1) * k(w)].to_vec();
    let land = land_of(w, b);
    sim2.edit(Edit::SetOwner { provinces: land, realm: a }).unwrap();
    sim2.run_days(40);
    let w = &sim2.world;
    assert!(!w.realms.alive(b), "realm {b} is dissolved");
    assert_eq!(w.realm_studying[b as usize], NONE, "a dissolved realm studies nothing");
    for (t, &day) in a_row.iter().enumerate() {
        if day == NONE && knows(w, a, t) {
            // Learned since by study: on a month start after the conquest.
            assert!(w.realm_tech[a as usize * k(w) + t] as u64 > sim2.day - 40);
        }
    }
    let b_row = &w.realm_tech[b as usize * k(w)..(b as usize + 1) * k(w)];
    assert!(b_row.iter().any(|&d| d != NONE), "a dissolved realm keeps its row");
}

/// Two neighbouring realms where the second knows something the first doesn't.
fn neighbours_differing(w: &World) -> (u32, u32) {
    for p in 0..w.provinces.count as u32 {
        let a = w.owner[p as usize];
        if a == NONE {
            continue;
        }
        for l in w.routes.of(p) {
            let b = w.owner[l.other(p) as usize];
            if b != NONE && b != a && w.realms.alive(b) && (0..k(w)).any(|t| knows(w, b, t) && !knows(w, a, t)) {
                return (a, b);
            }
        }
    }
    panic!("no neighbours that know different things");
}

#[test]
fn a_realm_the_editor_makes_knows_what_the_realm_that_held_most_of_its_land_knew() {
    let mut sim = world("food", 2);
    sim.run_days(365 * 15);
    let w = &sim.world;
    let (a, b) = neighbours_differing(w);
    // Three provinces of b's and one of a's.
    let mut land: Vec<u32> = land_of(w, b).into_iter().filter(|&p| p != w.realms.capital[b as usize]).take(3).collect();
    land.push(*land_of(w, a).iter().find(|&&p| p != w.realms.capital[a as usize]).unwrap());
    let b_row: Vec<u32> = w.realm_tech[b as usize * k(w)..(b as usize + 1) * k(w)].to_vec();
    let made = w.realms.count() as u32;
    sim.edit(Edit::NewRealm { provinces: land }).unwrap();
    let w = &sim.world;
    assert_eq!(&w.realm_tech[made as usize * k(w)..(made as usize + 1) * k(w)], &b_row[..]);
    // On land nobody held: nothing.
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn scholars_are_paid_from_the_treasury_and_coins_are_kept() {
    let mut sim = world("markets", 1);
    let all = quantity_total(&sim.world, money(&sim.world));
    sim.run_days(365 * 2);
    to_month_start(&mut sim);
    sim.run_days(1);
    let w = &sim.world;
    let wages: Vec<_> = sim.applied_today.iter().filter(|(r, _)| r.cause == Cause::Scholars).collect();
    assert!(!wages.is_empty(), "wages were paid on the month's first day");
    for (r, granted) in &wages {
        assert_eq!(r.field, crate::fields::known("realm.stock"));
        assert!(*granted <= r.take);
        let to = r.to.unwrap();
        assert_eq!(w.owner[to.entity as usize], r.entity, "into the realm's own province");
    }
    // About 0.5% of the people at 2 coins a month.
    let people: u64 = w.province_population().iter().zip(&w.owner).filter(|(_, o)| **o != NONE).map(|(p, _)| p).sum();
    let asked: u64 = wages.iter().map(|(r, _)| r.take).sum();
    let want = people as f64 * 0.005 * 24.0 / 12.0;
    assert!((asked as f64 - want).abs() < want * 0.2, "{asked} coins asked, about {want} expected");
    sim.run_days(365 * 3);
    assert_eq!(quantity_total(&sim.world, money(&sim.world)), all, "coins only move");
    assert_eq!(sim.problems, Problems::default());
    assert!(alive(&sim.world).iter().any(|&r| known_count(&sim.world, r) > 0));
}

#[test]
fn unpaid_scholars_dont_study() {
    // A realm with an empty treasury and no taxes coming in pays nobody and learns nothing.
    let mut c = config("markets", 1);
    c.modules.realms.as_mut().unwrap().taxes.per_month = 0.0;
    // Nor tolls.
    c.modules.markets.as_mut().unwrap().trade.as_mut().unwrap().toll = 0.0;
    let mut sim = Sim::new(&c).unwrap();
    sim.run_days(365 * 10);
    let w = &sim.world;
    for r in alive(w) {
        assert_eq!(coins(w, &w.realm_stock, r as usize), 0);
        assert_eq!(known_count(w, r), 0, "realm {r} learned without paying");
        assert_eq!(w.realm_knowledge[r as usize], 0);
    }
    // Paid for nothing (no wage): they learn as under Economics v0.
    c.modules.research.as_mut().unwrap().wage = 0.0;
    let mut sim = Sim::new(&c).unwrap();
    sim.run_days(365 * 10);
    assert!(alive(&sim.world).iter().any(|&r| known_count(&sim.world, r) > 0));
}

#[test]
fn the_editor_teaches_and_takes_away_techs() {
    let mut sim = world("food", 1);
    sim.run_days(40);
    let r = alive(&sim.world)[0];
    let t = col(&sim.world, "philosophy");
    sim.edit(Edit::SetTech { realm: r, tech: "philosophy".into(), known: true }).unwrap();
    assert_eq!(sim.world.realm_tech[r as usize * k(&sim.world) + t], sim.day as u32);
    let refused = |sim: &mut Sim, e: Edit| sim.edit(e).unwrap_err();
    assert!(refused(&mut sim, Edit::SetTech { realm: r, tech: "philosophy".into(), known: true }).contains("already knows"));
    assert!(refused(&mut sim, Edit::SetTech { realm: r, tech: "magic".into(), known: true }).contains("no tech magic"));
    assert!(refused(&mut sim, Edit::SetTech { realm: 9999, tech: "writing".into(), known: true }).contains("9999"));
    sim.edit(Edit::SetTech { realm: r, tech: "philosophy".into(), known: false }).unwrap();
    assert!(!knows(&sim.world, r, t));
    assert!(refused(&mut sim, Edit::SetTech { realm: r, tech: "philosophy".into(), known: false }).contains("doesn't know"));
    // The tech studied, taught: it is studied no more, and the realm chooses another next month.
    let s = sim.world.realm_studying[r as usize];
    assert_ne!(s, NONE);
    let name = sim.world.techs[s as usize].clone();
    sim.edit(Edit::SetTech { realm: r, tech: name, known: true }).unwrap();
    assert_eq!(sim.world.realm_studying[r as usize], NONE);
    to_month_start(&mut sim);
    sim.run_days(1);
    assert_ne!(sim.world.realm_studying[r as usize], s);
    assert_eq!(sim.problems, Problems::default());

    // Without Research there is nothing to set.
    let mut c = config("food", 1);
    c.modules.research = None;
    let mut off = Sim::new(&c).unwrap();
    let e = off.edit(Edit::SetTech { realm: 0, tech: "writing".into(), known: true }).unwrap_err();
    assert!(e.contains("no module keeps realm.tech"), "{e}");

    // Edits replay from a scenario file.
    let t = Timelines::new(sim);
    let again = Timelines::from_scenario_text(&t.scenario_text()).unwrap();
    assert_eq!(again.sim().fingerprint(), t.sim().fingerprint());
}

/// Saved, loaded and gone back to between a month's wages being asked and being paid, the world
/// runs on the same.
#[test]
fn saves_and_going_back_between_the_ask_and_the_pay_give_the_same_world() {
    for economics in ["food", "markets"] {
        let mut sim = world(economics, 7);
        sim.run_days(365 * 3);
        to_month_start(&mut sim);
        // The month's first day has run: wages are asked, not yet paid.
        sim.run_days(1);
        let day = sim.day;
        let mut loaded = Sim::load(&sim.save()).unwrap();
        let mut back = sim.fork();
        sim.run_days(500);
        loaded.run_days(500);
        assert_eq!(loaded.fingerprint(), sim.fingerprint(), "{economics}: loaded");
        back.run_days(300);
        back.goto(day).unwrap();
        back.run_days(500);
        assert_eq!(back.fingerprint(), sim.fingerprint(), "{economics}: gone back");
        assert_eq!(sim.problems, Problems::default(), "{economics}");
    }
}

#[test]
fn research_settings_change_while_the_world_runs() {
    for economics in ["food", "markets"] {
        let c = config(economics, 5);
        let (mut a, mut b) = (world(economics, 5), world(economics, 5));
        a.run_days(400);
        to_month_start(&mut a);
        a.run_days(1);
        b.run_days(a.day);
        // Every research setting set to the value it has: the module is made again and runs on as
        // if nothing happened.
        let changes: Vec<SettingChange> =
            crate::settings::items(&c).into_iter().filter(|i| i.key.starts_with("modules.research")).map(|i| SettingChange { key: i.key, value: i.value }).collect();
        assert!(changes.len() >= 8 + 24, "{}", changes.len());
        b.edit(Edit::Settings { changes }).unwrap();
        a.run_days(800);
        b.run_days(800);
        assert_eq!(a.fingerprint(), b.fingerprint(), "{economics}");
        // A real change takes effect.
        let mut fast = world(economics, 5);
        fast.run_days(400);
        fast.edit(Edit::Settings { changes: vec![SettingChange { key: "modules.research.speed".into(), value: 5.0 }] }).unwrap();
        fast.run_days(a.day - fast.day);
        let sum = |s: &Sim| alive(&s.world).iter().map(|&r| known_count(&s.world, r)).sum::<usize>();
        assert!(sum(&fast) > sum(&a), "{economics}: {} {}", sum(&fast), sum(&a));
    }
}

#[test]
fn research_leaves_the_rest_of_the_world_as_it_was_under_economics_v0() {
    // Under Economics v0 research asks nothing of anyone: every other field is as without it.
    let mut c = config("food", 4);
    let mut with = Sim::new(&c).unwrap();
    c.modules.research = None;
    let mut without = Sim::new(&c).unwrap();
    with.run_days(365 * 5);
    without.run_days(365 * 5);
    for id in crate::fields::FieldId::all() {
        if id.def().name.starts_with("realm.knowledge") || id.def().name.starts_with("realm.studying") || id.def().name.starts_with("realm.tech") {
            continue;
        }
        let print = |w: &World| { let mut h = crate::world::Fingerprint::default(); w.field(id).fingerprint(&mut h); h.0 };
        assert_eq!(print(&with.world), print(&without.world), "{}", id.def().name);
    }
}

#[test]
fn a_module_made_again_keeps_its_state() {
    let mut sim = world("markets", 1);
    sim.run_days(100);
    let m = sim.modules.iter().find(|m| m.manifest().slot == Slot::Research).unwrap();
    let mut again = Research::new(ResearchParams::default(), true);
    again.load_state(&m.save_state()).unwrap();
    assert_eq!(again.save_state(), m.save_state());
    assert!(again.load_state(&[1, 2, 3]).is_err());
}
