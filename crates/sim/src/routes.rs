//! Province neighbours and the ways between them, fixed by the land (roads v1).
//!
//! Two provinces are neighbours by land when they share at least one cell side, as the drawn
//! borders do (touching at a corner doesn't count). They are neighbours by sea when their coasts
//! face each other across water within [`SEA_REACH_KM`] of each shore. Water is every cell below
//! sea level, so lakes painted below it are crossed by boat too. A pair can be both.
//!
//! Each way has a travel time in days, from one province's seat to the other's. The seat is the
//! province's own land cell nearest its centre (the centre of a crescent-shaped province can lie
//! outside it). By land, a way goes from the seat across the province's own cells to the best
//! crossing of the shared border, then on to the other seat: slower on steep steps, and slower
//! where it gets onto or off a big river. By sea, it goes from the seat to a shore, takes ship (a
//! day in port), sails, and lands (another day). Every cost is the same in both directions.
//!
//! None of this is saved: it is worked out from the map whenever the map changes (see
//! [`World::rebuild_routes`]), so it can't disagree with the map. Roads that grow with traffic can
//! be a later version.

use crate::grid::{dot, Grid, NONE};
use crate::world::{World, RIVER_MIN_FLOW};
use std::cmp::Reverse;
use rayon::prelude::*;
use std::collections::{BTreeMap, BinaryHeap, HashMap};

/// A laden cart's pace on flat land.
pub const LAND_KM_PER_DAY: f64 = 30.0;
/// How much slopes slow travel: each metre climbed or descended takes as long as this many metres
/// on the flat, so a kilometre of climbing costs a day. Counted per step, so the total doesn't
/// depend on the size of the cells.
pub const SLOPE_SLOWING: f64 = 30.0;
/// Getting onto or off a big river, by ford or ferry: crossing one costs a day.
pub const RIVER_DAYS: f64 = 0.5;
/// A laden ship's pace.
pub const SEA_KM_PER_DAY: f64 = 100.0;
/// Loading or unloading a ship.
pub const PORT_DAYS: f64 = 1.0;
/// How far out from its own shore a province's ships go: two coasts are neighbours by sea when
/// the water between them is at most twice this across. Open oceans aren't crossed.
pub const SEA_REACH_KM: f64 = 400.0;
/// A way that has to leave the province (one cut in pieces by an edit) is this much slower on the
/// other provinces' land.
const DETOUR: f64 = 2.0;

/// One pair of provinces with a way between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    /// The lower-numbered province.
    pub a: u32,
    pub b: u32,
    /// Length of their shared border, 0 when they only face each other across water.
    pub border_km: f32,
    /// Days from seat to seat over land, if they share a border and a way over it exists.
    pub land_days: Option<f32>,
    /// Days from seat to seat by ship, if their coasts face each other.
    pub sea_days: Option<f32>,
}

impl Link {
    /// The quicker way, in days.
    pub fn days(&self) -> Option<f32> {
        match (self.land_days, self.sea_days) {
            (Some(l), Some(s)) => Some(l.min(s)),
            (l, s) => l.or(s),
        }
    }

    /// The province at the other end from `p`.
    pub fn other(&self, p: u32) -> u32 {
        if p == self.a { self.b } else { self.a }
    }
}

/// Every province's neighbours and the ways to them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Routes {
    /// Sorted by `(a, b)`, `a < b`.
    pub links: Vec<Link>,
    /// Each province's seat cell, [`NONE`] for a province with no land.
    pub seat: Vec<u32>,
    /// `of[first[p]..first[p + 1]]` are province `p`'s links, by the other province's number.
    first: Vec<u32>,
    of: Vec<u32>,
}

