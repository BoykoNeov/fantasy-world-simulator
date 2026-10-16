//! Troops (step 7, Warfare "troops"): peace-time forces, raising toward the war-time share and going
//! home at the peace, soldiers sharing food and deserting, kinds by the land, goods and bounties
//! under markets, occupied and rising provinces, land changing hands, and soldiers in flight through
//! saves, loads and going back.

use crate::config::Config;
use crate::diplomacy::WarParams;
use crate::edit::Edit;
use crate::events::Event;
use crate::fields::{quantity_total, Quantity};
use crate::grid::NONE;
use crate::module::Slot;
use crate::pops::PopParams;
use crate::realms::RealmParams;
use crate::requests::Cause;
use crate::sim::{Problems, Sim};
use crate::taxonomy::Taxonomy;
use crate::warfare::TroopParams;
use crate::world::{World, WorldParams};

/// Realms (no unrest, so wars come only from pokes), wars and troops; markets if `markets`.
fn config(seed: u64, markets: bool) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.realms = Some(RealmParams { provinces_per_realm: 8.0, unrest: None, ..RealmParams::default() });
    c.modules.diplomacy = Some(WarParams::default());
    c.modules.warfare = Some(TroopParams::default());
    if markets {
        c.modules.set_choice(Slot::Economics, "markets").unwrap();
    }
    c
}

fn world(seed: u64, markets: bool) -> Sim {
    let mut sim = Sim::new(&config(seed, markets)).unwrap();
    sim.strict = true;
    sim
}

