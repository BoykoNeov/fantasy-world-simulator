//! Story 2, the rich river city, as the checks piece of step 7 words it: markets, a world from a
//! seed, no pokes, 150 years. A "rich river city" is a coastal province at a big river's mouth
//! that stays among the richest 2% by coins for 10 years or more. The story passes on a seed if
//! such a province is occupied in a war and changes hands at the peace, and later its coins fall
//! below half their peak while the trade through it halves.
//!
//! Story 1's world (200 provinces) with every step-7 module on: realms with unrest, rulers,
//! armies and Diplomacy "treaties". Prints, for each river-mouth province that was a rich river
//! city: its years among the richest 2%, its peak, the days occupied, the peaces at which it
//! changed hands, and when its coins and trade fell to half; then, for comparison, how many of the
//! richest 2% (any province) changed hands at a peace.
//!
//! cargo run --release -p sim --example story2 -- [years] [seed ...]

use sim::config::Config;
use sim::diplomacy::{TreatyParams, WarParams};
use sim::events::Event;
use sim::grid::NONE;
use sim::module::Slot;
use sim::sim::Sim;
use sim::world::{World, WorldParams, RIVER_MIN_FLOW};

fn config(seed: u64) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    c.modules.realms = Some(Default::default());
    c.modules.characters = Some(Default::default());
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    c.modules.warfare = Some(Default::default());
    c
}

fn coins(w: &World, p: usize) -> u64 {
    let g = &w.goods;
    g.columns_under(g.tree.find("money").unwrap()).unwrap().iter().map(|&c| w.stock[g.index(p, c)]).sum()
}

/// Each coastal province where a big river (one the map draws) reaches the sea, with the most
/// water reaching the sea there, in big rivers' worth.
fn mouths(w: &World) -> Vec<(usize, f64)> {
    let mut best = vec![0u64; w.provinces.count];
    for c in 0..w.elevation.len() {
        let p = w.provinces.of_cell[c];
        if p == NONE || w.elevation[c] < 0.0 || w.river_flow[c] < RIVER_MIN_FLOW {
            continue;
        }
        let d = w.downstream[c];
        let to_sea = d == NONE || w.elevation[d as usize] < 0.0;
        if to_sea {
            best[p as usize] = best[p as usize].max(w.river_flow[c]);
        }
    }
    (0..w.provinces.count).filter(|&p| best[p] > 0 && w.provinces.coast_km[p] > 0.0).map(|p| (p, best[p] as f64 / RIVER_MIN_FLOW as f64)).collect()
}

#[derive(Clone, Default)]
struct Track {
    /// Years among the richest 2%, in a row now, and the most in a row.
    run: u32,
    longest: u32,
    /// The highest coins at a year's end, and the year's trade then.
    peak: u64,
    peak_trade: f64,
    occupied_days: u64,
    /// Years it ended as its realm's capital, and as a target of a war.
    capital_years: u32,
    targeted: u32,
    /// Years it changed hands at a peace.
    handed: Vec<i64>,
    /// The first year after a hand-over that its coins were under half the peak, and its trade
    /// under half the trade at the peak.
    poorer: Option<i64>,
    less_trade: Option<i64>,
}

