//! The editor of step 7: edits of realms, rulers and armies, the pokes to kill a ruler, break away
//! and march, and the repairs after edits (occupations, armies on land that went away).

use crate::characters::{Succession, NEVER};
use crate::config::Config;
use crate::diplomacy::{at_war, TreatyParams, WarParams};
use crate::edit::{Brush, Edit};
use crate::events::Event;
use crate::grid::{dir_to_lonlat, NONE};
use crate::module::Slot;
use crate::realms::RealmParams;
use crate::sim::{Problems, Sim};
use crate::warfare::{TroopParams, FIELD};
use crate::world::{World, WorldParams};
use std::collections::BTreeMap;

/// Realms (unrest as asked), rulers, wars and troops, under Economics v0.
fn config(seed: u64, unrest: bool) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    let realms = RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() };
    c.modules.realms = Some(if unrest { realms } else { RealmParams { unrest: None, ..realms } });
    c.modules.characters = Some(Default::default());
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
    (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r && w.provinces.cells[p as usize] > 0).collect()
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

/// A province of realm `r` other than its capital, bordering realm `other` by land if `other` is
/// given.
fn border_province(w: &World, r: u32, other: Option<u32>) -> u32 {
    let cap = w.realms.capital[r as usize];
    land_of(w, r)
        .into_iter()
        .filter(|&p| p != cap)
        .find(|&p| other.is_none_or(|o| w.routes.of(p).any(|l| l.land_days.is_some() && w.owner[l.other(p) as usize] == o)))
        .expect("such a province")
}