impl Routes {
    /// The links of province `p`, by the other province's number.
    pub fn of(&self, p: u32) -> impl Iterator<Item = &Link> + '_ {
        let (from, to) = match (self.first.get(p as usize), self.first.get(p as usize + 1)) {
            (Some(&f), Some(&t)) => (f as usize, t as usize),
            _ => (0, 0),
        };
        self.of[from..to].iter().map(|&k| &self.links[k as usize])
    }

    /// The link between two provinces, if they have one.
    pub fn between(&self, p: u32, q: u32) -> Option<&Link> {
        let (a, b) = (p.min(q), p.max(q));
        self.links.binary_search_by(|l| (l.a, l.b).cmp(&(a, b))).ok().map(|k| &self.links[k])
    }

    /// Work out every province's neighbours from the map.
    pub fn build(w: &World) -> Routes {
        let grid = &w.grid;
        let of_cell = &w.provinces.of_cell;
        let count = w.provinces.count;
        if of_cell.len() != grid.cells() || w.elevation.len() != grid.cells() {
            return Routes { first: vec![0; count + 1], seat: vec![NONE; count], ..Routes::default() };
        }
        let steps = Steps { grid, elevation: &w.elevation, river_flow: &w.river_flow, radius_km: w.params.radius_km };

        // Each province's cells, in cell order, and its seat.
        let mut cells_of: Vec<Vec<u32>> = vec![Vec::new(); count];
        for (c, &p) in of_cell.iter().enumerate() {
            if p != NONE && (p as usize) < count {
                cells_of[p as usize].push(c as u32);
            }
        }
        let seat: Vec<u32> = (0..count)
            .map(|p| {
                let centre = w.provinces.center[p];
                // The nearest cell; the lower number on a tie.
                cells_of[p].iter().copied().max_by(|&x, &y| dot(grid.center[x as usize], centre).total_cmp(&dot(grid.center[y as usize], centre)).then(y.cmp(&x))).unwrap_or(NONE)
            })
            .collect();

        let land = land_days(&steps, of_cell, &cells_of, &seat);

        // Land: the best crossing of each shared border.
        let mut found: BTreeMap<(u32, u32), Link> = BTreeMap::new();
        for (c, &p) in of_cell.iter().enumerate() {
            if p == NONE {
                continue;
            }
            for &m in &grid.neighbors[c] {
                let q = of_cell[m as usize];
                if q == NONE || q <= p {
                    continue;
                }
                let link = found.entry((p, q)).or_insert(Link { a: p, b: q, border_km: 0.0, land_days: None, sea_days: None });
                link.border_km += steps.side_km(c, m as usize) as f32;
                let days = land[c] + steps.land(c, m as usize) + land[m as usize];
                if days.is_finite() && link.land_days.is_none_or(|d| days < d as f64) {
                    link.land_days = Some(days as f32);
                }
            }
        }

        // Sea: ships spread out from every shore at once; each stretch of water is sailed by the
        // province whose ships reach it first. Where two provinces' waters meet, or one's waters
        // wash the other's shore, the two face each other.
        let cells = grid.cells();
        let mut sea = vec![f64::INFINITY; cells];
        let mut sea_km = vec![0f64; cells];
        let mut sailor = vec![NONE; cells];
        let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::new();
        for c in 0..cells {
            if of_cell[c] == NONE || land[c].is_infinite() {
                continue;
            }
            for s in steps.water_steps(c) {
                let d = land[c] + PORT_DAYS + steps.sail(c, s);
                if d < sea[s] || (d == sea[s] && of_cell[c] < sailor[s]) {
                    sea[s] = d;
                    sea_km[s] = steps.km(c, s);
                    sailor[s] = of_cell[c];
                    heap.push(Reverse((d.to_bits(), s as u32)));
                }
            }
        }
        let mut done = vec![false; cells];
        while let Some(Reverse((_, s))) = heap.pop() {
            let s = s as usize;
            if done[s] {
                continue;
            }
            done[s] = true;
            for t in steps.water_steps(s) {
                if done[t] {
                    continue;
                }
                let step = steps.km(s, t);
                let km = sea_km[s] + step;
                if km > SEA_REACH_KM {
                    continue;
                }
                let d = sea[s] + step / SEA_KM_PER_DAY;
                if d < sea[t] || (d == sea[t] && sailor[s] < sailor[t]) {
                    sea[t] = d;
                    sea_km[t] = km;
                    sailor[t] = sailor[s];
                    heap.push(Reverse((d.to_bits(), t as u32)));
                }
            }
        }
        let mut meet = |p: u32, q: u32, days: f64| {
            let (a, b) = (p.min(q), p.max(q));
            let link = found.entry((a, b)).or_insert(Link { a, b, border_km: 0.0, land_days: None, sea_days: None });
            if link.sea_days.is_none_or(|d| days < d as f64) {
                link.sea_days = Some(days as f32);
            }
        };
        for s in 0..cells {
            let p = sailor[s];
            if p == NONE {
                continue;
            }
            for t in grid.neighbors8(s) {
                let q = sailor[t];
                if q != NONE && q > p && steps.passable(s, t, true) {
                    meet(p, q, sea[s] + steps.sail(s, t) + sea[t]);
                }
                let q = of_cell[t];
                if q != NONE && q != p && land[t].is_finite() && steps.passable(s, t, true) {
                    meet(p, q, sea[s] + steps.sail(s, t) + PORT_DAYS + land[t]);
                }
            }
        }

        let links: Vec<Link> = found.into_values().collect();
        let mut first = vec![0u32; count + 1];
        for l in &links {
            first[l.a as usize + 1] += 1;
            first[l.b as usize + 1] += 1;
        }
        for p in 0..count {
            first[p + 1] += first[p];
        }
        let mut at: Vec<u32> = first.clone();
        let mut of = vec![0u32; first[count] as usize];
        // Links run in (a, b) order, so each province's list comes out by the other's number: the
        // lower neighbours (where it is `b`) all come before the higher ones.
        for (k, l) in links.iter().enumerate() {
            for p in [l.a, l.b] {
                of[at[p as usize] as usize] = k as u32;
                at[p as usize] += 1;
            }
        }
        Routes { links, seat, first, of }
    }
}

