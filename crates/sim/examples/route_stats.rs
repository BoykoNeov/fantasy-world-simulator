//! Province neighbours on big worlds: how long working them out takes, how many each province
//! has, how long the ways are, and which provinces have none.
//! `cargo run --release -p sim --example route_stats -- [small|medium|large|huge]`
use sim::routes::Routes;
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
}
