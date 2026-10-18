//! Tests of Natural world "regrowth" (build step 10): the model as pure functions (bounded, growing
//! back from nothing, its balance points), the module (its check, the years lived in before the
//! start, the monthly work, droughts, events, state), the SetNature edit, its place in the run
//! order, and what it leaves alone.

use crate::config::Config;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::module::{Module, Slot};
use crate::nature::{crowding, fish_step, game_step, woods_step, FishParams, ForestParams, Hands, Land, NatureParams, Regrowth, Wild, WildlifeParams};
use crate::sim::{Problems, Sim};
use crate::world::{World, WorldParams};

const DT: f64 = 1.0 / 12.0;

fn months(years: f64) -> usize {
    (years * 12.0).round() as usize
}

fn config(economics: &str, seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 32, provinces: 80, seed, land_fraction: 0.4, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.nature = Some(NatureParams::default());
    c
}

fn strict(c: &Config) -> Sim {
    let mut s = Sim::new(c).unwrap();
    s.strict = true;
    s
}

/// The module of a world, made again from its saved state.
fn nature(sim: &Sim) -> Regrowth {
    let m = sim.modules.iter().find(|m| m.manifest().slot == Slot::Nature).unwrap();
    let mut again = Regrowth::new(sim.settings.nature.clone().unwrap());
    again.load_state(&m.save_state()).unwrap();
    again
}

fn share(v: &[u64], p: usize) -> f64 {
    v[p] as f64 / 1e6
}

/// Run up to the start of the next month's first day (it hasn't run yet).
fn to_month_start(sim: &mut Sim) {
    let start = (sim.day + 1..).find(|&d| sim.calendar.is_month_start(d)).unwrap();
    sim.run_days(start - sim.day);
}

// --- The model ---

#[test]
fn crowding_is_people_a_km2_against_fully_crowded_up_to_1() {
    assert_eq!(crowding(200.0, 100.0, 4.0), 0.5);
    assert_eq!(crowding(1e9, 100.0, 4.0), 1.0);
    assert_eq!(crowding(5.0, 0.0, 4.0), 0.0, "no land, no crowding");
    assert_eq!(crowding(0.0, 100.0, 4.0), 0.0);
}

#[test]
fn every_step_stays_between_nothing_and_untouched_at_every_end_of_its_settings() {
    let ends: [f64; 6] = [0.0, 1e-9, 0.5, 1.0, 5.0, 1e4];
    let states = [0.0, 1e-12, 0.3, 0.999_999, 1.0];
    for &a in &ends {
        for &b in &ends {
            let f = ForestParams { most_cleared: a.min(1.0), clearing: b.min(1.0), regrow_years: a.max(1.0), felled_km2: b };
            let fish = FishParams { breeding: a, inflow: b.min(1.0), catch: a, fishers_per_km: b.max(0.01), river_share: a };
            let game = WildlifeParams { breeding: a, inflow: b.min(1.0), hunting: b };
            for &x in &states {
                for &y in &states {
                    for out in [woods_step(&f, x, y, a, y, DT), fish_step(&fish, x, b, DT), game_step(&game, x, y, y, x, DT), fish_step(&fish, x, 1e9, 1.0)] {
                        assert!((0.0..=1.0).contains(&out), "{out} from {x}, {y} with {a}, {b}");
                    }
                }
            }
        }
    }
}

#[test]
fn crowded_land_is_cleared_toward_its_aim_and_grows_back_when_people_leave() {
    let f = ForestParams::default();
    // Untouched land, half crowded: cleared toward 40% (0.8 × 0.5), a tenth of the gap a year.
    let mut lost = 0.0;
    for _ in 0..months(1.0) {
        lost = woods_step(&f, lost, 0.4, 0.0, 1.0, DT);
    }
    assert!((lost - 0.04).abs() < 1e-9, "{lost}");
    for _ in 0..months(199.0) {
        lost = woods_step(&f, lost, 0.4, 0.0, 1.0, DT);
    }
    assert!((lost - 0.4).abs() < 1e-6, "{lost}");
    // Everyone gone from fully cleared land: about two-thirds back in 60 years, nearly all in 180.
    let mut lost = 1.0;
    for _ in 0..months(60.0) {
        lost = woods_step(&f, lost, 0.0, 0.0, 1.0, DT);
    }
    assert!((lost - (-1.0f64).exp()).abs() < 1e-6, "{lost}");
    for _ in 0..months(120.0) {
        lost = woods_step(&f, lost, 0.0, 0.0, 1.0, DT);
    }
    assert!(lost < 0.05, "{lost}");
}

