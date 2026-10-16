//! Armies fighting: one war watched closely, or a long run with and without armies.
//!
//! `one` takes a world (realms, wars and troops; no unrest, so the war is the only one), declares
//! war between the two largest realms that border each other, and prints, month by month, each
//! side's soldiers, the battles, the provinces taken and freed, and the soldiers killed.
//!
//! `long` runs a story-1 world (realms with unrest, rulers, wars) for the years given, with armies
//! and without, and prints, each decade, the realms alive, the rebellions put down and won, the
//! provinces that changed hands at a peace, the battles, the soldiers killed, the people starved,
//! and the share under arms.
//!
//! `time` takes a large world (realms without unrest, rulers, wars) after a year, and times days with
//! armies and without: quiet days (no month's start) at peace, then the days of a year of war
//! between two rivals. Run it with `RAYON_NUM_THREADS=1` to time one worker thread.
//!
//! `time-treaties` times what Diplomacy "treaties" adds on the days it works (a month's first), on
//! two large worlds alike but for it, after a year.
//!
//! cargo run --release -p sim --example war_check -- [food|markets] [seed] [one|long|time|time-treaties] [years]

use sim::config::Config;
use sim::events::Event;
use sim::grid::NONE;
use sim::module::Slot;
use sim::realms::RealmParams;
use sim::sim::Sim;
use sim::world::WorldParams;

fn config(economics: &str, seed: u64, unrest: bool, armies: bool) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    c.modules.realms = Some(if unrest { RealmParams::default() } else { RealmParams { unrest: None, ..RealmParams::default() } });
    c.modules.characters = Some(Default::default());
    c.modules.diplomacy = Some(Default::default());
    if armies {
        c.modules.warfare = Some(Default::default());
    }
    c
}

fn land_of(sim: &Sim, r: u32) -> usize {
    sim.world.owner.iter().filter(|&&o| o == r).count()
}

fn soldiers_of(sim: &Sim, r: u32) -> (u64, u64) {
    let w = &sim.world;
    let (mut guards, mut field) = (0, 0);
    for a in 0..w.armies.count() {
        if w.armies.realm[a] == r {
            if w.armies.role[a] == sim::warfare::GUARD {
                guards += w.armies.size[a];
            } else {
                field += w.armies.size[a];
            }
        }
    }
    (guards, field)
}

fn one(economics: &str, seed: u64, years: i64) {
    let mut sim = Sim::new(&config(economics, seed, false, true)).unwrap();
    sim.strict = true;
    sim.run_days(2);
    let w = &sim.world;
    let mut best = (0usize, NONE, NONE);
    for l in &w.routes.links {
        let (a, b) = (w.owner[l.a as usize], w.owner[l.b as usize]);
        if a != NONE && b != NONE && a != b && l.land_days.is_some() {
            let size = land_of(&sim, a).min(land_of(&sim, b));
            if size > best.0 {
                best = (size, a.min(b), a.max(b));
            }
        }
    }
    let (_, a, b) = best;
    println!("war: realm {a} ({} provinces) against realm {b} ({} provinces)", land_of(&sim, a), land_of(&sim, b));
    sim.poke(Event::DeclareWar { attacker: a, defender: b });
    let end = sim.day + (years as u64) * sim.calendar.days_per_year() as u64;
    let mut seen = 0;
    let mut battles = 0;
    while sim.day < end {
        sim.run_days(1);
        for p in &sim.event_log.entries[seen..] {
            match p.event {
                Event::Battle { province, winner, loser, killed, .. } => {
                    battles += 1;
                    if killed == 0 {
                        println!("  day {}: a battle in {province} ({winner} over {loser}) killed nobody", p.day);
                    }
                }
                Event::Occupied { province, by } => {
                    let what = if sim.world.owner[province as usize] == by { "freed" } else { "took" };
                    println!("  day {}: realm {by} {what} province {province}", p.day)
                }
                Event::Peace { winner, .. } => println!("  day {}: peace, won by {winner}", p.day),
                _ => {}
            }
        }
        seen = sim.event_log.entries.len();
        if sim.calendar.is_month_start(sim.day) {
            let w = &sim.world;
            let occupied = |by: u32| w.occupier.iter().filter(|&&o| o == by).count();
            let groups = (0..w.armies.count()).filter(|&i| w.armies.group[i] != NONE && w.armies.size[i] > 0).count();
            let moving = (0..w.armies.count()).filter(|&i| w.armies.next[i] != NONE && w.armies.size[i] > 0).count();
            println!(
                "day {}: {a} guards/field {:?}, occupies {}; {b} {:?}, occupies {}; companies in groups {groups}, moving {moving}; battles {battles}, killed this year {}",
                sim.day,
                soldiers_of(&sim, a),
                occupied(a),
                soldiers_of(&sim, b),
                occupied(b),
                -sim.year_flows.get(sim::fields::Quantity::People, sim::requests::Cause::Killed)
            );
        }
    }
    println!("problems: {:?}", sim.problems);
}

