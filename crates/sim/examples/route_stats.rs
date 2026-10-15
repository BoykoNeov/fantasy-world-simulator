//! Province neighbours on big worlds: how long working them out takes, how many each province
//! has, how long the ways are, and which provinces have none. Then the sea rises 250 m, cutting
//! many provinces into islands, and they are worked out again.
//! `cargo run --release -p sim --example route_stats -- [small|medium|large|huge]`
use sim::config::Config;
use sim::edit::Edit;
use sim::routes::Routes;
use sim::sim::Sim;
use sim::world::{Size, World, WorldParams};
use std::time::Instant;

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(f32::NAN)
}

fn main() {
    let size = match std::env::args().nth(1).as_deref() {
        Some("small") => Size::Small,
        Some("medium") => Size::Medium,
        Some("huge") => Size::Huge,
        _ => Size::Large,
    };
    for seed in [7u64, 8] {
        let w = World::generate(&WorldParams::preset(size, seed));
        let t = Instant::now();
        let r = Routes::build(&w);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let p = &w.provinces;
        let alone: Vec<u32> = (0..p.count as u32).filter(|&q| p.cells[q as usize] > 0 && r.of(q).next().is_none()).collect();
        let mut per: Vec<usize> = (0..p.count as u32).map(|q| r.of(q).count()).collect();
        per.sort();
        let land: Vec<f32> = r.links.iter().filter_map(|l| l.land_days).collect();
        let sea: Vec<f32> = r.links.iter().filter_map(|l| l.sea_days).collect();
        let sea_only = r.links.iter().filter(|l| l.land_days.is_none()).count();
        println!(
            "{size:?} seed {seed}: {} provinces, worked out in {ms:.0} ms; {} links ({} by land, {} by sea, {sea_only} by sea only); neighbours per province median {} max {}; days by land median {:.1} max {:.1}, by sea median {:.1} max {:.1}; {} provinces with no neighbour, of {} cells at most",
            p.count,
            r.links.len(),
            land.len(),
            sea.len(),
            per[per.len() / 2],
            per[per.len() - 1],
            median(land.clone()),
            land.iter().copied().fold(0.0, f32::max),
            median(sea.clone()),
            sea.iter().copied().fold(0.0, f32::max),
            alone.len(),
            alone.iter().map(|&q| p.cells[q as usize]).max().unwrap_or(0),
        );
    }
    let mut sim = Sim::new(&Config::new(WorldParams::preset(size, 7))).unwrap();
    let t = Instant::now();
    sim.edit(Edit::SeaLevel { rise_m: 250.0 }).unwrap();
    let edit = t.elapsed().as_secs_f64() * 1000.0;
    let w = &sim.world;
    let t = Instant::now();
    let r = Routes::build(w);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    // Provinces whose land is now in more than one piece.
    let g = &w.grid;
    let mut pieces = vec![0u32; w.provinces.count];
    let mut seen = vec![false; g.cells()];
    for start in 0..g.cells() {
        let p = w.provinces.of_cell[start];
        if p == sim::grid::NONE || seen[start] {
            continue;
        }
        pieces[p as usize] += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(c) = stack.pop() {
            for &m in &g.neighbors[c] {
                if w.provinces.of_cell[m as usize] == p && !seen[m as usize] {
                    seen[m as usize] = true;
                    stack.push(m as usize);
                }
            }
        }
    }
    println!(
        "{size:?} seed 7 after the sea rose 250 m: the edit took {edit:.0} ms; {} provinces in pieces; neighbours worked out in {ms:.0} ms, {} links",
        pieces.iter().filter(|&&n| n > 1).count(),
        r.links.len()
    );
}
