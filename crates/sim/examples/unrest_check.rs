//! How often provinces rise up and break away, with the story-1 drought and without it.
//!
//! Story 1's world, with realms, rulers and wars, under the economy given. `quiet` runs it left
//! alone and prints, each decade, the uprisings begun, the realms that broke away and the provinces
//! in uprising at the end. `drought` pokes in the story-1 drought at year 3 and, for the provinces
//! it reaches, prints when the first rose up and whether any broke away within 5 years; and the
//! same for the same world without the drought. Then how many months of famine (more than half a
//! percent of the people starving) the provinces had, at most, within any two years.
//!
//! cargo run --release -p sim --example unrest_check -- [food|markets] [seed] [quiet|drought] [years] [famine] [armies]
//!
//! `famine` sets what a month of famine adds to unrest, to try other settings. `armies`, anywhere
//! among the arguments, has provinces raise troops too; `quiet` then also prints the people who
//! starved in each decade (soldiers included) and the share under arms at its end. `treaties`
//! (with armies) has realms choose war and make peace themselves (Diplomacy "treaties"): `quiet`
//! then prints the wars each decade (by cause; second wars; the most at once; peaces with a
//! winner; wars for riches aimed at a capital; provinces that changed hands at a peace, and how
//! many were among the richest 5%), and `drought` counts a war for food declared by a drought
//! realm as the story's war (only for food: a war for riches is not the story's). `untolled` runs
//! the world as it ran before trade across borders changed: no tolls, war doesn't stop trade, and
//! trade doesn't warm opinions.

use sim::config::Config;
use sim::diplomacy::WarCause;
use sim::events::Event;
use sim::module::Slot;
use sim::sim::Sim;
use sim::world::WorldParams;

const FAMINE: f64 = 0.005;
const WINDOW: usize = 24;

fn world(economics: &str, seed: u64, famine: Option<f64>, armies: bool, treaties: bool, untolled: bool) -> Sim {
    let mut config = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
    config.modules.set_choice(Slot::Economics, economics).unwrap();
    let mut realms = sim::realms::RealmParams::default();
    if let (Some(f), Some(u)) = (famine, realms.unrest.as_mut()) {
        u.famine = f;
    }
    config.modules.realms = Some(realms);
    config.modules.characters = Some(Default::default());
    config.modules.diplomacy = Some(Default::default());
    if armies || treaties {
        config.modules.warfare = Some(Default::default());
    }
    if treaties {
        config.modules.diplomacy.as_mut().unwrap().treaties = Some(Default::default());
    }
    if untolled {
        if let Some(t) = config.modules.markets.as_mut().and_then(|m| m.trade.as_mut()) {
            (t.toll, t.war_stops) = (0.0, 0.0);
        }
        if let Some(t) = config.modules.diplomacy.as_mut().and_then(|d| d.treaties.as_mut()) {
            t.trade_opinion = 0.0;
        }
    }
    Sim::new(&config).unwrap()
}

/// What happened over a run: uprisings begun (province, day), breakaways (realm, from, day), and
/// each province's most famine months within two years.
#[derive(Default)]
struct Seen {
    uprisings: Vec<(u32, u64)>,
    breakaways: Vec<(u32, u32, u64)>,
    most_famine: Vec<usize>,
    /// Wars declared: (war, attacker, cause, day).
    declared: Vec<(u32, u32, WarCause, u64)>,
    /// Wars for riches whose target was the defender's capital.
    at_capitals: usize,
    /// Peaces: (war, whether someone won).
    peaces: Vec<(u32, bool)>,
    /// Provinces that changed hands at a peace, and how many were among the richest 5% the day
    /// before.
    handed: usize,
    rich_handed: usize,
    /// The most wars going on at once, at a month's start.
    most_at_once: usize,
}