#[test]
fn woodcutters_fell_on_top_of_clearing_and_the_land_grows_back() {
    let f = ForestParams::default();
    // A woodcutter a km² fells 1% a year; with 60 years to grow back the cleared share settles
    // where regrowth (cleared beyond the aim ÷ 60 a year) meets the felling (1% of what stands).
    let mut lost = 0.2;
    for _ in 0..months(600.0) {
        lost = woods_step(&f, lost, 0.2, 0.01, 1.0, DT);
    }
    // (l − 0.2) / 60 = 0.01 × (1 − l): l = 0.2 + 0.6 × (1 − l) → l = 0.8 / 1.6.
    assert!((lost - 0.5).abs() < 0.01, "{lost}");
    // Felling stops: it grows back to the aim.
    for _ in 0..months(600.0) {
        lost = woods_step(&f, lost, 0.2, 0.0, 1.0, DT);
    }
    assert!((lost - 0.2).abs() < 1e-3, "{lost}");
}

#[test]
fn fish_settle_near_a_quarter_fully_fished_and_near_half_half_fished_and_never_die_out() {
    let f = FishParams::default();
    let settle = |work: f64, from: f64| (0..months(300.0)).fold(from, |s, _| fish_step(&f, s, work, DT));
    let full = settle(1.0, 1.0);
    let half = settle(0.5, 1.0);
    assert!((full - 0.23).abs() < 0.02, "fully fished: {full}");
    assert!((half - 0.55).abs() < 0.02, "half fished: {half}");
    assert_eq!(settle(0.0, 1.0), 1.0, "unfished waters stay full");
    // The same from below: a stock fished to nothing comes back (the inflow from the open sea).
    assert!((settle(0.5, 0.0) - half).abs() < 1e-6);
    assert!(settle(0.0, 0.0) > 0.99);
    // Fished ten times as hard as fully, it is low but never gone.
    let hard = settle(10.0, 1.0);
    assert!(hard > 0.0 && hard < 0.02, "{hard}");
    // With no inflow and no breeding, nothing comes back.
    let none = FishParams { breeding: 0.0, inflow: 0.0, ..f };
    assert_eq!(fish_step(&none, 0.0, 0.0, DT), 0.0);
}

#[test]
fn game_breeds_toward_its_room_falls_back_when_it_shrinks_and_is_hunted_where_people_crowd() {
    let g = WildlifeParams::default();
    let settle = |from: f64, room: f64, crowd: f64| (0..months(300.0)).fold(from, |x, _| game_step(&g, x, room, crowd, 1.0, DT));
    assert!((settle(0.0, 0.7, 0.0) - 0.7).abs() < 1e-3, "grows back from nothing to its room");
    assert!((settle(1.0, 0.3, 0.0) - 0.3).abs() < 1e-3, "dies back to a smaller room");
    let hunted = settle(1.0, 1.0, 1.0);
    assert!(hunted < 0.35 && hunted > 0.2, "hunted hard where crowded: {hunted}");
    assert!(settle(1.0, 1.0, 0.5) > hunted);
    assert_eq!(settle(0.0, 0.0, 0.0), 0.0, "no room, no game");
}

