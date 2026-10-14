//! Border extraction: every cell edge where the province (or land/sea) changes becomes a segment
//! in (lon, lat). Built once on the CPU; renderers upload it to the GPU and project in the shader.

use crate::cube::{dir_to_lonlat, STEPS};
use crate::world::{World, NO_PROVINCE};
use std::f64::consts::PI;

pub const KIND_COAST: u32 = 0;
pub const KIND_PROVINCE: u32 = 1;
pub const KIND_FACTION: u32 = 2;

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub kind: u32,
}

/// One line-quad vertex: segment endpoints, which corner of the quad, and the kind.
/// Six of these per segment (two triangles), no index buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    /// x: 0 at p0, 1 at p1. y: -1 / +1 side of the line.
    pub corner: [f32; 2],
    pub kind: f32,
}

pub fn extract_borders(world: &World) -> Vec<Segment> {
    let grid = &world.grid;
    let mut out = Vec::new();
    for c in 0..grid.cells() {
        let (face, i, j) = grid.coords(c);
        for (k, &nb) in grid.neighbors[c].iter().enumerate() {
            let nb = nb as usize;
            if nb <= c {
                continue; // each shared edge once
            }
            let (pa, pb) = (world.province[c], world.province[nb]);
            let kind = match (pa == NO_PROVINCE, pb == NO_PROVINCE) {
                (true, true) => continue,
                (true, false) | (false, true) => KIND_COAST,
                _ if pa == pb => continue,
                _ if world.province_faction[pa as usize] != world.province_faction[pb as usize] => KIND_FACTION,
                _ => KIND_PROVINCE,
            };
            let ((ci0, cj0), (ci1, cj1)) = match STEPS[k] {
                (1, 0) => ((i + 1, j), (i + 1, j + 1)),
                (-1, 0) => ((i, j), (i, j + 1)),
                (0, 1) => ((i, j + 1), (i + 1, j + 1)),
                _ => ((i, j), (i + 1, j)),
            };
            let (la, ta) = dir_to_lonlat(grid.corner_dir(face, ci0, cj0));
            let (lb, tb) = dir_to_lonlat(grid.corner_dir(face, ci1, cj1));
            push_wrapped(&mut out, (la, ta), (lb, tb), kind);
        }
    }
    out
}

/// A segment that crosses the ±180° line is emitted twice, once on each side, so neither copy
/// streaks across the whole map.
fn push_wrapped(out: &mut Vec<Segment>, a: (f64, f64), b: (f64, f64), kind: u32) {
    let seg = |a: (f64, f64), b: (f64, f64)| Segment { a: [a.0 as f32, a.1 as f32], b: [b.0 as f32, b.1 as f32], kind };
    if (b.0 - a.0).abs() <= PI {
        out.push(seg(a, b));
    } else if b.0 > a.0 {
        out.push(seg(a, (b.0 - 2.0 * PI, b.1)));
        out.push(seg((a.0 + 2.0 * PI, a.1), b));
    } else {
        out.push(seg(a, (b.0 + 2.0 * PI, b.1)));
        out.push(seg((a.0 - 2.0 * PI, a.1), b));
    }
}

pub fn line_vertices(segments: &[Segment]) -> Vec<LineVertex> {
    const CORNERS: [[f32; 2]; 6] = [[0.0, -1.0], [1.0, -1.0], [1.0, 1.0], [0.0, -1.0], [1.0, 1.0], [0.0, 1.0]];
    let mut v = Vec::with_capacity(segments.len() * 6);
    for s in segments {
        for corner in CORNERS {
            v.push(LineVertex { p0: s.a, p1: s.b, corner, kind: s.kind as f32 });
        }
    }
    v
}
