//! Edits: authorial changes that set values directly and bypass the world's rules ("this province
//! has 40,000 people"). A poke, by contrast, is an event the world reacts to by its own rules.
//!
//! Edits are commands like pokes: day-stamped and kept in the command log, so replays, saves and
//! undo repeat them. An edit logged for a day takes effect at the start of that day, before anything
//! runs, so the world shown for the day already has it.
//!
//! Applying an edit has three parts:
//!
//! 1. [`Edit::check`] refuses an edit that doesn't fit the world as it is. Nothing has changed yet.
//! 2. The map part changes cell and province-shape fields (the ones world generation writes). It
//!    depends on the map alone, so the engine keeps what it changed as a [`MapChange`]: going back
//!    puts the old values back, and replaying the same edit reuses the change instead of repairing
//!    the map again.
//! 3. The running part changes pops and stores, and reports every person or ration it creates or
//!    removes as a flow with [`Cause::Edit`], so the conservation check balances.
//!
//! Pops are only ever added, never removed or reordered: a grant on its way finds its pop by
//! position. A pop emptied by an edit keeps its place with nobody in it. Province numbers never
//! change either: a province merged away stays as an empty placeholder (owned by no realm; see
//! [`crate::realms::repair`]), and a new one is numbered after the last.
//!
//! When land changes province, its people and stored goods go with it, in proportion to the
//! farmland that moved: a province that gives away a third of its farmland gives away a third of
//! each pop and a third of its store of each good. Land with no farmland at all is shared by area
//! instead. Shares are whole people and units of goods by the largest-remainder rule, so nothing
//! is lost or made.

use crate::fields::{known, FieldData, FieldId, Quantity};
use crate::grid::{dot, lonlat_to_dir, Grid, NONE, V3};
use crate::module::Flows;
use crate::pops::{Pops, HUMAN};
use crate::requests::{split_proportional, Cause};
use crate::routes::{Routes, RoutesChange};
use crate::rng::{draw, mix, purpose, Module as Rng};
use crate::terrain::fbm;
use crate::world::World;
use rayon::prelude::*;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// A pop's size and what it is, as an edit sets them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopValues {
    pub size: u64,
    pub species: u16,
    pub culture: u16,
    pub religion: u16,
    pub class: u8,
}

impl PopValues {
    /// `size` humans of the first culture, religion and class.
    pub fn people(size: u64) -> PopValues {
        PopValues { size, species: HUMAN, culture: 0, religion: 0, class: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    /// Set one pop's size and what it is.
    SetPop { pop: u32, to: PopValues },
    /// A new pop in a province.
    AddPop { province: u32, values: PopValues },
    /// Set a province's food store: [`Edit::SetStock`] of "food", as older scenarios write it.
    SetFoodStock { province: u32, rations: u64 },
    /// Land cells join province `to` (redrawing a border).
    MoveCells { cells: Vec<u32>, to: u32 },
    /// Land cells become a new province, numbered after the last.
    Split { cells: Vec<u32> },
    /// Province `from` joins `into`, and stays as an empty placeholder.
    Merge { from: u32, into: u32 },
    /// One brush stroke on the land's height: the brush laid down at each point of `path`
    /// (longitude and latitude, degrees), `radius_km` across from its middle to its edge. Points
    /// should be at most half a radius apart.
    Paint { path: Vec<[f64; 2]>, radius_km: f64, brush: Brush },
    /// The sea rises by this many metres everywhere (falls, if negative).
    SeaLevel { rise_m: f32 },
    /// A brush stroke that sets the climate or the biome where it passes. The climate and biome
    /// modules keep what was set over what they work out, until a stroke clears it.
    Override { path: Vec<[f64; 2]>, radius_km: f64, set: Override },
    /// A riverbed carved along `path` (longitude and latitude, degrees), from its source to its
    /// mouth: it falls all the way and lies below its banks, so water follows it. Starting it on a
    /// river redirects that river. `spring` adds water at the source (thousand m³ a year), so a
    /// river can be drawn where little rain falls; with 0 it carries only the water already there.
    River { path: Vec<[f64; 2]>, spring: u64 },
    /// Set how much of one good a province has stored. The good is named as the goods tree names
    /// it, and must be one the world keeps on its own. (Last, so saved edits keep their numbers.)
    SetStock { province: u32, good: String, amount: u64 },
}

/// What an override stroke sets.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Override {
    /// Annual mean temperature, °C, blended into what was there at the brush's soft edge.
    Temperature(f32),
    /// Annual rainfall, mm, blended likewise.
    Rainfall(f32),
    /// A land biome (its number in [`crate::biome::ALL`]), on land wherever the brush reaches at
    /// least half strength.
    Biome(u8),
    /// Clear every override the brush reaches.
    Clear,
}

/// What a brush does to the land's height, at full strength: in the middle half of the brush. It
/// fades to nothing at the brush's edge.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Brush {
    /// Raise the land by this many metres.
    Raise(f32),
    /// Lower it by this many metres.
    Lower(f32),
    /// Move each height this share (0 to 1) of the way to the average of its neighbours.
    Smooth(f32),
    /// Add hills and hollows up to this many metres high or deep.
    Roughen(f32),
    /// Set the height to this many metres. Below 0 makes sea: an inland sea, a lake at sea level,
    /// or a bay.
    Flatten(f32),
}