#[test]
fn droughts_slow_regrowth_of_woods_and_game_but_kill_nothing() {
    let f = ForestParams::default();
    let g = WildlifeParams::default();
    let woods = |grow: f64| (0..months(10.0)).fold(1.0, |l, _| woods_step(&f, l, 0.0, 0.0, grow, DT));
    let game = |grow: f64| (0..months(10.0)).fold(0.1, |x, _| game_step(&g, x, 1.0, 0.0, grow, DT));
    assert!(woods(0.5) > woods(1.0), "a half drought grows back slower");
    assert_eq!(woods(0.0), 1.0, "a full drought stops regrowth");
    assert!(game(0.5) < game(1.0));
    assert_eq!(game(0.0), 0.1);
    // Through the module: half the rain missing halves the regrowth speed at "droughts" 1.
    let p = NatureParams::default();
    let land = Land { people: 0.0, area_km2: 100.0, forest_km2: 50.0, coast_km: 0.0, river_km: 0.0, dry: 0.5 };
    let (_, why) = p.month(Wild { woods: 0.5, fish: 0.0, game: 0.5 }, &land, Hands::default(), DT);
    assert_eq!(why.grow, 0.5);
    let calm = NatureParams { drought: 0.0, ..NatureParams::default() };
    assert_eq!(calm.month(Wild::default(), &land, Hands::default(), DT).1.grow, 1.0);
}

#[test]
fn a_month_follows_crowding_the_fishers_and_the_woodcutters() {
    let p = NatureParams::default();
    let land = Land { people: 200.0, area_km2: 100.0, forest_km2: 40.0, coast_km: 10.0, river_km: 3.0, dry: 0.0 };
    let (wild, why) = p.month(Wild::default(), &land, Hands::default(), DT);
    assert_eq!(why.crowding, 0.5);
    assert!((why.aim - 0.4).abs() < 1e-12);
    assert!((why.shore - 12.01).abs() < 1e-9);
    assert!((why.work - 0.25).abs() < 1e-12, "no fishers known: crowding × 0.5");
    assert_eq!(why.felled, 0.0, "no woodcutters known: nobody fells");
    // Open land 60 km² with 60% left uncleared, woodland 40 km² as it stands after the month.
    assert!(wild.woods > 0.0 && wild.woods < 0.01);
    assert!((why.room - (40.0 * (1.0 - wild.woods) + 60.0 * 0.6) / 100.0).abs() < 1e-12);
    // Markets' workers: 48 fishers fish 12 km of shore half (8 a km is full).
    let (_, why) = p.month(Wild::default(), &land, Hands { woodcutters: Some(100.0), fishers: Some(48.04) }, DT);
    assert!((why.work - 0.5).abs() < 1e-12);
    assert!((why.felled - 100.0 * 0.01 / 40.0).abs() < 1e-12);
    // A province with no woodland or shore keeps those untouched.
    let bare = Land { forest_km2: 0.0, coast_km: 0.0, river_km: 0.0, ..land };
    let (wild, _) = p.month(Wild::default(), &bare, Hands { woodcutters: Some(100.0), fishers: Some(100.0) }, DT);
    assert_eq!((wild.woods, wild.fish), (0.0, 0.0));
    // A part switched off leaves its measure as it is.
    let off = NatureParams { forests: None, fish: None, wildlife: None, ..NatureParams::default() };
    let was = Wild { woods: 0.3, fish: 0.4, game: 0.5 };
    assert_eq!(off.month(was, &land, Hands::default(), DT).0, was);
}

#[test]
fn a_drought_slows_the_woods_growing_back_in_the_world() {
    // Woods cut down by the player in a forested province grow back slower where a drought
    // (Weather's poke) takes 70% of the rain, and nobody dies of it.
    let mut c = config("food", 11);
    c.modules.without_realms();
    c.modules.nature = Some(NatureParams::default());
    let mut sim = strict(&c);
    sim.run_days(5);
    let p = &sim.world.provinces;
    let wooded: Vec<u32> = (0..p.count as u32).filter(|&i| p.forest_km2[i as usize] > 0.0).collect();
    let heart = *wooded.iter().max_by(|&&a, &&b| p.forest_km2[a as usize].total_cmp(&p.forest_km2[b as usize])).unwrap();
    let (lon, lat) = crate::grid::dir_to_lonlat(p.center[heart as usize]);
    sim.edit(Edit::SetNature { provinces: wooded.clone(), woods: Some(0.1), fish: None, game: Some(0.1) }).unwrap();
    let mut wet = sim.fork();
    sim.poke(Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 1500.0, shortfall: 700, days: 3 * 364 });
    sim.run_days(2);
    let dry: Vec<usize> = wooded.iter().map(|&i| i as usize).filter(|&i| sim.world.rain_shortfall[i] > 0).collect();
    assert!(dry.contains(&(heart as usize)));
    sim.run_days(2 * 364);
    wet.run_days(2 * 364 + 2);
    for &i in &dry {
        assert!(sim.world.woods_lost[i] > wet.world.woods_lost[i], "{i}: woods grow back slower in the drought");
        assert!(sim.world.game_lost[i] >= wet.world.game_lost[i], "{i}: game too");
    }
    assert_eq!(sim.problems, Problems::default());
}

