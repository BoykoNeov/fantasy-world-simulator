//! Lines drawn over the map: coastlines, province borders, rivers, the graticule, the ways between
//! provinces (roads and sea lanes), last month's trade, and the borders between realms.
//!
//! Lines are built once on the sphere and projected in the shader, so changing projection or
//! panning never rebuilds them. Borders are traced along cell edges, joined into chains between
//! junctions (where three or more regions meet), and each chain is smoothed once. Both sides of a
//! border therefore share one curve, with no gaps or double lines, and junctions stay put.
//! Smoothing works on 3D unit vectors, so the ±180° meridian and the poles need no special cases.

use sim::grid::{cross, dir_to_lonlat, normalize, V3, NONE, STEPS};
use sim::world::{World, RIVER_MIN_FLOW};
use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum Kind {
    Coast = 0,
    Province = 1,
    River = 2,
    Graticule = 3,
    /// A way over land between two provinces' seats.
    Road = 4,
    /// A way by sea between two provinces' seats.
    SeaLane = 5,
    /// Last month's trade along a way, with an arrowhead toward the buyer. Kept in a buffer of its
    /// own, as it changes every month.
    Trade = 6,
    /// A province border where the realms on either side differ (or one side belongs to none): a
    /// buffer of its own, copied from the province borders whenever who owns what changes.
    Realm = 7,
}

pub const KINDS: [Kind; 8] = [Kind::Coast, Kind::Province, Kind::River, Kind::Graticule, Kind::Road, Kind::SeaLane, Kind::Trade, Kind::Realm];

/// A polyline on the unit sphere.
#[derive(Clone, Debug)]
pub struct Chain {
    pub points: Vec<V3>,
    pub kind: Kind,
    /// Line width in points (logical pixels).
    pub width: f32,
    /// The last point joins back to the first.
    pub closed: bool,
    /// For a province border, the two provinces it runs between.
    pub between: Option<[u32; 2]>,
}

/// One line segment as the GPU sees it: endpoints as (longitude, latitude), its kind and width,
/// and its distance along its chain (radians) for dashes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub kind: u32,
    pub width: f32,
    pub dist: f32,
}

pub const COAST_WIDTH: f32 = 1.3;
pub const PROVINCE_WIDTH: f32 = 1.0;
pub const GRATICULE_WIDTH: f32 = 0.8;
pub const ROAD_WIDTH: f32 = 0.9;
pub const REALM_WIDTH: f32 = 2.4;
/// Trade lines run from this wide (the least trade shown) to `TRADE_WIDTH.1` (the most).
pub const TRADE_WIDTH: (f32, f32) = (0.8, 5.0);

/// Graph of border edges: nodes are cell corners, edges are cell sides between two regions.
pub struct BorderGraph {
    pub nodes: Vec<V3>,
    /// (node, node, kind)
    pub edges: Vec<(u32, u32, Kind)>,
    /// Each edge's two sides: the province on either side, the lower first; [`NONE`] for sea.
    pub sides: Vec<[u32; 2]>,
}

/// Every cell side where the province changes, including land against sea.
pub fn border_graph(world: &World) -> BorderGraph {
    let grid = &world.grid;
    let of_cell = &world.provinces.of_cell;
    let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
    let mut nodes = Vec::new();
    let mut node = |d: V3| -> u32 {
        // Corners on a face edge are computed from either face; they agree to ~1e-16, far below
        // this rounding step, so both land on the same key.
        let key = d.map(|v| (v * (1u64 << 30) as f64).round() as i64);
        *ids.entry(key).or_insert_with(|| {
            nodes.push(d);
            nodes.len() as u32 - 1
        })
    };
    let mut edges = Vec::new();
    let mut sides = Vec::new();
    for c in 0..grid.cells() {
        let (face, i, j) = grid.coords(c);
        for (k, &nb) in grid.neighbors[c].iter().enumerate() {
            let nb = nb as usize;
            if nb <= c {
                continue; // each shared side once
            }
            let (pa, pb) = (of_cell[c], of_cell[nb]);
            let kind = match (pa == NONE, pb == NONE) {
                (true, true) => continue,
                (true, false) | (false, true) => Kind::Coast,
                _ if pa == pb => continue,
                _ => Kind::Province,
            };
            let (c0, c1) = side_corners(STEPS[k], i, j);
            let a = node(grid.corner_dir(face, c0.0, c0.1));
            let b = node(grid.corner_dir(face, c1.0, c1.1));
            edges.push((a, b, kind));
            sides.push([pa.min(pb), pa.max(pb)]);
        }
    }
    BorderGraph { nodes, edges, sides }
}