impl Edit {
    /// Refuse an edit that doesn't fit the world as it is.
    pub fn check(&self, w: &World) -> Result<(), String> {
        let province = |p: u32| if (p as usize) < w.provinces.count { Ok(()) } else { Err(format!("no province {p}")) };
        let count = |what: &str, n: u64| {
            let max = known(what).def().max;
            if (n as f64) <= max { Ok(()) } else { Err(format!("{n} is more than {what} can hold")) }
        };
        match *self {
            Edit::SetPop { pop, to } => {
                if pop as usize >= w.pops.size.len() {
                    return Err(format!("no pop {pop}"));
                }
                count("pop.size", to.size)
            }
            Edit::AddPop { province: p, values } => {
                province(p)?;
                if w.provinces.cells[p as usize] == 0 {
                    return Err(format!("province {p} has no land"));
                }
                count("pop.size", values.size)
            }
            Edit::SetFoodStock { province: p, rations } => {
                province(p)?;
                stock_column(w, "food")?;
                count("province.stock", rations)
            }
            Edit::SetStock { province: p, ref good, amount } => {
                province(p)?;
                stock_column(w, good)?;
                count("province.stock", amount)
            }
            Edit::MoveCells { ref cells, to } => {
                province(to)?;
                land(w, cells)?;
                if cells.iter().all(|&c| w.provinces.of_cell[c as usize] == to) {
                    return Err(format!("those cells are already in province {to}"));
                }
                Ok(())
            }
            Edit::Split { ref cells } => land(w, cells),
            Edit::Merge { from, into } => {
                province(from)?;
                province(into)?;
                if from == into {
                    return Err("a province can't merge into itself".into());
                }
                if w.provinces.cells[from as usize] == 0 {
                    return Err(format!("province {from} has no land"));
                }
                Ok(())
            }
            Edit::River { ref path, .. } => {
                stroke(path, 1.0)?;
                let cells = river_cells(&w.grid, path);
                if cells.len() < 2 || w.elevation[cells[0]] < 0.0 {
                    return Err("a river needs a source on land and somewhere to run".into());
                }
                Ok(())
            }
            Edit::Override { ref path, radius_km, set } => {
                stroke(path, radius_km)?;
                let within = |what: &str, x: f32| {
                    let d = known(what).def();
                    if (d.min..=d.max).contains(&(x as f64)) { Ok(()) } else { Err(format!("{x} is outside {what}'s {} to {}", d.min, d.max)) }
                };
                match set {
                    Override::Temperature(t) => within("cell.temperature", t),
                    Override::Rainfall(r) => within("cell.rainfall", r),
                    Override::Biome(b) if b == crate::biome::Biome::Ocean as u8 => Err("paint sea by lowering the land".into()),
                    Override::Biome(b) if b as usize >= crate::biome::ALL.len() => Err(format!("no biome {b}")),
                    Override::Biome(_) | Override::Clear => Ok(()),
                }
            }
            Edit::Paint { ref path, radius_km, brush } => {
                stroke(path, radius_km)?;
                let x = match brush {
                    Brush::Raise(x) | Brush::Lower(x) | Brush::Roughen(x) | Brush::Flatten(x) => x,
                    Brush::Smooth(x) if !(0.0..=1.0).contains(&x) => return Err("smoothing goes from 0 to 1".into()),
                    Brush::Smooth(x) => x,
                };
                if x.is_finite() { Ok(()) } else { Err("the brush's strength must be a number".into()) }
            }
            Edit::SeaLevel { rise_m } => if rise_m.is_finite() { Ok(()) } else { Err("the rise must be a number".into()) },
        }
    }

    /// Whether the edit changes the map (cell and province-shape fields).
    pub fn changes_map(&self) -> bool {
        match self {
            Edit::SetPop { .. } | Edit::AddPop { .. } | Edit::SetFoodStock { .. } | Edit::SetStock { .. } => false,
            Edit::MoveCells { .. } | Edit::Split { .. } | Edit::Merge { .. } | Edit::Paint { .. } | Edit::SeaLevel { .. } | Edit::Override { .. } | Edit::River { .. } => true,
        }
    }

