//! Headless PNG export, for eyeballing a world without the app.
//!
//! The picture is equirectangular (longitude across, latitude down). That is the simplest flat
//! projection, chosen here only for convenience; it does not settle which projection the map view
//! uses.

use crate::biome::Biome;
use crate::grid::{lonlat_to_dir, normalize, Grid, NONE};
use crate::sim::Sim;
use crate::world::RIVER_MIN_FLOW;
use rayon::prelude::*;
use std::f64::consts::PI;
use std::io::BufWriter;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Biomes, sea depth, hill shading and rivers.
    Terrain,
    /// Province fills and borders over the terrain.
    Provinces,
    /// People per km², per province.
    Population,
    /// Share of each province's people who starved in the current year.
    Famine,
    Temperature,
    Rainfall,
}

pub const ALL_LAYERS: [Layer; 6] =
    [Layer::Terrain, Layer::Provinces, Layer::Population, Layer::Famine, Layer::Temperature, Layer::Rainfall];

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Terrain => "terrain",
            Layer::Provinces => "provinces",
            Layer::Population => "population",
            Layer::Famine => "famine",
            Layer::Temperature => "temperature",
            Layer::Rainfall => "rainfall",
        }
    }
}

type Rgb = [u8; 3];

/// Cell under each pixel of a `width` × `width / 2` equirectangular image, row by row.
pub fn pixel_cells(grid: &Grid, width: u32) -> Vec<u32> {
    let height = width / 2;
    (0..width * height)
        .into_par_iter()
        .map(|k| {
            let (x, y) = (k % width, k / width);
            let lon = (x as f64 + 0.5) / width as f64 * 2.0 * PI - PI;
            let lat = PI / 2.0 - (y as f64 + 0.5) / height as f64 * PI;
            grid.dir_to_cell(lonlat_to_dir(lon, lat)) as u32
        })
        .collect()
}

/// RGB bytes of one layer at `width` × `width / 2`.
pub fn render(sim: &Sim, layer: Layer, width: u32, pixels: &[u32]) -> Vec<u8> {
    let height = width / 2;
    let w = &sim.world;
    let terrain = terrain_colors(sim);
    let cell_color: Vec<Rgb> = match layer {
        Layer::Terrain => terrain.clone(),
        Layer::Provinces => (0..w.grid.cells())
            .map(|c| match w.provinces.of_cell[c] {
                NONE => terrain[c],
                p => mix(terrain[c], province_color(p, w.params.seed), 0.55),
            })
            .collect(),
        Layer::Population => {
            let people = sim.pops.province_population(w.provinces.count);
            let density: Vec<f64> = (0..w.provinces.count).map(|p| people[p] as f64 / w.provinces.area_km2[p].max(1e-9)).collect();
            per_province(sim, &terrain, |p| {
                let t = ((density[p].max(1e-3).log10() + 2.0) / 4.0).clamp(0.0, 1.0); // 0.01 .. 100 per km²
                ramp(&[[250, 245, 210], [240, 180, 90], [200, 70, 40], [90, 20, 30]], t)
            })
        }
        Layer::Famine => {
            let people = sim.pops.province_population(w.provinces.count);
            per_province(sim, &terrain, |p| {
                let starved = sim.pops.starved_this_year[p];
                if starved == 0 {
                    return [205, 212, 190];
                }
                let share = starved as f64 / (people[p] + starved) as f64;
                ramp(&[[250, 220, 160], [230, 120, 60], [170, 20, 20]], (share * 5.0).clamp(0.0, 1.0))
            })
        }
        Layer::Temperature => (0..w.grid.cells())
            .map(|c| {
                let t = ((w.temperature[c] + 30.0) / 65.0).clamp(0.0, 1.0) as f64;
                let col = ramp(&[[40, 60, 160], [120, 180, 230], [245, 245, 235], [240, 170, 70], [190, 40, 30]], t);
                if w.elevation[c] < 0.0 { mix(col, [20, 30, 60], 0.45) } else { col }
            })
            .collect(),
        Layer::Rainfall => (0..w.grid.cells())
            .map(|c| {
                if w.elevation[c] < 0.0 {
                    return [25, 35, 60];
                }
                let t = (w.rainfall[c] / 2500.0).clamp(0.0, 1.0) as f64;
                ramp(&[[220, 195, 140], [150, 190, 90], [50, 140, 80], [30, 80, 150]], t)
            })
            .collect(),
    };

    let draw_borders = matches!(layer, Layer::Provinces | Layer::Population | Layer::Famine);
    let mut out = vec![0u8; (width * height * 3) as usize];
    out.par_chunks_mut((width * 3) as usize).enumerate().for_each(|(y, row)| {
        for x in 0..width as usize {
            let c = pixels[y * width as usize + x] as usize;
            let mut col = cell_color[c];
            if draw_borders {
                let right = pixels[y * width as usize + (x + 1) % width as usize] as usize;
                let below = pixels[(y + 1).min(height as usize - 1) * width as usize + x] as usize;
                let p = w.provinces.of_cell[c];
                if p != NONE && (w.provinces.of_cell[right] != p || w.provinces.of_cell[below] != p) {
                    col = mix(col, [40, 30, 30], 0.7);
                }
            }
            row[x * 3..x * 3 + 3].copy_from_slice(&col);
        }
    });
    out
}