/// Days from each province's seat to each of its own cells, infinity where none reaches (sea, and
/// a piece of a province on land its seat isn't on).
fn land_days(steps: &Steps, of_cell: &[u32], cells_of: &[Vec<u32>], seat: &[u32]) -> Vec<f64> {
    // Provinces don't share cells, so each works out its own, all at once.
    let each: Vec<Vec<f64>> = cells_of
        .par_iter()
        .enumerate()
        .map(|(p, own)| {
            if seat[p] == NONE {
                return Vec::new();
            }
            let mut days = steps.spread(seat[p] as usize, own);
            if days.iter().any(|d| d.is_infinite()) {
                // An edit cut the province in pieces: reach the rest over other land, more slowly.
                for (c, d) in steps.detour(seat[p] as usize, p as u32, of_cell, own) {
                    let k = own.binary_search(&(c as u32)).expect("one of its own cells");
                    if days[k].is_infinite() {
                        days[k] = d;
                    }
                }
            }
            days
        })
        .collect();
    let mut land = vec![f64::INFINITY; of_cell.len()];
    for (own, days) in cells_of.iter().zip(each) {
        for (&c, d) in own.iter().zip(days) {
            land[c as usize] = d;
        }
    }
    land
}

/// The cost of single steps between neighbouring cells.
struct Steps<'a> {
    grid: &'a Grid,
    elevation: &'a [f32],
    river_flow: &'a [u64],
    radius_km: f64,
}

