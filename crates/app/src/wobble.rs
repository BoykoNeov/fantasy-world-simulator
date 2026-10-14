//! A gentle hand-drawn wobble on province borders.
//!
//! One smooth displacement field covers the planet, a few cells in wavelength and under half a cell
//! in reach. The map shader looks a point's province up a little way off, at the point moved by the
//! field (`warped` in `map.wgsl`), so province colours wander; each province border line is moved
//! to where that lookup puts its border, so lines and colours stay together; and picking uses the
//! same field, so the inspector names what is drawn. The wobble fades out next to the sea and along
//! big rivers: borders still meet the coastline exactly, and borders that follow a river stay on it.
//!
//! The field is stored per texel of the bordered faces (like the other layers) as an offset in that
//! face's texel coordinates, and read with bilinear interpolation written out by hand, in the same
//! order on the CPU and the GPU.

use crate::geometry::{Chain, Kind};
use sim::grid::{ab_to_uv, dir_to_face_ab, face_dir, normalize, uv_to_ab, Grid, V3, NONE};
use sim::rng::mix;
use sim::terrain::fbm;
use sim::world::{World, RIVER_MIN_FLOW};
use std::collections::VecDeque;

/// How far the wobble reaches, in cells. The bordered faces have a one-cell border, so lookups
/// moved by up to half a cell still find their four cells; and the field must change by less than
/// one cell per cell for border lines to be placed exactly (see [`unwarp`]).
pub const REACH: f64 = 0.4;
/// Wavelength of the wobble, in cells. One smooth layer: finer layers change too fast from cell to
/// cell for border lines to be placed exactly.
pub const WAVELENGTH: f64 = 4.0;
/// Most an offset may be, in texels per axis.
const CLAMP: f64 = 0.45;

/// Texel coordinates of a direction on a given face (cell `i` spans `i..i + 1`), including points
/// just past the face's edge.
pub fn texel_on_face(n: usize, face: usize, d: V3) -> (f64, f64) {
    let [x, y, z] = d;
    let (a, b) = match face {
        0 => (-z / x, y / x),
        1 => (z / -x, y / -x),
        2 => (x / y, -z / y),
        3 => (x / -y, z / -y),
        4 => (x / z, y / z),
        _ => (x / z, y / -z),
    };
    ((ab_to_uv(a) + 1.0) * 0.5 * n as f64, (ab_to_uv(b) + 1.0) * 0.5 * n as f64)
}

/// The direction at texel coordinates on a face (inverse of [`texel_on_face`]).
pub fn dir_on_face(n: usize, face: usize, tx: f64, ty: f64) -> V3 {
    let ab = |t: f64| uv_to_ab(t / n as f64 * 2.0 - 1.0);
    normalize(face_dir(face, ab(tx), ab(ty)))
}

/// The face a direction falls on, with its texel coordinates there.
pub fn face_texel(n: usize, d: V3) -> (usize, f64, f64) {
    let (face, _, _) = dir_to_face_ab(d);
    let (tx, ty) = texel_on_face(n, face, d);
    (face, tx, ty)
}

/// The wobble per texel of the bordered faces, as offsets in texels. `halo` gives the cell each
/// texel shows (`layers::halo_cells`).
pub fn halo_warp(world: &World, halo: &[u32]) -> Vec<[f32; 2]> {
    let grid = &world.grid;
    let n = grid.n;
    let m = n + 2;
    let fade = fade(world);
    let spacing = grid.mean_area().sqrt();
    let freq = 1.0 / (WAVELENGTH * spacing);
    let keys = [1u64, 2, 3].map(|k| mix(world.params.seed ^ mix(0x0b0b_1e00 + k)));
    let mut out = vec![[0f32; 2]; halo.len()];
    for face in 0..6 {
        for hj in 0..m {
            for hi in 0..m {
                let k = face * m * m + hj * m + hi;
                let cell = halo[k] as usize;
                if fade[cell] == 0.0 {
                    continue;
                }
                let (tx, ty) = (hi as f64 - 0.5, hj as f64 - 0.5);
                let d = dir_on_face(n, face, tx, ty);
                let p = [d[0] * freq, d[1] * freq, d[2] * freq];
                // A random direction, made tangent to the sphere at d.
                let v = keys.map(|key| 1.5 * fbm(p, 1, key));
                let along = v[0] * d[0] + v[1] * d[1] + v[2] * d[2];
                let s = REACH * spacing * fade[cell];
                let moved = normalize([d[0] + (v[0] - along * d[0]) * s, d[1] + (v[1] - along * d[1]) * s, d[2] + (v[2] - along * d[2]) * s]);
                let (mx, my) = texel_on_face(n, face, moved);
                out[k] = [(mx - tx).clamp(-CLAMP, CLAMP) as f32, (my - ty).clamp(-CLAMP, CLAMP) as f32];
            }
        }
    }
    out
}