/// The same, with generals who never attack: raising alone, without the fighting.
fn quiet(seed: u64, markets: bool) -> Sim {
    let mut c = config(seed, markets);
    c.modules.warfare.as_mut().unwrap().outmatch = 1e9;
    let mut sim = Sim::new(&c).unwrap();
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

/// Soldiers from province `p` serving realm `r`.
fn soldiers(w: &World, p: u32, r: u32) -> u64 {
    (0..w.armies.count()).filter(|&a| w.armies.home[a] == p && w.armies.realm[a] == r).map(|a| w.armies.size[a]).sum()
}

/// Realm `r`'s soldiers as a share of its provinces' people, soldiers included.
fn under_arms(w: &World, r: u32) -> f64 {
    let people = w.province_population();
    let land = land_of(w, r);
    let s: u64 = land.iter().map(|&p| soldiers(w, p, r)).sum();
    let all: u64 = land.iter().map(|&p| people[p as usize]).sum::<u64>() + s;
    s as f64 / all as f64
}

fn tree() -> Taxonomy {
    Taxonomy::named(&TroopParams::default().tree, "troop tree").unwrap()
}

/// Run to the start of the next month, then `then` days more.
fn next_month(sim: &mut Sim, then: u64) {
    sim.run_days(1);
    while !sim.calendar.is_month_start(sim.day) {
        sim.run_days(1);
    }
    sim.run_days(then);
}

#[test]
fn troops_are_off_by_default_and_need_realms() {
    assert!(Config::new(WorldParams::preset(crate::world::Size::Small, 1)).modules.warfare.is_none());
    let mut c = config(1, false);
    c.modules.realms = None;
    c.modules.diplomacy = None;
    let e = Sim::new(&c).err().expect("troops without realms are refused").0;
    assert!(e.contains("requires realm.capital"), "{e}");
    // Under Economics v0 they ask for food alone; under markets for goods and coins too.
    let asks = |markets| Sim::new(&config(1, markets)).unwrap().manifests().iter().find(|m| m.slot == Slot::Warfare).unwrap().asks.to_vec();
    assert_eq!(asks(false), ["food"]);
    assert_eq!(asks(true), ["food", "tools", "timber", "money"]);
    // Bad parameters are refused when the world is made.
    let refused = |f: &dyn Fn(&mut TroopParams)| {
        let mut c = config(1, true);
        f(c.modules.warfare.as_mut().unwrap());
        Sim::new(&c).err().expect("refused").0
    };
    assert!(refused(&|p| p.fallback = "ships".into()).contains("must need no goods"));
    assert!(refused(&|p| { p.kinds.remove("cavalry"); }).contains("cavalry has no table"));
    assert!(refused(&|p| p.war_share = 0.001).contains("war-time one no smaller"));
    assert!(refused(&|p| p.kinds.get_mut("ships").unwrap().good = Some("spice".into())).contains("spice is not a good"));
}

#[test]
fn in_peace_every_province_keeps_half_a_percent_under_arms_at_home() {
    let mut sim = world(1, false);
    let leaves = tree().leaves_under(0);
    for months in [0, 6, 24] {
        for _ in 0..months {
            next_month(&mut sim, 0);
        }
        // Raised on a month's first day, booked in on its second.
        sim.run_days(2);
        let w = &sim.world;
        let people = w.province_population();
        let mut shares = Vec::new();
        for p in 0..w.provinces.count as u32 {
            let r = w.owner[p as usize];
            let s = if r == NONE { 0 } else { soldiers(w, p, r) };
            let total = people[p as usize] + s;
            if r == NONE || total < 20_000 {
                continue;
            }
            // Below a quarter over what is wanted nobody goes home; a hungry province's soldiers
            // desert, so it may sit a little under until next month's raising.
            let share = s as f64 / total as f64;
            assert!((0.003..0.0063).contains(&share), "month {months}, province {p}: {s} of {total}");
            shares.push(share);
        }
        shares.sort_by(f64::total_cmp);
        let middle = shares[shares.len() / 2];
        assert!((0.0045..0.0053).contains(&middle), "month {months}: the middle province has {middle}");
        for a in 0..w.armies.count() {
            assert_eq!(w.armies.realm[a], w.owner[w.armies.home[a] as usize], "company {a} serves its home's realm");
            assert_eq!(w.armies.province[a], w.armies.home[a], "company {a} stands at home");
            assert!(leaves.contains(&w.armies.kind[a]), "company {a} is of a kind");
        }
    }
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn a_war_raises_both_realms_to_five_percent_over_three_months_and_the_peace_sends_them_home() {
    let mut sim = quiet(2, false);
    let (a, b) = neighbours(&sim.world, 3);
    sim.run_days(2);
    let calm = (0..sim.world.realms.count() as u32).find(|&r| r != a && r != b && land_of(&sim.world, r).len() >= 3).unwrap();
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    let mut shares = Vec::new();
    for _ in 0..4 {
        next_month(&mut sim, 2);
        shares.push((under_arms(&sim.world, a), under_arms(&sim.world, b), under_arms(&sim.world, calm)));
    }
    // A third of the war-time number a month, on top of the peace-time half percent.
    let (first, third) = (shares[0], shares[2]);
    for s in [first.0, first.1] {
        assert!((0.015..0.03).contains(&s), "{shares:?}");
    }
    for s in [third.0, third.1, shares[3].0, shares[3].1] {
        assert!((0.045..0.053).contains(&s), "{shares:?}");
    }
    assert!(shares.iter().all(|s| s.2 < 0.0065), "a realm at peace stays at its peace-time force: {shares:?}");

    sim.poke(Event::ForcePeace { a, b, winner: NONE });
    next_month(&mut sim, 2);
    let home: u64 = sim.event_log.entries.iter().filter_map(|p| match p.event {
        Event::Arrived { people, cause: Cause::Returned, .. } => Some(people),
        _ => None,
    }).sum();
    assert!(home > 0, "the extra soldiers went home");
    for r in [a, b] {
        let s = under_arms(&sim.world, r);
        assert!((0.0045..0.0063).contains(&s), "realm {r} back to its peace-time force: {s}");
    }
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn hungry_soldiers_share_food_with_civilians_and_half_the_unfed_desert() {
    // More people than the land feeds, from the start: hunger everywhere.
    let mut c = config(3, false);
    c.modules.population = Some(PopParams { initial_fill: 1.4, ..PopParams::default() });
    let mut sim = Sim::new(&c).unwrap();
    sim.strict = true;
    // A month in which many companies go hungry: run its first day, when everyone asks for food,
    // shared out at the day's end.
    let hungry_places = |sim: &Sim| sim.applied_today.iter().filter(|(r, g)| r.requester == Slot::Warfare && r.cause == Cause::Eating && *g < r.take).count();
    for _ in 0..24 {
        next_month(&mut sim, 0);
        sim.run_days(1);
        if hungry_places(&sim) >= 10 {
            break;
        }
    }
    let w = &sim.world;
    let eating: Vec<_> = sim.applied_today.iter().filter(|(r, _)| r.cause == Cause::Eating).copied().collect();
    let mut hungry = 0;
    for p in 0..w.provinces.count as u32 {
        // Each share is what was granted over what was asked, so whole rations make it differ by
        // up to a ration in each request.
        let fed = |slot| -> Vec<(f64, f64)> {
            eating.iter().filter(|(r, _)| r.entity == p && r.requester == slot && r.take > 0).map(|(r, g)| (*g as f64 / r.take as f64, 1.0 / r.take as f64)).collect()
        };
        let (people, troops) = (fed(Slot::Population), fed(Slot::Warfare));
        for ((x, dx), (y, dy)) in people.iter().flat_map(|x| troops.iter().map(move |y| (x, y))) {
            assert!((x - y).abs() <= dx + dy + 1e-12, "province {p}: civilians got {x:.4} of what they asked, soldiers {y:.4}");
        }
        hungry += troops.iter().filter(|f| f.0 < 0.99).count();
    }
    assert!(hungry >= 10, "only {hungry} provinces with hungry soldiers");

    // The next day the grants come: half the unfed desert, up to rounding in each company.
    // A company asks for a ration a soldier a day of the month: the unfed are the rations short over
    // the days.
    let days = sim.calendar.def.months[sim.calendar.date(sim.day - 1).month].days as f64;
    let unfed: f64 = eating.iter().filter(|(r, _)| r.requester == Slot::Warfare).map(|(r, g)| (r.take - g) as f64 / days).sum();
    // The companies of a province ask together; each rounds its deserters on its own.
    let short: Vec<u32> = eating.iter().filter(|(r, g)| r.requester == Slot::Warfare && g < &r.take).map(|(r, _)| r.entity).collect();
    let w = &sim.world;
    let companies = (0..w.armies.count()).filter(|&a| w.armies.size[a] > 0 && short.contains(&w.armies.province[a])).count() as f64;
    let before = sim.year_flows.get(Quantity::People, Cause::Deserted);
    sim.run_days(1);
    let deserted = (before - sim.year_flows.get(Quantity::People, Cause::Deserted)) as f64;
    assert!((deserted - 0.5 * unfed).abs() <= companies, "{deserted} deserted of {unfed} unfed in {companies} companies");
    // They arrive home the day after.
    sim.run_days(1);
    assert_eq!(sim.year_flows.get(Quantity::People, Cause::Deserted), 0, "every deserter is home");
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn the_land_decides_the_kinds_under_both_economies() {
    for markets in [false, true] {
        let mut sim = world(4, markets);
        sim.run_days(3);
        let (w, t) = (&sim.world, tree());
        let p = &w.provinces;
        let mut seen = std::collections::BTreeSet::new();
        for a in (0..w.armies.count()).filter(|&a| w.armies.size[a] > 0) {
            let h = w.armies.home[a] as usize;
            let kind = t.name(w.armies.kind[a]);
            let fits = match kind {
                "levies" => true,
                "heavy_infantry" => p.ore_km2[h] > 0.0,
                "cavalry" => p.grazing_capacity[h] > 0,
                "ships" => p.coast_km[h] > 0.0 && p.forest_km2[h] > 0.0,
                other => panic!("no kind {other}"),
            };
            assert!(fits, "markets {markets}: {kind} from province {h}");
            seen.insert(kind);
        }
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), ["cavalry", "heavy_infantry", "levies", "ships"], "markets {markets}");
        // Levies, needing nothing, are raised everywhere; the rest only where the land suits them.
        assert_eq!(sim.problems, Problems::default());
    }
}

#[test]
fn under_markets_soldiers_use_up_goods_and_realms_pay_a_bounty_for_each() {
    let mut sim = world(5, true);
    // Day 0 asks for recruits; day 1 books them in and asks for their goods and bounty; day 2 those
    // come.
    sim.run_days(2);
    let w = &sim.world;
    let recruits = |a: usize| w.armies.size[a];
    let bounties: Vec<_> = sim.applied_today.iter().filter(|(r, _)| r.cause == Cause::Bounty).copied().collect();
    assert!(!bounties.is_empty());
    for (r, _) in &bounties {
        let a = r.tag as usize;
        assert_eq!(r.take, 10 * recruits(a), "company {a}: 10 coins a recruit");
        assert_eq!((r.entity, r.to.unwrap().entity), (w.armies.realm[a], w.armies.home[a]), "from its realm's treasury to its home");
    }
    let equipped: Vec<_> = sim.applied_today.iter().filter(|(r, _)| r.cause == Cause::Equipped).copied().collect();
    let names: std::collections::BTreeSet<&str> = equipped.iter().map(|(r, _)| w.goods.tree.name(r.good.unwrap())).collect();
    assert_eq!(names.into_iter().collect::<Vec<_>>(), ["timber", "tools"]);
    // The goods are used up, booked by class.
    let used = |class: u8| sim.year_flows.get(Quantity::Goods(class), Cause::Equipped);
    assert!(used(1) < 0 && used(2) < 0, "timber (raw) and tools (manufactured) used: {} {}", used(1), used(2));

    // The same world with no tools anywhere: heavy infantry recruits become levies.
    let mut bare = world(5, true);
    bare.run_days(1);
    for p in 0..bare.world.provinces.count as u32 {
        bare.edit(Edit::SetStock { province: p, good: "tools".into(), amount: 0 }).unwrap();
    }
    bare.run_days(1);
    assert!(bare.applied_today.iter().any(|(r, g)| r.cause == Cause::Equipped && *g < r.take));
    bare.run_days(1);
    let (w, t) = (&bare.world, tree());
    let heavy = t.find("heavy_infantry").unwrap();
    let left: u64 = (0..w.armies.count()).filter(|&a| w.armies.kind[a] == heavy).map(|a| w.armies.size[a]).sum();
    assert_eq!(left, 0, "no heavy infantry without tools");
    // Every soldier raised is still there, as levies.
    let all: u64 = sim.world.armies.size.iter().sum();
    assert_eq!(w.armies.size.iter().sum::<u64>(), all);
    // One company of each kind from each province for each realm, the levies that take the
    // unequipped included, after months of raising.
    for s in [&mut sim, &mut bare] {
        s.run_days(100);
        let w = &s.world;
        let mut seen = std::collections::BTreeSet::new();
        for a in 0..w.armies.count() {
            assert!(seen.insert((w.armies.realm[a], w.armies.home[a], w.armies.kind[a])), "company {a} is a second of its kind");
        }
    }
    assert_eq!((sim.problems, bare.problems), (Problems::default(), Problems::default()));
}

#[test]
fn occupied_and_rising_provinces_raise_nobody_and_land_changing_hands_sends_its_soldiers_home() {
    let mut sim = quiet(6, false);
    let (a, b) = neighbours(&sim.world, 3);
    sim.run_days(2);
    let land_b = land_of(&sim.world, b);
    let people = sim.world.province_population();
    let mut by_size = land_b.clone();
    by_size.sort_by_key(|&p| std::cmp::Reverse(people[p as usize]));
    let (taken, rising) = (by_size[0], by_size[1]);
    let before = (soldiers(&sim.world, taken, b), soldiers(&sim.world, rising, b));
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    sim.run_days(2);
    sim.poke(Event::Occupied { province: taken, by: a });
    // Unrest is off in these worlds, so an uprising set by hand stays.
    sim.world.uprising[rising as usize] = rising;
    for _ in 0..3 {
        next_month(&mut sim, 2);
    }
    let w = &sim.world;
    assert!(soldiers(w, taken, b) <= before.0 && soldiers(w, rising, b) <= before.1, "neither raised: {before:?}");
    assert!(under_arms(w, b) > 0.03, "the rest of the realm did");

    // The peace hands the occupied province to a: b's soldiers there go home at the next month's
    // start, and a raises its own.
    sim.poke(Event::ForcePeace { a, b, winner: a });
    sim.run_days(3);
    assert_eq!(sim.world.owner[taken as usize], a);
    next_month(&mut sim, 2);
    let w = &sim.world;
    assert_eq!(soldiers(w, taken, b), 0);
    assert!(soldiers(w, taken, a) > 0);
    let went = sim.event_log.entries.iter().any(|p| matches!(p.event, Event::Arrived { province, cause: Cause::Returned, .. } if province == taken));
    assert!(went);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn soldiers_in_flight_survive_saving_loading_and_going_back_and_every_year_balances() {
    let fresh = || {
        let mut sim = world(7, true);
        let (a, b) = neighbours(&sim.world, 2);
        sim.schedule(5, Event::DeclareWar { attacker: a, defender: b }).unwrap();
        sim
    };
    let mut sim = fresh();
    let year = sim.calendar.days_per_year() as u64;
    // Recruits granted last night; goods and bounties granted last night; a new year.
    let stops = [1, 2, 31, 32, year, year + 1];
    let mut prints = Vec::new();
    for &d in &stops {
        sim.run_days(d - sim.day);
        let mut loaded = Sim::load(&sim.save()).unwrap();
        loaded.strict = true;
        let mut twin = fresh();
        twin.run_days(d + 40);
        loaded.run_days(40);
        assert_eq!(loaded.fingerprint(), twin.fingerprint(), "saved on day {d}");
        prints.push(sim.fingerprint());
    }
    let today = sim.day;
    for (&d, &print) in stops.iter().zip(&prints).rev() {
        sim.goto(d).unwrap();
        assert_eq!(sim.fingerprint(), print, "back to day {d}");
    }
    sim.goto(today).unwrap();

    // Two years of war and peace-time raising, desertion and soldiers' deaths: each balances.
    let mut start = quantity_total(&Sim::new(&config(7, true)).unwrap().world, Quantity::People);
    sim.run_to_year(sim.calendar.date(0).year + 3);
    sim.run_days(1);
    for y in &sim.history {
        let l = &y.ledger;
        let end = start as i64 + l.births as i64 - l.deaths as i64 + l.people_clamped + l.people_edited + l.people_on_the_way;
        assert_eq!(y.population as i64, end, "year {}", y.year);
        start = y.population;
    }
    assert_eq!(sim.problems, Problems::default());
}