fn alive_realms(w: &World) -> Vec<u32> {
    (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect()
}

/// Run to the next month's start and through it.
fn past_month_start(sim: &mut Sim) {
    sim.run_days(1);
    while !sim.calendar.is_month_start(sim.day) {
        sim.run_days(1);
    }
    sim.run_days(1);
}

fn coins(w: &World, values: &[u64], row: usize) -> u64 {
    let g = &w.goods;
    g.columns_under(g.tree.find("money").unwrap()).unwrap().iter().map(|&c| values[g.index(row, c)]).sum()
}

fn refusals(sim: &Sim) -> Vec<String> {
    sim.event_log.entries.iter().filter_map(|p| if let Event::Refused { why } = &p.event { Some(why.clone()) } else { None }).collect()
}

fn people(sim: &Sim) -> u64 {
    sim.world.pops.size.iter().sum::<u64>() + sim.world.armies.size.iter().sum::<u64>()
}

#[test]
fn provinces_change_hands_and_realms_merge_split_and_dissolve() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    let p = border_province(&sim.world, a, Some(b));
    sim.edit(Edit::SetOwner { provinces: vec![p], realm: b }).unwrap();
    assert_eq!(sim.world.owner[p as usize], b);
    assert!(sim.edit(Edit::SetOwner { provinces: vec![p], realm: b }).is_err(), "already b's");

    // Its capital given away, a realm's capital moves to its most populous province left.
    let cap = sim.world.realms.capital[a as usize];
    sim.edit(Edit::SetOwner { provinces: vec![cap], realm: NONE }).unwrap();
    let w = &sim.world;
    assert_eq!(w.owner[cap as usize], NONE);
    let people = w.province_population();
    let best = land_of(w, a).into_iter().max_by(|&x, &y| people[x as usize].cmp(&people[y as usize]).then(y.cmp(&x))).unwrap();
    assert_eq!(w.realms.capital[a as usize], best);

    // A quiet split: a new realm at peace with everyone, its capital its most populous province.
    let mut part = land_of(&sim.world, b);
    part.retain(|&q| q != sim.world.realms.capital[b as usize]);
    part.truncate(2);
    let realms = sim.world.realms.count() as u32;
    sim.edit(Edit::NewRealm { provinces: part.clone() }).unwrap();
    let w = &sim.world;
    let new = realms;
    assert!(w.realms.alive(new) && part.iter().all(|&q| w.owner[q as usize] == new));
    assert_eq!(w.realms.broke_from[new as usize], NONE);
    let people = w.province_population();
    assert_eq!(w.realms.capital[new as usize], *part.iter().max_by(|&&x, &&y| people[x as usize].cmp(&people[y as usize]).then(y.cmp(&x))).unwrap());
    assert!(alive_realms(w).iter().all(|&r| !at_war(w, new, r)));
    // It gets a founding ruler at the month's start.
    assert_eq!(w.realms.ruler[new as usize], NONE);
    past_month_start(&mut sim);
    let ruler = sim.world.realms.ruler[new as usize];
    assert!(ruler != NONE && Succession::from_u8(sim.world.characters.succession[ruler as usize]) == Succession::Founded);

    // Merging: all of a's land to b. a is dissolved, its ruler set aside at the month's start.
    let all = land_of(&sim.world, a);
    sim.edit(Edit::SetOwner { provinces: all, realm: b }).unwrap();
    assert!(!sim.world.realms.alive(a));
    past_month_start(&mut sim);
    assert_eq!(sim.world.realms.ruler[a as usize], NONE);
    assert!(sim.edit(Edit::SetOwner { provinces: vec![p], realm: a }).is_err(), "a is no more");

    // Dissolving: all its land to nobody.
    sim.edit(Edit::SetOwner { provinces: land_of(&sim.world, new), realm: NONE }).unwrap();
    assert!(!sim.world.realms.alive(new));
    assert!(sim.edit(Edit::NewRealm { provinces: vec![] }).is_err());
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn realms_merged_or_dissolved_at_war_end_their_wars_at_the_month_start() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    let war = sim.world.wars.current(a, b).expect("at war");
    // a merges into b, its enemy: no realm at war with itself, and nothing between them goes on.
    sim.edit(Edit::SetOwner { provinces: land_of(&sim.world, a), realm: b }).unwrap();
    assert!(!at_war(&sim.world, a, b), "a war with a dissolved realm counts as over");
    past_month_start(&mut sim);
    assert!(!sim.world.wars.ongoing(war), "and ends at the month's start");
    assert!(!sim.world.occupier.contains(&a));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn an_occupation_ends_when_the_land_goes_to_a_realm_not_at_war_with_the_occupier() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    let others: Vec<u32> = alive_realms(&sim.world).into_iter().filter(|&r| r != a && r != b).collect();
    let (c, d) = (others[0], others[1]);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.poke(Event::DeclareWar { attacker: b, defender: d });
    sim.run_days(2);
    let [p, q] = [border_province(&sim.world, a, None), land_of(&sim.world, a).into_iter().rev().find(|&x| x != sim.world.realms.capital[a as usize]).unwrap()];
    assert_ne!(p, q);
    // b occupies two of a's provinces (set by hand, as Warfare would).
    sim.world.occupier[p as usize] = b;
    sim.world.occupier[q as usize] = b;
    // Given to c, at peace with b: the occupation ends. Given to d, at war with b: it stays.
    sim.edit(Edit::SetOwner { provinces: vec![p], realm: c }).unwrap();
    sim.edit(Edit::SetOwner { provinces: vec![q], realm: d }).unwrap();
    assert_eq!((sim.world.occupier[p as usize], sim.world.occupier[q as usize]), (NONE, b));
    // Given to the occupier itself: it simply owns it.
    sim.edit(Edit::SetOwner { provinces: vec![q], realm: b }).unwrap();
    assert_eq!(sim.world.occupier[q as usize], NONE);
}

