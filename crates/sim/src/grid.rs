//! Equi-angular cube-sphere grid: six N×N faces folded onto the unit sphere.
//!
//! Cell index = face * n * n + j * n + i, with `i` along the face's `a` axis and `j` along `b`.
//! This face/axis convention is the one the renderer trial's shader (`spikes/shaders/map_common.wgsl`)
//! uses; the map view will mirror it, so keep them in sync.

use rayon::prelude::*;
use std::f64::consts::{FRAC_PI_4, PI};

pub const FACES: usize = 6;

/// Marks a missing diagonal: the three cells at each cube corner have 7 diagonals, not 8.
pub const NONE: u32 = u32::MAX;

pub type V3 = [f64; 3];

/// Point on the cube surface for face-local cube coordinates `a`, `b` in [-1, 1] (not normalised).
pub fn face_dir(face: usize, a: f64, b: f64) -> V3 {
    match face {
        0 => [1.0, b, -a],
        1 => [-1.0, b, a],
        2 => [a, 1.0, -b],
        3 => [a, -1.0, b],
        4 => [a, b, 1.0],
        _ => [-a, b, -1.0],
    }
}

/// Inverse of [`face_dir`]: which face a direction hits, and its cube coordinates there.
pub fn dir_to_face_ab(d: V3) -> (usize, f64, f64) {
    let [x, y, z] = d;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax >= ay && ax >= az {
        if x > 0.0 { (0, -z / x, y / x) } else { (1, z / -x, y / -x) }
    } else if ay >= az {
        if y > 0.0 { (2, x / y, -z / y) } else { (3, x / -y, z / -y) }
    } else if z > 0.0 {
        (4, x / z, y / z)
    } else {
        (5, x / z, y / -z)
    }
}

/// Cube coordinate -> equi-angular coordinate in [-1, 1].
pub fn ab_to_uv(a: f64) -> f64 {
    a.atan() / FRAC_PI_4
}

/// Equi-angular coordinate -> cube coordinate.
pub fn uv_to_ab(u: f64) -> f64 {
    (u * FRAC_PI_4).tan()
}