// --- The module ---

#[test]
fn bad_settings_are_refused_with_a_reason() {
    let c = config("food", 1);
    let refused = |p: NatureParams| {
        let mut c = c.clone();
        c.modules.nature = Some(p);
        Sim::new(&c).err().map(|e| e.0).unwrap_or_default()
    };
    let d = NatureParams::default;
    assert!(refused(NatureParams { low: 0.6, back: 0.5, ..d() }).contains("\"back above\" must be at least \"warned below\""));
    assert!(refused(NatureParams { crowd_full: 0.0, ..d() }).contains("fully crowded"));
    assert!(refused(NatureParams { fishers: "spice".into(), ..d() }).contains("spice, which is not a good"));
    assert!(refused(NatureParams { forests: Some(ForestParams { regrow_years: 0.0, ..Default::default() }), ..d() }).contains("grows back"));
    assert!(refused(NatureParams { fish: Some(FishParams { fishers_per_km: 0.0, ..Default::default() }), ..d() }).contains("fully fished"));
    assert!(refused(NatureParams { wildlife: Some(WildlifeParams { hunting: -1.0, ..Default::default() }), ..d() }).contains("wildlife"));
    assert!(refused(NatureParams { lived_in_years: f64::NAN, ..d() }).contains("lived in"));
    assert_eq!(refused(d()), "");
    // Needs people.
    let mut c = c.clone();
    c.modules.population = None;
    c.modules.economics = None;
    c.modules.without_realms();
    c.modules.nature = Some(d());
    assert!(Sim::new(&c).err().unwrap().0.contains("pop"));
}

#[test]
fn a_file_naming_the_module_but_not_a_part_has_that_part_off() {
    let p: NatureParams = toml::from_str("crowd_full = 3.0\n[fish]\ncatch = 0.4\n").unwrap();
    assert_eq!((p.crowd_full, p.low, p.forests.is_none(), p.wildlife.is_none()), (3.0, 0.2, true, true));
    assert_eq!(p.fish.unwrap(), FishParams { catch: 0.4, ..FishParams::default() });
    // The module's defaults have every part on.
    let d = NatureParams::default();
    assert!(d.forests.is_some() && d.fish.is_some() && d.wildlife.is_some());
    // Off by default for new worlds while it is built.
    assert!(crate::config::ModuleConfig::default().nature.is_none());
}