#[test]
fn provinces_break_away_at_war_by_the_players_poke() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 6);
    let cap = sim.world.realms.capital[a as usize];
    let part: Vec<u32> = land_of(&sim.world, a).into_iter().filter(|&p| p != cap).take(1).collect();
    let realms = sim.world.realms.count() as u32;
    sim.poke(Event::Breakaway { provinces: part.clone() });
    sim.run_days(2);
    let w = &sim.world;
    assert!(w.realms.alive(realms) && w.owner[part[0] as usize] == realms, "one province may break away");
    assert_eq!(w.realms.broke_from[realms as usize], a);
    sim.run_days(1);
    assert!(at_war(&sim.world, realms, a), "at war with the realm it left from the next day");
    // Refused: a capital, or provinces of two realms.
    sim.poke(Event::Breakaway { provinces: vec![cap] });
    sim.poke(Event::Breakaway { provinces: vec![border_province(&sim.world, a, None), border_province(&sim.world, b, None)] });
    sim.run_days(3);
    let why = refusals(&sim);
    assert_eq!(why.len(), 2, "{why:?}");
    assert!(why.iter().any(|w| w.contains("capital")) && why.iter().any(|w| w.contains("one realm")), "{why:?}");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn treasuries_are_set_under_markets_only() {
    let mut c = config(5, false);
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    let mut sim = strict(&c);
    sim.run_days(40);
    let r = alive_realms(&sim.world)[0];
    sim.edit(Edit::SetRealmStock { realm: r, good: "coin".into(), amount: 123_456 }).unwrap();
    assert_eq!(coins(&sim.world, &sim.world.realm_stock, r as usize), 123_456);
    sim.run_days(40);
    assert_eq!(sim.problems, Problems::default(), "the coins add up");
    assert!(sim.edit(Edit::SetRealmStock { realm: r, good: "nothing".into(), amount: 1 }).is_err());

    let mut v0 = strict(&config(5, false));
    v0.run_days(2);
    let e = v0.edit(Edit::SetRealmStock { realm: r, good: "food".into(), amount: 1 }).unwrap_err();
    assert!(e.contains("no module keeps"), "{e}");
}

#[test]
fn opinions_are_set_where_realms_keep_them() {
    let mut c = config(5, false);
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    let mut sim = strict(&c);
    sim.run_days(2);
    let alive = alive_realms(&sim.world);
    // Two realms far apart have no relation yet; setting one makes it.
    let (a, b) = (alive[0], *alive.last().unwrap());
    let had = sim.world.relations.find(a, b);
    sim.edit(Edit::SetOpinion { a: b, b: a, a_of_b: -40.0, b_of_a: 25.0 }).unwrap();
    let rel = &sim.world.relations;
    assert!(rel.find(a, b).is_some());
    assert_eq!((rel.opinion(b, a), rel.opinion(a, b)), (-40.0, 25.0));
    if had.is_none() {
        assert_eq!(rel.find(a, b), Some(rel.count() as u32 - 1), "a new relation, numbered after the last");
    }
    assert!(sim.edit(Edit::SetOpinion { a, b, a_of_b: 101.0, b_of_a: 0.0 }).is_err());
    assert!(sim.edit(Edit::SetOpinion { a, b: a, a_of_b: 0.0, b_of_a: 0.0 }).is_err());
    sim.run_days(60);
    assert_eq!(sim.problems, Problems::default());

    let mut wars = strict(&config(5, false));
    wars.run_days(2);
    let e = wars.edit(Edit::SetOpinion { a, b, a_of_b: 0.0, b_of_a: 0.0 }).unwrap_err();
    assert!(e.contains("no module keeps"), "{e}");
}