pub fn normalize(v: V3) -> V3 {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// (longitude, latitude) in radians. Longitude 0 faces +Z, north pole is +Y.
pub fn dir_to_lonlat(d: V3) -> (f64, f64) {
    (d[0].atan2(d[2]), d[1].clamp(-1.0, 1.0).asin())
}

pub fn lonlat_to_dir(lon: f64, lat: f64) -> V3 {
    let c = lat.cos();
    [c * lon.sin(), lat.sin(), c * lon.cos()]
}

/// Wrap a longitude into [-PI, PI).
pub fn wrap_lon(lon: f64) -> f64 {
    (lon + PI).rem_euclid(2.0 * PI) - PI
}

/// Edge-neighbour order in [`Grid::neighbors`]: +i, -i, +j, -j.
pub const STEPS: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Diagonal order in [`Grid::diagonals`], as pairs of edge-neighbour slots: (+i,+j), (+i,-j),
/// (-i,+j), (-i,-j).
pub const DIAGONAL_PAIRS: [(usize, usize); 4] = [(0, 2), (0, 3), (1, 2), (1, 3)];

pub struct Grid {
    pub n: usize,
    /// Four edge neighbours per cell, crossing face seams through the sphere.
    pub neighbors: Vec<[u32; 4]>,
    /// Four diagonal neighbours per cell, [`NONE`] where a cube corner removes one.
    pub diagonals: Vec<[u32; 4]>,
    /// Unit vector to the cell centre.
    pub center: Vec<V3>,
    /// Longitude and latitude of the centre, radians.
    pub lon: Vec<f32>,
    pub lat: Vec<f32>,
    /// Spherical area, steradians (the whole sphere is 4π).
    pub area: Vec<f64>,
    /// Local unit east and north directions at the centre, for vector fields (wind, currents).
    pub east: Vec<[f32; 3]>,
    pub north: Vec<[f32; 3]>,
}

impl Grid {
    pub fn new(n: usize) -> Self {
        assert!(n >= 2, "cube face size must be at least 2");
        let cells = FACES * n * n;
        let shape = Shape { n };

        let center: Vec<V3> = (0..cells).into_par_iter().map(|c| shape.center_dir(c)).collect();
        let neighbors: Vec<[u32; 4]> = (0..cells).into_par_iter().map(|c| shape.edge_neighbors(c)).collect();
        let diagonals: Vec<[u32; 4]> = (0..cells).into_par_iter().map(|c| diagonals_of(c, &neighbors)).collect();
        let area: Vec<f64> = (0..cells).into_par_iter().map(|c| shape.cell_area(c)).collect();
        let (lon, lat): (Vec<f32>, Vec<f32>) = center
            .par_iter()
            .map(|&d| {
                let (lo, la) = dir_to_lonlat(d);
                (lo as f32, la as f32)
            })
            .unzip();
        let (east, north): (Vec<[f32; 3]>, Vec<[f32; 3]>) = center.par_iter().map(|&d| local_frame(d)).unzip();

        Grid { n, neighbors, diagonals, center, lon, lat, area, east, north }
    }

    pub fn cells(&self) -> usize {
        FACES * self.n * self.n
    }

    pub fn index(&self, face: usize, i: usize, j: usize) -> usize {
        Shape { n: self.n }.index(face, i, j)
    }

    pub fn coords(&self, idx: usize) -> (usize, usize, usize) {
        Shape { n: self.n }.coords(idx)
    }

    pub fn face_of(&self, idx: usize) -> usize {
        idx / (self.n * self.n)
    }

    pub fn dir_to_cell(&self, d: V3) -> usize {
        Shape { n: self.n }.dir_to_cell(d)
    }

    /// Direction of a cell corner; `ci`, `cj` run 0..=n. A corner on a face edge is shared with the
    /// next face, which gives the same point (to rounding) under its own coordinates.
    pub fn corner_dir(&self, face: usize, ci: usize, cj: usize) -> V3 {
        Shape { n: self.n }.corner_dir(face, ci, cj)
    }

    /// Mean cell area in steradians.
    pub fn mean_area(&self) -> f64 {
        4.0 * PI / self.cells() as f64
    }

    /// One step of exactly conserved spreading over edge neighbours: every cell hands an eighth of
    /// its amount (rounded down) to each of its 4 edge neighbours and keeps the rest.
    pub fn diffuse_step(&self, values: &[u64], out: &mut [u64]) {
        out.par_iter_mut().enumerate().for_each(|(c, o)| {
            let kept = values[c] - 4 * (values[c] / 8);
            *o = kept + self.neighbors[c].iter().map(|&m| values[m as usize] / 8).sum::<u64>();
        });
    }

    /// The up to 8 neighbours of a cell: 4 edge neighbours then the existing diagonals.
    pub fn neighbors8(&self, idx: usize) -> impl Iterator<Item = usize> + '_ {
        self.neighbors[idx]
            .iter()
            .chain(self.diagonals[idx].iter())
            .filter(|&&m| m != NONE)
            .map(|&m| m as usize)
    }
}

/// Index arithmetic and geometry that only need N; used while the tables are being built.
#[derive(Clone, Copy)]
struct Shape {
    n: usize,
}

impl Shape {
    fn index(self, face: usize, i: usize, j: usize) -> usize {
        face * self.n * self.n + j * self.n + i
    }

    fn coords(self, idx: usize) -> (usize, usize, usize) {
        let nn = self.n * self.n;
        let face = idx / nn;
        let r = idx % nn;
        (face, r % self.n, r / self.n)
    }

    /// Equi-angular coordinate of a cell centre along one axis.
    fn cell_uv(self, i: usize) -> f64 {
        (i as f64 + 0.5) / self.n as f64 * 2.0 - 1.0
    }

    fn center_dir(self, idx: usize) -> V3 {
        let (face, i, j) = self.coords(idx);
        normalize(face_dir(face, uv_to_ab(self.cell_uv(i)), uv_to_ab(self.cell_uv(j))))
    }

