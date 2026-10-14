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
//! change either: a province merged away stays as an empty placeholder, and a new one is numbered
//! after the last.
//!
//! When land changes province, its people and food go with it, in proportion to the farmland that
//! moved: a province that gives away a third of its farmland gives away a third of each pop and a
//! third of its store. Land with no farmland at all is shared by area instead. Shares are whole
//! people and rations by the largest-remainder rule, so nothing is lost or made.

use crate::fields::{known, FieldData, FieldId, Quantity};
use crate::grid::NONE;
use crate::module::Flows;
use crate::pops::{Pops, HUMAN};
use crate::requests::{split_proportional, Cause};
use crate::world::World;
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
    /// Set a province's food store.
    SetFoodStock { province: u32, rations: u64 },
    /// Land cells join province `to` (redrawing a border).
    MoveCells { cells: Vec<u32>, to: u32 },
    /// Land cells become a new province, numbered after the last.
    Split { cells: Vec<u32> },
    /// Province `from` joins `into`, and stays as an empty placeholder.
    Merge { from: u32, into: u32 },
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
                if w.food_stock.len() <= p as usize {
                    return Err("nobody keeps food stores in this world".into());
                }
                count("province.food_stock", rations)
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
        }
    }

    /// Whether the edit changes the map (cell and province-shape fields).
    pub fn changes_map(&self) -> bool {
        match self {
            Edit::SetPop { .. } | Edit::AddPop { .. } | Edit::SetFoodStock { .. } => false,
            Edit::MoveCells { .. } | Edit::Split { .. } | Edit::Merge { .. } => true,
        }
    }

    /// The map part. Depends on the map alone; called after [`Edit::check`] passed.
    pub(crate) fn change_map(&self, w: &mut World) {
        let count = w.provinces.count;
        let of_cell = &mut w.provinces.of_cell;
        let count = match *self {
            Edit::SetPop { .. } | Edit::AddPop { .. } | Edit::SetFoodStock { .. } => return,
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
            Edit::SetFoodStock { province, rations } => {
                let s = &mut w.food_stock[province as usize];
                flows.add(Quantity::Food, Cause::Edit, delta(*s, rations));
                *s = rations;
            }
            Edit::MoveCells { .. } | Edit::Split { .. } | Edit::Merge { .. } => {
                w.grow_province_fields();
                let moved = map.expect("a province edit changes the map").moved_cells();
                if let Edit::Split { .. } = self {
                    // The new province's land had the rain of the province that gave the most of it.
                    let mut from: Vec<u32> = moved.iter().map(|m| m.1).collect();
                    from.sort_unstable();
                    if let Some(run) = from.chunk_by(|a, b| a == b).max_by_key(|run| run.len()) {
                        let new = w.provinces.count - 1;
                        w.rain_shortfall[new] = w.rain_shortfall[run[0] as usize];
                    }
                }
                share_out(w, &moved);
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
            Edit::AddPop { province, .. } | Edit::SetFoodStock { province, .. } => vec![province],
            Edit::MoveCells { ref cells, to } => of(cells, to),
            Edit::Split { ref cells } => of(cells, w.provinces.count as u32),
            Edit::Merge { from, into } => vec![from, into],
        }
    }

    /// The edit in words, for the inspector.
    pub fn describe(&self) -> String {
        match self {
            Edit::SetPop { pop, to } => format!("pop {pop} set to {} people", to.size),
            Edit::AddPop { province, values } => format!("a pop of {} people added to province {province}", values.size),
            Edit::SetFoodStock { province, rations } => format!("province {province}'s food store set to {rations} rations"),
            Edit::MoveCells { cells, to } => format!("{} cells of land moved into province {to}", cells.len()),
            Edit::Split { cells } => format!("{} cells of land made a new province", cells.len()),
            Edit::Merge { from, into } => format!("province {from} merged into province {into}"),
        }
    }
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
/// land gives its people and food with it, in proportion to the farmland that moved.
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
    let mut pops_of: HashMap<u32, Vec<usize>> = HashMap::new();
    for (k, &p) in w.pops.province.iter().enumerate() {
        pops_of.entry(p).or_default().push(k);
    }
    let key = |p: &Pops, j: usize| (p.species[j], p.culture[j], p.religion[j], p.class[j]);

    for (from, to) in gifts {
        let f = from as usize;
        // The giver keeps the share of what it still has.
        let ids: Vec<u64> = std::iter::once(from).chain(to.iter().map(|g| g.0)).map(u64::from).collect();
        let farmland: Vec<u64> = std::iter::once(w.provinces.food_capacity[f]).chain(to.iter().map(|g| g.1.round() as u64)).collect();
        let weights = if farmland.iter().any(|&x| x > 0) { farmland } else { std::iter::once(w.provinces.cells[f] as u64).chain(to.iter().map(|g| g.2)).collect() };

        let stock = split_proportional(w.food_stock[f], &weights, &ids);
        w.food_stock[f] = stock[0];
        for (g, &s) in to.iter().zip(&stock[1..]) {
            w.food_stock[g.0 as usize] += s;
        }
        for k in pops_of.get(&from).cloned().unwrap_or_default() {
            let parts = split_proportional(w.pops.size[k], &weights, &ids);
            w.pops.size[k] = parts[0];
            for (g, &n) in to.iter().zip(&parts[1..]).filter(|(_, n)| **n > 0) {
                // People join a pop of their own kind there, or make one.
                let there = pops_of.entry(g.0).or_default();
                match there.iter().find(|&&j| key(&w.pops, j) == key(&w.pops, k)) {
                    Some(&j) => w.pops.size[j] += n,
                    None => {
                        let (species, culture, religion, class) = key(&w.pops, k);
                        w.pops.push(g.0, n);
                        let j = w.pops.size.len() - 1;
                        (w.pops.species[j], w.pops.culture[j], w.pops.religion[j], w.pops.class[j]) = (species, culture, religion, class);
                        there.push(j);
                    }
                }
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
}

#[derive(Clone, Debug)]
enum Change {
    Sparse { at: Vec<u32>, old: FieldData, new: FieldData },
    Whole { old: FieldData, new: FieldData },
}

impl MapChange {
    /// The change from `before` (each field's values before the edit) to the world as it is now.
    pub(crate) fn between(edit: Edit, before: Vec<(FieldId, FieldData)>, w: &World) -> MapChange {
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
        MapChange { edit, fields }
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
            let data = match change {
                Change::Whole { old, new } => if back { old } else { new }.clone(),
                Change::Sparse { at, old, new } => {
                    let mut d = w.field(*id);
                    d.scatter(at, if back { old } else { new });
                    d
                }
            };
            w.set_field(*id, data).expect("a change of this world");
        }
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
            .sum()
    }
}