#[test]
fn rulers_are_given_traits_put_on_the_throne_and_killed() {
    let mut sim = strict(&config(5, true));
    sim.run_days(2);
    let r = alive_realms(&sim.world)[0];
    let old = sim.world.realms.ruler[r as usize];
    sim.edit(Edit::SetTraits { character: old, martial: 90, greed: 5, ability: 100 }).unwrap();
    let ch = &sim.world.characters;
    assert_eq!((ch.martial[old as usize], ch.greed[old as usize], ch.ability[old as usize]), (90, 5, 100));
    assert!(sim.edit(Edit::SetTraits { character: old, martial: 101, greed: 0, ability: 0 }).is_err());

    // A new ruler: no succession event, the old one set aside alive, the heir still heir.
    let heir = sim.world.realms.heir[r as usize];
    let seen = sim.event_log.entries.len();
    sim.edit(Edit::NewRuler { realm: r, age: 30.0, martial: 10, greed: 20, ability: 30 }).unwrap();
    let w = &sim.world;
    let new = w.realms.ruler[r as usize];
    assert_ne!(new, old);
    assert!(w.characters.alive(old) && w.characters.alive(new));
    assert_eq!(w.realms.heir[r as usize], heir);
    assert_eq!(Succession::from_u8(w.characters.succession[new as usize]), Succession::Placed);
    assert!((w.characters.age(new, sim.day, sim.calendar.days_per_year()) - 30.0).abs() < 0.01);
    sim.run_days(3);
    assert!(!sim.event_log.entries[seen..].iter().any(|p| matches!(p.event, Event::Succession { .. })));
    // Set aside, the old ruler can't be killed as one.
    sim.poke(Event::KillRuler { character: old });
    sim.run_days(3);
    assert_eq!(refusals(&sim).len(), 1);

    // Killed with a child heir: the child takes the throne at once, and unrest stirs.
    let child = sim.world.characters.count() as u32;
    sim.edit(Edit::NewRuler { realm: r, age: 40.0, martial: 50, greed: 50, ability: 50 }).unwrap();
    let ruler = sim.world.realms.ruler[r as usize];
    assert_eq!(ruler, child);
    // An heir aged 5.
    let kid = sim.world.characters.count() as u32;
    sim.world.characters.push(r, ruler, kid, sim.day as f64 - 5.0 * sim.calendar.days_per_year() as f64, [50, 50, 50]);
    sim.world.realms.heir[r as usize] = kid;
    let land = land_of(&sim.world, r);
    let before: Vec<u64> = land.iter().map(|&p| sim.world.unrest[p as usize]).collect();
    sim.poke(Event::KillRuler { character: ruler });
    sim.run_days(3);
    let w = &sim.world;
    assert_eq!(w.characters.died[ruler as usize], sim.day - 2, "killed the day the poke arrives");
    assert_eq!(w.realms.ruler[r as usize], kid);
    assert_eq!(Succession::from_u8(w.characters.succession[kid as usize]), Succession::Child);
    assert!(land.iter().zip(&before).all(|(&p, &b)| w.unrest[p as usize] > b || w.unrest[p as usize] == crate::realms::LINE), "a child heir stirs unrest");

    // Killed with no heir: a claimant. And an heir killed leaves the place empty at once.
    let next = sim.world.realms.heir[r as usize];
    assert_eq!(next, NONE, "the child had no heir yet");
    sim.poke(Event::KillRuler { character: kid });
    sim.run_days(2);
    let w = &sim.world;
    let claimant = w.realms.ruler[r as usize];
    assert_eq!(Succession::from_u8(w.characters.succession[claimant as usize]), Succession::Claimant);
    sim.edit(Edit::NewRuler { realm: r, age: 50.0, martial: 50, greed: 50, ability: 50 }).unwrap();
    let ruler = sim.world.realms.ruler[r as usize];
    let h = sim.world.characters.count() as u32;
    sim.world.characters.push(r, ruler, h, sim.day as f64 - 20.0 * sim.calendar.days_per_year() as f64, [1, 2, 3]);
    sim.world.realms.heir[r as usize] = h;
    sim.poke(Event::KillRuler { character: h });
    sim.run_days(2);
    let w = &sim.world;
    assert!(w.characters.died[h as usize] != NEVER && w.realms.heir[r as usize] == NONE && w.realms.ruler[r as usize] == ruler);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn soldiers_placed_at_home_in_peace_go_home_at_the_month_start() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let r = alive_realms(&sim.world)[0];
    let p = sim.world.realms.capital[r as usize];
    let before = people(&sim);
    sim.edit(Edit::PlaceArmy { province: p, realm: r, kind: "levies".into(), size: 500 }).unwrap();
    let a = sim.world.armies.count() as u32 - 1;
    assert_eq!(people(&sim), before + 500, "made from nothing");
    let arm = &sim.world.armies;
    assert_eq!((arm.size[a as usize], arm.role[a as usize], arm.province[a as usize], arm.home[a as usize]), (500, FIELD, p, p));
    // A second placing joins the same company.
    sim.edit(Edit::PlaceArmy { province: p, realm: r, kind: "levies".into(), size: 100 }).unwrap();
    assert_eq!((sim.world.armies.count() as u32 - 1, sim.world.armies.size[a as usize]), (a, 600));
    past_month_start(&mut sim);
    assert_eq!(sim.world.armies.size[a as usize], 0, "peace keeps only guards");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn soldiers_are_placed_only_where_their_realm_may_stand() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    let theirs = border_province(&sim.world, b, Some(a));
    let e = sim.edit(Edit::PlaceArmy { province: theirs, realm: a, kind: "levies".into(), size: 10 }).unwrap_err();
    assert!(e.contains("own land"), "{e}");
    let mine = sim.world.realms.capital[a as usize];
    for (kind, why) in [("dragons", "no troop kind"), ("sea", "class")] {
        let e = sim.edit(Edit::PlaceArmy { province: mine, realm: a, kind: kind.into(), size: 10 }).unwrap_err();
        assert!(e.contains(why), "{e}");
    }
    let inland = (0..sim.world.provinces.count).find(|&p| sim.world.owner[p] == a && sim.world.provinces.coast_km[p] <= 0.0);
    if let Some(p) = inland {
        assert!(sim.edit(Edit::PlaceArmy { province: p as u32, realm: a, kind: "ships".into(), size: 10 }).unwrap_err().contains("coast"));
    }
    assert!(sim.edit(Edit::PlaceArmy { province: mine, realm: a, kind: "levies".into(), size: 0 }).is_err());
}

