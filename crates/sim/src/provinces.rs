//! Provinces v0: land cells clustered into provinces, grown organically from seed cells.
//!
//! Seeds are picked with a preference for fertile land, and growth is slower through fertile land,
//! so farmland is shared out more evenly than area: deserts and tundra make big provinces, river
//! valleys small ones. Growth also slows on steep slopes, so borders tend to follow mountains.
//! Unreached land (islands without a seed) gets new seeds of its own.

use crate::grid::{Grid, NONE, V3};
use crate::module::{Manifest, Module as SimModule, Slot};
use crate::world::World;
use crate::rng::{draw, purpose, unit, Module};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Provinces v0: organic growth from fertile seeds.
pub struct ProvinceGrowth;

static V0: Manifest = Manifest::generation(
    Slot::Provinces,
    "growth",
    &["cell.elevation", "cell.food_yield"],
    &["cell.province", "province.cells", "province.area_km2", "province.center", "province.lat", "province.food_capacity"],
);

impl SimModule for ProvinceGrowth {
    fn manifest(&self) -> &'static Manifest {
        &V0
    }
    fn generate(&self, w: &mut World) {
        let p = &w.params;
        let of_cell = generate(&w.grid, &w.elevation, &w.food_per_cell, p.provinces, p.seed);
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

/// `fertility[c]` is food per year in a cell (any non-negative scale); sea is `elevation < 0`.
pub fn generate(grid: &Grid, elevation: &[f32], fertility: &[f64], target: usize, seed: u64) -> Vec<u32> {
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
    let step_cost = |from: usize, to: usize| -> u64 {
        let jitter = draw(seed, Module::Provinces, 0, (from as u64) << 32 | to as u64, purpose("step")) % 1000;
        let slope = (elevation[to] - elevation[from]).abs() as u64;
        let fertile = (weight(to) * 1500.0) as u64;
        500 + fertile + jitter * 2 + slope * 3
    };
    let grow = |heap: &mut BinaryHeap<Reverse<(u64, u32, u32)>>, of_cell: &mut [u32], cost: &mut [u64]| {
        while let Some(Reverse((c, cell, id))) = heap.pop() {
            let cell = cell as usize;
            if of_cell[cell] != NONE {
                continue;
            }
            of_cell[cell] = id;
            for &nb in &grid.neighbors[cell] {
                let nb = nb as usize;
                if elevation[nb] >= 0.0 && of_cell[nb] == NONE {
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
    of_cell
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
        let of_cell = generate(&g, &elevation, &fertility, 60, 5);
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
}
