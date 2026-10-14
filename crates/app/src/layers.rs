//! The data behind the map layers, laid out for the GPU.
//!
//! Per-cell layers are 6-layer array textures, one layer per cube face, each (n + 2) × (n + 2): the
//! face plus a one-cell border copied from the neighbouring faces. With that border, smooth
//! (bilinear) sampling reads real data across face edges, so the seams don't show.
//!
//! Per-province layers are small tables indexed by province id. When the simulation advances, only
//! those tables change.

use sim::biome::Biome;
use sim::grid::{normalize, Grid, DIAGONAL_PAIRS, NONE};
use sim::sim::Sim;
use sim::world::World;

pub type Rgba = [u8; 4];

/// Width of the province tables, in provinces per row.
pub const TABLE_WIDTH: usize = 256;

/// Province table layers.
pub const TABLE_FILL: usize = 0;
pub const TABLE_POPULATION: usize = 1;
pub const TABLE_FAMINE: usize = 2;
pub const TABLE_LAYERS: usize = 3;

/// For every texel of the bordered faces, the cell it shows. Border texels show the cell just
/// across the face edge; the corner texels show the diagonal cell (at the 8 cube corners, where it
/// doesn't exist, the edge neighbour instead).
pub fn halo_cells(grid: &Grid) -> Vec<u32> {
    let n = grid.n;
    let m = n + 2;
    let mut out = vec![0u32; 6 * m * m];
    for face in 0..6 {
        for hj in 0..m {
            for hi in 0..m {
                let (i, j) = (hi as i64 - 1, hj as i64 - 1);
                let ci = i.clamp(0, n as i64 - 1) as usize;
                let cj = j.clamp(0, n as i64 - 1) as usize;
                let inner = grid.index(face, ci, cj);
                let (out_i, out_j) = (i != ci as i64, j != cj as i64);
                // Edge-neighbour slots: +i, -i, +j, -j.
                let slot_i = if i > ci as i64 { 0 } else { 1 };
                let slot_j = if j > cj as i64 { 2 } else { 3 };
                let cell = match (out_i, out_j) {
                    (false, false) => inner as u32,
                    (true, false) => grid.neighbors[inner][slot_i],
                    (false, true) => grid.neighbors[inner][slot_j],
                    (true, true) => {
                        let k = DIAGONAL_PAIRS.iter().position(|&p| p == (slot_i, slot_j)).unwrap();
                        match grid.diagonals[inner][k] {
                            NONE => grid.neighbors[inner][slot_i],
                            d => d,
                        }
                    }
                };
                out[face * m * m + hj * m + hi] = cell;
            }
        }
    }
    out
}

/// Spread per-cell values over the bordered layout.
pub fn spread<T: Copy>(halo: &[u32], values: &[T]) -> Vec<T> {
    halo.iter().map(|&c| values[c as usize]).collect()
}

/// Land colour per cell: biome, a touch of rock and snow with height, and hill shading lit from the
/// north-west. Sea cells take the colour of a neighbouring land cell, so smooth sampling at the
/// coast doesn't pull blue into the land. Alpha is 255 on land, 0 at sea: sampled smoothly, its
/// halfway line is the coastline, and it runs through the middle of each land/sea cell side, as the
/// drawn coastline does.
pub fn terrain(world: &World) -> Vec<Rgba> {
    let g = &world.grid;
    let step = g.mean_area().sqrt();
    let height_at = |c: usize, dir: [f32; 3], sign: f64| -> f32 {
        let d = g.center[c];
        let moved = normalize([d[0] + dir[0] as f64 * step * sign, d[1] + dir[1] as f64 * step * sign, d[2] + dir[2] as f64 * step * sign]);
        world.elevation[g.dir_to_cell(moved)].max(0.0)
    };
    let land: Vec<[u8; 3]> = (0..g.cells())
        .map(|c| {
            let h = world.elevation[c];
            if h < 0.0 {
                return [0, 0, 0];
            }
            let base = biome_color(world.biome[c]);
            let high = mix(base, [168, 152, 132], ((h - 1500.0) / 3000.0).clamp(0.0, 0.7) as f64);
            let gx = (height_at(c, g.east[c], 1.0) - height_at(c, g.east[c], -1.0)) * 0.5;
            let gy = (height_at(c, g.north[c], 1.0) - height_at(c, g.north[c], -1.0)) * 0.5;
            let shade = (1.0 + 0.0008 * (gx - gy) as f64).clamp(0.62, 1.35);
            high.map(|v| (v as f64 * shade).min(255.0) as u8)
        })
        .collect();
    (0..g.cells())
        .map(|c| {
            if world.elevation[c] >= 0.0 {
                let [r, gg, b] = land[c];
                return [r, gg, b, 255];
            }
            let mut sum = [0u32; 3];
            let mut k = 0;
            for m in g.neighbors8(c).filter(|&m| world.elevation[m] >= 0.0) {
                for i in 0..3 {
                    sum[i] += land[m][i] as u32;
                }
                k += 1;
            }
            if k == 0 {
                return [200, 190, 150, 0];
            }
            [(sum[0] / k) as u8, (sum[1] / k) as u8, (sum[2] / k) as u8, 0]
        })
        .collect()
}

