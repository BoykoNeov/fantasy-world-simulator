//! How long edits take on a big world, and what they keep in memory: a brush stroke, a sea-level
//! change, a province split, taking the last one back, and going back past them.
//! `cargo run --release -p sim --example edit_timing -- [small|medium|large|huge]`
use sim::config::Config;
use sim::edit::{Brush, Edit};
use sim::grid::dir_to_lonlat;
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let size = match std::env::args().nth(1).as_deref() {
        Some("small") => Size::Small,
        Some("medium") => Size::Medium,
        Some("huge") => Size::Huge,
        _ => Size::Large,
    };
    let mut sim = Sim::new(&Config::new(WorldParams::preset(size, 7))).unwrap();
    sim.run_to_year(3);
    sim.run_days(100);
    let people = sim.world.province_population();
    let p = (0..people.len()).max_by_key(|&i| people[i]).unwrap();
    let (lon, lat) = dir_to_lonlat(sim.world.provinces.center[p]);
    let (lon, lat) = (lon.to_degrees(), lat.to_degrees());
    println!("{size:?} world, {} provinces, day {}", sim.world.provinces.count, sim.day);

    let path: Vec<[f64; 2]> = (0..20).map(|k| [lon + k as f64 * 0.4, lat]).collect();
    let t = Instant::now();
    sim.edit(Edit::Paint { path: path.clone(), radius_km: 150.0, brush: Brush::Raise(800.0) }).unwrap();
    println!("brush stroke (20 points, 150 km): {:.0} ms, map change kept {:.2} MB", ms(t), sim.map_change_bytes() as f64 / 1e6);

    let kept = sim.map_change_bytes();
    let t = Instant::now();
    sim.edit(Edit::SeaLevel { rise_m: 50.0 }).unwrap();
    println!("sea rises 50 m: {:.0} ms, map change kept {:.2} MB", ms(t), (sim.map_change_bytes() - kept) as f64 / 1e6);

    let cells: Vec<u32> = (0..sim.world.grid.cells() as u32).filter(|&c| sim.world.provinces.of_cell[c as usize] == p as u32).collect();
    let kept = sim.map_change_bytes();
    let t = Instant::now();
    sim.edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }).unwrap();
    println!("province split: {:.0} ms, map change kept {:.3} MB", ms(t), (sim.map_change_bytes() - kept) as f64 / 1e6);

    let last = sim.commands.len() - 1;
    let t = Instant::now();
    let split = sim.withdraw(last).unwrap();
    println!("take the split back (replays from the year's start, reusing the stroke and the sea): {:.0} ms", ms(t));
    let t = Instant::now();
    sim.reinstate(split).unwrap();
    println!("give it again: {:.0} ms", ms(t));

    // Painting while paused, as the window does it: the simulation has run ahead, goes back to the
    // day shown, takes the stroke, and runs ahead again. The cost of a stroke, of taking one back
    // and of explaining the day should stay flat however many strokes the day already has.
    let shown = sim.day;
    let province = p as u32;
    let stroke = |sim: &mut Sim, k: usize| {
        let x = lon + (k % 20) as f64 * 0.3;
        let path = vec![[x, lat - 1.0], [x, lat + 1.0]];
        sim.rewind(shown).unwrap();
        let t = Instant::now();
        sim.edit(Edit::Paint { path, radius_km: 120.0, brush: Brush::Raise(30.0) }).unwrap();
        let ms_edit = ms(t);
        sim.run_days(120);
        ms_edit
    };
    let t = Instant::now();
    for k in 0..200 {
        let ms_edit = stroke(&mut sim, k);
        if [0, 9, 49, 199].contains(&k) {
            sim.rewind(shown).unwrap();
            let t = Instant::now();
            let undone = sim.withdraw(sim.commands.len() - 1).unwrap();
            let ms_undo = ms(t);
            sim.reinstate(undone).unwrap();
            sim.run_days(5);
            let t = Instant::now();
            sim.trace(shown, province).unwrap();
            println!("stroke {}: {ms_edit:.0} ms; taking it back {ms_undo:.0} ms; explaining the day {:.0} ms", k + 1, ms(t));
        }
    }
    println!("200 strokes on one day, each with going back and running 120 days ahead: {:.1} s; map changes kept {:.1} MB", t.elapsed().as_secs_f64(), sim.map_change_bytes() as f64 / 1e6);

    sim.run_days(200);
    let t = Instant::now();
    sim.rewind(sim.day - 250).unwrap();
    println!("go back 250 days, past all three: {:.0} ms", ms(t));
    let t = Instant::now();
    sim.run_days(250);
    println!("run those 250 days again, edits included: {:.0} ms", ms(t));
}