    /// The map part. Depends on the map alone; called after [`Edit::check`] passed. `regenerate`
    /// works out again everything world generation derives from the land's height, except the
    /// provinces (climate, biomes, rivers, farmland), with the world's own module versions.
    pub(crate) fn change_map(&self, w: &mut World, regenerate: &mut dyn FnMut(&mut World)) {
        let count = w.provinces.count;
        let of_cell = &mut w.provinces.of_cell;
        let count = match *self {
            Edit::SetPop { .. } | Edit::AddPop { .. } | Edit::SetFoodStock { .. } | Edit::SetStock { .. } => return,
            Edit::Paint { ref path, radius_km, brush } => {
                paint(w, path, radius_km, brush);
                return reshape_after_height(w, regenerate);
            }
            Edit::SeaLevel { rise_m } => {
                w.elevation.iter_mut().for_each(|h| *h -= rise_m);
                return reshape_after_height(w, regenerate);
            }
            Edit::Override { ref path, radius_km, set } => {
                paint_override(w, path, radius_km, set);
                return reshape_after_height(w, regenerate);
            }
            Edit::River { ref path, spring } => {
                let cells = river_cells(&w.grid, path);
                carve(w, &cells);
                w.spring[cells[0]] += spring;
                return reshape_after_height(w, regenerate);
            }
            Edit::MoveCells { ref cells, to } => {
                cells.iter().for_each(|&c| of_cell[c as usize] = to);
                count
            }
            Edit::Split { ref cells } => {
                cells.iter().for_each(|&c| of_cell[c as usize] = count as u32);
                count + 1
            }
            Edit::Merge { from, into } => {
                of_cell.iter_mut().filter(|p| **p == from).for_each(|p| *p = into);
                count
            }
        };
        crate::provinces::reaggregate(w, count);
    }

    /// The running part, after the map part (`map` is what it changed). Returns what was created
    /// or removed. Called after [`Edit::check`] passed.
    pub(crate) fn change_running(&self, w: &mut World, map: Option<&MapChange>) -> Flows {
        let mut flows = Flows::default();
        let delta = |before: u64, after: u64| after as i64 - before as i64;
        match *self {
            Edit::SetPop { pop, to } => {
                let k = pop as usize;
                flows.add(Quantity::People, Cause::Edit, delta(w.pops.size[k], to.size));
                let p = &mut w.pops;
                (p.size[k], p.species[k], p.culture[k], p.religion[k], p.class[k]) = (to.size, to.species, to.culture, to.religion, to.class);
            }
            Edit::AddPop { province, values: v } => {
                flows.add(Quantity::People, Cause::Edit, v.size as i64);
                let p = &mut w.pops;
                p.push(province, v.size);
                let k = p.size.len() - 1;
                (p.species[k], p.culture[k], p.religion[k], p.class[k]) = (v.species, v.culture, v.religion, v.class);
            }
            Edit::SetFoodStock { province, rations: amount } => set_stock(w, province, "food", amount, &mut flows),
            Edit::SetStock { province, ref good, amount } => set_stock(w, province, good, amount, &mut flows),
            Edit::MoveCells { .. } | Edit::Split { .. } | Edit::Merge { .. } => {
                w.grow_province_fields();
                let moved = map.expect("a province edit changes the map").moved_cells();
                if let Edit::Split { .. } = self {
                    // The new province's land had the rain, and the owner, of the province that gave
                    // the most of it.
                    let mut from: Vec<u32> = moved.iter().map(|m| m.1).collect();
                    from.sort_unstable();
                    if let Some(run) = from.chunk_by(|a, b| a == b).max_by_key(|run| run.len()) {
                        let new = w.provinces.count - 1;
                        w.rain_shortfall[new] = w.rain_shortfall[run[0] as usize];
                        w.owner[new] = w.owner[run[0] as usize];
                    }
                }
                share_out(w, &moved);
                crate::realms::repair(w);
            }
            Edit::Paint { .. } | Edit::SeaLevel { .. } | Edit::Override { .. } | Edit::River { .. } => {
                w.grow_province_fields();
                let emptied = map.expect("a height edit changes the map").emptied_provinces();
                flows = rehouse(w, &emptied);
                crate::realms::repair(w);
            }
        }
        flows
    }

    /// The provinces it is about, in the world as it is before the edit.
    pub fn provinces(&self, w: &World) -> Vec<u32> {
        let of = |cells: &[u32], also: u32| {
            let mut v: Vec<u32> = cells.iter().filter_map(|&c| w.provinces.of_cell.get(c as usize).copied()).filter(|&p| p != NONE).collect();
            v.push(also);
            v.sort_unstable();
            v.dedup();
            v
        };
        match *self {
            Edit::SetPop { pop, .. } => w.pops.province.get(pop as usize).copied().into_iter().collect(),
            Edit::AddPop { province, .. } | Edit::SetFoodStock { province, .. } | Edit::SetStock { province, .. } => vec![province],
            Edit::MoveCells { ref cells, to } => of(cells, to),
            Edit::Split { ref cells } => of(cells, w.provinces.count as u32),
            Edit::Merge { from, into } => vec![from, into],
            Edit::Paint { ref path, radius_km, .. } | Edit::Override { ref path, radius_km, .. } => {
                let reach = brush_reach(&w.grid, path, radius_km / w.params.radius_km);
                let mut v: Vec<u32> = (0..reach.len()).filter(|&c| reach[c] > 0.0).map(|c| w.provinces.of_cell[c]).filter(|&p| p != NONE).collect();
                v.sort_unstable();
                v.dedup();
                v
            }
            Edit::SeaLevel { .. } => (0..w.provinces.count as u32).collect(),
            Edit::River { ref path, .. } => {
                let mut v: Vec<u32> = river_cells(&w.grid, path).into_iter().map(|c| w.provinces.of_cell[c]).filter(|&p| p != NONE).collect();
                v.sort_unstable();
                v.dedup();
                v
            }
        }
    }