#[test]
fn the_world_starts_lived_in_its_crowded_land_cleared_and_fished() {
    for economics in ["food", "markets"] {
        // Without armies, whose guards leave the pops before the first day: the people the years
        // lived in were worked out with are the people now.
        let mut c = config(economics, 3);
        c.modules.without_realms();
        c.modules.nature = Some(NatureParams::default());
        let sim = strict(&c);
        let w = &sim.world;
        let p = NatureParams::default();
        let people = w.province_population();
        let f = ForestParams::default();
        let mut checked = 0;
        for i in 0..w.provinces.count {
            let area = w.provinces.area_km2[i];
            if area <= 0.0 {
                continue;
            }
            let c = crowding(people[i] as f64, area, p.crowd_full);
            if w.provinces.forest_km2[i] > 0.0 {
                // 200 years of clearing a tenth of the gap a year: at the aim.
                assert!((share(&w.woods_lost, i) - f.most_cleared * c).abs() < 1e-5, "{economics} {i}");
                checked += 1;
            } else {
                assert_eq!(w.woods_lost[i], 0);
            }
            let shore = w.provinces.coast_km[i] + 0.67 * w.provinces.river_km[i];
            if shore > 0.0 {
                // Fished at crowding × 0.5 for 200 years: settled where the stock bears it.
                // (Twelve even months here; the calendar's months differ in length, which moves
                // where it settles a little.)
                let settled = (0..months(400.0)).fold(1.0, |s, _| fish_step(&FishParams::default(), s, c * p.crowd_effort, DT));
                let left = 1.0 - share(&w.fish_lost, i);
                assert!((left - settled).abs() < 1e-3, "{economics} {i}: {left} against {settled} at crowding {c}");
                let pr = &w.provinces;
                let land = Land { people: people[i] as f64, area_km2: area, forest_km2: pr.forest_km2[i], coast_km: pr.coast_km[i], river_km: pr.river_km[i], dry: 0.0 };
                assert!((left - (1.0 - p.lived_in(&land, &sim.calendar).fish)).abs() <= 1e-6);
            } else {
                assert_eq!(w.fish_lost[i], 0);
            }
        }
        assert!(checked > 10, "{checked}");
        assert!(w.woods_lost.iter().any(|&x| x > 0) && w.game_lost.iter().any(|&x| x > 0) && w.fish_lost.iter().any(|&x| x > 0));
        // Provinces already low at the start are told so, without an event.
        let module = nature(&sim);
        let told = module.told();
        for i in 0..w.provinces.count {
            let low = w.provinces.forest_km2[i] > 0.0 && 1.0 - share(&w.woods_lost, i) < p.low;
            assert_eq!(told[i] & 1 != 0, low, "{i}");
        }
    }
    // Not lived in: untouched.
    let mut c = config("food", 3);
    c.modules.nature.as_mut().unwrap().lived_in_years = 0.0;
    let w = Sim::new(&c).unwrap().world;
    assert!(w.woods_lost.iter().chain(&w.fish_lost).chain(&w.game_lost).all(|&x| x == 0));
}

#[test]
fn each_month_the_wild_moves_and_nothing_breaks() {
    for economics in ["food", "markets"] {
        let mut c = config(economics, 5);
        // Started untouched, so the first years move the measures a lot.
        c.modules.nature.as_mut().unwrap().lived_in_years = 0.0;
        let mut sim = strict(&c);
        sim.run_days(1);
        let first = sim.world.woods_lost.clone();
        assert!(first.iter().any(|&x| x > 0), "{economics}: the first month start clears");
        sim.run_days(5 * 365);
        assert!(sim.world.woods_lost.iter().zip(&first).any(|(a, b)| a > b));
        assert!(sim.world.fish_lost.iter().any(|&x| x > 0) && sim.world.game_lost.iter().any(|&x| x > 0));
        assert_eq!(sim.problems, Problems::default(), "{economics}");
        // Only on month starts.
        to_month_start(&mut sim);
        sim.run_days(1);
        let then = (sim.world.woods_lost.clone(), sim.world.fish_lost.clone());
        sim.run_days(5);
        assert_eq!((sim.world.woods_lost.clone(), sim.world.fish_lost.clone()), then);
    }
}

#[test]
fn under_markets_the_fishing_is_markets_fishers_as_the_day_began() {
    let mut sim = strict(&config("markets", 2));
    sim.run_days(40);
    to_month_start(&mut sim);
    let w = &sim.world;
    let g = &w.goods;
    let fish = g.column("fish").unwrap();
    // A coastal province with fishers, traced: its note names them (as the day began: before
    // the month's work moves them).
    let p = (0..w.provinces.count).find(|&i| w.provinces.coast_km[i] > 0.0 && w.workers[g.index(i, fish)] > 10).unwrap();
    let fishers = w.workers[g.index(p, fish)];
    let day = sim.day;
    sim.run_days(1);
    let t = sim.trace(day, p as u32).unwrap();
    let notes: Vec<String> = t.steps.iter().filter_map(|s| if let crate::sim::Traced::Note(n) = &s.what { Some(n.clone()) } else { None }).collect();
    assert!(notes.iter().any(|n| n.contains(&format!("{fishers} fishers"))), "{notes:?}");
    assert!(notes.iter().any(|n| n.starts_with("woodland") || n.starts_with("game")), "{notes:?}");
}