    /// Direction of a cell corner; `ci`, `cj` run 0..=n.
    fn corner_dir(self, face: usize, ci: usize, cj: usize) -> V3 {
        let u = ci as f64 / self.n as f64 * 2.0 - 1.0;
        let v = cj as f64 / self.n as f64 * 2.0 - 1.0;
        normalize(face_dir(face, uv_to_ab(u), uv_to_ab(v)))
    }

    fn dir_to_cell(self, d: V3) -> usize {
        let (face, a, b) = dir_to_face_ab(d);
        let n = self.n as f64;
        let to_i = |c: f64| -> usize { (((ab_to_uv(c) + 1.0) * 0.5 * n).floor().max(0.0) as usize).min(self.n - 1) };
        self.index(face, to_i(a), to_i(b))
    }

    fn edge_neighbors(self, idx: usize) -> [u32; 4] {
        let n = self.n;
        let (face, i, j) = self.coords(idx);
        let mut nb = [0u32; 4];
        for (k, (di, dj)) in STEPS.iter().enumerate() {
            let (ii, jj) = (i as i64 + di, j as i64 + dj);
            nb[k] = if ii >= 0 && jj >= 0 && (ii as usize) < n && (jj as usize) < n {
                self.index(face, ii as usize, jj as usize) as u32
            } else {
                // Step just across the face edge at this cell's edge midpoint. Equi-angular faces
                // divide a shared edge identically, so this lands in the matching cell.
                let eps = 1e-7;
                let edge = |c: i64, centre: usize| -> f64 {
                    if c >= n as i64 {
                        1.0 + eps
                    } else if c < 0 {
                        -1.0 - eps
                    } else {
                        self.cell_uv(centre)
                    }
                };
                let d = normalize(face_dir(face, uv_to_ab(edge(ii, i)), uv_to_ab(edge(jj, j))));
                self.dir_to_cell(d) as u32
            };
        }
        nb
    }

    /// Spherical area of a cell (steradians), from its four corners as two triangles.
    fn cell_area(self, idx: usize) -> f64 {
        let (face, i, j) = self.coords(idx);
        let c = [
            self.corner_dir(face, i, j),
            self.corner_dir(face, i + 1, j),
            self.corner_dir(face, i + 1, j + 1),
            self.corner_dir(face, i, j + 1),
        ];
        tri_area(c[0], c[1], c[2]) + tri_area(c[0], c[2], c[3])
    }
}

/// A diagonal is the cell, other than this one, that touches both of two perpendicular edge
/// neighbours. At a cube corner those two neighbours touch only each other and this cell, so the
/// diagonal does not exist.
fn diagonals_of(idx: usize, nb: &[[u32; 4]]) -> [u32; 4] {
    let mut out = [NONE; 4];
    for (k, &(p, q)) in DIAGONAL_PAIRS.iter().enumerate() {
        let (a, b) = (nb[idx][p] as usize, nb[idx][q] as usize);
        let mut found = NONE;
        for &x in &nb[a] {
            if x as usize != idx && nb[b].contains(&x) {
                assert!(found == NONE || found == x, "cell {idx}: two candidate diagonals");
                found = x;
            }
        }
        out[k] = found;
    }
    out
}

/// Local east and north unit vectors. At an exact pole (odd N puts a cell centre there) east is
/// undefined; we pick +X, which makes north point along -Z at the north pole and +Z at the south.
fn local_frame(d: V3) -> ([f32; 3], [f32; 3]) {
    let up = [0.0, 1.0, 0.0];
    let e = cross(up, d);
    let len = dot(e, e).sqrt();
    let east = if len < 1e-9 { [1.0, 0.0, 0.0] } else { [e[0] / len, e[1] / len, e[2] / len] };
    let north = normalize(cross(d, east));
    (east.map(|v| v as f32), north.map(|v| v as f32))
}