#[test]
fn soldiers_placed_in_enemy_land_besiege_it() {
    let mut c = config(5, false);
    // Quiet generals: they never attack, so only the placed company does anything.
    c.modules.warfare.as_mut().unwrap().outmatch = 1e9;
    let mut sim = strict(&c);
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    let p = border_province(&sim.world, b, Some(a));
    let guards: u64 = (0..sim.world.armies.count()).filter(|&k| sim.world.armies.province[k] == p && sim.world.armies.size[k] > 0).map(|k| sim.world.armies.size[k]).sum();
    sim.edit(Edit::PlaceArmy { province: p, realm: a, kind: "heavy_infantry".into(), size: 20 * guards.max(100) }).unwrap();
    let k = sim.world.armies.count() - 1;
    assert_eq!((sim.world.armies.province[k], sim.world.armies.home[k]), (p, sim.world.realms.capital[a as usize]));
    let mut taken = None;
    for _ in 0..400 {
        sim.run_days(1);
        if sim.world.occupier[p as usize] == a {
            taken = Some(sim.day);
            break;
        }
    }
    assert!(taken.is_some(), "the placed company takes the province");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_company_is_resized_on_the_day_its_food_comes() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 4);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    // Past a month's start, then on the next month's second day (its food comes that day).
    past_month_start(&mut sim);
    while !sim.calendar.is_month_start(sim.day) {
        sim.run_days(1);
    }
    sim.run_days(1);
    let k = (0..sim.world.armies.count()).find(|&k| sim.world.armies.realm[k] == a && sim.world.armies.size[k] > 10).expect("a company with soldiers") as u32;
    let kind = crate::taxonomy::Taxonomy::named(&TroopParams::default().tree, "troop tree").unwrap().name(sim.world.armies.kind[k as usize]).to_string();
    sim.edit(Edit::SetArmy { army: k, size: 1, kind: kind.clone() }).unwrap();
    sim.run_days(3);
    assert!(sim.world.armies.size[k as usize] <= 1);
    sim.edit(Edit::SetArmy { army: k, size: 0, kind: "cavalry".into() }).unwrap();
    let arm = &sim.world.armies;
    assert_eq!((arm.size[k as usize], arm.province[k as usize], arm.group[k as usize], arm.next[k as usize]), (0, arm.home[k as usize], NONE, NONE));
    sim.run_days(40);
    assert_eq!(sim.problems, Problems::default(), "people add up");
}