#[derive(Default, Clone, Copy, Debug)]
struct Decade {
    rebellions: usize,
    put_down: usize,
    rebels_won: usize,
    changed_hands: usize,
    battles: usize,
    /// Battles the side that came into the province won.
    attackers_won: usize,
    /// Days land companies spent crossing the sea.
    afloat: u64,
    /// Food soldiers asked for, and were given.
    asked: u64,
    given: u64,
    /// Famines (a province's month), and those where field companies stood away from home.
    famines: usize,
    famines_with_armies: usize,
    killed: u64,
    starved: u64,
}

fn long(economics: &str, seed: u64, years: i64) {
    for armies in [false, true] {
        let mut sim = Sim::new(&config(economics, seed, true, armies)).unwrap();
        let start = sim.calendar.date(0).year;
        println!("{} armies:", if armies { "with" } else { "without" });
        // The log is pruned each year: events are taken by the day they were sent.
        let mut since = 0;
        let mut d = Decade::default();
        let ships = sim.config.modules.warfare.as_ref().map(|t| {
            let tree = sim::taxonomy::Taxonomy::named(&t.tree, "troop tree").unwrap();
            tree.leaves_under(tree.find(&t.sea).unwrap())
        });
        for year in 1..=years {
            let owners = sim.world.owner.clone();
            let end = sim.calendar.year_start(start + year);
            while sim.day < end {
                sim.run_days(1);
                let w = &sim.world;
                for (r, g) in &sim.applied_today {
                    if r.requester == Slot::Warfare && r.cause == sim::requests::Cause::Eating {
                        d.asked += r.take;
                        d.given += g;
                    }
                }
                for p in sim.delivered_today() {
                    if let Event::Famine { province, .. } = p.event {
                        d.famines += 1;
                        let armies_there = (0..w.armies.count()).any(|a| {
                            w.armies.province[a] == province && w.armies.home[a] != province && w.armies.size[a] > 0 && w.armies.next[a] == NONE
                        });
                        if armies_there {
                            d.famines_with_armies += 1;
                        }
                    }
                }
                if let Some(ships) = &ships {
                    for a in 0..w.armies.count() {
                        let (at, next) = (w.armies.province[a], w.armies.next[a]);
                        if next != NONE && w.armies.size[a] > 0 && !ships.contains(&w.armies.kind[a]) && w.routes.between(at, next).is_some_and(|l| l.land_days.is_none()) {
                            d.afloat += 1;
                        }
                    }
                }
            }
            for p in sim.event_log.entries.iter().filter(|p| p.day >= since) {
                match p.event {
                    Event::Rebellion { .. } => d.rebellions += 1,
                    Event::Peace { rebellion: true, winner, attacker, .. } => {
                        if winner == attacker {
                            d.rebels_won += 1
                        } else {
                            d.put_down += 1
                        }
                    }
                    Event::Battle { province, winner, loser, .. } => {
                        d.battles += 1;
                        let owner = sim.world.owner[province as usize];
                        if winner != NONE && loser != NONE && (winner == owner) != (loser == owner) && loser == owner {
                            d.attackers_won += 1;
                        }
                    }
                    _ => {}
                }
            }
            // What was sent on the last day is delivered (logged) the next.
            since = sim.day - 1;
            d.changed_hands += owners.iter().zip(&sim.world.owner).filter(|(a, b)| a != b && **a != NONE && **b != NONE).count();
            if let Some(y) = sim.history.last() {
                d.killed += y.ledger.killed;
                d.starved += y.ledger.starved;
            }
            if year % 10 == 0 {
                let w = &sim.world;
                let alive = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).count();
                let soldiers: u64 = w.armies.size.iter().sum();
                let people = sim::fields::quantity_total(w, sim::fields::Quantity::People).max(1);
                println!(
                    "  year {year}: {alive} realms; rebellions {} (put down {}, won {}), provinces changed hands {}, battles {} (attackers won {}), company-days at sea {}, killed {}, starved {} ({:.1} per 1,000 a year), under arms {:.2}%; soldiers fed {:.1}%; famines {} ({} where armies stood away from home)",
                    d.rebellions,
                    d.put_down,
                    d.rebels_won,
                    d.changed_hands,
                    d.battles,
                    d.attackers_won,
                    d.afloat,
                    d.killed,
                    d.starved,
                    1000.0 * d.starved as f64 / people as f64 / 10.0,
                    100.0 * soldiers as f64 / people as f64,
                    100.0 * d.given as f64 / d.asked.max(1) as f64,
                    d.famines,
                    d.famines_with_armies
                );
                d = Decade::default();
            }
        }
    }
}

