//! Rivers v0: every land cell drains to one downstream cell, and yearly runoff is added up along
//! those links. Water is counted in whole units, so conservation is exact.
//!
//! The drainage tree comes from a priority flood out of the sea: land is reached lowest-first, and
//! each cell drains to the cell the flood reached it from. Pits and lakes are filled in implicitly,
//! so every drop reaches the sea (or, on land with no sea at all, the lowest point of that land).

use crate::grid::{Grid, NONE};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub struct Drainage {
    /// Downstream cell, or [`NONE`] for a sink (sea, or the low point of a landmass with no sea).
    pub receiver: Vec<u32>,
    /// Every non-sink cell, each listed after its receiver.
    pub order: Vec<u32>,
}

/// f32 → u32 key with the same ordering (for the priority queue).
fn order_key(h: f32) -> u32 {
    let b = h.to_bits();
    if b & 0x8000_0000 != 0 { !b } else { b | 0x8000_0000 }
}

pub fn drainage(grid: &Grid, elevation: &[f32]) -> Drainage {
    let cells = grid.cells();
    let mut receiver = vec![NONE; cells];
    let mut visited = vec![false; cells];
    let mut order = Vec::with_capacity(cells);
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();

    let flood = |heap: &mut BinaryHeap<Reverse<(u32, u32)>>,
                 visited: &mut [bool],
                 receiver: &mut [u32],
                 order: &mut Vec<u32>| {
        while let Some(Reverse((level, c))) = heap.pop() {
            for m in grid.neighbors8(c as usize) {
                if !visited[m] {
                    visited[m] = true;
                    receiver[m] = c;
                    order.push(m as u32);
                    heap.push(Reverse((level.max(order_key(elevation[m])), m as u32)));
                }
            }
        }
    };

    for c in 0..cells {
        if elevation[c] < 0.0 {
            visited[c] = true;
            heap.push(Reverse((order_key(0.0), c as u32)));
        }
    }
    flood(&mut heap, &mut visited, &mut receiver, &mut order);

    // Land with no route to any sea: its lowest cell becomes a sink, lowest landmass first.
    let mut stranded: Vec<u32> = (0..cells as u32).filter(|&c| !visited[c as usize]).collect();
    stranded.sort_by_key(|&c| (order_key(elevation[c as usize]), c));
    for c in stranded {
        if !visited[c as usize] {
            visited[c as usize] = true;
            heap.push(Reverse((order_key(elevation[c as usize]), c)));
            flood(&mut heap, &mut visited, &mut receiver, &mut order);
        }
    }
    Drainage { receiver, order }
}

/// Total water passing through each cell: its own input plus everything upstream. At a sink this is
/// everything that drains into it.
pub fn accumulate(d: &Drainage, input: &[u64]) -> Vec<u64> {
    let mut flow = input.to_vec();
    for &c in d.order.iter().rev() {
        let c = c as usize;
        flow[d.receiver[c] as usize] += flow[c];
    }
    flow
}

/// Yearly runoff per cell in thousands of cubic metres (1 mm of water over 1 km²): rainfall minus a
/// simple evaporation that grows with temperature. Sea cells contribute nothing.
pub fn runoff(grid: &Grid, elevation: &[f32], temperature: &[f32], rainfall: &[f32], radius_km: f64) -> Vec<u64> {
    let km2_per_sr = radius_km * radius_km;
    (0..grid.cells())
        .into_par_iter()
        .map(|c| {
            if elevation[c] < 0.0 {
                return 0;
            }
            let evaporation = 200.0 + 25.0 * temperature[c].max(0.0) as f64;
            let mm = (rainfall[c] as f64 - evaporation).max(0.0);
            (mm * grid.area[c] * km2_per_sr).round() as u64
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::dot;

    fn check_tree(g: &Grid, d: &Drainage) {
        let mut seen = vec![false; g.cells()];
        for c in 0..g.cells() {
            if d.receiver[c] == NONE {
                seen[c] = true;
            }
        }
        for &c in &d.order {
            let r = d.receiver[c as usize];
            assert!(g.neighbors8(c as usize).any(|m| m as u32 == r), "{c} drains to a non-neighbour");
            assert!(seen[r as usize], "{c} listed before its receiver");
            assert!(!seen[c as usize], "{c} listed twice");
            seen[c as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "some cell neither sink nor listed");
    }

    /// A required test from the design: water routed across a face seam is conserved exactly.
    /// All land slopes down toward one sea cell in the middle of face 4; rain falls only on face 0,
    /// so every unit must cross from face 0 onto another face to arrive.
    #[test]
    fn river_crosses_a_seam_and_conserves_water() {
        for n in [16, 17] {
            let g = Grid::new(n);
            let sea = g.dir_to_cell([0.0, 0.0, 1.0]);
            assert_eq!(g.face_of(sea), 4);
            let target = g.center[sea];
            let elevation: Vec<f32> = (0..g.cells())
                .map(|c| if c == sea { -1.0 } else { (dot(g.center[c], target).clamp(-1.0, 1.0).acos() * 1000.0) as f32 })
                .collect();
            let d = drainage(&g, &elevation);
            check_tree(&g, &d);

            let input: Vec<u64> = (0..g.cells()).map(|c| if g.face_of(c) == 0 { 1000 } else { 0 }).collect();
            let total: u64 = input.iter().sum();
            let flow = accumulate(&d, &input);
            assert_eq!(flow[sea], total, "n={n}: water lost or created");

            let r = |c: usize| d.receiver[c] as usize;
            let out: u64 = d.order.iter().map(|&c| c as usize).filter(|&c| g.face_of(c) == 0 && g.face_of(r(c)) != 0).map(|c| flow[c]).sum();
            let back: u64 = d.order.iter().map(|&c| c as usize).filter(|&c| g.face_of(c) != 0 && g.face_of(r(c)) == 0).map(|c| flow[c]).sum();
            assert!(out > 0, "n={n}: nothing crossed the seam");
            assert_eq!(out - back, total, "n={n}: net flow across the face 0 seam");
        }
    }

    #[test]
    fn landlocked_basins_still_drain_somewhere() {
        let g = Grid::new(12);
        // No sea anywhere: one landmass whose lowest point becomes the sink.
        let elevation: Vec<f32> = (0..g.cells()).map(|c| 100.0 + (c % 7) as f32 * 3.0).collect();
        let d = drainage(&g, &elevation);
        check_tree(&g, &d);
        let sinks = d.receiver.iter().filter(|&&r| r == NONE).count();
        assert_eq!(sinks, 1);
        let flow = accumulate(&d, &vec![1; g.cells()]);
        let sink = d.receiver.iter().position(|&r| r == NONE).unwrap();
        assert_eq!(flow[sink], g.cells() as u64);
    }
}