/// 0 on sea and big-river cells, on their neighbours and the cells next to those (so every cell
/// corner on a coast or river is still), rising to 1 two cells further away.
fn fade(world: &World) -> Vec<f64> {
    let grid: &Grid = &world.grid;
    let far = u32::MAX;
    let mut dist = vec![far; grid.cells()];
    let mut queue = VecDeque::new();
    for (c, d) in dist.iter_mut().enumerate() {
        if world.elevation[c] < 0.0 || world.river_flow[c] >= RIVER_MIN_FLOW {
            *d = 0;
            queue.push_back(c);
        }
    }
    while let Some(c) = queue.pop_front() {
        for &m in &grid.neighbors[c] {
            let m = m as usize;
            if dist[m] == far {
                dist[m] = dist[c] + 1;
                queue.push_back(m);
            }
        }
    }
    dist.iter().map(|&d| if d == far { 1.0 } else { ((d as f64 - 2.0) / 2.0).clamp(0.0, 1.0) }).collect()
}

/// The wobble at texel coordinates on a face: the bilinear blend of the four texels around the
/// point, as `warped` in `map.wgsl` does it.
pub fn warp_at(n: usize, warp: &[[f32; 2]], face: usize, fx: f64, fy: f64) -> (f64, f64) {
    let m = n as i64 + 2;
    let (px, py) = (fx - 0.5, fy - 0.5);
    let (bx, by) = (px.floor() as i64 + 1, py.floor() as i64 + 1);
    let (tx, ty) = (px - px.floor(), py - py.floor());
    let at = |ox: i64, oy: i64| {
        let (hx, hy) = ((bx + ox).clamp(0, m - 1), (by + oy).clamp(0, m - 1));
        let w = warp[(face as i64 * m * m + hy * m + hx) as usize];
        (w[0] as f64, w[1] as f64)
    };
    let lerp = |a: (f64, f64), b: (f64, f64), t: f64| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    let top = lerp(at(0, 0), at(1, 0), tx);
    let bottom = lerp(at(0, 1), at(1, 1), tx);
    lerp(top, bottom, ty)
}

/// The province drawn at texel coordinates on a face, before the wobble: of the four cells around
/// the point, the province with the largest share of the bilinear weights (`province_at` in
/// `map.wgsl`). `NONE` where the point is mostly sea.
pub fn province_at(n: usize, halo_ids: &[u32], face: usize, fx: f64, fy: f64) -> u32 {
    let m = n as i64 + 2;
    let (px, py) = (fx - 0.5, fy - 0.5);
    let (bx, by) = (px.floor() as i64 + 1, py.floor() as i64 + 1);
    let (tx, ty) = (px - px.floor(), py - py.floor());
    let mut ids = [NONE; 4];
    let mut wt = [0.0; 4];
    let mut land = 0.0;
    for k in 0..4 {
        let (ox, oy) = (k as i64 & 1, k as i64 >> 1);
        let (hx, hy) = ((bx + ox).clamp(0, m - 1), (by + oy).clamp(0, m - 1));
        ids[k] = halo_ids[(face as i64 * m * m + hy * m + hx) as usize];
        wt[k] = (if ox == 1 { tx } else { 1.0 - tx }) * (if oy == 1 { ty } else { 1.0 - ty });
        if ids[k] != NONE {
            land += wt[k];
        }
    }
    if land < 0.5 {
        return NONE;
    }
    let mut best = (NONE, 0.0);
    for k in 0..4 {
        if ids[k] == NONE {
            continue;
        }
        let share: f64 = (0..4).filter(|&j| ids[j] == ids[k]).map(|j| wt[j]).sum();
        if share > best.1 {
            best = (ids[k], share);
        }
    }
    best.0
}

