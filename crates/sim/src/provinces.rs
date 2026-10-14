//! Provinces v0: land cells clustered into provinces, grown organically from seed cells.
//!
//! Seeds are picked with a preference for fertile land, and growth is slower through fertile land,
//! so farmland is shared out more evenly than area: deserts and tundra make big provinces, river
//! valleys small ones. Growth also slows on steep slopes and when crossing a big river, so borders
//! tend to follow ridges and rivers. How hard the land is to cross also varies smoothly from place
//! to place, at about a province's size and finer: growth bends around rough country, so borders
//! meander instead of running straight along the grid's directions. Unreached land (islands
//! without a seed) gets new seeds of its own.

use crate::grid::{Grid, NONE, V3};
use crate::module::{Manifest, Module as SimModule, Slot};
use crate::rng::{draw, purpose, unit, Module};
use crate::terrain::fbm;
use crate::world::{World, RIVER_MIN_FLOW};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Provinces v0: organic growth from fertile seeds.
pub struct ProvinceGrowth;

static V0: Manifest = Manifest::generation(Slot::Provinces, "growth", crate::fields::Tier::Province,
    &["cell.elevation", "cell.food_yield", "cell.river_flow"],
    &["cell.province", "province.cells", "province.area_km2", "province.center", "province.lat", "province.food_capacity"],
);

impl SimModule for ProvinceGrowth {
    fn manifest(&self) -> &'static Manifest {
        &V0
    }
    fn generate(&self, w: &mut World) {
        let p = &w.params;
        let of_cell = generate(&w.grid, &w.elevation, &w.food_per_cell, &w.river_flow, p.provinces, p.seed);
        w.provinces = aggregate(&w.grid, of_cell, &w.food_per_cell, p.radius_km);
    }
}

#[derive(Clone, Debug, Default)]
pub struct Provinces {
    /// Province per cell, [`NONE`] for sea.
    pub of_cell: Vec<u32>,
    pub count: usize,
    pub cells: Vec<u32>,
    pub area_km2: Vec<f64>,
    /// Unit vector to the area-weighted centre.
    pub center: Vec<V3>,
    /// Latitude of `center`, radians.
    pub lat: Vec<f32>,
    /// Food the province's land yields in an average year, in rations.
    pub food_capacity: Vec<u64>,
}

/// How strongly the smooth roughness field bends growth: the cost of a step is scaled by
/// `exp(ROUGHNESS * noise)`, with the noise roughly in [-1, 1].
const ROUGHNESS: f64 = 1.6;
/// Extra cost of stepping onto or off a big river, against a typical step of about 1,500.
const RIVER_CROSSING: u64 = 6_000;