    /// The edit in words, for the inspector.
    pub fn describe(&self) -> String {
        match self {
            Edit::SetPop { pop, to } => format!("pop {pop} set to {} people", to.size),
            Edit::AddPop { province, values } => format!("a pop of {} people added to province {province}", values.size),
            Edit::SetFoodStock { province, rations } => format!("province {province}'s food store set to {rations} rations"),
            Edit::SetStock { province, good, amount } => format!("province {province}'s store of {good} set to {amount}"),
            Edit::MoveCells { cells, to } => format!("{} cells of land moved into province {to}", cells.len()),
            Edit::Split { cells } => format!("{} cells of land made a new province", cells.len()),
            Edit::Merge { from, into } => format!("province {from} merged into province {into}"),
            Edit::Paint { path, radius_km, brush } => {
                let what = match brush {
                    Brush::Raise(m) => format!("land raised by up to {m:.0} m"),
                    Brush::Lower(m) => format!("land lowered by up to {m:.0} m"),
                    Brush::Smooth(a) => format!("land smoothed ({:.0}%)", a * 100.0),
                    Brush::Roughen(m) => format!("land roughened by up to {m:.0} m"),
                    Brush::Flatten(m) => format!("land set to {m:.0} m"),
                };
                format!("{what} along a {}-point stroke {radius_km:.0} km wide", path.len())
            }
            Edit::SeaLevel { rise_m } if *rise_m >= 0.0 => format!("the sea rose {rise_m:.0} m"),
            Edit::SeaLevel { rise_m } => format!("the sea fell {:.0} m", -rise_m),
            Edit::Override { path, radius_km, set } => {
                let what = match set {
                    Override::Temperature(t) => format!("temperature set to {t:.0} °C"),
                    Override::Rainfall(r) => format!("rainfall set to {r:.0} mm a year"),
                    Override::Biome(b) => format!("biome set to {}", crate::biome::ALL.get(*b as usize).map_or("?", |b| b.name())),
                    Override::Clear => "painted climate and biomes cleared".into(),
                };
                format!("{what} along a {}-point stroke {radius_km:.0} km wide", path.len())
            }
            Edit::River { path, spring: 0 } => format!("a riverbed carved along {} points", path.len()),
            Edit::River { path, spring } => format!("a river drawn along {} points, its spring adding {spring} thousand m³ a year", path.len()),
        }
    }

    /// The fields it sets directly, which a module must own for it to mean anything: an edit that
    /// sets the food store in a world where nobody keeps goods would set a number nothing reads or
    /// runs. Map edits list none: they move whatever there is, which is nothing when nobody keeps it.
    pub fn sets(&self) -> &'static [&'static str] {
        match self {
            Edit::SetPop { .. } | Edit::AddPop { .. } => &["pop.size"],
            Edit::SetFoodStock { .. } | Edit::SetStock { .. } => &["province.stock"],
            Edit::MoveCells { .. } | Edit::Split { .. } | Edit::Merge { .. } | Edit::Paint { .. } | Edit::SeaLevel { .. } | Edit::Override { .. } | Edit::River { .. } => &[],
        }
    }
}

/// The column of the stores that keeps the good named `good` on its own, or why there is none.
fn stock_column(w: &World, good: &str) -> Result<usize, String> {
    let g = &w.goods;
    if let Some(c) = g.column(good) {
        return Ok(c);
    }
    let node = g.tree.find(good).ok_or_else(|| format!("there is no good called {good}"))?;
    match g.columns_under(node) {
        None => {
            let whole = g.columns.iter().find(|&&c| g.tree.is_under(node, c)).map_or("?", |&c| g.tree.name(c));
            Err(format!("{good} is kept only as part of {whole}; set the {whole} store instead"))
        }
        Some(cols) if cols.is_empty() => Err(format!("this world keeps no {good}")),
        Some(cols) => {
            let names: Vec<&str> = cols.iter().map(|&c| g.tree.name(g.columns[c])).collect();
            Err(format!("{good} is kept as {}; set each of those", list(&names)))
        }
    }
}

/// "a", "a and b", "a, b and c".
fn list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Set province `p`'s store of `good` (checked: kept on its own) and report the change.
fn set_stock(w: &mut World, p: u32, good: &str, amount: u64, flows: &mut Flows) {
    let c = stock_column(w, good).expect("checked: the good is kept");
    let at = w.goods.index(p as usize, c);
    let class = Quantity::Goods(w.goods.class_of_column(c));
    flows.add(class, Cause::Edit, amount as i64 - w.stock[at] as i64);
    w.stock[at] = amount;
}