/// What the CPU needs to find the province drawn at a point, as the GPU does: the province and the
/// wobble per texel of the bordered faces of size `n`.
pub struct ProvinceLookup {
    pub n: usize,
    pub ids: Vec<u32>,
    pub warp: Vec<[f32; 2]>,
}

impl ProvinceLookup {
    /// The province drawn at a direction, or `NONE` where it is mostly sea.
    pub fn at(&self, d: V3) -> u32 {
        let (face, fx, fy) = face_texel(self.n, d);
        let (wx, wy) = warp_at(self.n, &self.warp, face, fx, fy);
        province_at(self.n, &self.ids, face, fx + wx, fy + wy)
    }
}

/// Where a point of an unwobbled border is drawn: the point x with x + wobble(x) = b, found by
/// repeating x = b − wobble(x). That settles because the wobble changes by less than one cell per
/// cell.
pub fn unwarp(n: usize, warp: &[[f32; 2]], b: V3) -> V3 {
    let (face, bx, by) = face_texel(n, b);
    let (mut x, mut y) = (bx, by);
    for _ in 0..50 {
        let (wx, wy) = warp_at(n, warp, face, x, y);
        let next = (bx - wx, by - wy);
        let step = (next.0 - x).abs().max((next.1 - y).abs());
        (x, y) = next;
        if step < 1e-10 {
            break;
        }
    }
    dir_on_face(n, face, x, y)
}

/// Move every province border line onto the wobbled border.
pub fn warp_borders(chains: &mut [Chain], n: usize, warp: &[[f32; 2]]) {
    for c in chains.iter_mut().filter(|c| c.kind == Kind::Province) {
        for p in &mut c.points {
            *p = unwarp(n, warp, *p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::world::WorldParams;

    #[test]
    fn texel_mapping_round_trips_past_the_face_edge() {
        let n = 40;
        for face in 0..6 {
            for &(tx, ty) in &[(0.5, 0.5), (20.3, 7.9), (-0.7, 12.0), (40.6, 39.9)] {
                let (x, y) = texel_on_face(n, face, dir_on_face(n, face, tx, ty));
                assert!((x - tx).abs() < 1e-9 && (y - ty).abs() < 1e-9, "face {face}: ({tx}, {ty}) -> ({x}, {y})");
            }
        }
    }

    #[test]
    fn border_points_land_where_the_lookup_puts_them_and_the_coast_stays_put() {
        let world = World::generate(&WorldParams { n: 48, provinces: 60, seed: 11, land_fraction: 0.4, radius_km: 6371.0 });
        let halo = crate::layers::halo_cells(&world.grid);
        let warp = halo_warp(&world, &halo);
        let n = world.grid.n;
        let mut moved = 0;
        for c in 0..world.grid.cells() {
            let b = world.grid.center[c];
            let x = unwarp(n, &warp, b);
            // Looking up from the drawn point lands back on the border point.
            let (face, bx, by) = face_texel(n, b);
            let (xx, xy) = texel_on_face(n, face, x);
            let (wx, wy) = warp_at(n, &warp, face, xx, xy);
            let miss = (xx + wx - bx).abs().max((xy + wy - by).abs());
            assert!(miss < 1e-6, "cell {c}: the drawn point is {miss} texels off");
            if (xx - bx).abs() + (xy - by).abs() > 0.05 {
                moved += 1;
            }
            // No wobble at sea or on the coast.
            let coastal = world.elevation[c] < 0.0 || world.grid.neighbors[c].iter().any(|&m| world.elevation[m as usize] < 0.0);
            if coastal {
                assert!((xx - bx).abs() < 1e-9 && (xy - by).abs() < 1e-9, "coastal cell {c} moved");
            }
        }
        let free = fade(&world).iter().filter(|&&f| f == 1.0).count();
        assert!(moved * 2 > free, "the wobble should move most points away from coasts and rivers: {moved} of {free}");
    }
}