/// The two corners (ci, cj) of the cell side facing `step`.
fn side_corners(step: (i64, i64), i: usize, j: usize) -> ((usize, usize), (usize, usize)) {
    match step {
        (1, 0) => ((i + 1, j), (i + 1, j + 1)),
        (-1, 0) => ((i, j), (i, j + 1)),
        (0, 1) => ((i, j + 1), (i + 1, j + 1)),
        _ => ((i, j), (i + 1, j)),
    }
}

impl BorderGraph {
    pub fn degrees(&self) -> Vec<u32> {
        let mut deg = vec![0u32; self.nodes.len()];
        for &(a, b, _) in &self.edges {
            deg[a as usize] += 1;
            deg[b as usize] += 1;
        }
        deg
    }

    /// Join edges into chains. A chain runs between junctions (any node not joining exactly two
    /// edges); loops with no junction at all come out closed. Each comes with its first edge: a
    /// corner joining exactly two border sides has only two regions around it, so every edge of a
    /// chain has the same two sides.
    pub fn chains(&self) -> Vec<(Vec<u32>, Kind, bool, u32)> {
        let deg = self.degrees();
        let mut incident: Vec<Vec<u32>> = vec![Vec::new(); self.nodes.len()];
        for (e, &(a, b, _)) in self.edges.iter().enumerate() {
            incident[a as usize].push(e as u32);
            incident[b as usize].push(e as u32);
        }
        let mut used = vec![false; self.edges.len()];
        let other = |e: u32, n: u32| -> u32 {
            let (a, b, _) = self.edges[e as usize];
            if a == n { b } else { a }
        };
        let walk = |start: u32, first: u32, used: &mut Vec<bool>| -> Vec<u32> {
            let mut nodes = vec![start];
            let (mut at, mut e) = (start, first);
            loop {
                used[e as usize] = true;
                at = other(e, at);
                nodes.push(at);
                if deg[at as usize] != 2 || at == start {
                    return nodes;
                }
                match incident[at as usize].iter().find(|&&x| !used[x as usize]) {
                    Some(&next) => e = next,
                    None => return nodes,
                }
            }
        };
        let mut out = Vec::new();
        for n in 0..self.nodes.len() as u32 {
            if deg[n as usize] == 2 {
                continue;
            }
            for &e in &incident[n as usize] {
                if !used[e as usize] {
                    let kind = self.edges[e as usize].2;
                    out.push((walk(n, e, &mut used), kind, false, e));
                }
            }
        }
        // What is left are loops through degree-2 nodes only.
        for e in 0..self.edges.len() as u32 {
            if !used[e as usize] {
                let start = self.edges[e as usize].0;
                let kind = self.edges[e as usize].2;
                let mut nodes = walk(start, e, &mut used);
                nodes.pop(); // the walk ends back at the start
                out.push((nodes, kind, true, e));
            }
        }
        out
    }
}

/// Coastlines and province borders, smoothed.
pub fn border_chains(world: &World) -> Vec<Chain> {
    let g = border_graph(world);
    g.chains()
        .into_iter()
        .map(|(nodes, kind, closed, first)| {
            let p: Vec<V3> = nodes.iter().map(|&n| g.nodes[n as usize]).collect();
            // Edge midpoints turn the cell staircase into diagonals; one corner-cutting pass then
            // rounds them off. Open chains keep their junction ends.
            let mids: Vec<V3> = if closed {
                (0..p.len()).map(|k| mid(p[k], p[(k + 1) % p.len()])).collect()
            } else {
                let mut m = vec![p[0]];
                m.extend(p.windows(2).map(|w| mid(w[0], w[1])));
                m.push(p[p.len() - 1]);
                m
            };
            let width = if kind == Kind::Coast { COAST_WIDTH } else { PROVINCE_WIDTH };
            let between = (kind == Kind::Province).then(|| g.sides[first as usize]);
            Chain { points: chaikin(&mids, closed, 1), kind, width, closed, between }
        })
        .collect()
}