/// The cells a river drawn along `path` runs through, source first: each one of the eight
/// neighbours of the one before, no cell twice, ending at the first sea cell it reaches.
pub fn river_cells(grid: &Grid, path: &[[f64; 2]]) -> Vec<usize> {
    let dirs: Vec<V3> = path.iter().map(|p| lonlat_to_dir(p[0].to_radians(), p[1].to_radians())).collect();
    let step = grid.mean_area().sqrt() / 4.0;
    let mut out: Vec<usize> = Vec::new();
    let add = |c: usize, out: &mut Vec<usize>| match out.last() {
        Some(&l) if l == c => {}
        // A path that comes back on itself drops the loop.
        _ if out.contains(&c) => {
            let k = out.iter().position(|&x| x == c).expect("contained");
            out.truncate(k + 1);
        }
        Some(&l) if !grid.neighbors8(l).any(|m| m == c) => {}
        _ => out.push(c),
    };
    if let Some(&d) = dirs.first() {
        add(grid.dir_to_cell(d), &mut out);
    }
    for pair in dirs.windows(2) {
        let angle = dot(pair[0], pair[1]).clamp(-1.0, 1.0).acos();
        let steps = (angle / step).ceil().max(1.0) as usize;
        for k in 1..=steps {
            let t = k as f64 / steps as f64;
            // Along the great circle, near enough for short steps.
            let p = crate::grid::normalize(std::array::from_fn(|i| pair[0][i] * (1.0 - t) + pair[1][i] * t));
            add(grid.dir_to_cell(p), &mut out);
        }
    }
    out
}

/// Carve a riverbed through `cells`, source first: each cell at least 1 cm lower than the one
/// before and, where the land allows, 1 m below its banks (its neighbours off the river), while
/// staying above the sea. The bed ends at the first sea cell.
fn carve(w: &mut World, cells: &[usize]) {
    let n = cells.len();
    let end = cells.iter().position(|&c| w.elevation[c] < 0.0).unwrap_or(n);
    let mut prev = f32::INFINITY;
    for (i, &c) in cells[..end].iter().enumerate() {
        let bank = w.grid.neighbors8(c).filter(|m| !cells.contains(m)).map(|m| w.elevation[m]).fold(f32::INFINITY, f32::min);
        // Room left above the sea for the rest of the bed to keep falling.
        let floor = 0.01 * (end - i) as f32;
        w.elevation[c] = w.elevation[c].min(bank - 1.0).min(prev - 0.01).max(floor);
        prev = w.elevation[c];
    }
}

/// Set (or clear) painted climate or biomes along a stroke.
fn paint_override(w: &mut World, path: &[[f64; 2]], radius_km: f64, set: Override) {
    let reach = brush_reach(&w.grid, path, radius_km / w.params.radius_km);
    for c in (0..reach.len()).filter(|&c| reach[c] > 0.0) {
        let r = reach[c];
        match set {
            Override::Temperature(t) => w.temperature_set[c] = w.temperature[c] + r * (t - w.temperature[c]),
            Override::Rainfall(x) => w.rainfall_set[c] = w.rainfall[c] + r * (x - w.rainfall[c]),
            Override::Biome(b) if r >= 0.5 && w.elevation[c] >= 0.0 => w.biome_set[c] = b,
            Override::Biome(_) => {}
            Override::Clear => {
                (w.temperature_set[c], w.rainfall_set[c]) = (f32::NAN, f32::NAN);
                w.biome_set[c] = crate::biome::NOT_SET;
            }
        }
    }
}

/// The cells a brush stroke covers at least half strongly (out to three quarters of its radius):
/// what a province stroke moves.
pub fn brush_cells(w: &World, path: &[[f64; 2]], radius_km: f64) -> Vec<u32> {
    let reach = brush_reach(&w.grid, path, radius_km / w.params.radius_km);
    (0..reach.len() as u32).filter(|&c| reach[c as usize] >= 0.5).collect()
}

/// How strongly a brush laid along `path` reaches each cell, 0 to 1: fully in the middle half of
/// the brush around each point, fading smoothly to nothing at `radius` (radians).
fn brush_reach(grid: &Grid, path: &[[f64; 2]], radius: f64) -> Vec<f32> {
    let points: Vec<V3> = path.iter().map(|p| lonlat_to_dir(p[0].to_radians(), p[1].to_radians())).collect();
    let near = radius.min(std::f64::consts::PI).cos();
    grid.center
        .par_iter()
        .map(|&d| {
            let mut best = 0.0f64;
            for &p in &points {
                let cos = dot(d, p);
                if cos > near {
                    let x = cos.clamp(-1.0, 1.0).acos() / radius;
                    let t = ((1.0 - x) / 0.5).clamp(0.0, 1.0);
                    best = best.max(t * t * (3.0 - 2.0 * t));
                }
            }
            best as f32
        })
        .collect()
}