#[test]
fn nature_runs_before_markets_and_last_under_economics_v0() {
    let order = |c: &Config| -> Vec<Slot> {
        let s = Sim::new(c).unwrap();
        s.plan.runtime.iter().map(|&i| s.modules[i].manifest().slot).collect()
    };
    let markets = order(&config("markets", 1));
    let at = |o: &[Slot], s: Slot| o.iter().position(|&x| x == s).unwrap();
    assert!(at(&markets, Slot::Nature) < at(&markets, Slot::Economics), "{markets:?}");
    assert!(at(&markets, Slot::Population) < at(&markets, Slot::Nature));
    let food = order(&config("food", 1));
    assert_eq!(*food.last().unwrap(), Slot::Nature, "{food:?}");
    // The others keep their order with it on.
    let mut c = config("markets", 1);
    c.modules.nature = None;
    let without = order(&c);
    let with: Vec<Slot> = markets.iter().copied().filter(|&s| s != Slot::Nature).collect();
    assert_eq!(with, without);
    let mut c = config("food", 1);
    c.modules.nature = None;
    assert_eq!(food[..food.len() - 1], order(&c)[..]);
}

#[test]
fn under_economics_v0_it_changes_nothing_else() {
    // Under v0 nothing reads the wild: every other field is as without the module.
    let c = config("food", 4);
    let mut with = strict(&c);
    let mut c = c.clone();
    c.modules.nature = None;
    let mut without = strict(&c);
    with.run_days(365 * 5);
    without.run_days(365 * 5);
    let mut compared = 0;
    for id in crate::fields::FieldId::all() {
        if ["province.woods_lost", "province.fish_lost", "province.game_lost"].contains(&id.def().name) {
            continue;
        }
        let print = |w: &World| {
            let mut h = crate::world::Fingerprint::default();
            w.field(id).fingerprint(&mut h);
            h.0
        };
        assert_eq!(print(&with.world), print(&without.world), "{}", id.def().name);
        compared += 1;
    }
    assert!(compared > 50);
    assert_eq!(with.history, without.history);
    assert!(with.world.woods_lost.iter().any(|&x| x > 0));
}

#[test]
fn warnings_are_told_once_and_taken_back_past_the_gap() {
    let mut c = config("food", 6);
    c.modules.nature.as_mut().unwrap().lived_in_years = 0.0;
    let mut sim = strict(&c);
    sim.run_days(3);
    let w = &sim.world;
    let p = (0..w.provinces.count).find(|&i| w.provinces.forest_km2[i] > 0.0 && w.provinces.coast_km[i] > 0.0).unwrap() as u32;
    let events = |sim: &Sim, k: EventKind| sim.event_log.entries.iter().filter(|e| e.event.kind() == k && e.event.province() == Some(p)).count();
    // Cleared and fished out by the player: told at the next month start, once.
    sim.edit(Edit::SetNature { provinces: vec![p], woods: Some(0.1), fish: Some(0.05), game: None }).unwrap();
    to_month_start(&mut sim);
    sim.run_days(2);
    assert_eq!((events(&sim, EventKind::WoodsCleared), events(&sim, EventKind::FisheryCollapsed)), (1, 1));
    let left = sim.event_log.entries.iter().find_map(|e| if let Event::WoodsCleared { province, left } = e.event { (province == p).then_some(left) } else { None }).unwrap();
    assert!((90_000..110_000).contains(&left), "{left}");
    // Between the lines: nothing more.
    sim.edit(Edit::SetNature { provinces: vec![p], woods: Some(0.3), fish: Some(0.3), game: None }).unwrap();
    to_month_start(&mut sim);
    sim.run_days(2);
    assert_eq!((events(&sim, EventKind::WoodsCleared), events(&sim, EventKind::WoodsRegrown)), (1, 0));
    // Above "back above": told as back, once.
    sim.edit(Edit::SetNature { provinces: vec![p], woods: Some(0.9), fish: Some(0.9), game: None }).unwrap();
    to_month_start(&mut sim);
    sim.run_days(2);
    to_month_start(&mut sim);
    sim.run_days(2);
    assert_eq!((events(&sim, EventKind::WoodsRegrown), events(&sim, EventKind::FisheryRecovered)), (1, 1));
    assert_eq!(nature(&sim).told()[p as usize], 0);
    assert_eq!(sim.problems, Problems::default());
}