/// River width in points, from the water it carries.
fn river_width(flow: u64) -> f32 {
    (0.7 + 0.75 * (flow as f64 / RIVER_MIN_FLOW as f64).log10()).clamp(0.7, 2.6) as f32
}

/// Rivers: cell centre to downstream cell centre wherever the flow is big enough. Chains break at
/// confluences, which stay pinned while smoothing. A river reaching the sea ends on the coast,
/// halfway to the first sea cell.
pub fn river_chains(world: &World) -> Vec<Chain> {
    let grid = &world.grid;
    let rx = &world.downstream;
    let is_river = |c: usize| world.river_flow[c] >= RIVER_MIN_FLOW && world.elevation[c] >= 0.0 && rx[c] != NONE;
    let mut inflow = vec![0u8; grid.cells()];
    for c in 0..grid.cells() {
        if is_river(c) {
            inflow[rx[c] as usize] = inflow[rx[c] as usize].saturating_add(1);
        }
    }
    let mut out = Vec::new();
    for c in 0..grid.cells() {
        // Chains start at sources and at confluences.
        if !is_river(c) || inflow[c] == 1 {
            continue;
        }
        let mut points = vec![grid.center[c]];
        let mut at = c;
        loop {
            let next = rx[at] as usize;
            if world.elevation[next] < 0.0 {
                points.push(mid(grid.center[at], grid.center[next]));
                break;
            }
            points.push(grid.center[next]);
            if inflow[next] != 1 || !is_river(next) {
                break;
            }
            at = next;
        }
        let width = river_width(world.river_flow[at]);
        out.push(Chain { points: chaikin(&points, false, 2), kind: Kind::River, width, closed: false, between: None });
    }
    out
}

/// Meridians and parallels every 30°.
pub fn graticule() -> Vec<Chain> {
    let deg = PI / 180.0;
    let line = |points: Vec<V3>| Chain { points, kind: Kind::Graticule, width: GRATICULE_WIDTH, closed: false, between: None };
    let mut out = Vec::new();
    for k in -6..6 {
        let lon = k as f64 * 30.0 * deg;
        // Stop short of the poles, where every meridian meets.
        out.push(line((-88..=88).map(|l| sim::grid::lonlat_to_dir(lon, l as f64 * deg)).collect()));
    }
    for k in -2..=2 {
        let lat = k as f64 * 30.0 * deg;
        let mut ring: Vec<V3> = (-180..=180).map(|l| sim::grid::lonlat_to_dir(l as f64 * deg, lat)).collect();
        ring.pop();
        out.push(Chain { closed: true, ..line(ring) });
    }
    out
}

fn mid(a: V3, b: V3) -> V3 {
    normalize([a[0] + b[0], a[1] + b[1], a[2] + b[2]])
}

fn lerp_dir(a: V3, b: V3, t: f64) -> V3 {
    normalize([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t])
}

/// Chaikin corner cutting on the sphere. Open chains keep both end points.
pub fn chaikin(points: &[V3], closed: bool, passes: usize) -> Vec<V3> {
    let mut p = points.to_vec();
    for _ in 0..passes {
        if p.len() < 3 {
            break;
        }
        let mut q = Vec::with_capacity(p.len() * 2 + 2);
        let segs = if closed { p.len() } else { p.len() - 1 };
        if !closed {
            q.push(p[0]);
        }
        for k in 0..segs {
            let (a, b) = (p[k], p[(k + 1) % p.len()]);
            q.push(lerp_dir(a, b, 0.25));
            q.push(lerp_dir(a, b, 0.75));
        }
        if !closed {
            q.push(p[p.len() - 1]);
        }
        p = q;
    }
    p
}

/// Longest step in longitude a drawn segment may take. Projections bend straight lines, and near
/// the poles one cell side can span a wide range of longitude.
const MAX_DLON: f64 = 0.02;

/// Flatten chains into GPU segments, grouped by kind. Returns the segments and each kind's range.
pub fn instances(chains: &[Chain]) -> (Vec<LineInstance>, [std::ops::Range<u32>; KINDS.len()]) {
    let mut sorted: Vec<&Chain> = chains.iter().collect();
    sorted.sort_by_key(|c| c.kind);
    let mut out = Vec::new();
    let mut ranges: [std::ops::Range<u32>; KINDS.len()] = Default::default();
    for kind in KINDS {
        let start = out.len() as u32;
        for c in sorted.iter().filter(|c| c.kind == kind) {
            segments(c, &mut out);
        }
        ranges[kind as usize] = start..out.len() as u32;
    }
    (out, ranges)
}