/// Lay a brush stroke on the land's height, then keep heights inside their field's bounds.
fn paint(w: &mut World, path: &[[f64; 2]], radius_km: f64, brush: Brush) {
    let radius = radius_km / w.params.radius_km;
    let reach = brush_reach(&w.grid, path, radius);
    let old = w.elevation.clone();
    let grid = &w.grid;
    // Bumps about a fifth of the brush across, different for every stroke.
    let stroke = path.iter().flatten().fold(0u64, |h, x| mix(h ^ x.to_bits()));
    let key = draw(w.params.seed, Rng::Editor, 0, stroke, purpose("roughen"));
    let freq = 5.0 / radius;
    w.elevation.par_iter_mut().enumerate().filter(|(c, _)| reach[*c] > 0.0).for_each(|(c, h)| {
        let r = reach[c];
        *h = match brush {
            Brush::Raise(m) => old[c] + m * r,
            Brush::Lower(m) => old[c] - m * r,
            Brush::Smooth(a) => {
                let (sum, n) = grid.neighbors8(c).fold((0.0f64, 0u32), |(s, n), m| (s + old[m] as f64, n + 1));
                old[c] + a * r * ((sum / n as f64) as f32 - old[c])
            }
            Brush::Roughen(m) => {
                let d = grid.center[c];
                old[c] + m * r * fbm([d[0] * freq, d[1] * freq, d[2] * freq], 3, key) as f32
            }
            Brush::Flatten(m) => old[c] + r * (m - old[c]),
        };
    });
    let def = known("cell.elevation").def();
    w.elevation.iter_mut().for_each(|h| *h = h.clamp(def.min as f32, def.max as f32));
}

/// After the land's height changed: work out climate, biomes, rivers and farmland again; take land
/// that sank out of its province; give new land to the province next to it, spreading out from
/// the old coast a ring of cells at a time (the lowest-numbered neighbour wins a tie); make each
/// new island that touches no province a province of its own. Province numbers never change.
fn reshape_after_height(w: &mut World, regenerate: &mut dyn FnMut(&mut World)) {
    regenerate(w);
    let grid = w.grid.clone();
    let of_cell = &mut w.provinces.of_cell;
    let mut new_land = Vec::new();
    for c in 0..grid.cells() {
        match (w.elevation[c] >= 0.0, of_cell[c] != NONE) {
            (false, true) => of_cell[c] = NONE,
            (true, false) => new_land.push(c),
            _ => {}
        }
    }
    loop {
        let (mut joined, mut left) = (Vec::new(), Vec::new());
        for &c in &new_land {
            match grid.neighbors[c].iter().map(|&m| of_cell[m as usize]).filter(|&p| p != NONE).min() {
                Some(p) => joined.push((c, p)),
                None => left.push(c),
            }
        }
        if joined.is_empty() {
            break;
        }
        for (c, p) in joined {
            of_cell[c] = p;
        }
        new_land = left;
    }
    let mut count = w.provinces.count;
    for &start in &new_land {
        if of_cell[start] != NONE {
            continue;
        }
        let id = count as u32;
        count += 1;
        of_cell[start] = id;
        let mut island = vec![start];
        while let Some(c) = island.pop() {
            for &m in &grid.neighbors[c] {
                let m = m as usize;
                if w.elevation[m] >= 0.0 && of_cell[m] == NONE {
                    of_cell[m] = id;
                    island.push(m);
                }
            }
        }
    }
    crate::provinces::reaggregate(w, count);
}

/// People and stored goods of provinces whose land all sank move to the nearest province that still has
/// land (by distance between centres; the lower number wins a tie). With no land left anywhere they
/// are lost, and the flows say so.
fn rehouse(w: &mut World, emptied: &[u32]) -> Flows {
    let mut flows = Flows::default();
    if emptied.is_empty() {
        return flows;
    }
    let mut pops_of = pops_by_province(&w.pops);
    for &from in emptied {
        let f = from as usize;
        let c = w.provinces.center[f];
        let to = (0..w.provinces.count).filter(|&p| w.provinces.cells[p] > 0).max_by(|&a, &b| {
            dot(c, w.provinces.center[a]).total_cmp(&dot(c, w.provinces.center[b])).then(b.cmp(&a))
        });
        for k in pops_of.get(&from).cloned().unwrap_or_default() {
            let n = std::mem::take(&mut w.pops.size[k]);
            match to {
                Some(t) => join(&mut w.pops, &mut pops_of, k, t as u32, n),
                None => flows.add(Quantity::People, Cause::Edit, -(n as i64)),
            }
        }
        // Every good kept goes with them.
        let goods = w.goods.clone();
        for c in 0..goods.stride() {
            let amount = std::mem::take(&mut w.stock[goods.index(f, c)]);
            match to {
                Some(t) => w.stock[goods.index(t, c)] += amount,
                None => flows.add(Quantity::Goods(goods.class_of_column(c)), Cause::Edit, -(amount as i64)),
            }
        }
    }
    flows
}

/// Pops of each province.
fn pops_by_province(pops: &Pops) -> HashMap<u32, Vec<usize>> {
    let mut out: HashMap<u32, Vec<usize>> = HashMap::new();
    for (k, &p) in pops.province.iter().enumerate() {
        out.entry(p).or_default().push(k);
    }
    out
}