impl Steps<'_> {
    fn km(&self, a: usize, b: usize) -> f64 {
        dot(self.grid.center[a], self.grid.center[b]).clamp(-1.0, 1.0).acos() * self.radius_km
    }

    /// The length of the side two edge neighbours share, roughly: the cells' mean width.
    fn side_km(&self, a: usize, b: usize) -> f64 {
        (self.grid.area[a].sqrt() + self.grid.area[b].sqrt()) / 2.0 * self.radius_km
    }

    fn land_cell(&self, c: usize) -> bool {
        self.elevation[c] >= 0.0
    }

    fn river(&self, c: usize) -> bool {
        self.river_flow.get(c).is_some_and(|&f| f >= RIVER_MIN_FLOW)
    }

    /// Whether a step between neighbours `a` and `b` is allowed: a diagonal step passes a corner,
    /// and needs land on one side of it (over land) or water on one side (by sea), so a way never
    /// jumps a strait or an isthmus.
    fn passable(&self, a: usize, b: usize, by_sea: bool) -> bool {
        let g = self.grid;
        if g.neighbors[a].contains(&(b as u32)) {
            return true;
        }
        g.neighbors[a].iter().any(|&m| g.neighbors[b].contains(&m) && self.land_cell(m as usize) != by_sea)
    }

    /// Days to walk from land cell `a` to land cell `b`. The same both ways.
    fn land(&self, a: usize, b: usize) -> f64 {
        let km = self.km(a, b);
        let climb = (self.elevation[b] - self.elevation[a]).abs() as f64 / 1000.0;
        let river = if self.river(a) != self.river(b) { RIVER_DAYS } else { 0.0 };
        (km + SLOPE_SLOWING * climb) / LAND_KM_PER_DAY + river
    }

    /// Days to sail between neighbouring cells.
    fn sail(&self, a: usize, b: usize) -> f64 {
        self.km(a, b) / SEA_KM_PER_DAY
    }

    /// The water cells a ship can reach from cell `c` in one step.
    fn water_steps(&self, c: usize) -> impl Iterator<Item = usize> + '_ {
        self.grid.neighbors8(c).filter(move |&t| !self.land_cell(t) && self.passable(c, t, true))
    }

    /// Cheapest days over land from `seat` to each of `own` (its province's cells, in cell order),
    /// staying inside them: infinity where that can't reach. Ties go to the lower cell number.
    fn spread(&self, seat: usize, own: &[u32]) -> Vec<f64> {
        let at = |c: usize| own.binary_search(&(c as u32)).ok();
        let mut days = vec![f64::INFINITY; own.len()];
        let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::new();
        days[at(seat).expect("the seat is its own")] = 0.0;
        heap.push(Reverse((0f64.to_bits(), seat as u32)));
        while let Some(Reverse((bits, c))) = heap.pop() {
            let (c, here) = (c as usize, f64::from_bits(bits));
            if here > days[at(c).expect("only its own cells are queued")] {
                continue;
            }
            for nb in self.grid.neighbors8(c) {
                let Some(k) = at(nb) else { continue };
                if !self.passable(c, nb, false) {
                    continue;
                }
                let d = here + self.land(c, nb);
                if d < days[k] {
                    days[k] = d;
                    heap.push(Reverse((d.to_bits(), nb as u32)));
                }
            }
        }
        days
    }

    /// Days from `seat` to each of `own` (province `p`'s cells) over any land, other provinces'
    /// land costing [`DETOUR`] times as much. Stops once all of `own` it can reach are reached.
    fn detour(&self, seat: usize, p: u32, of_cell: &[u32], own: &[u32]) -> Vec<(usize, f64)> {
        let mut days: HashMap<usize, f64> = HashMap::from([(seat, 0.0)]);
        let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::from([Reverse((0f64.to_bits(), seat as u32))]);
        let mut left = own.len();
        let mut settled = std::collections::HashSet::new();
        while let Some(Reverse((bits, c))) = heap.pop() {
            let c = c as usize;
            if !settled.insert(c) {
                continue;
            }
            if of_cell[c] == p {
                left -= 1;
                if left == 0 {
                    break;
                }
            }
            let at = f64::from_bits(bits);
            for nb in self.grid.neighbors8(c) {
                if of_cell[nb] == NONE || settled.contains(&nb) || !self.passable(c, nb, false) {
                    continue;
                }
                let slower = if of_cell[nb] == p { 1.0 } else { DETOUR };
                let d = at + self.land(c, nb) * slower;
                if days.get(&nb).is_none_or(|&old| d < old) {
                    days.insert(nb, d);
                    heap.push(Reverse((d.to_bits(), nb as u32)));
                }
            }
        }
        own.iter().filter_map(|&c| days.get(&(c as usize)).map(|&d| (c as usize, d))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WorldParams;

    fn small() -> World {
        let mut w = World::generate(&WorldParams { n: 48, provinces: 120, seed: 11, land_fraction: 0.36, radius_km: 6371.0 });
        w.rebuild_routes();
        w
    }

    #[test]
    fn land_neighbours_are_exactly_the_provinces_that_share_a_side() {
        let w = small();
        let r = &w.routes;
        let g = &w.grid;
        let mut sides = std::collections::BTreeSet::new();
        for c in 0..g.cells() {
            let p = w.provinces.of_cell[c];
            for &m in &g.neighbors[c] {
                let q = w.provinces.of_cell[m as usize];
                if p != NONE && q != NONE && p != q {
                    sides.insert((p.min(q), p.max(q)));
                }
            }
        }
        let by_land: std::collections::BTreeSet<(u32, u32)> = r.links.iter().filter(|l| l.border_km > 0.0).map(|l| (l.a, l.b)).collect();
        assert_eq!(by_land, sides);
        for l in &r.links {
            assert!(l.a < l.b);
            assert!(l.days().is_some_and(|d| d.is_finite() && d > 0.0), "{l:?}");
            // Sharing a border means a way over it: these provinces are all in one piece.
            assert_eq!(l.border_km > 0.0, l.land_days.is_some(), "{l:?}");
            assert!(r.of(l.a).any(|m| m == l) && r.of(l.b).any(|m| m == l));
            assert_eq!(r.between(l.b, l.a), Some(l));
        }
        for p in 0..w.provinces.count as u32 {
            let others: Vec<u32> = r.of(p).map(|l| l.other(p)).collect();
            assert!(others.is_sorted(), "{others:?}");
            assert!(w.provinces.of_cell[r.seat[p as usize] as usize] == p);
        }
        assert!(r.links.iter().any(|l| l.sea_days.is_some() && l.land_days.is_none()), "some coasts face each other across water");
    }

    #[test]
    fn only_lone_islands_have_no_neighbours() {
        let w = small();
        for p in 0..w.provinces.count as u32 {
            if w.routes.of(p).next().is_some() {
                continue;
            }
            // Every land cell around it is its own: a one-province island, far out at sea.
            let g = &w.grid;
            for c in (0..g.cells()).filter(|&c| w.provinces.of_cell[c] == p) {
                assert!(g.neighbors[c].iter().all(|&m| w.provinces.of_cell[m as usize] == p || w.provinces.of_cell[m as usize] == NONE));
            }
        }
    }

    #[test]
    fn steep_land_and_big_rivers_slow_the_way() {
        let w = small();
        let steps = Steps { grid: &w.grid, elevation: &w.elevation, river_flow: &w.river_flow, radius_km: 6371.0 };
        let c = (0..w.grid.cells()).find(|&c| w.elevation[c] >= 0.0).unwrap();
        let m = w.grid.neighbors[c][0] as usize;
        let mut flat = w.elevation.clone();
        flat[c] = 100.0;
        flat[m] = 100.0;
        let mut steep = flat.clone();
        steep[m] = 1100.0;
        let mut river = vec![0; w.grid.cells()];
        let of = |e: &[f32], r: &[u64]| Steps { elevation: e, river_flow: r, ..steps }.land(c, m);
        let base = of(&flat, &river);
        assert!((base - steps.km(c, m) / LAND_KM_PER_DAY).abs() < 1e-9);
        // A kilometre's climb costs a day.
        assert!((of(&steep, &river) - base - 1.0).abs() < 1e-6);
        assert_eq!(of(&steep, &river), Steps { elevation: &steep, river_flow: &river, ..steps }.land(m, c));
        river[m] = RIVER_MIN_FLOW;
        assert_eq!(of(&flat, &river), base + RIVER_DAYS);
        river[c] = RIVER_MIN_FLOW;
        assert_eq!(of(&flat, &river), base, "going along a river crosses nothing");
    }

    #[test]
    fn a_province_cut_in_two_still_reaches_its_far_piece() {
        let w = small();
        let p = (0..w.provinces.count).max_by_key(|&p| w.provinces.cells[p]).unwrap() as u32;
        let seat = w.routes.seat[p as usize];
        let g = &w.grid;
        let mut cells_of: Vec<Vec<u32>> = vec![Vec::new(); w.provinces.count];
        for (c, &q) in w.provinces.of_cell.iter().enumerate() {
            if q != NONE {
                cells_of[q as usize].push(c as u32);
            }
        }
        // Keep only the seat and the cell farthest from it; give the rest to a neighbour.
        let far = *cells_of[p as usize].iter().min_by(|&&x, &&y| dot(g.center[x as usize], g.center[seat as usize]).total_cmp(&dot(g.center[y as usize], g.center[seat as usize]))).unwrap();
        assert!(!g.neighbors[seat as usize].contains(&far));
        let q = w.routes.of(p).find(|l| l.border_km > 0.0).unwrap().other(p);
        let mut of_cell = w.provinces.of_cell.clone();
        for &c in &cells_of[p as usize] {
            if c != seat && c != far {
                of_cell[c as usize] = q;
            }
        }
        let moved: Vec<u32> = cells_of[p as usize].iter().copied().filter(|&c| c != seat && c != far).collect();
        cells_of[q as usize].extend(moved);
        cells_of[q as usize].sort_unstable();
        cells_of[p as usize] = vec![seat.min(far), seat.max(far)];
        let steps = Steps { grid: g, elevation: &w.elevation, river_flow: &w.river_flow, radius_km: 6371.0 };
        let land = land_days(&steps, &of_cell, &cells_of, &w.routes.seat);
        // The far piece is reached over the neighbour's land, more slowly than a walk straight there.
        let straight = steps.km(seat as usize, far as usize) / LAND_KM_PER_DAY;
        assert!(land[far as usize].is_finite() && land[far as usize] > straight, "{} days", land[far as usize]);
        assert_eq!(land[seat as usize], 0.0);
    }
}
