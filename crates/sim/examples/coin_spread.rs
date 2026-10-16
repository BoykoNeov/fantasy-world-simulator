//! How unevenly coins sit, with and without realms' taxes: on the large world under markets, the
//! provinces' coins a person against the world's (middle, 90th and 99th percentile, most) and the
//! share of all coins in the richest 50 provinces, every few years; with realms, also the
//! treasuries' share of the coins their land holds.
//!
//! cargo run --release -p sim --example coin_spread -- [seed] [years] [armies]
//!
//! `armies` also runs the world with realms, wars and troops, whose bounties for soldiers raised
//! move coins from the treasuries back into the provinces.

use sim::config::Config;
use sim::grid::NONE;
use sim::module::Slot;
use sim::realms::RealmParams;
use sim::sim::Sim;
use sim::world::{Size, World, WorldParams};

fn coins(w: &World, values: &[u64], row: usize) -> f64 {
    let g = &w.goods;
    let money = g.tree.find("money").unwrap();
    g.columns_under(money).unwrap().iter().map(|&c| values[g.index(row, c)]).sum::<u64>() as f64
}

fn report(sim: &Sim, year: i64) {
    let w = &sim.world;
    let people = w.province_population();
    let settled: Vec<usize> = (0..w.provinces.count).filter(|&p| people[p] > 0).collect();
    let all: f64 = settled.iter().map(|&p| coins(w, &w.stock, p)).sum();
    let each = all / settled.iter().map(|&p| people[p] as f64).sum::<f64>();
    let mut rich: Vec<f64> = settled.iter().map(|&p| coins(w, &w.stock, p) / people[p] as f64 / each).collect();
    rich.sort_by(f64::total_cmp);
    let at = |q: f64| rich[((rich.len() - 1) as f64 * q).round() as usize];
    let mut held: Vec<f64> = settled.iter().map(|&p| coins(w, &w.stock, p)).collect();
    held.sort_by(|a, b| b.total_cmp(a));
    let top: f64 = held.iter().take(50).sum();
    let mut line = format!(
        "year {year:>3}: coins a person against the world's: middle {:.2}, 90% {:.2}, 99% {:.2}, most {:.1}; richest 50 provinces hold {:.1}%",
        at(0.5),
        at(0.9),
        at(0.99),
        rich[rich.len() - 1],
        100.0 * top / all
    );
    if w.realms.count() > 0 {
        let alive: Vec<u32> = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect();
        let treasuries: f64 = alive.iter().map(|&r| coins(w, &w.realm_stock, r as usize)).sum();
        let land: f64 = (0..w.provinces.count).filter(|&p| w.owner[p] != NONE).map(|p| coins(w, &w.stock, p)).sum();
        line += &format!("; treasuries {:.1}% of their land's coins", 100.0 * treasuries / land);
    }
    println!("{line}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: u64 = args.first().map_or(7, |s| s.parse().expect("seed"));
    let years: i64 = args.get(1).map_or(50, |s| s.parse().expect("years"));
    let armies = args.get(2).is_some_and(|s| s == "armies");
    let runs: &[(bool, bool)] = if armies { &[(true, true)] } else { &[(false, false), (true, false)] };
    for &(realms, armies) in runs {
        let mut config = Config::new(WorldParams::preset(Size::Large, seed));
        config.modules.set_choice(Slot::Economics, "markets").unwrap();
        if realms {
            config.modules.realms = Some(RealmParams::default());
        }
        if armies {
            config.modules.diplomacy = Some(Default::default());
            config.modules.warfare = Some(Default::default());
        }
        println!("{}", match (realms, armies) {
            (_, true) => "with realms, taxes, wars and troops",
            (true, false) => "with realms and taxes",
            _ => "without realms",
        });
        let mut sim = Sim::new(&config).unwrap();
        let start = sim.calendar.date(0).year;
        for y in [1, 2, 3, 5, 10, 20, 30, 40, 50].into_iter().filter(|&y| y <= years) {
            sim.run_to_year(start + y);
            report(&sim, y);
        }
    }
}