/// `n` people of pop `k`'s kind arrive in province `to`: they join a pop of their kind there, or
/// make one.
fn join(pops: &mut Pops, pops_of: &mut HashMap<u32, Vec<usize>>, k: usize, to: u32, n: u64) {
    if n == 0 {
        return;
    }
    let key = |p: &Pops, j: usize| (p.species[j], p.culture[j], p.religion[j], p.class[j]);
    let there = pops_of.entry(to).or_default();
    match there.iter().find(|&&j| key(pops, j) == key(pops, k)) {
        Some(&j) => pops.size[j] += n,
        None => {
            let (species, culture, religion, class) = key(pops, k);
            pops.push(to, n);
            let j = pops.size.len() - 1;
            (pops.species[j], pops.culture[j], pops.religion[j], pops.class[j]) = (species, culture, religion, class);
            there.push(j);
        }
    }
}

/// Refuse a stroke with no points, or points or a radius that aren't numbers.
fn stroke(path: &[[f64; 2]], radius_km: f64) -> Result<(), String> {
    if path.is_empty() {
        return Err("a stroke needs at least one point".into());
    }
    if path.iter().flatten().any(|x| !x.is_finite()) || !(radius_km > 0.0 && radius_km.is_finite()) {
        return Err("the stroke's points and radius must be numbers, the radius above 0".into());
    }
    Ok(())
}

/// Refuse cell lists that are empty, off the planet or at sea.
fn land(w: &World, cells: &[u32]) -> Result<(), String> {
    if cells.is_empty() {
        return Err("no cells".into());
    }
    for &c in cells {
        match w.provinces.of_cell.get(c as usize) {
            None => return Err(format!("no cell {c}")),
            Some(&NONE) => return Err(format!("cell {c} is sea")),
            Some(_) => {}
        }
    }
    Ok(())
}

/// Land moved between provinces (`(cell, from, to)`, after the map part): each province that gave
/// land gives its people and stored goods with it, in proportion to the farmland that moved.
fn share_out(w: &mut World, moved: &[(u32, u32, u32)]) {
    /// Land one province gave another: (receiver, farmland, cells).
    type Gift = (u32, f64, u64);
    // Per giving province, in number order: what went to each receiver.
    let mut gifts: Vec<(u32, Vec<Gift>)> = Vec::new();
    for &(c, from, to) in moved {
        let food = w.food_per_cell[c as usize];
        let i = gifts.iter().position(|g| g.0 == from).unwrap_or_else(|| {
            gifts.push((from, Vec::new()));
            gifts.len() - 1
        });
        match gifts[i].1.iter_mut().find(|g| g.0 == to) {
            Some(g) => {
                g.1 += food;
                g.2 += 1;
            }
            None => gifts[i].1.push((to, food, 1)),
        }
    }
    gifts.sort_by_key(|g| g.0);
    let mut pops_of = pops_by_province(&w.pops);

    for (from, to) in gifts {
        let f = from as usize;
        // The giver keeps the share of what it still has.
        let ids: Vec<u64> = std::iter::once(from).chain(to.iter().map(|g| g.0)).map(u64::from).collect();
        let farmland: Vec<u64> = std::iter::once(w.provinces.food_capacity[f]).chain(to.iter().map(|g| g.1.round() as u64)).collect();
        let weights = if farmland.iter().any(|&x| x > 0) { farmland } else { std::iter::once(w.provinces.cells[f] as u64).chain(to.iter().map(|g| g.2)).collect() };

        let goods = w.goods.clone();
        for c in 0..goods.stride() {
            let stock = split_proportional(w.stock[goods.index(f, c)], &weights, &ids);
            w.stock[goods.index(f, c)] = stock[0];
            for (g, &s) in to.iter().zip(&stock[1..]) {
                w.stock[goods.index(g.0 as usize, c)] += s;
            }
        }
        for k in pops_of.get(&from).cloned().unwrap_or_default() {
            let parts = split_proportional(w.pops.size[k], &weights, &ids);
            w.pops.size[k] = parts[0];
            for (g, &n) in to.iter().zip(&parts[1..]) {
                join(&mut w.pops, &mut pops_of, k, g.0, n);
            }
        }
    }
}

/// What one edit changed on the map: for each field it changed, the values before and after at the
/// entities that changed (or the whole field, if its length changed).
#[derive(Clone, Debug)]
pub struct MapChange {
    /// The edit that made it. A replay that reaches the same point with the same edit reuses it.
    pub edit: Edit,
    fields: Vec<(FieldId, Change)>,
    /// What it did to the province neighbours, worked out once when the change is made: making
    /// the change again, or undoing it, needn't work them out again.
    routes: RoutesChange,
}

#[derive(Clone, Debug)]
enum Change {
    Sparse { at: Vec<u32>, old: FieldData, new: FieldData },
    Whole { old: FieldData, new: FieldData },
}