/// Flatten one chain into GPU segments, appended to `out`.
fn segments(c: &Chain, out: &mut Vec<LineInstance>) {
    let n = c.points.len();
    let segs = if c.closed { n } else { n.saturating_sub(1) };
    let mut dist = 0.0f64;
    for k in 0..segs {
        let (a, b) = (c.points[k], c.points[(k + 1) % n]);
        let (la, ta) = dir_to_lonlat(a);
        let (lb, tb) = dir_to_lonlat(b);
        let dl = sim::grid::wrap_lon(lb - la).abs();
        // Near a pole longitude changes fast; split so each piece stays short in longitude.
        let pieces = if ta.abs().max(tb.abs()) > FRAC_PI_2 - 1e-9 { 1 } else { ((dl / MAX_DLON).ceil() as usize).clamp(1, 64) };
        let mut prev = (la, ta, a);
        for s in 1..=pieces {
            let d = if s == pieces { b } else { lerp_dir(a, b, s as f64 / pieces as f64) };
            let (lo, la2) = if s == pieces { (lb, tb) } else { dir_to_lonlat(d) };
            let len = sim::grid::dot(prev.2, d).clamp(-1.0, 1.0).acos();
            out.push(LineInstance { a: [prev.0 as f32, prev.1 as f32], b: [lo as f32, la2 as f32], kind: c.kind as u32, width: c.width, dist: dist as f32 });
            dist += len;
            prev = (lo, la2, d);
        }
    }
}

/// Every province border, flattened once per map with the two provinces it runs between, so the
/// borders between realms are a copy of some of them ([`RealmBorders::lines`]) whenever who owns
/// what changes, not worked out again.
#[derive(Default)]
pub struct RealmBorders {
    /// The province borders' segments, as realm borders.
    segments: Vec<LineInstance>,
    /// Each border's two provinces and its segments.
    pieces: Vec<([u32; 2], std::ops::Range<u32>)>,
}

impl RealmBorders {
    /// From the map's lines, after the wobble has moved them.
    pub fn of(chains: &[Chain]) -> Self {
        let mut out = RealmBorders::default();
        for c in chains {
            if let Some(pair) = c.between {
                let start = out.segments.len() as u32;
                segments(&Chain { kind: Kind::Realm, width: REALM_WIDTH, ..c.clone() }, &mut out.segments);
                out.pieces.push((pair, start..out.segments.len() as u32));
            }
        }
        out
    }

    /// The segments of every province border whose two sides belong to different realms, or to a
    /// realm and nobody (`owner`: each province's realm, [`NONE`] for none; provinces past its end
    /// belong to nobody).
    pub fn lines(&self, owner: &[u32]) -> Vec<LineInstance> {
        let of = |p: u32| owner.get(p as usize).copied().unwrap_or(NONE);
        let mut out = Vec::new();
        for ([a, b], range) in &self.pieces {
            if of(*a) != of(*b) {
                out.extend_from_slice(&self.segments[range.start as usize..range.end as usize]);
            }
        }
        out
    }
}

/// Everything drawn as lines, for a world.
pub fn all_lines(world: &World) -> Vec<Chain> {
    let mut chains = border_chains(world);
    chains.extend(river_chains(world));
    chains.extend(graticule());
    chains.extend(way_chains(world));
    chains
}

/// Each way between two provinces, seat to seat: a road where they share a way over land, a sea
/// lane where ships go between them (a pair can have both).
pub fn way_chains(world: &World) -> Vec<Chain> {
    let r = &world.routes;
    let seat = |p: u32| r.seat.get(p as usize).copied().filter(|&c| c != NONE).map(|c| world.grid.center[c as usize]);
    let mut out = Vec::new();
    for l in &r.links {
        let (Some(a), Some(b)) = (seat(l.a), seat(l.b)) else { continue };
        if l.land_days.is_some() {
            out.push(Chain { points: arc(a, b), kind: Kind::Road, width: ROAD_WIDTH, closed: false, between: None });
        }
        if l.sea_days.is_some() {
            out.push(Chain { points: arc(a, b), kind: Kind::SeaLane, width: ROAD_WIDTH, closed: false, between: None });
        }
    }
    out
}

