//! The war state (step 7, Diplomacy "wars"): wars declared and ended by pokes, truces, peace
//! terms, refusals, and wars kept through saves, going back and scenario files. Nothing occupies
//! land yet (armies come later), so the tests poke occupation in directly, as warfare will send it.

use crate::branch::Timelines;
use crate::characters::RulerParams;
use crate::config::Config;
use crate::diplomacy::{at_war, enemies, standing, Refusal, Standing, WarCause, WarParams, NEVER};
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::fields::Tier;
use crate::grid::NONE;
use crate::module::{plan, Cadence, Manifest, Slot};
use crate::realms::RealmParams;
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};

fn config(seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.characters = Some(RulerParams::default());
    c.modules.diplomacy = Some(WarParams::default());
    c
}

fn world(seed: u64) -> Sim {
    let mut sim = Sim::new(&config(seed)).unwrap();
    sim.strict = true;
    sim
}

fn land_of(w: &World, r: u32) -> Vec<u32> {
    (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r).collect()
}

/// Two realms that border each other, each with at least `min` provinces.
fn neighbours(w: &World, min: usize) -> (u32, u32) {
    for p in 0..w.provinces.count as u32 {
        let a = w.owner[p as usize];
        if a == NONE || land_of(w, a).len() < min {
            continue;
        }
        for l in w.routes.of(p) {
            let b = w.owner[l.other(p) as usize];
            if b != NONE && b != a && land_of(w, b).len() >= min {
                return (a, b);
            }
        }
    }
    panic!("no two realms of {min} provinces border each other");
}

/// Poke today and run until the poke and what it sets off (a peace's land changing hands, two days
/// after) are done.
fn poke(sim: &mut Sim, e: Event) {
    sim.poke(e);
    sim.run_days(3);
}

/// What the event log says happened to pokes about war, oldest first.
fn refusals(sim: &Sim) -> Vec<Refusal> {
    sim.event_log.entries.iter().filter_map(|p| if let Event::WarRefused { why, .. } = p.event { Some(why) } else { None }).collect()
}