/// Solid angle of a spherical triangle (Van Oosterom–Strackee).
fn tri_area(a: V3, b: V3, c: V3) -> f64 {
    let num = dot(a, cross(b, c)).abs();
    let den = 1.0 + dot(a, b) + dot(b, c) + dot(c, a);
    2.0 * num.atan2(den)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot32(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn face_roundtrip() {
        for face in 0..FACES {
            for &(a, b) in &[(0.3, -0.7), (-0.99, 0.99), (0.0, 0.0)] {
                let (f2, a2, b2) = dir_to_face_ab(normalize(face_dir(face, a, b)));
                assert_eq!(f2, face);
                assert!((a - a2).abs() < 1e-12 && (b - b2).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn edge_neighbours_are_symmetric_and_distinct() {
        for n in [2, 3, 17] {
            let g = Grid::new(n);
            for idx in 0..g.cells() {
                let nb = g.neighbors[idx];
                for (k, &m) in nb.iter().enumerate() {
                    assert_ne!(m as usize, idx, "n={n}: cell {idx} is its own neighbour");
                    assert!(g.neighbors[m as usize].contains(&(idx as u32)), "n={n}: {idx} -> {m} not symmetric");
                    assert!(!nb[k + 1..].contains(&m), "n={n}: cell {idx} has a repeated neighbour");
                }
            }
        }
    }

    #[test]
    fn edge_neighbours_are_close() {
        let g = Grid::new(32);
        let cell_angle = std::f64::consts::FRAC_PI_2 / 32.0;
        for idx in 0..g.cells() {
            for &m in &g.neighbors[idx] {
                let ang = dot(g.center[idx], g.center[m as usize]).clamp(-1.0, 1.0).acos();
                assert!(ang < cell_angle * 1.6, "neighbour too far: {ang}");
            }
        }
    }

    /// Exactly the 24 cells at the 8 cube corners (3 each) miss one diagonal; every other cell
    /// has 8 neighbours. Diagonals are symmetric, distinct, and not edge neighbours.
    #[test]
    fn diagonals_cube_corners_have_seven() {
        for n in [3, 8, 17] {
            let g = Grid::new(n);
            let mut seven = Vec::new();
            for idx in 0..g.cells() {
                let diag = g.diagonals[idx];
                let missing = diag.iter().filter(|&&m| m == NONE).count();
                assert!(missing <= 1, "n={n}: cell {idx} misses {missing} diagonals");
                if missing == 1 {
                    let (_, i, j) = g.coords(idx);
                    assert!((i == 0 || i == n - 1) && (j == 0 || j == n - 1), "n={n}: non-corner cell {idx} misses a diagonal");
                    seven.push(idx);
                }
                let all: Vec<usize> = g.neighbors8(idx).collect();
                for (k, &m) in all.iter().enumerate() {
                    assert_ne!(m, idx);
                    assert!(!all[k + 1..].contains(&m), "n={n}: cell {idx} repeats neighbour {m}");
                }
                for &m in diag.iter().filter(|&&m| m != NONE) {
                    assert!(g.diagonals[m as usize].contains(&(idx as u32)), "n={n}: diagonal {idx} -> {m} not symmetric");
                }
            }
            assert_eq!(seven.len(), 24, "n={n}");
        }
    }

    #[test]
    fn diagonals_are_close() {
        let g = Grid::new(32);
        let cell_angle = std::f64::consts::FRAC_PI_2 / 32.0;
        for idx in 0..g.cells() {
            for &m in g.diagonals[idx].iter().filter(|&&m| m != NONE) {
                let ang = dot(g.center[idx], g.center[m as usize]).clamp(-1.0, 1.0).acos();
                assert!(ang < cell_angle * 2.1, "diagonal too far: {ang}");
            }
        }
    }

    #[test]
    fn areas_sum_to_sphere_and_are_even() {
        let g = Grid::new(64);
        let total: f64 = g.area.iter().sum();
        assert!((total - 4.0 * PI).abs() < 1e-6, "total {total}");
        let max = g.area.iter().cloned().fold(0.0, f64::max);
        let min = g.area.iter().cloned().fold(f64::MAX, f64::min);
        assert!(max / min < 1.5, "ratio {}", max / min);
    }

    /// Odd N puts a cell centre exactly on each pole, where "east" has to be chosen by hand.
    #[test]
    fn local_frames_are_orthonormal_including_poles() {
        let g = Grid::new(17);
        let mut poles = 0;
        for idx in 0..g.cells() {
            let d = g.center[idx].map(|v| v as f32);
            let (e, nn) = (g.east[idx], g.north[idx]);
            for v in [e, nn] {
                assert!((dot32(v, v) - 1.0).abs() < 1e-5, "cell {idx} frame not unit");
                assert!(dot32(v, d).abs() < 1e-5, "cell {idx} frame not tangent");
            }
            assert!(dot32(e, nn).abs() < 1e-5, "cell {idx} frame not orthogonal");
            if d[1].abs() > 1.0 - 1e-6 {
                poles += 1;
            } else {
                // Away from the poles, north points toward +Y and east toward increasing longitude.
                assert!(nn[1] > 0.0, "cell {idx} north does not point up");
                let (lon, lat) = (g.lon[idx] as f64, g.lat[idx] as f64);
                let ahead = lonlat_to_dir(lon + 1e-3, lat);
                let step = [ahead[0] - g.center[idx][0], ahead[1] - g.center[idx][1], ahead[2] - g.center[idx][2]];
                assert!(dot(step, e.map(|v| v as f64)) > 0.0, "cell {idx} east points west");
            }
        }
        assert_eq!(poles, 2);
    }

    /// A required test from the design: spreading across a cube corner, where three faces meet.
    /// Faces 0 (+X), 2 (+Y) and 4 (+Z) share the corner (1,1,1), and turning the cube a third of a
    /// way around that corner swaps them. Starting with equal amounts in the three corner cells, the
    /// result must be exactly the same after that turn, cell for cell, and nothing may be lost.
    #[test]
    fn diffusion_across_a_cube_corner_is_conserved_and_symmetric() {
        for n in [8, 9] {
            let g = Grid::new(n);
            let corner = normalize([1.0, 1.0, 1.0]);
            let corner_cells: Vec<usize> = [[1.02, 1.0, 1.0], [1.0, 1.02, 1.0], [1.0, 1.0, 1.02]]
                .iter()
                .map(|&d| g.dir_to_cell(normalize(d)))
                .collect();
            for &c in &corner_cells {
                assert!(dot(g.center[c], corner) > 0.99, "n={n}: {c} is not at the corner");
                assert_eq!(g.diagonals[c].iter().filter(|&&m| m == NONE).count(), 1);
            }
            let mut v = vec![0u64; g.cells()];
            for &c in &corner_cells {
                v[c] = 1_000_000;
            }
            let total: u64 = v.iter().sum();
            let mut out = vec![0u64; g.cells()];
            for _ in 0..40 {
                g.diffuse_step(&v, &mut out);
                std::mem::swap(&mut v, &mut out);
                assert_eq!(v.iter().sum::<u64>(), total, "n={n}: amount not conserved");
            }
            let turn = |d: V3| [d[2], d[0], d[1]];
            for c in 0..g.cells() {
                let m = g.dir_to_cell(turn(g.center[c]));
                assert_eq!(v[c], v[m], "n={n}: cells {c} and {m} differ after the turn");
            }
            // It actually spread well beyond the three starting cells, onto all three faces.
            for f in [0, 2, 4] {
                let on_face: u64 = (0..g.cells()).filter(|&c| g.face_of(c) == f).map(|c| v[c]).sum();
                assert!(on_face > 0 && v[corner_cells[0]] < 1_000_000);
            }
        }
    }

    #[test]
    fn lonlat_matches_centres() {
        let g = Grid::new(9);
        for idx in 0..g.cells() {
            let d = lonlat_to_dir(g.lon[idx] as f64, g.lat[idx] as f64);
            assert!(dot(d, g.center[idx]) > 1.0 - 1e-6);
            assert_eq!(g.dir_to_cell(d), idx);
        }
    }
}