/// Other per-cell fields, scaled to 0..255: sea depth (0 on land), temperature, rainfall.
pub fn fields(world: &World) -> Vec<Rgba> {
    (0..world.grid.cells())
        .map(|c| {
            let e = world.elevation[c];
            let depth = if e < 0.0 { (-e / 6000.0).clamp(0.0, 1.0).sqrt() } else { 0.0 };
            let temp = ((world.temperature[c] + 30.0) / 65.0).clamp(0.0, 1.0);
            let rain = (world.rainfall[c] / 2500.0).clamp(0.0, 1.0);
            [to_u8(depth), to_u8(temp), to_u8(rain), 255]
        })
        .collect()
}

fn to_u8(v: f32) -> u8 {
    (v * 255.0).round() as u8
}

/// Rows in the province tables for `count` provinces (at least one).
pub fn table_rows(count: usize) -> usize {
    count.div_ceil(TABLE_WIDTH).max(1)
}

/// Province tables, layer after layer, each `TABLE_WIDTH` × `table_rows` texels. Alpha scales how
/// strongly the colour covers the map (famine is clear where nobody starved).
pub fn province_tables(sim: &Sim) -> Vec<Rgba> {
    let w = &sim.world;
    let count = w.provinces.count;
    let size = TABLE_WIDTH * table_rows(count);
    let mut out = vec![[0u8; 4]; size * TABLE_LAYERS];
    let people = sim.pops.province_population(count);
    for p in 0..count {
        let [r, g, b] = province_color(p as u32, w.params.seed);
        out[TABLE_FILL * size + p] = [r, g, b, 255];

        let density = people[p] as f64 / w.provinces.area_km2[p].max(1e-9);
        let t = ((density.max(1e-3).log10() + 2.0) / 4.0).clamp(0.0, 1.0); // 0.01 .. 100 per km²
        let [r, g, b] = ramp(&[[250, 245, 210], [240, 180, 90], [200, 70, 40], [90, 20, 30]], t);
        out[TABLE_POPULATION * size + p] = [r, g, b, 255];

        let starved = sim.pops.starved_this_year[p];
        out[TABLE_FAMINE * size + p] = if starved == 0 {
            [0, 0, 0, 0]
        } else {
            let share = starved as f64 / (people[p] + starved) as f64;
            let [r, g, b] = ramp(&[[250, 200, 120], [230, 110, 50], [160, 15, 15]], (share * 5.0).clamp(0.0, 1.0));
            [r, g, b, 255]
        };
    }
    out
}

fn biome_color(b: Biome) -> [u8; 3] {
    // Muted, slightly warm: printed-map greens and ochres rather than satellite colours.
    match b {
        Biome::Ocean => [70, 110, 140],
        Biome::IceCap => [238, 240, 240],
        Biome::Tundra => [176, 172, 146],
        Biome::BorealForest => [92, 118, 82],
        Biome::TemperateForest => [114, 146, 84],
        Biome::Grassland => [172, 182, 110],
        Biome::Desert => [222, 198, 146],
        Biome::Savanna => [196, 178, 108],
        Biome::TropicalForest => [74, 128, 70],
        Biome::Alpine => [156, 146, 132],
    }
}

/// A stable, distinct-ish colour per province.
fn province_color(p: u32, seed: u64) -> [u8; 3] {
    let h = sim::rng::mix(seed ^ (p as u64).wrapping_mul(0x9E37_79B9));
    let hue = (p as f64 * 0.618_033_988_75).fract();
    let light = 0.5 + (h % 1000) as f64 / 1000.0 * 0.18;
    hsl(hue, 0.5, light)
}

fn hsl(h: f64, s: f64, l: f64) -> [u8; 3] {
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

fn mix(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    [0, 1, 2].map(|k| (a[k] as f64 * (1.0 - t) + b[k] as f64 * t).round() as u8)
}

/// Piecewise-linear colour ramp over evenly spaced stops, `t` in [0, 1].
fn ramp(stops: &[[u8; 3]], t: f64) -> [u8; 3] {
    let x = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let i = (x.floor() as usize).min(stops.len() - 2);
    mix(stops[i], stops[i + 1], x - i as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::grid::{face_dir, uv_to_ab};

    /// Each border texel shows the cell found by stepping just past the face edge on the sphere.
    #[test]
    fn border_texels_show_the_cell_across_the_edge() {
        for n in [5, 8] {
            let g = Grid::new(n);
            let halo = halo_cells(&g);
            let m = n + 2;
            for face in 0..6 {
                for hj in 0..m {
                    for hi in 0..m {
                        let corner = (hi == 0 || hi == m - 1) && (hj == 0 || hj == m - 1);
                        if corner {
                            continue;
                        }
                        // Texel centre in equi-angular coordinates, nudged just inside the next face.
                        let uv = |h: usize| ((h as f64 - 0.5) / n as f64 * 2.0 - 1.0).clamp(-1.0 - 1e-6, 1.0 + 1e-6);
                        let d = normalize(face_dir(face, uv_to_ab(uv(hi)), uv_to_ab(uv(hj))));
                        let expected = g.dir_to_cell(d) as u32;
                        let got = halo[face * m * m + hj * m + hi];
                        let ok = got == expected || g.neighbors[got as usize].contains(&expected);
                        assert!(ok, "n={n} face {face} texel ({hi}, {hj}): {got} vs {expected}");
                        if (1..=n).contains(&hi) && (1..=n).contains(&hj) {
                            assert_eq!(got, expected);
                        }
                    }
                }
            }
        }
    }
}
