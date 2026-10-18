//! The realm and ruler page, and realms and rulers by name in the words the inspector shows, built
//! from the window's copy of the world the way the window keeps it: copied once when it opens, then
//! given only the values the simulation sends each day. Realms that break away and rulers born
//! later have every value the page reads, and nothing shows as a raw "none" number. With Research
//! and Culture on, techs and cultures are named too, and the culture and tech layers have a value
//! for every province.

use app::inspector::{self, LogEntry, Names, RealmNames};
use app::playback::{DayState, MapState};
use sim::config::Config;
use sim::events::Event;
use sim::fields::known;
use sim::grid::NONE;
use sim::module::Slot;
use sim::realms::RealmParams;
use sim::sim::Sim;
use sim::world::WorldParams;
use std::sync::Arc;

#[test]
fn every_realm_has_a_page_from_what_the_window_is_sent() {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed: 21, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.without_realms();
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, ..RealmParams::default() });
    c.modules.characters = Some(Default::default());
    c.modules.diplomacy = Some(Default::default());
    let mut sim = Sim::new(&c).unwrap();
    let first = sim.calendar.def.first_year;
    sim.run_to_year(first + 1);
    // The window opens here.
    let mut geo = sim.world.clone();
    let (realms_then, people_then) = (geo.realms.count(), geo.characters.count());

    // Decades of successions and uprisings, a war by poke, and a peace with a truce still on.
    sim.run_to_year(first + 50);
    let alive: Vec<u32> = (0..sim.world.realms.count() as u32).filter(|&r| sim.world.realms.alive(r)).collect();
    let (a, b) = (alive[0], alive[1]);
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(400);
    sim.poke(Event::ForcePeace { a, b, winner: a });
    sim.run_days(30);

    // What the window is sent for the day, copied in as it does.
    let state = DayState::of(&sim, Arc::new(MapState::of(0, 0, &sim)));
    for (id, data) in state.map.fields.iter().chain(&state.fields) {
        geo.set_field(*id, data.clone()).unwrap();
    }
    assert!(geo.realms.count() > realms_then, "no realm broke away in 50 years: the test checks nothing new");
    assert!(geo.characters.count() > people_then, "nobody was born or crowned in 50 years");

    let mut names = RealmNames::default();
    names.update(&geo, &sim.settings);
    let n = Names { world: &geo, realms: &names };
    let raw = |s: &str| s.contains("4,294,967,295") || s.contains("4294967295") || s.contains("18,446,744") || s.contains("18446744");
    let money = &sim.config.modules.markets.as_ref().unwrap().money;
    let coin = geo.goods.columns_under(geo.goods.tree.find(money).unwrap()).unwrap().first().copied();
    let mut broke_away = 0;
    let mut earlier_rulers = 0;
    let mut truces = 0;
    for r in 0..geo.realms.count() as u32 {
        let page = inspector::realm_page(&n, r, sim.day, &sim.calendar, coin, &sim.world.trade, &sim.settings);
        let mut all = vec![page.name.clone()];
        all.extend(page.facts.iter().cloned());
        all.extend(page.past_rulers.iter().cloned());
        all.extend(page.relations.iter().map(|x| x.1.clone()));
        for p in [&page.ruler, &page.heir].into_iter().flatten() {
            all.push(p.name.clone());
            all.extend(p.lines.iter().cloned());
        }
        for line in &all {
            assert!(!raw(line), "realm {r}: {line}");
            assert!(!line.contains("character "), "realm {r}: a person without a name: {line}");
        }
        assert!(!page.name.starts_with("realm "), "realm {r} has no name");
        if page.alive {
            assert!(page.ruler.is_some(), "{} has no ruler", page.name);
            assert!(page.facts[0].contains("province"), "{}", page.facts[0]);
            assert!(page.facts.iter().any(|f| f.starts_with("treasury: ")), "{}: no treasury under markets", page.name);
        }
        broke_away += page.facts.iter().any(|f| f.starts_with("broke away from")) as usize;
        earlier_rulers += page.past_rulers.len();
        truces += page.relations.iter().filter(|x| x.1.starts_with("truce with")).count();
    }
    assert!(broke_away > 0 && earlier_rulers > 0, "breakaways {broke_away}, earlier rulers {earlier_rulers}");
    assert!(truces >= 1, "the forced peace should leave a truce on both pages");

    // The Wars section: wars going on, and the forced peace among the recent ones.
    let (going, peaces) = inspector::wars(&n, sim.day, &sim.calendar, 5 * sim.calendar.days_per_year() as u64);
    for line in going.iter().flat_map(|g| std::iter::once(&g.title).chain(&g.lines)).chain(&peaces) {
        assert!(!raw(line) && !line.contains("realm "), "{line}");
    }
    assert!(going.iter().all(|g| sim.world.realms.alive(g.attacker) && sim.world.realms.alive(g.defender)));
    let (na, nb) = (n.realm(a), n.realm(b));
    assert!(peaces.iter().any(|p| p.contains(&na) && p.contains(&nb) && p.contains(&format!("{na} won"))), "{peaces:?}");

    // Every event in the log, and every province's realm values, in words.
    let mut about_realms = 0;
    for p in &sim.event_log.entries {
        let text = LogEntry::of_event(p, &geo.goods).text(&sim.calendar, &geo.goods, &n);
        assert!(!raw(&text), "{text}");
        assert!(!text.contains("realm ") && !text.contains("character "), "a realm or person by number: {text}");
        about_realms += !inspector::realms_in(&p.event).is_empty() as usize;
    }
    assert!(about_realms > 0);
    for name in ["province.owner", "province.occupier", "province.uprising", "province.unrest"] {
        let data = geo.field(known(name));
        for p in 0..geo.provinces.count {
            let v = inspector::value(known(name), &data, p, &n);
            assert!(!raw(&v), "{name} of province {p}: {v}");
        }
    }
    let rising = (0..geo.provinces.count).find(|&p| geo.uprising[p] != NONE);
    if let Some(p) = rising {
        let v = inspector::value(known("province.uprising"), &geo.field(known("province.uprising")), p, &n);
        assert!(v == "risen up" || v.starts_with("joined the uprising"), "{v}");
    }
    assert_eq!(inspector::value(known("province.unrest"), &sim::fields::FieldData::U64(vec![250]), 0, &n), "250 of 1,000 toward an uprising");
}