#[test]
fn its_state_is_saved_loaded_and_kept_through_going_back() {
    let mut sim = strict(&config("markets", 7));
    sim.run_days(400);
    let p = (0..sim.world.provinces.count).find(|&i| sim.world.provinces.forest_km2[i] > 0.0).unwrap() as u32;
    sim.edit(Edit::SetNature { provinces: vec![p], woods: Some(0.05), fish: None, game: Some(0.5) }).unwrap();
    sim.run_days(100);
    assert!(nature(&sim).told()[p as usize] & 1 != 0);
    let mut again = Regrowth::new(NatureParams::default());
    again.load_state(&nature(&sim).save_state()).unwrap();
    assert_eq!(again.told(), nature(&sim).told());
    assert!(again.load_state(&[200, 200, 200]).is_err());
    let mut loaded = Sim::load(&sim.save()).unwrap();
    let mut back = sim.fork();
    back.goto(200).unwrap();
    back.goto(sim.day).unwrap();
    sim.run_days(500);
    loaded.run_days(500);
    back.run_days(500);
    assert_eq!(loaded.fingerprint(), sim.fingerprint());
    assert_eq!(back.fingerprint(), sim.fingerprint());
}

/// How long a month start of the natural world takes on the Large world, by the clock (rough
/// while other work runs): `cargo test --release -p sim --lib nature_month_start_time -- --ignored --nocapture`.
#[test]
#[ignore]
fn nature_month_start_time() {
    use crate::module::{Ctx, Outbox};
    for economics in ["food", "markets"] {
        let mut c = Config::new(WorldParams::preset(crate::world::Size::Large, 21));
        c.modules.set_choice(Slot::Economics, economics).unwrap();
        c.modules.nature = Some(NatureParams::default());
        let made = std::time::Instant::now();
        let mut s = Sim::new(&c).unwrap();
        println!("{economics}: made in {:.2} s", made.elapsed().as_secs_f64());
        s.run_days(3 * 364);
        to_month_start(&mut s);
        // The month start's copy of the fields read as the day began.
        s.world.earlier = std::sync::Arc::new(crate::world::Earlier::copy(&s.world, s.day, &[crate::fields::known("province.workers")]));
        let time = |s: &Sim, params: NatureParams, what: &str| {
            let state = nature(s).save_state();
            let goods = s.world.goods.clone();
            let mut times = Vec::new();
            for _ in 0..30 {
                let mut w = s.world.clone();
                let mut m = Regrowth::new(params.clone());
                m.load_state(&state).unwrap();
                let mut out = Outbox::default();
                let mut cx = Ctx { day: s.day, seed: w.params.seed, calendar: &s.calendar, goods: &goods, slot: Slot::Nature, out: &mut out, trace: None };
                let t = std::time::Instant::now();
                m.tick(&mut w, &mut cx);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(f64::total_cmp);
            println!("  {what}: {:.3} ms (fastest {:.3})", times[times.len() / 2], times[0]);
        };
        println!("  {} provinces, {} pop rows", s.world.provinces.count, s.world.pops.size.len());
        time(&s, NatureParams::default(), "a month's start");
        time(&s, NatureParams { forests: None, fish: None, wildlife: None, ..NatureParams::default() }, "  of which bookkeeping alone");
    }
}

// --- The edit ---

#[test]
fn set_nature_sets_the_shares_left_and_refuses_what_doesnt_fit() {
    let mut sim = strict(&config("food", 8));
    sim.run_days(10);
    let w = &sim.world;
    let p = (0..w.provinces.count).find(|&i| w.provinces.cells[i] > 0).unwrap() as u32;
    let (woods, fish) = (w.woods_lost[p as usize], w.fish_lost[p as usize]);
    sim.edit(Edit::SetNature { provinces: vec![p], woods: None, fish: None, game: Some(0.25) }).unwrap();
    assert_eq!(sim.world.game_lost[p as usize], 750_000, "applied at once");
    assert_eq!((sim.world.woods_lost[p as usize], sim.world.fish_lost[p as usize]), (woods, fish), "the rest as it was");
    sim.edit(Edit::SetNature { provinces: vec![p], woods: Some(1.0), fish: Some(0.0), game: None }).unwrap();
    assert_eq!((sim.world.woods_lost[p as usize], sim.world.fish_lost[p as usize]), (0, 1_000_000));
    let refused = |sim: &mut Sim, e: Edit| sim.edit(e).unwrap_err();
    assert!(refused(&mut sim, Edit::SetNature { provinces: vec![p], woods: Some(1.5), fish: None, game: None }).contains("from 0 to 1"));
    assert!(refused(&mut sim, Edit::SetNature { provinces: vec![], woods: Some(0.5), fish: None, game: None }).contains("no provinces"));
    assert!(refused(&mut sim, Edit::SetNature { provinces: vec![99_999], woods: Some(0.5), fish: None, game: None }).contains("no province"));
    // A part switched off keeps nothing to set.
    let mut c = config("food", 8);
    c.modules.nature.as_mut().unwrap().fish = None;
    let mut s = strict(&c);
    assert!(refused(&mut s, Edit::SetNature { provinces: vec![p], woods: None, fish: Some(0.5), game: None }).contains("doesn't keep its fish"));
    // The module off: nothing keeps the wild.
    c.modules.nature = None;
    let mut s = strict(&c);
    assert!(refused(&mut s, Edit::SetNature { provinces: vec![p], woods: Some(0.5), fish: None, game: None }).contains("no module keeps"));
}

#[test]
fn the_edit_is_replayed_undone_and_kept_in_scenarios() {
    let mut sim = strict(&config("markets", 9));
    sim.run_days(100);
    let p = (0..sim.world.provinces.count).find(|&i| sim.world.provinces.coast_km[i] > 0.0).unwrap() as u32;
    sim.edit(Edit::SetNature { provinces: vec![p], woods: Some(0.2), fish: Some(0.1), game: Some(0.3) }).unwrap();
    sim.run_days(300);
    let end = sim.fingerprint();
    // A scenario file replays it on its day.
    let t = crate::branch::Timelines::new(sim);
    let again = crate::branch::Timelines::from_scenario_text(&t.scenario_text()).unwrap();
    assert_eq!(again.sim().fingerprint(), end);
    // Taken back: the world that never had it.
    let mut s = t.list.into_iter().next().unwrap().sim;
    let at = s.commands.iter().position(|c| matches!(&c.kind, crate::sim::CommandKind::Edit { edit: Edit::SetNature { .. } })).unwrap();
    s.withdraw(at).unwrap();
    let mut never = strict(&config("markets", 9));
    never.run_days(s.day);
    assert_eq!(s.fingerprint(), never.fingerprint());
}

#[test]
fn a_new_province_starts_untouched_and_the_rest_keep_their_shares() {
    let mut sim = strict(&config("food", 10));
    sim.run_days(40);
    let w = &sim.world;
    let n = w.provinces.count;
    // The cells of a big province with woodland make a new province.
    let p = (0..n).filter(|&i| w.provinces.forest_km2[i] > 0.0 && w.woods_lost[i] > 0).max_by_key(|&i| w.provinces.cells[i]).unwrap();
    let cells: Vec<u32> = (0..w.provinces.of_cell.len() as u32).filter(|&c| w.provinces.of_cell[c as usize] == p as u32).take(w.provinces.cells[p] as usize / 2).collect();
    let before = w.woods_lost.clone();
    sim.edit(Edit::Split { cells }).unwrap();
    let w = &sim.world;
    assert_eq!(w.provinces.count, n + 1);
    assert_eq!((w.woods_lost[n], w.fish_lost[n], w.game_lost[n]), (0, 0, 0));
    assert_eq!(w.woods_lost[..n], before[..]);
    assert_eq!(nature(&sim).told().len(), n + 1);
    sim.run_days(400);
    assert_eq!(sim.problems, Problems::default());
}