#[test]
fn an_ordered_company_marches_where_sent_and_the_general_leaves_it_alone_until_then() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let (a, b) = rivals(&sim.world, 6);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    // A company at a's capital, sent to a neighbouring province of a's own (a longer way could
    // cross enemy land, where enemies standing in the way would stop it).
    let cap = sim.world.realms.capital[a as usize];
    sim.edit(Edit::PlaceArmy { province: cap, realm: a, kind: "levies".into(), size: 300 }).unwrap();
    let k = sim.world.armies.count() - 1;
    let to = sim.world.routes.of(cap).filter(|l| l.land_days.is_some()).map(|l| l.other(cap)).find(|&q| sim.world.owner[q as usize] == a).expect("a neighbour of its own");
    sim.poke(Event::March { realm: a, from: cap, to });
    sim.run_days(3);
    assert!(refusals(&sim).is_empty(), "{:?}", refusals(&sim));
    assert_eq!(sim.world.armies.dest[k], to);
    // Every day until it stops, it keeps going there, whatever the general plans; it stops there,
    // or where enemies stand in its way.
    let mut days = 0;
    while sim.world.armies.next[k] != NONE {
        assert_eq!(sim.world.armies.dest[k], to, "day {days}");
        sim.run_days(1);
        days += 1;
        assert!(days < 400);
    }
    let at = sim.world.armies.province[k];
    let foes_there = (0..sim.world.armies.count()).any(|j| sim.world.armies.province[j] == at && sim.world.armies.size[j] > 0 && sim.world.armies.realm[j] == b && sim.world.armies.next[j] == NONE);
    assert!(at == to || foes_there, "it arrived, or enemies stopped it in {at}");
    // Ordered away from a battle: refused. Both sides placed in one province fight.
    let p = border_province(&sim.world, a, Some(b));
    sim.edit(Edit::PlaceArmy { province: p, realm: a, kind: "levies".into(), size: 5000 }).unwrap();
    sim.edit(Edit::PlaceArmy { province: p, realm: b, kind: "levies".into(), size: 5000 }).unwrap();
    sim.run_days(2);
    sim.poke(Event::March { realm: a, from: p, to: cap });
    sim.run_days(3);
    assert!(refusals(&sim).iter().any(|w| w.contains("battle")), "{:?}", refusals(&sim));
    // No company there: refused.
    sim.poke(Event::March { realm: b, from: cap, to: p });
    sim.run_days(3);
    assert!(refusals(&sim).iter().any(|w| w.contains("no field companies")));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn companies_on_land_that_goes_away_move_to_land() {
    let mut sim = strict(&config(5, false));
    sim.run_days(2);
    let r = alive_realms(&sim.world)[0];
    let land = land_of(&sim.world, r);
    let cap = sim.world.realms.capital[r as usize];
    let (from, into) = {
        let w = &sim.world;
        let from = *land.iter().find(|&&p| p != cap).unwrap();
        let into = w.routes.of(from).map(|l| l.other(from)).find(|&q| w.owner[q as usize] == r).unwrap();
        (from, into)
    };
    sim.edit(Edit::PlaceArmy { province: from, realm: r, kind: "levies".into(), size: 50 }).unwrap();
    let k = sim.world.armies.count() - 1;
    sim.edit(Edit::Merge { from, into }).unwrap();
    assert_eq!((sim.world.armies.province[k], sim.world.armies.home[k]), (into, into), "merged away: into the province it joined");

    // Sunk: to the nearest land.
    sim.edit(Edit::PlaceArmy { province: into, realm: r, kind: "levies".into(), size: 50 }).unwrap();
    let (lon, lat) = dir_to_lonlat(sim.world.provinces.center[into as usize]);
    sim.edit(Edit::Paint { path: vec![[lon.to_degrees(), lat.to_degrees()]], radius_km: 3000.0, brush: Brush::Flatten(-200.0) }).unwrap();
    let w = &sim.world;
    assert_eq!(w.provinces.cells[into as usize], 0, "the province sank");
    {
        for k in 0..w.armies.count() {
            let (at, home) = (w.armies.province[k] as usize, w.armies.home[k] as usize);
            assert!(w.provinces.cells[at] > 0 && w.provinces.cells[home] > 0, "company {k}");
        }
    }
    sim.run_days(60);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn every_new_edit_and_poke_survives_saving_loading_going_back_and_a_scenario_file() {
    let mut c = config(5, true);
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    let fresh = || strict(&c);
    let mut sim = fresh();
    sim.run_days(40);
    let (a, b) = rivals(&sim.world, 6);
    let cap = sim.world.realms.capital[a as usize];
    let p = border_province(&sim.world, a, Some(b));
    let ruler = sim.world.realms.ruler[b as usize];
    let part: Vec<u32> = land_of(&sim.world, b).into_iter().filter(|&q| q != sim.world.realms.capital[b as usize]).take(2).collect();
    sim.edit(Edit::SetOpinion { a, b, a_of_b: -90.0, b_of_a: -90.0 }).unwrap();
    sim.edit(Edit::SetRealmStock { realm: a, good: "coin".into(), amount: 1_000_000 }).unwrap();
    sim.edit(Edit::SetTraits { character: ruler, martial: 100, greed: 100, ability: 0 }).unwrap();
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(3);
    sim.edit(Edit::PlaceArmy { province: cap, realm: a, kind: "levies".into(), size: 2000 }).unwrap();
    sim.poke(Event::March { realm: a, from: cap, to: p });
    sim.run_days(5);
    sim.edit(Edit::NewRuler { realm: a, age: 33.0, martial: 70, greed: 10, ability: 90 }).unwrap();
    sim.poke(Event::Breakaway { provinces: part.clone() });
    sim.run_days(10);
    let k = sim.world.armies.count() as u32 - 1;
    sim.edit(Edit::SetArmy { army: k, size: 1500, kind: "levies".into() }).unwrap();
    sim.poke(Event::KillRuler { character: sim.world.realms.ruler[a as usize] });
    sim.run_days(10);
    let spare = land_of(&sim.world, a).into_iter().rev().find(|&q| q != sim.world.realms.capital[a as usize]).unwrap();
    sim.edit(Edit::NewRealm { provinces: vec![spare] }).unwrap();
    sim.edit(Edit::SetOwner { provinces: vec![spare], realm: a }).unwrap();
    sim.run_days(30);
    assert!(refusals(&sim).is_empty(), "{:?}", refusals(&sim));

    let stops = [sim.day, sim.day + 50];
    let mut prints = Vec::new();
    for &d in &stops {
        sim.run_days(d - sim.day);
        let mut loaded = Sim::load(&sim.save()).unwrap();
        loaded.strict = true;
        let mut twin = sim.fork();
        twin.run_days(30);
        loaded.run_days(30);
        assert_eq!(loaded.fingerprint(), twin.fingerprint(), "saved on day {d}");
        prints.push(sim.fingerprint());
    }
    let (today, now) = (sim.day, sim.fingerprint());
    for (&d, &print) in stops.iter().zip(&prints).rev() {
        sim.goto(d).unwrap();
        assert_eq!(sim.fingerprint(), print, "back to day {d}");
    }
    sim.goto(40).unwrap();
    sim.goto(today).unwrap();
    assert_eq!(sim.fingerprint(), now, "forward again, every edit and poke made again");

    let text = crate::branch::Timelines::new(sim.fork()).scenario_text();
    for word in ["SetOpinion", "SetRealmStock", "SetTraits", "PlaceArmy", "March", "NewRuler", "Breakaway", "SetArmy", "KillRuler", "NewRealm", "SetOwner"] {
        assert!(text.contains(word), "{word} is written");
    }
    let mut read = crate::branch::Timelines::from_scenario_text(&text).unwrap();
    let back = read.list.swap_remove(read.active).sim;
    assert_eq!((back.day, back.fingerprint()), (sim.day, sim.fingerprint()));
    assert_eq!(sim.problems, Problems::default());
}