impl MapChange {
    /// The change from `before` (each field's values before the edit) to the world as it is now.
    /// Works out the province neighbours of the new map and gives them to the world.
    pub(crate) fn between(edit: Edit, before: Vec<(FieldId, FieldData)>, w: &mut World) -> MapChange {
        let fields = before
            .into_iter()
            .filter_map(|(id, old)| {
                let new = w.field(id);
                if old.len() != new.len() {
                    return Some((id, Change::Whole { old, new }));
                }
                let at = old.differences(&new);
                (!at.is_empty()).then(|| (id, Change::Sparse { old: old.gather(&at), new: new.gather(&at), at }))
            })
            .collect();
        let routes = Routes::build(w);
        let change = RoutesChange::between(&w.routes, &routes);
        w.routes = std::sync::Arc::new(routes);
        MapChange { edit, fields, routes: change }
    }

    /// Make the change again.
    pub(crate) fn redo(&self, w: &mut World) {
        self.put(w, false);
    }

    /// Put back what was there before.
    pub(crate) fn undo(&self, w: &mut World) {
        self.put(w, true);
    }

    fn put(&self, w: &mut World, back: bool) {
        for (id, change) in &self.fields {
            match change {
                Change::Whole { old, new } => w.set_field(*id, if back { old } else { new }.clone()),
                // In place: the cost follows the cells that changed, not the planet.
                Change::Sparse { at, old, new } => w.scatter_field(*id, at, if back { old } else { new }),
            }
            .expect("a change of this world");
        }
        std::sync::Arc::make_mut(&mut w.routes).apply(&self.routes, back);
    }

    /// Land that changed province: `(cell, from, to)`, in cell order. Land that was sea before, or
    /// is sea now, isn't listed.
    pub fn moved_cells(&self) -> Vec<(u32, u32, u32)> {
        let province = known("cell.province");
        let Some((_, change)) = self.fields.iter().find(|f| f.0 == province) else { return Vec::new() };
        let (at, old, new): (Vec<u32>, &[u32], &[u32]) = match change {
            Change::Sparse { at, old: FieldData::U32(o), new: FieldData::U32(n) } => (at.clone(), o, n),
            Change::Whole { old: FieldData::U32(o), new: FieldData::U32(n) } => ((0..n.len() as u32).collect(), o, n),
            _ => unreachable!("cell.province holds u32s"),
        };
        at.into_iter().zip(old.iter().zip(new)).filter(|&(_, (&o, &n))| o != n && o != NONE && n != NONE).map(|(c, (&o, &n))| (c, o, n)).collect()
    }

    /// Provinces it touched, in `w` as it is after the change: those whose land changed (before or
    /// after), whose cells changed in any way, or whose own values changed. In number order.
    pub fn provinces(&self, w: &World) -> Vec<u32> {
        let mut out = Vec::new();
        for (id, change) in &self.fields {
            let (at, old, new) = match change {
                Change::Sparse { at, old, new } => (at.clone(), Some(old), Some(new)),
                Change::Whole { new, .. } => ((0..new.len() as u32).collect(), None, None),
            };
            match id.def().tier {
                crate::fields::Tier::Cell => out.extend(at.iter().map(|&c| w.provinces.of_cell[c as usize])),
                crate::fields::Tier::Province => out.extend(at.iter().copied()),
                crate::fields::Tier::ProvinceGood => out.extend(at.iter().map(|&e| w.goods.split(e as usize).0 as u32)),
                crate::fields::Tier::Army => out.extend(at.iter().filter_map(|&a| w.armies.province.get(a as usize).copied())),
                crate::fields::Tier::Pop | crate::fields::Tier::Realm | crate::fields::Tier::RealmGood | crate::fields::Tier::Character => {}
            }
            if id.def().name == "cell.province"
                && let (Some(FieldData::U32(o)), Some(FieldData::U32(n))) = (old, new)
            {
                out.extend(o.iter().chain(n));
            }
        }
        out.retain(|&p| p != NONE);
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Provinces that had land before and have none now, in number order.
    pub fn emptied_provinces(&self) -> Vec<u32> {
        let cells = known("province.cells");
        let Some((_, change)) = self.fields.iter().find(|f| f.0 == cells) else { return Vec::new() };
        let (at, old, new): (Vec<u32>, &[u32], &[u32]) = match change {
            Change::Sparse { at, old: FieldData::U32(o), new: FieldData::U32(n) } => (at.clone(), o, n),
            Change::Whole { old: FieldData::U32(o), new: FieldData::U32(n) } => ((0..o.len() as u32).collect(), o, &n[..o.len().min(n.len())]),
            _ => unreachable!("province.cells holds u32s"),
        };
        at.into_iter().zip(old.iter().zip(new)).filter(|&(_, (&o, &n))| o > 0 && n == 0).map(|(p, _)| p).collect()
    }

    /// The fields it changed.
    pub fn fields(&self) -> impl Iterator<Item = FieldId> + '_ {
        self.fields.iter().map(|f| f.0)
    }

    /// Roughly how many bytes it holds.
    pub fn bytes(&self) -> usize {
        let size = |d: &FieldData| d.len() * 8;
        self.fields
            .iter()
            .map(|(_, c)| match c {
                Change::Sparse { at, old, new } => at.len() * 4 + size(old) + size(new),
                Change::Whole { old, new } => size(old) + size(new),
            })
            .sum::<usize>()
            + self.routes.bytes()
    }
}
