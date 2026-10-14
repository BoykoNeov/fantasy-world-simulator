//! Equi-angular cube-sphere grid: six N×N faces folded onto the unit sphere.
//!
//! Cell index = face * n * n + j * n + i, with `i` along the face's `a` axis and `j` along `b`.
//! The same face/axis convention is mirrored in `shaders/map_common.wgsl`; keep them in sync.

use std::f64::consts::{FRAC_PI_4, PI};

pub const FACES: usize = 6;

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

/// Neighbour order in [`Grid::neighbors`]: +i, -i, +j, -j.
pub const STEPS: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

pub struct Grid {
    pub n: usize,
    /// Four edge neighbours per cell, crossing face seams through the sphere.
    pub neighbors: Vec<[u32; 4]>,
}

impl Grid {
    pub fn new(n: usize) -> Self {
        let mut grid = Grid { n, neighbors: Vec::new() };
        let cells = grid.cells();
        let mut neighbors = Vec::with_capacity(cells);
        for idx in 0..cells {
            let (face, i, j) = grid.coords(idx);
            let mut nb = [0u32; 4];
            for (k, (di, dj)) in STEPS.iter().enumerate() {
                let (ii, jj) = (i as i64 + di, j as i64 + dj);
                nb[k] = if ii >= 0 && jj >= 0 && (ii as usize) < n && (jj as usize) < n {
                    grid.index(face, ii as usize, jj as usize) as u32
                } else {
                    // Step just across the face edge at this cell's edge midpoint. Equi-angular
                    // faces divide a shared edge identically, so this lands in the matching cell.
                    let eps = 1e-7;
                    let edge = |c: i64, centre: usize| -> f64 {
                        if c >= n as i64 {
                            1.0 + eps
                        } else if c < 0 {
                            -1.0 - eps
                        } else {
                            grid.cell_uv(centre)
                        }
                    };
                    let u = edge(ii, i);
                    let v = edge(jj, j);
                    let d = normalize(face_dir(face, uv_to_ab(u), uv_to_ab(v)));
                    grid.dir_to_cell(d) as u32
                };
            }
            neighbors.push(nb);
        }
        grid.neighbors = neighbors;
        grid
    }

    pub fn cells(&self) -> usize {
        FACES * self.n * self.n
    }

    pub fn index(&self, face: usize, i: usize, j: usize) -> usize {
        face * self.n * self.n + j * self.n + i
    }

    pub fn coords(&self, idx: usize) -> (usize, usize, usize) {
        let nn = self.n * self.n;
        let face = idx / nn;
        let r = idx % nn;
        (face, r % self.n, r / self.n)
    }

    /// Equi-angular coordinate of a cell centre along one axis.
    fn cell_uv(&self, i: usize) -> f64 {
        (i as f64 + 0.5) / self.n as f64 * 2.0 - 1.0
    }

    pub fn center_dir(&self, idx: usize) -> V3 {
        let (face, i, j) = self.coords(idx);
        normalize(face_dir(face, uv_to_ab(self.cell_uv(i)), uv_to_ab(self.cell_uv(j))))
    }

    /// Direction of a cell corner; `ci`, `cj` run 0..=n.
    pub fn corner_dir(&self, face: usize, ci: usize, cj: usize) -> V3 {
        let u = ci as f64 / self.n as f64 * 2.0 - 1.0;
        let v = cj as f64 / self.n as f64 * 2.0 - 1.0;
        normalize(face_dir(face, uv_to_ab(u), uv_to_ab(v)))
    }

    pub fn dir_to_cell(&self, d: V3) -> usize {
        let (face, a, b) = dir_to_face_ab(d);
        let n = self.n as f64;
        let to_i = |c: f64| -> usize { (((ab_to_uv(c) + 1.0) * 0.5 * n).floor().max(0.0) as usize).min(self.n - 1) };
        self.index(face, to_i(a), to_i(b))
    }

    /// Spherical area of a cell (steradians), from its four corners as two triangles.
    pub fn cell_area(&self, idx: usize) -> f64 {
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

/// Solid angle of a spherical triangle (Van Oosterom–Strackee).
fn tri_area(a: V3, b: V3, c: V3) -> f64 {
    let cross = [
        b[1] * c[2] - b[2] * c[1],
        b[2] * c[0] - b[0] * c[2],
        b[0] * c[1] - b[1] * c[0],
    ];
    let num = dot(a, cross).abs();
    let den = 1.0 + dot(a, b) + dot(b, c) + dot(c, a);
    2.0 * num.atan2(den)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn neighbours_are_symmetric_and_distinct() {
        let g = Grid::new(17);
        for idx in 0..g.cells() {
            let nb = g.neighbors[idx];
            for (k, &m) in nb.iter().enumerate() {
                assert_ne!(m as usize, idx, "cell {idx} is its own neighbour");
                assert!(g.neighbors[m as usize].contains(&(idx as u32)), "cell {idx} -> {m} not symmetric");
                for &o in &nb[k + 1..] {
                    assert_ne!(m, o, "cell {idx} has a repeated neighbour");
                }
            }
        }
    }

    #[test]
    fn neighbours_are_close() {
        let g = Grid::new(32);
        let cell_angle = std::f64::consts::FRAC_PI_2 / 32.0;
        for idx in 0..g.cells() {
            for &m in &g.neighbors[idx] {
                let ang = dot(g.center_dir(idx), g.center_dir(m as usize)).clamp(-1.0, 1.0).acos();
                assert!(ang < cell_angle * 1.6, "neighbour too far: {ang}");
            }
        }
    }

    #[test]
    fn areas_sum_to_sphere_and_are_even() {
        let g = Grid::new(64);
        let areas: Vec<f64> = (0..g.cells()).map(|i| g.cell_area(i)).collect();
        let total: f64 = areas.iter().sum();
        assert!((total - 4.0 * PI).abs() < 1e-6, "total {total}");
        let max = areas.iter().cloned().fold(0.0, f64::max);
        let min = areas.iter().cloned().fold(f64::MAX, f64::min);
        let ratio = max / min;
        println!("equi-angular max/min cell area ratio: {ratio:.3}");
        assert!(ratio < 1.5, "ratio {ratio}");
    }
}