/// Run to day `end`, keeping what happened.
fn run(sim: &mut Sim, end: u64, seen: &mut Seen) {
    let n = sim.world.provinces.count;
    seen.most_famine.resize(n, 0);
    let mut starved = vec![0u64; n];
    let mut recent: Vec<Vec<bool>> = vec![Vec::new(); n];
    let coins = sim.world.goods.tree.find("money").and_then(|m| sim.world.goods.columns_under(m)).is_some_and(|c| !c.is_empty());
    let treaties = sim.config.modules.diplomacy.as_ref().is_some_and(|d| d.treaties.is_some());
    while sim.day < end {
        // Owners and the richest provinces the day before (coins under markets, people otherwise).
        let (owners, rich) = if treaties {
            let w = &sim.world;
            let people = w.province_population();
            let worth: Vec<u64> = (0..n).map(|p| if coins { w.stock_under(p, "money").unwrap_or(0) } else { people[p] }).collect();
            let mut sorted = worth.clone();
            sorted.sort_unstable();
            let line = sorted[n - n / 20 - 1];
            (w.owner.clone(), worth.iter().map(|&v| v > 0 && v >= line).collect())
        } else {
            (Vec::new(), Vec::new())
        };
        if treaties && sim.calendar.is_month_start(sim.day) {
            let w = &sim.world;
            let at_once = (0..w.wars.count()).filter(|&k| w.wars.ongoing(k as u32) && w.realms.alive(w.wars.attacker[k]) && w.realms.alive(w.wars.defender[k])).count();
            seen.most_at_once = seen.most_at_once.max(at_once);
        }
        if sim.calendar.is_month_start(sim.day) && sim.day > 0 {
            let people = sim.world.province_population();
            for p in (0..n).filter(|&p| people[p] > 0) {
                let share = starved[p] as f64 / (people[p] + starved[p]) as f64;
                let r = &mut recent[p];
                r.push(share > FAMINE);
                if r.len() > WINDOW {
                    r.remove(0);
                }
                seen.most_famine[p] = seen.most_famine[p].max(r.iter().filter(|&&f| f).count());
            }
            starved.iter_mut().for_each(|s| *s = 0);
        }
        sim.step_day();
        for p in sim.delivered_today() {
            match p.event {
                Event::Famine { province, starved: s } => starved[province as usize] += s,
                Event::Uprising { province, seat, .. } if province == seat => seen.uprisings.push((province, p.day)),
                Event::Rebellion { realm, from, .. } => seen.breakaways.push((realm, from, p.day)),
                Event::WarDeclared { war, attacker, defender, cause } => {
                    seen.declared.push((war, attacker, cause, p.day));
                    let w = &sim.world;
                    if cause == WarCause::Riches && w.realms.capital.get(defender as usize) == w.wars.target.get(war as usize) {
                        seen.at_capitals += 1;
                    }
                }
                Event::Peace { war, winner, .. } => {
                    seen.peaces.push((war, winner != sim::grid::NONE));
                    let w = &sim.world;
                    for q in 0..owners.len() {
                        if w.owner[q] != owners[q] && owners[q] != sim::grid::NONE && w.owner[q] != sim::grid::NONE {
                            seen.handed += 1;
                            seen.rich_handed += rich[q] as usize;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Wars realms chose while already fighting another.
fn second_wars(sim: &Sim, of: &[(u32, u32, WarCause, u64)]) -> usize {
    let wars = &sim.world.wars;
    of.iter()
        .filter(|&&(k, a, c, _)| {
            let began = wars.began[k as usize];
            matches!(c, WarCause::Food | WarCause::Riches)
                && (0..wars.count()).any(|j| j != k as usize && (wars.attacker[j] == a || wars.defender[j] == a) && wars.began[j] <= began && wars.ended[j] > began)
        })
        .count()
}

fn famine_months(most: &[usize], of: impl Iterator<Item = usize>) -> String {
    let mut hist = [0usize; WINDOW + 1];
    for p in of {
        hist[most[p]] += 1;
    }
    hist.iter().enumerate().filter(|(_, c)| **c > 0).map(|(k, c)| format!("{k}:{c}")).collect::<Vec<_>>().join(" ")
}

fn main() {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let armies = all.iter().any(|a| a == "armies");
    let treaties = all.iter().any(|a| a == "treaties");
    let untolled = all.iter().any(|a| a == "untolled");
    let args: Vec<String> = all.into_iter().filter(|a| a != "armies" && a != "treaties" && a != "untolled").collect();
    let economics = args.first().map_or("food", |s| s.as_str());
    let seed: u64 = args.get(1).map_or(21, |s| s.parse().expect("seed"));
    let drought = args.get(2).is_some_and(|s| s == "drought");
    let years: i64 = args.get(3).map_or(if drought { 5 } else { 150 }, |s| s.parse().expect("years"));
    let famine: Option<f64> = args.get(4).map(|s| s.parse().expect("famine"));
    let mut sim = world(economics, seed, famine, armies, treaties, untolled);
    let start = sim.calendar.date(0).year;
    let n = sim.world.provinces.count;
    let year = sim.calendar.days_per_year() as u64;
    let u = sim.config.modules.realms.as_ref().unwrap().unrest.clone().unwrap();
    println!("{economics}, seed {seed}, {n} provinces, {} realms; a famine month adds {}; armies {}; treaties {treaties}", sim.world.realms.count(), u.famine, armies || treaties);

    if !drought {
        for decade in 0..(years + 9) / 10 {
            let mut seen = Seen::default();
            let end = sim.calendar.year_start(start + ((decade + 1) * 10).min(years));
            run(&mut sim, end, &mut seen);
            let w = &sim.world;
            let risen = w.uprising.iter().filter(|&&s| s != sim::grid::NONE).count();
            let alive = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).count();
            // The decade's summaries: people who starved (soldiers too), and the people on average.
            let decade_years: Vec<_> = sim.history.iter().filter(|y| y.year >= start + decade * 10).collect();
            let starved: u64 = decade_years.iter().map(|y| y.ledger.starved).sum();
            let people = decade_years.iter().map(|y| y.population as f64).sum::<f64>() / decade_years.len().max(1) as f64;
            let soldiers: u64 = w.armies.size.iter().sum();
            let all = sim::fields::quantity_total(w, sim::fields::Quantity::People);
            let wars = (0..w.wars.count() as u32).filter(|&k| w.wars.ongoing(k)).count();
            println!(
                "years to {:>3}: {} uprisings begun, {} realms broke away; at the end {risen} provinces in uprising, {alive} realms, {wars} wars going on, {:.2}% under arms; starved {:.2} per 1,000 a year (people {:.1} million); famine months in two years, most: {}",
                ((decade + 1) * 10).min(years),
                seen.uprisings.len(),
                seen.breakaways.len(),
                100.0 * soldiers as f64 / all as f64,
                1000.0 * starved as f64 / people.max(1.0) / decade_years.len().max(1) as f64,
                people / 1e6,
                famine_months(&seen.most_famine, 0..n)
            );
            if treaties {
                let by = |c: WarCause| seen.declared.iter().filter(|d| d.2 == c).count();
                let won = seen.peaces.iter().filter(|p| p.1).count();
                println!(
                    "    wars declared: {} for food, {} for riches ({} at a capital), {} by rebels, {} by pokes; {} second wars; at most {} at once; {} peaces, {won} with a winner; {} provinces changed hands at a peace, {} of them among the richest 5%",
                    by(WarCause::Food),
                    by(WarCause::Riches),
                    seen.at_capitals,
                    by(WarCause::Rebellion),
                    by(WarCause::Poke),
                    second_wars(&sim, &seen.declared),
                    seen.most_at_once,
                    seen.peaces.len(),
                    seen.handed,
                    seen.rich_handed
                );
            }
        }
        // Rebellions' wars: put down (the realm the rebels left won) or won by the rebels.
        let wars = &sim.world.wars;
        let rebellions: Vec<usize> = (0..wars.count()).filter(|&k| WarCause::from_u8(wars.cause[k]) == WarCause::Rebellion && wars.ended[k] != sim::diplomacy::NEVER).collect();
        let down = rebellions.iter().filter(|&&k| wars.winner[k] == wars.defender[k]).count();
        let free = rebellions.iter().filter(|&&k| wars.winner[k] == wars.attacker[k]).count();
        println!("rebellions' wars ended: {} ({down} put down, {free} won by the rebels, {} with nobody winning)", rebellions.len(), rebellions.len() - down - free);
        return;
    }

    sim.run_to_year(start + 3);
    let mut twin = sim.fork();
    let p = &sim.world.provinces;
    let heart = (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap();
    let (lon, lat) = sim::grid::dir_to_lonlat(p.center[heart]);
    sim.poke(Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days: 3 * 364 });
    let from = sim.day;
    let mut probe = sim.fork();
    probe.run_days(2);
    let region: Vec<u32> = (0..n as u32).filter(|&i| probe.world.rain_shortfall[i as usize] > 0).collect();
    let realms: Vec<u32> = {
        let mut r: Vec<u32> = region.iter().map(|&p| sim.world.owner[p as usize]).filter(|&r| r != sim::grid::NONE).collect();
        r.sort_unstable();
        r.dedup();
        r
    };
    println!("drought region: {} provinces, of realms {realms:?}", region.len());
    let end = from + years as u64 * year;
    for (name, s) in [("drought", &mut sim), ("without", &mut twin)] {
        let mut seen = Seen::default();
        run(s, end, &mut seen);
        let first = seen.uprisings.iter().filter(|(p, _)| region.contains(p)).map(|&(_, d)| d).min();
        let broke = seen.breakaways.iter().filter(|(_, f, _)| realms.contains(f)).count();
        let rose_in_realms = seen.uprisings.iter().filter(|(p, _)| realms.contains(&s.world.owner[*p as usize]) || region.contains(p)).count();
        let food_war = seen.declared.iter().filter(|d| d.2 == WarCause::Food && realms.contains(&d.1)).map(|d| d.3).min();
        let riches_wars = seen.declared.iter().filter(|d| d.2 == WarCause::Riches && realms.contains(&d.1)).count();
        println!(
            "{name}: first uprising in the region {}; first war for food by its realms {}; wars for riches by them {riches_wars}; uprisings in its realms {rose_in_realms}; breakaways from them {broke}; famine months in two years, region: {}",
            first.map_or("none".to_string(), |d| format!("after {:.1} years", (d - from) as f64 / year as f64)),
            food_war.map_or("none".to_string(), |d| format!("after {:.1} years", (d - from) as f64 / year as f64)),
            famine_months(&seen.most_famine, region.iter().map(|&p| p as usize))
        );
    }
}
