//! How long the map's picture data takes to build, as after an edit that changes the map: per-cell
//! layers, province ids, the border wobble, and every line. `cargo run --release -p app --example
//! map_rebuild -- [small|medium|large|huge]`
use app::gpu::MapData;
use app::layers::ProvinceView;
use sim::config::Config;
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::time::Instant;

fn main() {
    let size = match std::env::args().nth(1).as_deref() {
        Some("small") => Size::Small,
        Some("medium") => Size::Medium,
        Some("huge") => Size::Huge,
        _ => Size::Large,
    };
    let sim = Sim::new(&Config::new(WorldParams::preset(size, 7))).unwrap();
    let view = ProvinceView::now(&sim);
    for _ in 0..3 {
        let t = Instant::now();
        let data = MapData::of(&sim.world, &view);
        println!("{size:?}: map data in {:.0} ms ({} line segments)", t.elapsed().as_secs_f64() * 1000.0, data.lines.len());
    }
}