pub fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    writer.write_image_data(rgb).map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)
}

/// Render every layer to `<dir>/<prefix><layer>.png`; returns the files written.
pub fn write_all(sim: &Sim, dir: &Path, prefix: &str, width: u32) -> std::io::Result<Vec<std::path::PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let pixels = pixel_cells(&sim.world.grid, width);
    let mut written = Vec::new();
    for layer in ALL_LAYERS {
        let rgb = render(sim, layer, width, &pixels);
        let path = dir.join(format!("{prefix}{}.png", layer.name()));
        write_png(&path, width, width / 2, &rgb)?;
        written.push(path);
    }
    Ok(written)
}

fn per_province(sim: &Sim, terrain: &[Rgb], f: impl Fn(usize) -> Rgb + Sync) -> Vec<Rgb> {
    let w = &sim.world;
    let by_province: Vec<Rgb> = (0..w.provinces.count).into_par_iter().map(&f).collect();
    (0..w.grid.cells())
        .map(|c| match w.provinces.of_cell[c] {
            NONE => mix(terrain[c], [30, 40, 60], 0.6),
            p => by_province[p as usize],
        })
        .collect()
}

fn terrain_colors(sim: &Sim) -> Vec<Rgb> {
    let w = &sim.world;
    let g = &w.grid;
    let step = (g.mean_area()).sqrt();
    let height_at = |c: usize, dir: [f32; 3], sign: f64| -> f32 {
        let d = g.center[c];
        let moved = normalize([d[0] + dir[0] as f64 * step * sign, d[1] + dir[1] as f64 * step * sign, d[2] + dir[2] as f64 * step * sign]);
        w.elevation[g.dir_to_cell(moved)].max(0.0)
    };
    (0..g.cells())
        .into_par_iter()
        .map(|c| {
            let h = w.elevation[c];
            if h < 0.0 {
                return ramp(&[[70, 130, 180], [35, 75, 135], [15, 30, 75]], (-h / 5500.0).clamp(0.0, 1.0) as f64);
            }
            let base = biome_color(w.biome[c]);
            let high = mix(base, [175, 160, 140], ((h - 1500.0) / 3000.0).clamp(0.0, 0.7) as f64);
            // Light from the north-west: slopes facing it are brighter.
            let gx = (height_at(c, g.east[c], 1.0) - height_at(c, g.east[c], -1.0)) * 0.5;
            let gy = (height_at(c, g.north[c], 1.0) - height_at(c, g.north[c], -1.0)) * 0.5;
            let shade = (1.0 + 0.0008 * (gx - gy) as f64).clamp(0.6, 1.4);
            let mut col = high.map(|v| (v as f64 * shade).min(255.0) as u8);
            if w.river_flow[c] >= RIVER_MIN_FLOW {
                let strength = ((w.river_flow[c] as f64 / RIVER_MIN_FLOW as f64).log10() / 2.0 + 0.45).clamp(0.45, 0.95);
                col = mix(col, [45, 95, 190], strength);
            }
            col
        })
        .collect()
}

fn biome_color(b: Biome) -> Rgb {
    match b {
        Biome::Ocean => [35, 75, 135],
        Biome::IceCap => [240, 245, 250],
        Biome::Tundra => [170, 170, 145],
        Biome::BorealForest => [65, 105, 75],
        Biome::TemperateForest => [85, 140, 70],
        Biome::Grassland => [160, 185, 95],
        Biome::Desert => [228, 205, 145],
        Biome::Savanna => [195, 180, 100],
        Biome::TropicalForest => [40, 120, 55],
        Biome::Alpine => [150, 140, 130],
    }
}

/// A stable, distinct-ish colour per province.
fn province_color(p: u32, seed: u64) -> Rgb {
    let h = crate::rng::mix(seed ^ (p as u64).wrapping_mul(0x9E37_79B9));
    let hue = (p as f64 * 0.618_033_988_75).fract();
    let light = 0.45 + (h % 1000) as f64 / 1000.0 * 0.2;
    hsl(hue, 0.55, light)
}

fn hsl(h: f64, s: f64, l: f64) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h * 6.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
}

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    [0, 1, 2].map(|k| (a[k] as f64 * (1.0 - t) + b[k] as f64 * t).round() as u8)
}

/// Piecewise-linear colour ramp over evenly spaced stops, `t` in [0, 1].
fn ramp(stops: &[Rgb], t: f64) -> Rgb {
    let x = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let i = (x.floor() as usize).min(stops.len() - 2);
    mix(stops[i], stops[i + 1], x - i as f64)
}