/// Milliseconds a day over `days` days; with `quiet`, only days that aren't a month's start.
fn per_day(sim: &mut Sim, days: u64, quiet: bool) -> f64 {
    let mut total = 0.0;
    let mut n = 0;
    for _ in 0..days {
        let skip = quiet && sim.calendar.is_month_start(sim.day);
        let t = std::time::Instant::now();
        sim.run_days(1);
        if !skip {
            total += t.elapsed().as_secs_f64() * 1000.0;
            n += 1;
        }
    }
    total / n.max(1) as f64
}

fn time(economics: &str, seed: u64) {
    let large = WorldParams::preset(sim::world::Size::Large, seed);
    let mut runs = Vec::new();
    for armies in [false, true] {
        // No unrest: no rebellions start wars in the quiet days.
        let mut c = config(economics, seed, false, armies);
        c.world = large.clone();
        let mut sim = Sim::new(&c).unwrap();
        sim.run_days(364);
        // End every war, so the quiet days are at peace.
        for k in 0..sim.world.wars.count() {
            if sim.world.wars.ended[k] == sim::diplomacy::NEVER {
                let (a, b) = (sim.world.wars.attacker[k], sim.world.wars.defender[k]);
                sim.poke(Event::ForcePeace { a, b, winner: NONE });
            }
        }
        sim.run_days(40);
        let quiet = per_day(&mut sim, 120, true);
        let peace = per_day(&mut sim, 120, false);
        let w = &sim.world;
        let mut best = (0usize, NONE, NONE);
        for l in &w.routes.links {
            let (a, b) = (w.owner[l.a as usize], w.owner[l.b as usize]);
            if a != NONE && b != NONE && a != b && l.land_days.is_some() {
                let size = land_of(&sim, a).min(land_of(&sim, b));
                if size > best.0 {
                    best = (size, a.min(b), a.max(b));
                }
            }
        }
        sim.poke(Event::DeclareWar { attacker: best.1, defender: best.2 });
        let war = per_day(&mut sim, 364, false);
        let battles = sim.event_log.entries.iter().filter(|p| matches!(p.event, Event::Battle { .. })).count();
        println!("{} armies: days that aren't a month's first {quiet:.3} ms, every day of peace {peace:.3} ms, of war {war:.3} ms ({battles} battles in the log)", if armies { "with" } else { "without" });
        runs.push((quiet, peace, war));
    }
    println!("armies add {:.3} ms a day in peace and {:.3} ms a day in war", runs[1].1 - runs[0].1, runs[1].2 - runs[0].2);
}

/// What Diplomacy "treaties" costs: two large worlds alike but for it (realms without unrest,
/// rulers, armies; "treaties" with no chance of war, so no war tells them apart), timed on the
/// days its month's work falls on.
fn time_treaties(economics: &str, seed: u64) {
    let large = WorldParams::preset(sim::world::Size::Large, seed);
    let mut runs = Vec::new();
    for treaties in [false, true] {
        let mut c = config(economics, seed, false, true);
        c.world = large.clone();
        if treaties {
            c.modules.diplomacy.as_mut().unwrap().treaties = Some(sim::diplomacy::TreatyParams { war_chance: 0.0, ..Default::default() });
        }
        let mut sim = Sim::new(&c).unwrap();
        sim.run_days(364);
        let (mut spent, mut months) = (0.0, 0);
        while months < 24 {
            if sim.calendar.is_month_start(sim.day) {
                let t = std::time::Instant::now();
                sim.run_days(1);
                spent += t.elapsed().as_secs_f64() * 1000.0;
                months += 1;
            } else {
                sim.run_days(1);
            }
        }
        println!("{}: a month's first day {:.2} ms; {} realms, {} relations", if treaties { "treaties" } else { "wars" }, spent / months as f64, sim.world.realms.count(), sim.world.relations.count());
        runs.push(spent / months as f64);
    }
    println!("treaties add {:.2} ms a month, {:.3} ms a day on average", runs[1] - runs[0], (runs[1] - runs[0]) * 12.0 / 364.0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let economics = args.first().map_or("food", |s| s.as_str());
    let seed: u64 = args.get(1).map_or(21, |s| s.parse().expect("seed"));
    let what = args.get(2).map_or("one", |s| s.as_str());
    let years: i64 = args.get(3).map_or(2, |s| s.parse().expect("years"));
    match what {
        "one" => one(economics, seed, years),
        "long" => long(economics, seed, years),
        "time" => time(economics, seed),
        "time-treaties" => time_treaties(economics, seed),
        other => panic!("no {other}: one, long, time or time-treaties"),
    }
}