/// Last month's trade as lines: one per pair of provinces that traded, seat to seat, as wide as
/// the trade's net worth (`coins`, from `from` to `to`, against the most any pair traded), with an
/// arrowhead halfway pointing at the buyer. Pairs under 3% of the most are left out, so the whole world stays readable.
pub fn trade_chains(world: &World, net: &[(u32, u32, f64)]) -> Vec<Chain> {
    let r = &world.routes;
    let seat = |p: u32| r.seat.get(p as usize).copied().filter(|&c| c != NONE).map(|c| world.grid.center[c as usize]);
    let most = net.iter().map(|x| x.2).fold(0.0, f64::max);
    let mut out = Vec::new();
    if most <= 0.0 {
        return out;
    }
    for &(from, to, coins) in net {
        let share = coins / most;
        let (Some(a), Some(b)) = (seat(from), seat(to)) else { continue };
        if share < 0.03 {
            continue;
        }
        let width = TRADE_WIDTH.0 + (TRADE_WIDTH.1 - TRADE_WIDTH.0) * share.sqrt() as f32;
        out.push(Chain { points: arc(a, b), kind: Kind::Trade, width, closed: false, between: None });
        // The arrowhead: two short strokes back from just past the middle, a fifth of the way long.
        let (tip, back) = (lerp_dir(a, b, 0.6), lerp_dir(a, b, 0.42));
        let side = normalize(cross(tip, sub(b, a)));
        let reach = sim::grid::dot(tip, back).clamp(-1.0, 1.0).acos() * 0.6;
        for sign in [1.0, -1.0] {
            let wing = normalize([back[0] + side[0] * reach * sign, back[1] + side[1] * reach * sign, back[2] + side[2] * reach * sign]);
            out.push(Chain { points: vec![wing, tip], kind: Kind::Trade, width, closed: false, between: None });
        }
    }
    out
}