fn run(seed: u64, years: i64) -> bool {
    let mut sim = Sim::new(&config(seed)).unwrap();
    let start = sim.calendar.date(0).year;
    let n = sim.world.provinces.count;
    let mouth = mouths(&sim.world);
    let is_mouth: Vec<bool> = (0..n).map(|p| mouth.iter().any(|m| m.0 == p)).collect();
    let mut t = vec![Track::default(); n];
    let mut year_trade = vec![0.0f64; n];
    let mut last_trade = 0;
    let (mut rich_handed, mut handed_all) = (0usize, 0usize);
    let mut rich = vec![false; n];
    for year in 1..=years {
        let end = sim.calendar.year_start(start + year);
        while sim.day < end {
            let owners = sim.world.owner.clone();
            sim.step_day();
            let w = &sim.world;
            if w.trade.day != last_trade && !w.trade.shipments.is_empty() {
                last_trade = w.trade.day;
                for s in &w.trade.shipments {
                    year_trade[s.from as usize] += s.coins;
                    year_trade[s.to as usize] += s.coins;
                }
            }
            for p in 0..n {
                if is_mouth[p] && w.occupier[p] != NONE {
                    t[p].occupied_days += 1;
                }
            }
            for e in sim.delivered_today() {
                if let Event::WarDeclared { war, .. } = e.event
                    && let Some(&q) = w.wars.target.get(war as usize)
                    && (q as usize) < n
                {
                    t[q as usize].targeted += 1;
                }
            }
            if sim.delivered_today().iter().any(|e| matches!(e.event, Event::Peace { .. })) {
                for p in 0..n {
                    if owners[p] != NONE && w.owner[p] != NONE && w.owner[p] != owners[p] {
                        handed_all += 1;
                        if rich[p] {
                            rich_handed += 1;
                        }
                        if t[p].longest >= 10 {
                            t[p].handed.push(year);
                        }
                    }
                }
            }
        }
        let w = &sim.world;
        let worth: Vec<u64> = (0..n).map(|p| coins(w, p)).collect();
        let mut sorted = worth.clone();
        sorted.sort_unstable();
        let line = sorted[n - n.div_ceil(50)];
        for p in 0..n {
            rich[p] = worth[p] > 0 && worth[p] >= line;
            let k = &mut t[p];
            k.run = if rich[p] { k.run + 1 } else { 0 };
            k.longest = k.longest.max(k.run);
            if w.owner[p] != NONE && w.realms.capital[w.owner[p] as usize] == p as u32 {
                k.capital_years += 1;
            }
            if worth[p] > k.peak {
                k.peak = worth[p];
                k.peak_trade = year_trade[p];
            }
            if !k.handed.is_empty() {
                if k.poorer.is_none() && worth[p] * 2 < k.peak {
                    k.poorer = Some(year);
                }
                if k.less_trade.is_none() && year_trade[p] * 2.0 < k.peak_trade {
                    k.less_trade = Some(year);
                }
            }
        }
        year_trade.iter_mut().for_each(|x| *x = 0.0);
    }
    println!("seed {seed}: {} river-mouth provinces on the coast", mouth.len());
    let mut passed = false;
    for &(p, flow) in &mouth {
        let k = &t[p];
        if k.longest < 10 {
            continue;
        }
        let pass = !k.handed.is_empty() && k.occupied_days > 0 && k.poorer.is_some() && k.less_trade.is_some();
        passed |= pass;
        println!(
            "  province {p} (river {flow:.1}× a big river's water): {} years in a row among the richest 2%, peak {} coins, a capital {} years, a war's target {} times, occupied {} days, changed hands at a peace in years {:?}, coins under half the peak from year {:?}, trade under half from year {:?}{}",
            k.longest,
            k.peak,
            k.capital_years,
            k.targeted,
            k.occupied_days,
            k.handed,
            k.poorer,
            k.less_trade,
            if pass { ": PASSES" } else { "" }
        );
    }
    let cities = mouth.iter().filter(|m| t[m.0].longest >= 10).count();
    println!("  {cities} rich river cities; story 2 {}", if passed { "passes" } else { "does not pass" });
    println!("  for comparison: {handed_all} provinces changed hands at a peace, {rich_handed} of them among the richest 2% at the time");
    let p = sim.problems;
    println!("  problems: {} contract violations, {} conservation breaks", p.contract_violations, p.conservation_breaks);
    passed
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let years: i64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(150);
    let seeds: Vec<u64> = args.iter().skip(1).filter_map(|a| a.parse().ok()).collect();
    let seeds = if seeds.is_empty() { vec![21, 7, 1] } else { seeds };
    let passed = seeds.iter().filter(|&&s| run(s, years)).count();
    println!("story 2 passes on {passed} of {} seeds", seeds.len());
}
