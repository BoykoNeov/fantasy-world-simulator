//! How many army markers the window would draw: on a large world under Economics v0 with realms
//! (unrest), rulers, armies and Diplomacy "treaties", at each year's start, the wars going on and
//! the places (a realm's field companies in one province, or on one way) with field soldiers, and
//! how many of those are recruits idle at home (standing in their home province, in no group).
//!
//! cargo run --release -p sim --example army_markers -- [seed] [years]

use sim::config::Config;
use sim::diplomacy::{TreatyParams, WarParams};
use sim::grid::NONE;
use sim::sim::Sim;
use sim::warfare::FIELD;
use sim::world::{Size, WorldParams};
use std::collections::BTreeSet;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: u64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(7);
    let years: i64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(20);
    let mut c = Config::new(WorldParams::preset(Size::Large, seed));
    c.modules.realms = Some(Default::default());
    c.modules.characters = Some(Default::default());
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    c.modules.warfare = Some(Default::default());
    let mut sim = Sim::new(&c).unwrap();
    let start = sim.calendar.date(0).year;
    for year in 1..=years {
        let end = sim.calendar.year_start(start + year);
        sim.run_days(end - sim.day);
        let w = &sim.world;
        let arm = &w.armies;
        let (mut places, mut idle, mut moving) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for a in 0..arm.count() {
            if arm.size[a] == 0 || arm.role[a] != FIELD {
                continue;
            }
            let key = (arm.realm[a], arm.province[a], arm.next[a]);
            places.insert(key);
            if arm.next[a] != NONE {
                moving.insert(key);
            } else if arm.province[a] == arm.home[a] && arm.group[a] == NONE {
                idle.insert(key);
            }
        }
        let wars = (0..w.wars.count() as u32).filter(|&k| w.wars.ongoing(k)).count();
        let busy: Vec<_> = places.difference(&idle).collect();
        println!(
            "year {year:>3}: {wars} wars; field markers {} ({} idle at home, {} on the move, {} others)",
            places.len(),
            idle.len(),
            moving.len(),
            busy.len() - moving.len()
        );
    }
}