/// The great circle from `a` to `b`, as points evenly spaced along it at most half of
/// `MAX_DLON` apart: [`instances`] splits a long segment evenly along its chord, which is uneven
/// along the arc.
fn arc(a: V3, b: V3) -> Vec<V3> {
    let angle = sim::grid::dot(a, b).clamp(-1.0, 1.0).acos();
    let n = ((angle / (MAX_DLON * 0.5)).ceil() as usize).max(1);
    if n == 1 || angle.sin() < 1e-12 {
        return vec![a, b];
    }
    (0..=n)
        .map(|k| {
            let t = k as f64 / n as f64;
            let (wa, wb) = (((1.0 - t) * angle).sin() / angle.sin(), (t * angle).sin() / angle.sin());
            normalize([a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb, a[2] * wa + b[2] * wb])
        })
        .collect()
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::world::WorldParams;

    fn world() -> World {
        World::generate(&WorldParams { n: 48, provinces: 60, seed: 11, land_fraction: 0.4, radius_km: 6371.0 })
    }

    #[test]
    fn borders_never_dead_end() {
        // A border separates regions, so it cannot just stop: every corner on it joins 2, 3 or 4
        // border sides. A dead end would mean the same corner got two different ids.
        let w = world();
        let g = border_graph(&w);
        assert!(!g.edges.is_empty());
        for (n, &d) in g.degrees().iter().enumerate() {
            assert!((2..=4).contains(&d), "corner {n} joins {d} border sides");
        }
    }

    #[test]
    fn chains_use_every_side_once_and_end_at_junctions() {
        let w = world();
        let g = border_graph(&w);
        let deg = g.degrees();
        let chains = g.chains();
        let sides: usize = chains.iter().map(|(n, _, closed, _)| if *closed { n.len() } else { n.len() - 1 }).sum();
        assert_eq!(sides, g.edges.len());
        for (nodes, _, closed, _) in &chains {
            if !closed {
                assert_ne!(deg[nodes[0] as usize], 2);
                assert_ne!(deg[*nodes.last().unwrap() as usize], 2);
            }
        }
    }

    #[test]
    fn every_side_of_a_chain_separates_the_same_two_provinces() {
        let w = world();
        let g = border_graph(&w);
        let mut incident: HashMap<(u32, u32), usize> = HashMap::new();
        for (e, &(a, b, _)) in g.edges.iter().enumerate() {
            incident.insert((a.min(b), a.max(b)), e);
        }
        let mut province_chains = 0;
        for (nodes, kind, closed, first) in g.chains() {
            let pair = g.sides[first as usize];
            let mut steps: Vec<(u32, u32)> = nodes.windows(2).map(|s| (s[0], s[1])).collect();
            if closed {
                steps.push((*nodes.last().unwrap(), nodes[0]));
            }
            for (a, b) in steps {
                let e = incident[&(a.min(b), a.max(b))];
                assert_eq!(g.sides[e], pair, "a chain changes its two sides partway");
                assert_eq!(g.edges[e].2, kind);
            }
            province_chains += (kind == Kind::Province) as usize;
        }
        assert!(province_chains > 0);
        // And the drawn chains carry it: province borders between two provinces, nothing else.
        for c in border_chains(&w) {
            match c.kind {
                Kind::Province => assert!(c.between.is_some_and(|[a, b]| a < b && b != NONE)),
                _ => assert_eq!(c.between, None),
            }
        }
    }

    #[test]
    fn realm_borders_run_where_owners_differ() {
        let w = world();
        let chains = border_chains(&w);
        let borders = RealmBorders::of(&chains);
        let count = w.provinces.count as u32;
        let segments_between = |keep: &dyn Fn(u32, u32) -> bool| -> usize {
            let picked: Vec<Chain> = chains.iter().filter(|c| c.between.is_some_and(|[a, b]| keep(a, b))).cloned().collect();
            instances(&picked).0.len()
        };
        // Nobody owns anything: no realm borders.
        assert!(borders.lines(&vec![NONE; count as usize]).is_empty());
        // Every province its own realm: every province border is a realm border.
        let own: Vec<u32> = (0..count).collect();
        assert_eq!(borders.lines(&own).len(), segments_between(&|_, _| true));
        // Two realms split by number, the last provinces nobody's (and past the end of the list).
        let owner: Vec<u32> = (0..count - 5).map(|p| (p >= count / 2) as u32).collect();
        let of = |p: u32| owner.get(p as usize).copied().unwrap_or(NONE);
        let lines = borders.lines(&owner);
        assert_eq!(lines.len(), segments_between(&|a, b| of(a) != of(b)));
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|l| l.kind == Kind::Realm as u32 && l.width == REALM_WIDTH));
    }

    #[test]
    fn smoothing_keeps_junctions_and_stays_on_the_sphere() {
        let w = world();
        let g = border_graph(&w);
        let deg = g.degrees();
        let junctions: Vec<V3> = (0..g.nodes.len()).filter(|&n| deg[n] != 2).map(|n| g.nodes[n]).collect();
        for c in border_chains(&w) {
            for p in &c.points {
                assert!((sim::grid::dot(*p, *p) - 1.0).abs() < 1e-12);
            }
            if !c.closed {
                for end in [c.points[0], *c.points.last().unwrap()] {
                    assert!(junctions.contains(&end), "a chain end moved off its junction");
                }
            }
        }
    }

    #[test]
    fn rivers_reach_the_sea_on_the_coast() {
        let w = world();
        let rivers = river_chains(&w);
        assert!(!rivers.is_empty(), "the test world should have rivers");
        let at_sea = rivers.iter().filter(|c| {
            let end = *c.points.last().unwrap();
            let cell = w.grid.dir_to_cell(end);
            // The end sits on the edge between a land cell and a sea cell.
            w.grid.neighbors8(cell).chain([cell]).any(|m| w.elevation[m] < 0.0)
        });
        assert!(at_sea.count() > 0);
    }

    #[test]
    fn segments_stay_short_in_longitude() {
        let w = world();
        let (lines, ranges) = instances(&all_lines(&w));
        // Every kind in turn, with nothing left over; the test world has ways between provinces.
        assert_eq!(ranges[KINDS.len() - 1].end as usize, lines.len());
        assert!(ranges.windows(2).all(|r| r[0].end == r[1].start));
        assert!(!ranges[Kind::Road as usize].is_empty());
        for l in &lines {
            let near_pole = l.a[1].abs().max(l.b[1].abs()) > 1.5;
            let dl = sim::grid::wrap_lon((l.b[0] - l.a[0]) as f64).abs();
            // Pieces are split evenly along the great circle, which is only roughly even in longitude.
            assert!(near_pole || dl <= MAX_DLON * 1.1, "segment spans {dl} rad of longitude");
        }
    }
}