/// `fertility[c]` is food per year in a cell (any non-negative scale); sea is `elevation < 0`.
/// `river_flow` marks the big rivers that make natural borders.
pub fn generate(grid: &Grid, elevation: &[f32], fertility: &[f64], river_flow: &[u64], target: usize, seed: u64) -> Vec<u32> {
    let cells = grid.cells();
    let mut of_cell = vec![NONE; cells];
    let land: Vec<usize> = (0..cells).filter(|&c| elevation[c] >= 0.0).collect();
    if land.is_empty() || target == 0 {
        return of_cell;
    }

    // Weighted sampling without replacement: the `target` smallest `-ln(u) / w` win.
    let max_f = land.iter().map(|&c| fertility[c]).fold(0.0, f64::max).max(1e-12);
    let weight = |c: usize| 0.15 + fertility[c] / max_f;
    let mut ranked: Vec<(f64, usize)> = land
        .iter()
        .map(|&c| (-(1.0 - unit(seed, Module::Provinces, 0, c as u64, purpose("seed"))).ln() / weight(c), c))
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut next_id = 0u32;
    let mut heap: BinaryHeap<Reverse<(u64, u32, u32)>> = BinaryHeap::new();
    let mut cost = vec![u64::MAX; cells];
    for &(_, c) in ranked.iter().take(target) {
        cost[c] = 0;
        heap.push(Reverse((0, c as u32, next_id)));
        next_id += 1;
    }
    // Roughness, smooth over the sphere: one layer about a province across and one about a
    // quarter of that, at equal strength, so borders wander at both scales.
    let land_area = 4.0 * std::f64::consts::PI * land.len() as f64 / cells as f64;
    let freq = 1.0 / (land_area / target as f64).sqrt();
    let broad_key = draw(seed, Module::Provinces, 0, 0, purpose("roughness"));
    let fine_key = draw(seed, Module::Provinces, 0, 0, purpose("fine roughness"));
    let rough: Vec<f64> = grid
        .center
        .par_iter()
        .map(|&d| {
            let at = |f: f64| [d[0] * f, d[1] * f, d[2] * f];
            (ROUGHNESS * (fbm(at(freq), 2, broad_key) + fbm(at(4.0 * freq), 2, fine_key))).exp()
        })
        .collect();
    let spacing = grid.mean_area().sqrt();
    let river = |c: usize| river_flow[c] >= RIVER_MIN_FLOW;
    // Steps go to all eight neighbours and cost their true length, so growth fronts are round and
    // borders can run at any angle (four-way steps make borders run along the grid's axes and
    // diagonals).
    let step_cost = |from: usize, to: usize| -> u64 {
        let len = crate::grid::dot(grid.center[from], grid.center[to]).clamp(-1.0, 1.0).acos() / spacing;
        let jitter = draw(seed, Module::Provinces, 0, (from as u64) << 32 | to as u64, purpose("step")) % 1000;
        let slope = (elevation[to] - elevation[from]).abs() as f64;
        let fertile = weight(to) * 1500.0;
        let crossing = if river(from) != river(to) { RIVER_CROSSING } else { 0 };
        ((500.0 + fertile + slope * 6.0) * rough[to] * len) as u64 + jitter / 2 + crossing
    };
    let grow = |heap: &mut BinaryHeap<Reverse<(u64, u32, u32)>>, of_cell: &mut [u32], cost: &mut [u64]| {
        while let Some(Reverse((c, cell, id))) = heap.pop() {
            let cell = cell as usize;
            if of_cell[cell] != NONE {
                continue;
            }
            of_cell[cell] = id;
            for nb in grid.neighbors8(cell) {
                // A diagonal step needs land on at least one side of the corner it passes, so
                // growth never jumps a strait.
                let diagonal = !grid.neighbors[cell].contains(&(nb as u32));
                let over_land = || grid.neighbors[cell].iter().any(|&m| elevation[m as usize] >= 0.0 && grid.neighbors[nb].contains(&m));
                if elevation[nb] >= 0.0 && of_cell[nb] == NONE && (!diagonal || over_land()) {
                    let nc = c + step_cost(cell, nb);
                    if nc < cost[nb] {
                        cost[nb] = nc;
                        heap.push(Reverse((nc, nb as u32, id)));
                    }
                }
            }
        }
    };
    grow(&mut heap, &mut of_cell, &mut cost);
    for &c in &land {
        if of_cell[c] == NONE {
            cost[c] = 0;
            heap.push(Reverse((0, c as u32, next_id)));
            next_id += 1;
            grow(&mut heap, &mut of_cell, &mut cost);
        }
    }
    join_pieces(grid, &mut of_cell, next_id as usize);
    of_cell
}

/// Diagonal steps can leave part of a province touching the rest only at a corner. Every piece but
/// each province's largest goes to the neighbouring province it shares the most cell sides with
/// (ties to the lower id), until every province is one piece joined by cell sides. A piece with no
/// land neighbour (a one-province island) stays.
fn join_pieces(grid: &Grid, of_cell: &mut [u32], count: usize) {
    loop {
        // Pieces, by flood over edge neighbours, in cell order.
        let mut piece = vec![u32::MAX; of_cell.len()];
        let mut pieces: Vec<(u32, Vec<usize>)> = Vec::new();
        for start in 0..of_cell.len() {
            if of_cell[start] == NONE || piece[start] != u32::MAX {
                continue;
            }
            let id = of_cell[start];
            let k = pieces.len() as u32;
            let mut members = vec![start];
            piece[start] = k;
            let mut at = 0;
            while at < members.len() {
                let c = members[at];
                at += 1;
                for &m in &grid.neighbors[c] {
                    let m = m as usize;
                    if of_cell[m] == id && piece[m] == u32::MAX {
                        piece[m] = k;
                        members.push(m);
                    }
                }
            }
            pieces.push((id, members));
        }
        let mut largest = vec![usize::MAX; count];
        for (k, (id, members)) in pieces.iter().enumerate() {
            let best = largest[*id as usize];
            if best == usize::MAX || members.len() > pieces[best].1.len() {
                largest[*id as usize] = k;
            }
        }
        let mut moved = false;
        for (k, (id, members)) in pieces.iter().enumerate() {
            if largest[*id as usize] == k {
                continue;
            }
            let mut shared: Vec<(u32, u32)> = Vec::new();
            for &c in members {
                for &m in &grid.neighbors[c] {
                    let other = of_cell[m as usize];
                    if other != NONE && other != *id {
                        match shared.iter_mut().find(|s| s.0 == other) {
                            Some(s) => s.1 += 1,
                            None => shared.push((other, 1)),
                        }
                    }
                }
            }
            if let Some(&(to, _)) = shared.iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))) {
                for &c in members {
                    of_cell[c] = to;
                }
                moved = true;
            }
        }
        if !moved {
            return;
        }
    }
}

