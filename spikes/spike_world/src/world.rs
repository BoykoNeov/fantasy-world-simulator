//! A throwaway planet: noise terrain, organically grown provinces, and factions.
//! Good enough to load a renderer realistically; not a design for the real `sim` crate.

use crate::cube::{dot, normalize, Grid, V3};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub const NO_PROVINCE: u32 = u32::MAX;

pub struct World {
    pub grid: Grid,
    /// Metres; below 0 is sea.
    pub elevation: Vec<f32>,
    /// Province id per cell, [`NO_PROVINCE`] for sea.
    pub province: Vec<u32>,
    pub province_faction: Vec<u32>,
    pub province_center: Vec<V3>,
    pub province_cells: Vec<u32>,
    /// RGBA per province, for the political layer.
    pub province_color: Vec<[u8; 4]>,
    pub faction_count: usize,
}

pub fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash_u64(x: u64) -> u64 {
    let mut s = x;
    splitmix(&mut s)
}

fn lattice(x: i64, y: i64, z: i64, seed: u64) -> f64 {
    let h = hash_u64(
        (x as u64).wrapping_mul(0x8DA6_B343)
            ^ (y as u64).wrapping_mul(0xD816_3841)
            ^ (z as u64).wrapping_mul(0xCB1A_B31F)
            ^ seed.wrapping_mul(0x1656_67B1),
    );
    (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// Trilinear value noise with smoothstep, in [-1, 1].
fn value_noise(p: V3, seed: u64) -> f64 {
    let f = [p[0].floor(), p[1].floor(), p[2].floor()];
    let t = [p[0] - f[0], p[1] - f[1], p[2] - f[2]];
    let s = t.map(|v| v * v * (3.0 - 2.0 * v));
    let (x, y, z) = (f[0] as i64, f[1] as i64, f[2] as i64);
    let mut acc = 0.0;
    for dz in 0..2 {
        for dy in 0..2 {
            for dx in 0..2 {
                let w = (if dx == 1 { s[0] } else { 1.0 - s[0] })
                    * (if dy == 1 { s[1] } else { 1.0 - s[1] })
                    * (if dz == 1 { s[2] } else { 1.0 - s[2] });
                acc += w * lattice(x + dx, y + dy, z + dz, seed);
            }
        }
    }
    acc
}

fn fbm(p: V3, octaves: u32, seed: u64) -> f64 {
    let (mut amp, mut freq, mut sum, mut norm) = (1.0, 1.0, 0.0, 0.0);
    for o in 0..octaves {
        sum += amp * value_noise([p[0] * freq, p[1] * freq, p[2] * freq], seed + o as u64);
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm
}

/// Raw terrain height before sea level: continents, warped, plus detail.
fn raw_height(d: V3, seed: u64) -> f64 {
    let warp = [
        fbm([d[0] * 2.0 + 5.2, d[1] * 2.0, d[2] * 2.0], 3, seed + 100),
        fbm([d[0] * 2.0, d[1] * 2.0 + 1.3, d[2] * 2.0], 3, seed + 200),
        fbm([d[0] * 2.0, d[1] * 2.0, d[2] * 2.0 + 7.1], 3, seed + 300),
    ];
    let q = [d[0] * 1.7 + warp[0] * 0.6, d[1] * 1.7 + warp[1] * 0.6, d[2] * 1.7 + warp[2] * 0.6];
    let continents = fbm(q, 4, seed);
    let detail = fbm([d[0] * 9.0, d[1] * 9.0, d[2] * 9.0], 5, seed + 50);
    // Ridged term gives mountain chains.
    let ridge = 1.0 - fbm([d[0] * 4.0, d[1] * 4.0, d[2] * 4.0], 4, seed + 70).abs() * 2.0;
    continents + 0.25 * detail + 0.12 * ridge.max(0.0).powi(3)
}

pub struct GenParams {
    pub n: usize,
    pub provinces: usize,
    pub factions: usize,
    pub seed: u64,
    pub land_fraction: f64,
}

impl Default for GenParams {
    fn default() -> Self {
        GenParams { n: 408, provinces: 5000, factions: 48, seed: 7, land_fraction: 0.36 }
    }
}

pub fn generate(p: &GenParams) -> World {
    let grid = Grid::new(p.n);
    let cells = grid.cells();

    // Heights in parallel by face; plain std threads keep this crate dependency-free.
    let mut raw = vec![0f64; cells];
    let face_len = p.n * p.n;
    std::thread::scope(|s| {
        for (face, chunk) in raw.chunks_mut(face_len).enumerate() {
            let grid = &grid;
            s.spawn(move || {
                for (k, h) in chunk.iter_mut().enumerate() {
                    *h = raw_height(grid.center_dir(face * face_len + k), p.seed);
                }
            });
        }
    });

    let mut sorted = raw.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let sea = sorted[((1.0 - p.land_fraction) * cells as f64) as usize];
    let (lo, hi) = (sorted[0], sorted[cells - 1]);
    let elevation: Vec<f32> = raw
        .iter()
        .map(|&h| {
            if h >= sea {
                (((h - sea) / (hi - sea)).powf(1.4) * 5200.0) as f32
            } else {
                (-((sea - h) / (sea - lo)).powf(0.8) * 5500.0 - 1.0) as f32
            }
        })
        .collect();

    let province = grow_provinces(&grid, &elevation, p.provinces, p.seed);
    let province_count = province.iter().filter(|&&x| x != NO_PROVINCE).map(|&x| x + 1).max().unwrap_or(0) as usize;

    let mut sum = vec![[0f64; 3]; province_count];
    let mut province_cells = vec![0u32; province_count];
    for idx in 0..cells {
        let pr = province[idx];
        if pr != NO_PROVINCE {
            let d = grid.center_dir(idx);
            for a in 0..3 {
                sum[pr as usize][a] += d[a];
            }
            province_cells[pr as usize] += 1;
        }
    }
    let province_center: Vec<V3> = sum.into_iter().map(normalize).collect();

    // Factions: nearest seed province on the sphere (handles islands without a graph walk).
    let mut rng = p.seed ^ 0xFAC7;
    let mut order: Vec<usize> = (0..province_count).collect();
    shuffle(&mut order, &mut rng);
    let seeds: Vec<V3> = order.iter().take(p.factions.min(province_count)).map(|&i| province_center[i]).collect();
    let province_faction: Vec<u32> = province_center
        .iter()
        .map(|&c| {
            let mut best = (f64::MIN, 0u32);
            for (f, &s) in seeds.iter().enumerate() {
                let d = dot(c, s);
                if d > best.0 {
                    best = (d, f as u32);
                }
            }
            best.1
        })
        .collect();

    let faction_count = seeds.len();
    let province_color = (0..province_count)
        .map(|pi| {
            let f = province_faction[pi] as f64;
            let hue = (f * 0.618_033_988_75).fract();
            let jitter = (hash_u64(pi as u64 ^ p.seed) % 1000) as f64 / 1000.0;
            let [r, g, b] = hsl_to_rgb(hue, 0.55, 0.45 + jitter * 0.15);
            [r, g, b, 255]
        })
        .collect();

    World { grid, elevation, province, province_faction, province_center, province_cells, province_color, faction_count }
}

fn shuffle<T>(v: &mut [T], rng: &mut u64) {
    for i in (1..v.len()).rev() {
        let j = (splitmix(rng) % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

/// Grow provinces from random land seeds with randomised, slope-aware step costs, so shapes are
/// organic and borders tend to follow mountains. Unreached land (small islands) gets new seeds.
fn grow_provinces(grid: &Grid, elev: &[f32], target: usize, seed: u64) -> Vec<u32> {
    let cells = grid.cells();
    let mut province = vec![NO_PROVINCE; cells];
    let mut land: Vec<usize> = (0..cells).filter(|&i| elev[i] >= 0.0).collect();
    let mut rng = seed ^ 0x9A0;
    shuffle(&mut land, &mut rng);

    let mut next_id = 0u32;
    let mut heap: BinaryHeap<Reverse<(u64, u32, u32)>> = BinaryHeap::new();
    let mut cost = vec![u64::MAX; cells];
    for &c in land.iter().take(target) {
        cost[c] = 0;
        heap.push(Reverse((0, c as u32, next_id)));
        next_id += 1;
    }
    let step_cost = |from: usize, to: usize| -> u64 {
        let jitter = hash_u64((from as u64) << 32 | to as u64 ^ seed) % 1000;
        let slope = (elev[to] - elev[from]).abs() as u64;
        1000 + jitter * 2 + slope * 3
    };
    let grow = |heap: &mut BinaryHeap<Reverse<(u64, u32, u32)>>, province: &mut Vec<u32>, cost: &mut Vec<u64>| {
        while let Some(Reverse((c, cell, id))) = heap.pop() {
            let cell = cell as usize;
            if province[cell] != NO_PROVINCE {
                continue;
            }
            province[cell] = id;
            for &nb in &grid.neighbors[cell] {
                let nb = nb as usize;
                if elev[nb] >= 0.0 && province[nb] == NO_PROVINCE {
                    let nc = c + step_cost(cell, nb);
                    if nc < cost[nb] {
                        cost[nb] = nc;
                        heap.push(Reverse((nc, nb as u32, id)));
                    }
                }
            }
        }
    };
    grow(&mut heap, &mut province, &mut cost);
    for &c in &land {
        if province[c] == NO_PROVINCE {
            cost[c] = 0;
            heap.push(Reverse((0, c as u32, next_id)));
            next_id += 1;
            grow(&mut heap, &mut province, &mut cost);
        }
    }
    province
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [u8; 3] {
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

/// Invented place names for labels.
pub fn place_name(i: usize, seed: u64) -> String {
    const SYL: [&str; 24] = [
        "ar", "bel", "cor", "dun", "el", "fal", "gar", "hal", "is", "kor", "lin", "mar", "nor", "or", "ral", "sil",
        "tor", "ul", "ven", "yr", "tha", "mir", "esk", "dra",
    ];
    let mut rng = seed ^ (i as u64).wrapping_mul(0x51ED);
    let count = 2 + (splitmix(&mut rng) % 2) as usize;
    let mut s: String = (0..count).map(|_| SYL[(splitmix(&mut rng) % SYL.len() as u64) as usize]).collect();
    s[..1].make_ascii_uppercase();
    s
}

/// Raise or lower land around a cell. Returns the touched rectangle per face (inclusive), for
/// partial texture uploads.
pub fn apply_brush(world: &mut World, center: usize, radius: usize, amount: f32) -> Vec<DirtyRect> {
    let grid = &world.grid;
    let mut seen = std::collections::HashMap::new();
    let mut frontier = vec![center];
    seen.insert(center, 0usize);
    for depth in 1..=radius {
        let mut next = Vec::new();
        for &c in &frontier {
            for &nb in &grid.neighbors[c] {
                let nb = nb as usize;
                if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(nb) {
                    e.insert(depth);
                    next.push(nb);
                }
            }
        }
        frontier = next;
    }
    let mut rects: Vec<DirtyRect> = Vec::new();
    for (&c, &depth) in &seen {
        let t = 1.0 - depth as f32 / (radius as f32 + 1.0);
        world.elevation[c] += amount * t * t;
        let (face, i, j) = grid.coords(c);
        match rects.iter_mut().find(|r| r.face == face) {
            Some(r) => {
                r.i0 = r.i0.min(i);
                r.i1 = r.i1.max(i);
                r.j0 = r.j0.min(j);
                r.j1 = r.j1.max(j);
            }
            None => rects.push(DirtyRect { face, i0: i, i1: i, j0: j, j1: j }),
        }
    }
    rects
}

#[derive(Clone, Copy, Debug)]
pub struct DirtyRect {
    pub face: usize,
    pub i0: usize,
    pub i1: usize,
    pub j0: usize,
    pub j1: usize,
}

impl World {
    /// Copy a face sub-rectangle of elevation into a row-packed buffer (row = j).
    pub fn elevation_rect(&self, r: &DirtyRect) -> Vec<f32> {
        let n = self.grid.n;
        let mut out = Vec::with_capacity((r.i1 - r.i0 + 1) * (r.j1 - r.j0 + 1));
        for j in r.j0..=r.j1 {
            let row = r.face * n * n + j * n;
            out.extend_from_slice(&self.elevation[row + r.i0..=row + r.i1]);
        }
        out
    }

    /// Palette texture contents: 256 provinces per row, RGBA8.
    pub fn palette_rgba(&self) -> (u32, u32, Vec<u8>) {
        let w = 256u32;
        let h = (self.province_color.len() as u32).div_ceil(w).max(1);
        let mut data = vec![0u8; (w * h * 4) as usize];
        for (i, c) in self.province_color.iter().enumerate() {
            data[i * 4..i * 4 + 4].copy_from_slice(c);
        }
        (w, h, data)
    }

    /// (lon, lat) of the largest province: the benchmark's zoom and brush target.
    pub fn bench_target(&self) -> (f64, f64) {
        crate::cube::dir_to_lonlat(self.province_center[self.label_provinces(1)[0]])
    }

    /// The largest provinces, for labels.
    pub fn label_provinces(&self, count: usize) -> Vec<usize> {
        let mut ids: Vec<usize> = (0..self.province_cells.len()).collect();
        ids.sort_by_key(|&i| Reverse(self.province_cells[i]));
        ids.truncate(count);
        ids
    }
}
