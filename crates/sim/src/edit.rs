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
//! position. A pop emptied by an edit keeps its place with nobody in it.

use crate::fields::{known, FieldData, FieldId, Quantity};
use crate::module::Flows;
use crate::pops::HUMAN;
use crate::requests::Cause;
use crate::world::World;
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
        }
    }

    /// Whether the edit changes the map (cell and province-shape fields).
    pub fn changes_map(&self) -> bool {
        match self {
            Edit::SetPop { .. } | Edit::AddPop { .. } | Edit::SetFoodStock { .. } => false,
        }
    }

    /// The map part. Depends on the map alone; called after [`Edit::check`] passed.
    pub(crate) fn change_map(&self, _w: &mut World) {}

    /// The running part, after the map part (`map` is what it changed). Returns what was created
    /// or removed. Called after [`Edit::check`] passed.
    pub(crate) fn change_running(&self, w: &mut World, _map: Option<&MapChange>) -> Flows {
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
        }
        flows
    }

    /// The provinces it is about, in the world as it is before the edit.
    pub fn provinces(&self, w: &World) -> Vec<u32> {
        match *self {
            Edit::SetPop { pop, .. } => w.pops.province.get(pop as usize).copied().into_iter().collect(),
            Edit::AddPop { province, .. } | Edit::SetFoodStock { province, .. } => vec![province],
        }
    }

    /// The edit in words, for the inspector.
    pub fn describe(&self) -> String {
        match self {
            Edit::SetPop { pop, to } => format!("pop {pop} set to {} people", to.size),
            Edit::AddPop { province, values } => format!("a pop of {} people added to province {province}", values.size),
            Edit::SetFoodStock { province, rations } => format!("province {province}'s food store set to {rations} rations"),
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