/// Per-province aggregates of their cells. Sums run in cell order, so they are the same every run.
pub fn aggregate(grid: &Grid, of_cell: Vec<u32>, food_per_cell: &[f64], radius_km: f64) -> Provinces {
    let count = of_cell.iter().filter(|&&p| p != NONE).map(|&p| p as usize + 1).max().unwrap_or(0);
    let km2_per_sr = radius_km * radius_km;
    let mut cells = vec![0u32; count];
    let mut area_km2 = vec![0f64; count];
    let mut sum = vec![[0f64; 3]; count];
    let mut food = vec![0f64; count];
    for (c, &p) in of_cell.iter().enumerate() {
        if p == NONE {
            continue;
        }
        let p = p as usize;
        let a = grid.area[c] * km2_per_sr;
        cells[p] += 1;
        area_km2[p] += a;
        food[p] += food_per_cell[c];
        for k in 0..3 {
            sum[p][k] += grid.center[c][k] * a;
        }
    }
    let center: Vec<V3> = sum.into_iter().map(crate::grid::normalize).collect();
    let lat = center.iter().map(|&d| crate::grid::dir_to_lonlat(d).1 as f32).collect();
    let food_capacity = food.iter().map(|&f| f.round() as u64).collect();
    Provinces { of_cell, count, cells, area_km2, center, lat, food_capacity }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_land_cell_gets_a_province_and_provinces_are_connected() {
        let g = Grid::new(24);
        let elevation = crate::terrain::generate(&g, 5, 0.4);
        let fertility: Vec<f64> = (0..g.cells()).map(|c| if elevation[c] >= 0.0 { (c % 5) as f64 } else { 0.0 }).collect();
        let of_cell = generate(&g, &elevation, &fertility, &vec![0; g.cells()], 60, 5);
        for c in 0..g.cells() {
            assert_eq!(elevation[c] >= 0.0, of_cell[c] != NONE, "cell {c}");
        }
        let p = aggregate(&g, of_cell, &fertility, 6371.0);
        assert!(p.count >= 60, "{}", p.count);
        // Each province is one connected piece: a flood from any of its cells reaches all of them.
        for id in 0..p.count as u32 {
            let members: Vec<usize> = (0..g.cells()).filter(|&c| p.of_cell[c] == id).collect();
            let mut seen = std::collections::HashSet::from([members[0]]);
            let mut stack = vec![members[0]];
            while let Some(c) = stack.pop() {
                for &m in &g.neighbors[c] {
                    if p.of_cell[m as usize] == id && seen.insert(m as usize) {
                        stack.push(m as usize);
                    }
                }
            }
            assert_eq!(seen.len(), members.len(), "province {id} is in pieces");
            assert_eq!(p.cells[id as usize] as usize, members.len());
        }
    }

    #[test]
    fn farmland_is_shared_more_evenly_than_area() {
        let w = crate::world::World::generate(&crate::world::WorldParams { n: 64, provinces: 300, seed: 3, land_fraction: 0.36, radius_km: 6371.0 });
        let spread = |v: Vec<f64>| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt() / m
        };
        let area = spread(w.provinces.area_km2.clone());
        let food = spread(w.provinces.food_capacity.iter().map(|&f| f as f64).collect());
        // Small worlds share farmland only a little more evenly (the step-2 growth: 0.89 against 1.00).
        assert!(food < area, "farmland spread {food:.2}, area spread {area:.2}");
    }
}