/// Whether `s` names a culture or tech by its bare number ("culture 12", "tech 3").
fn by_number(s: &str) -> bool {
    ["culture ", "tech "].iter().any(|w| s.match_indices(w).any(|(i, _)| s[i + w.len()..].starts_with(|c: char| c.is_ascii_digit())))
}

#[test]
fn techs_and_cultures_are_named_from_what_the_window_is_sent() {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed: 21, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    assert!(c.modules.research.is_some() && c.modules.culture.is_some(), "both on by default");
    let mut sim = Sim::new(&c).unwrap();
    let first = sim.calendar.def.first_year;
    sim.run_to_year(first + 1);
    let mut geo = sim.world.clone();
    sim.run_to_year(first + 60);
    let state = DayState::of(&sim, Arc::new(MapState::of(0, 0, &sim)));
    for (id, data) in state.map.fields.iter().chain(&state.fields) {
        geo.set_field(*id, data.clone()).unwrap();
    }
    let mut names = RealmNames::default();
    names.update(&geo, &sim.settings);
    let n = Names { world: &geo, realms: &names };

    let (mut knowing, mut with_culture) = (0, 0);
    for r in (0..geo.realms.count() as u32).filter(|&r| geo.realms.alive(r)) {
        let page = inspector::realm_page(&n, r, sim.day, &sim.calendar, None, &sim.world.trade, &sim.settings);
        let lines = page.facts.iter().chain(&page.knowledge).chain(page.techs.iter().flat_map(|f| &f.1));
        for l in lines {
            assert!(!by_number(l), "{}: {l}", page.name);
        }
        assert!(page.knowledge.first().is_some_and(|l| l.starts_with("knows ")), "{}: {:?}", page.name, page.knowledge);
        assert!(page.knowledge.iter().any(|l| l.starts_with("what its techs do") || l == "its techs do nothing yet"), "{:?}", page.knowledge);
        knowing += !page.techs.is_empty() as usize;
        with_culture += page.facts.iter().any(|f| f.starts_with("culture: ")) as usize;
    }
    assert!(knowing > 0 && with_culture > 0, "realms knowing techs {knowing}, with a culture {with_culture}");

    let lines = inspector::cultures(&n, &sim.calendar);
    assert!(!lines.is_empty());
    for l in &lines {
        assert!(!by_number(&l.text), "{}", l.text);
    }
    for p in 0..geo.provinces.count as u32 {
        let words = inspector::cultures_in_words(&n, &inspector::province_cultures(&geo, p));
        assert!(!by_number(&words), "province {p}: {words}");
    }
    let mut named = 0;
    for p in &sim.event_log.entries {
        if matches!(p.event, Event::Discovery { .. } | Event::CultureBorn { .. } | Event::CultureGone { .. } | Event::Assimilated { .. }) {
            let text = LogEntry::of_event(p, &geo.goods).text(&sim.calendar, &geo.goods, &n);
            assert!(!by_number(&text), "{text}");
            named += 1;
        }
    }
    assert!(named > 0, "no discovery or culture event in 60 years");

    // The layers: a main culture wherever people live, and techs wherever a realm owns land.
    let v = app::layers::SocietyView::of(&geo, true, true);
    let people = geo.province_population();
    for (p, &here) in people.iter().enumerate() {
        assert_eq!(v.culture[p].0 != NONE, here > 0, "province {p}");
        assert!(v.culture[p].1 <= 1.0);
        assert_eq!(!v.techs[p].is_nan(), geo.owner[p] != NONE, "province {p}");
    }
}