#[test]
fn a_war_declared_by_poke_goes_on_until_a_peace_then_a_truce_then_peace() {
    let mut sim = world(1);
    let (a, b) = neighbours(&sim.world, 1);
    assert_eq!(standing(&sim.world, a, b, sim.day), Standing::Peace);
    let declared = sim.day + 1;
    poke(&mut sim, Event::DeclareWar { attacker: a, defender: b });
    let w = &sim.world;
    assert_eq!(w.wars.count(), 1);
    assert_eq!((w.wars.attacker[0], w.wars.defender[0], w.wars.began[0], w.wars.ended[0]), (a, b, declared, NEVER));
    assert_eq!(WarCause::from_u8(w.wars.cause[0]), WarCause::Poke);
    assert!(at_war(w, a, b) && at_war(w, b, a) && !at_war(w, a, a));
    assert_eq!(standing(w, b, a, sim.day), Standing::War(0));
    assert_eq!((enemies(w)[a as usize].clone(), enemies(w)[b as usize].clone()), (vec![b], vec![a]));
    let told = sim.event_log.entries.iter().any(|p| p.event == Event::WarDeclared { war: 0, attacker: a, defender: b, cause: WarCause::Poke });
    assert!(told, "the war is announced");

    // Months of war with nothing occupied: the peace hands over nothing.
    sim.run_days(100);
    let owners = sim.world.owner.clone();
    let ended = sim.day + 1;
    poke(&mut sim, Event::ForcePeace { a: b, b: a, winner: a });
    let w = &sim.world;
    assert_eq!((w.wars.ended[0], w.wars.winner[0]), (ended, a));
    let year = sim.calendar.days_per_year() as u64;
    assert_eq!(w.wars.truce_until[0], ended + 5 * year);
    assert_eq!(w.owner, owners);
    assert!(!at_war(w, a, b) && enemies(w).iter().all(|e| e.is_empty()));
    assert_eq!(standing(w, a, b, sim.day), Standing::Truce(ended + 5 * year));
    assert_eq!(standing(w, a, b, ended + 5 * year), Standing::Peace);
    assert!(refusals(&sim).is_empty());
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn the_winner_keeps_what_it_occupies_and_everything_else_goes_back() {
    // Three peaces from the same war: won by the attacker, by the defender, and by nobody.
    for winner in [0, 1, 2] {
        let mut sim = world(2);
        let (a, b) = neighbours(&sim.world, 3);
        let (land_a, land_b) = (land_of(&sim.world, a), land_of(&sim.world, b));
        // a takes two of b's provinces, b one of a's; a takes one of its own back at once.
        let (b1, b2, a1) = (land_b[0], land_b[1], land_a[0]);
        sim.poke(Event::DeclareWar { attacker: a, defender: b });
        sim.run_days(2);
        for (province, by) in [(b1, a), (b2, a), (a1, b), (land_a[1], b)] {
            sim.poke(Event::Occupied { province, by });
        }
        sim.run_days(1);
        poke(&mut sim, Event::Occupied { province: land_a[1], by: a });
        let w = &sim.world;
        assert_eq!((w.occupier[b1 as usize], w.occupier[b2 as usize], w.occupier[a1 as usize], w.occupier[land_a[1] as usize]), (a, a, b, NONE));
        assert_eq!(w.owner[b1 as usize], b, "occupied land changes hands only at the peace");

        let won = [a, b, NONE][winner];
        poke(&mut sim, Event::ForcePeace { a, b, winner: won });
        let w = &sim.world;
        assert!(w.occupier.iter().all(|&o| o == NONE), "every occupation between the two ends");
        let (to_b1, to_a1) = match winner {
            0 => (a, a),
            1 => (b, b),
            _ => (b, a),
        };
        assert_eq!((w.owner[b1 as usize], w.owner[b2 as usize], w.owner[a1 as usize]), (to_b1, to_b1, to_a1), "won by {won}");
        // Nothing else changed hands.
        for &p in land_a.iter().chain(&land_b).filter(|&&p| ![b1, b2, a1].contains(&p)) {
            assert_eq!(w.owner[p as usize], if land_a.contains(&p) { a } else { b });
        }
        for r in [a, b] {
            assert_eq!(w.owner[w.realms.capital[r as usize] as usize], r, "realm {r} holds its capital");
        }
        assert_eq!(sim.problems, Problems::default());
    }
}

#[test]
fn a_realm_that_loses_all_its_land_is_dissolved_and_its_other_wars_end() {
    let mut sim = Sim::new(&Config { modules: { let mut m = config(3).modules; m.realms = Some(RealmParams { settled: 0.0, city_states: 1.0, ..RealmParams::default() }); m }, ..config(3) }).unwrap();
    sim.strict = true;
    let (a, b) = neighbours(&sim.world, 1);
    // A third realm, at war with b too.
    let c = (0..sim.world.realms.count() as u32).find(|&r| r != a && r != b && sim.world.realms.alive(r)).unwrap();
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.poke(Event::DeclareWar { attacker: c, defender: b });
    sim.run_days(2);
    for province in land_of(&sim.world, b) {
        sim.poke(Event::Occupied { province, by: a });
    }
    sim.run_days(2);
    let capital = sim.world.realms.capital[b as usize];
    poke(&mut sim, Event::ForcePeace { a, b, winner: a });
    let w = &sim.world;
    assert!(!w.realms.alive(b) && land_of(w, b).is_empty());
    assert_eq!(w.owner[capital as usize], a);
    // The war with c counts as over at once, and ends at the next month's start, won by c.
    assert!(!at_war(w, c, b));
    let war = w.wars.current(c, b).expect("its entry ends at the month's start");
    let month = (sim.day..).find(|&d| sim.calendar.is_month_start(d)).unwrap();
    sim.run_days(month + 2 - sim.day);
    let w = &sim.world;
    assert_eq!((w.wars.ended[war as usize], w.wars.winner[war as usize]), (month, c));
    assert!(sim.event_log.entries.iter().any(|p| p.event == Event::Peace { war, attacker: c, defender: b, winner: c, rebellion: false }));
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn pokes_that_dont_fit_are_refused_and_say_why() {
    let mut sim = world(4);
    let (a, b) = neighbours(&sim.world, 1);
    let dead = sim.world.realms.count() as u32 + 3;
    poke(&mut sim, Event::DeclareWar { attacker: a, defender: a });
    poke(&mut sim, Event::DeclareWar { attacker: a, defender: dead });
    poke(&mut sim, Event::ForcePeace { a, b, winner: NONE });
    poke(&mut sim, Event::DeclareWar { attacker: a, defender: b });
    poke(&mut sim, Event::DeclareWar { attacker: b, defender: a });
    poke(&mut sim, Event::ForcePeace { a, b, winner: dead });
    use Refusal::*;
    assert_eq!(refusals(&sim), [SameRealm, NoSuchRealm, NotAtWar, AlreadyAtWar, WinnerNotInWar]);
    assert_eq!(sim.world.wars.count(), 1);
    assert!(at_war(&sim.world, a, b));
    for why in refusals(&sim) {
        assert!(!why.describe().is_empty());
    }
}

#[test]
fn a_war_poke_breaks_a_truce_and_the_players_order_holds_on_one_day() {
    let mut sim = world(5);
    let (a, b) = neighbours(&sim.world, 1);
    // Declared and ended by two pokes on the same day: the war goes on for none of it.
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.poke(Event::ForcePeace { a, b, winner: NONE });
    sim.run_days(3);
    let w = &sim.world;
    assert_eq!(w.wars.count(), 1);
    assert_eq!(w.wars.began[0], w.wars.ended[0]);
    assert!(matches!(standing(w, a, b, sim.day), Standing::Truce(_)));
    // Under the truce, the player's poke still starts a war: the truce ends that day.
    sim.run_days(30);
    let day = sim.day + 1;
    poke(&mut sim, Event::DeclareWar { attacker: b, defender: a });
    let w = &sim.world;
    assert_eq!((w.wars.count(), w.wars.truce_until[0], w.wars.attacker[1]), (2, day, b));
    assert_eq!(standing(w, a, b, sim.day), Standing::War(1));
    assert!(refusals(&sim).is_empty());
}

#[test]
fn wars_survive_saving_loading_going_back_and_scenario_files() {
    let mut sim = world(6);
    let (a, b) = neighbours(&sim.world, 2);
    let first = land_of(&sim.world, b)[0];
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    sim.poke(Event::Occupied { province: first, by: a });
    sim.run_days(40);
    let (print, today) = (sim.fingerprint(), sim.day);
    assert_eq!(sim.world.occupier[first as usize], a);

    let mut loaded = Sim::load(&sim.save()).unwrap();
    assert_eq!(loaded.fingerprint(), print);
    for s in [&mut sim, &mut loaded] {
        s.poke(Event::ForcePeace { a, b, winner: a });
        s.run_days(400);
    }
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    assert_eq!(sim.world.owner[first as usize], a);

    let end = sim.fingerprint();
    sim.goto(today).unwrap();
    assert_eq!(sim.fingerprint(), print);
    assert_eq!((sim.world.occupier[first as usize], sim.world.owner[first as usize]), (a, b));
    sim.goto(today + 400).unwrap();
    assert_eq!(sim.fingerprint(), end);

    let text = Timelines::new(sim.fork()).scenario_text();
    assert!(text.contains("truce_years") && text.contains("DeclareWar"), "the settings and pokes are written");
    let mut read = Timelines::from_scenario_text(&text).unwrap();
    let back = read.list.swap_remove(read.active).sim;
    assert_eq!((back.day, back.fingerprint()), (sim.day, sim.fingerprint()));
    assert_eq!(back.world.wars, sim.world.wars);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_split_province_keeps_its_occupier_and_an_emptied_one_is_freed() {
    let mut sim = world(7);
    let (a, b) = neighbours(&sim.world, 2);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    let w = &sim.world;
    let p = *land_of(w, b).iter().max_by_key(|&&p| w.provinces.cells[p as usize]).unwrap();
    poke(&mut sim, Event::Occupied { province: p, by: a });
    let cells: Vec<u32> = (0..sim.world.grid.cells() as u32).filter(|&c| sim.world.provinces.of_cell[c as usize] == p).collect();
    sim.edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    let new = sim.world.provinces.count - 1;
    assert_eq!((sim.world.owner[new], sim.world.occupier[new]), (b, a));
    // Merged away, it has no land, no owner and no occupier.
    let q = sim.world.routes.of(new as u32).map(|l| l.other(new as u32)).find(|&q| q != p).unwrap();
    sim.edit(Edit::Merge { from: new as u32, into: q }).unwrap();
    assert_eq!((sim.world.owner[new], sim.world.occupier[new]), (NONE, NONE));
    sim.run_days(40);
    sim.world.check_lengths().unwrap();
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn wars_need_realms_and_a_sensible_truce() {
    let mut c = config(1);
    c.modules.realms = None;
    c.modules.characters = None;
    let e = Sim::new(&c).err().expect("refused without realms").0;
    assert!(e.contains("realm.capital"), "{e}");
    for bad in [-1.0, f64::NAN, f64::INFINITY] {
        let mut c = config(1);
        c.modules.diplomacy = Some(WarParams { truce_years: bad, ..WarParams::default() });
        assert!(Sim::new(&c).is_err(), "{bad}");
    }
    // Realms without wars: no war entries, nobody occupied, and it saves and loads.
    let mut c = config(1);
    c.modules.diplomacy = None;
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    sim.run_days(40);
    assert!(sim.world.wars.count() == 0 && sim.world.occupier.iter().all(|&o| o == NONE));
    assert_eq!(Sim::load(&sim.save()).unwrap().fingerprint(), sim.fingerprint());
}

#[test]
fn without_pokes_wars_change_nothing() {
    // Without unrest, as rebels breaking away start wars of their own.
    let mut c = config(2);
    c.modules.realms.as_mut().unwrap().unrest = None;
    let mut wars = Sim::new(&c).unwrap();
    c.modules.diplomacy = None;
    let mut plain = Sim::new(&c).unwrap();
    for s in [&mut plain, &mut wars] {
        s.run_to_year(s.calendar.date(0).year + 3);
    }
    // The world's values only: the whole simulation's fingerprint also counts each module.
    let print = |s: &Sim| {
        let mut h = crate::world::Fingerprint::default();
        s.world.fingerprint(&mut h);
        h.0
    };
    assert_eq!(print(&plain), print(&wars));
    assert_eq!(wars.problems, Problems::default());
}

/// What armies will be: a module that reads the wars and occupation, and sends occupation.
static TAKER: Manifest = Manifest {
    slot: Slot::Warfare,
    version: "taker",
    tier: Tier::Army,
    cadence: Cadence::Daily,
    reads: &["province.owner", "province.occupier", "war.attacker", "war.defender", "war.ended"],
    owns: &["army.realm", "army.province", "army.size", "army.kind", "army.home"],
    requests: &[],
    asks: &[],
    moves: &[],
    requires: &[],
    emits: &[EventKind::Occupied],
    handles: &[EventKind::Peace],
    state_schema: "none",
    goods: &[],
};

#[test]
fn realms_run_before_wars_and_wars_before_armies() {
    // The read graph the rest of step 7 builds on: realms keep the land and occupation, the wars
    // read realms, armies read both and send occupation back by event.
    for economics in ["food", "markets"] {
        let mut c = config(1);
        c.modules.set_choice(Slot::Economics, economics).unwrap();
        let modules = c.modules.build();
        let mut manifests: Vec<&Manifest> = modules.iter().map(|m| m.manifest()).collect();
        manifests.push(&TAKER);
        let p = plan(&manifests).unwrap();
        let at = |slot: Slot| p.runtime.iter().position(|&i| manifests[i].slot == slot).unwrap();
        assert!(at(Slot::Population) < at(Slot::Realms) && at(Slot::Realms) < at(Slot::Diplomacy) && at(Slot::Diplomacy) < at(Slot::Warfare), "{economics}");
        assert!(at(Slot::Realms) < at(Slot::Characters));
    }
}
