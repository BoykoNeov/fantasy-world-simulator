//! The module contract.
//!
//! Every module fills a slot and declares a manifest: the fields it reads, owns and sends change
//! requests to, its cadence, and the events it emits and handles. Modules never call each other.
//! They read fields, write only the fields they own, ask owners for changes, and send events.
//!
//! The engine checks a configuration once, at world creation or load ([`plan`]): no field has two
//! owners, every request goes to an owner, everything read exists, and there are no cycles. The
//! run order within a day is derived from the manifests (owners before their readers), with ties
//! broken by slot id, so it is the same every time.

use crate::calendar::Calendar;
use crate::events::{Event, EventKind};
use crate::fields::{self, FieldId, Goods, Quantity, Tier};
use crate::requests::{Cause, Dest, Priority, Request};
use crate::taxonomy::NodeId;
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// A domain that versions fill. The order is the tie-break for run order, and saves store slots by
/// position: append new slots at the end, never reorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Slot {
    Terrain,
    Climate,
    Biomes,
    Rivers,
    Fertility,
    Provinces,
    /// Economics v0 is "food vs population": harvests and food stores.
    Economics,
    Population,
    /// Weather v0 is "droughts": years of missing rain, started by a poke.
    Weather,
    /// Who owns each province, realms' capitals, taxes, unrest (step 7).
    Realms,
    /// Rulers and heirs (step 7).
    Characters,
    /// Opinions, wars, peace and truces between realms (step 7).
    Diplomacy,
    /// Armies and fleets: raising, moving, fighting (step 7).
    Warfare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cadence {
    /// Runs once, while the world is generated.
    Generation,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Cadence {
    pub fn due(self, cal: &Calendar, day: u64) -> bool {
        match self {
            Cadence::Generation => false,
            Cadence::Daily => true,
            Cadence::Weekly => cal.is_week_start(day),
            Cadence::Monthly => cal.is_month_start(day),
            Cadence::Yearly => cal.is_year_start(day),
        }
    }
}

pub struct Manifest {
    pub slot: Slot,
    /// Name of this version within its slot, e.g. "null" or "noise".
    pub version: &'static str,
    /// The level the module works on.
    pub tier: Tier,
    pub cadence: Cadence,
    pub reads: &'static [&'static str],
    /// Fields it reads as they stood when the day began, before the day's events were delivered
    /// (see [`crate::world::Earlier`]). They add no run-order edge, so a module may read this way a
    /// field whose owner runs after it, or reads what it owns.
    pub reads_earlier: &'static [&'static str],
    pub owns: &'static [&'static str],
    /// Fields this module asks the owner to change. Grants come back the next day as
    /// [`Event::Granted`], addressed to this module.
    pub requests: &'static [&'static str],
    /// The nodes of the goods tree it asks for in requests to per-good fields (see
    /// [`Ctx::ask_good`]). The goods kept must be able to serve each.
    pub asks: &'static [&'static str],
    /// Pairs of fields `(from, into)`: requests on `from` that ask its owner to move what it grants
    /// into a value of `into` instead of using it up (see [`Ctx::ask_move`]). Both must be kept by
    /// the same owner and hold the same kind of quantity, so nothing changes hands between owners.
    pub moves: &'static [(&'static str, &'static str)],
    /// Fields that must have an active owner (beyond what `reads` needs).
    pub requires: &'static [&'static str],
    pub emits: &'static [EventKind],
    pub handles: &'static [EventKind],
    /// Layout of the module's private state in saves. Change it whenever that layout changes.
    pub state_schema: &'static str,
    /// For owners of per-good fields: the nodes of the goods tree it keeps values of, one column
    /// each (see [`Goods`]). Empty for everyone else.
    pub goods: &'static [&'static str],
}

impl Manifest {
    /// A generation module: it reads some fields and writes the ones it owns, once.
    pub const fn generation(slot: Slot, version: &'static str, tier: Tier, reads: &'static [&'static str], owns: &'static [&'static str]) -> Manifest {
        Manifest { slot, version, tier, cadence: Cadence::Generation, reads, reads_earlier: &[], owns, requests: &[], asks: &[], moves: &[], requires: &[], emits: &[], handles: &[], state_schema: "none", goods: &[] }
    }
}

/// Per-day running totals of conserved quantities that moved, by cause.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flows(pub Vec<(Quantity, Cause, i64)>);

impl Flows {
    pub fn add(&mut self, q: Quantity, cause: Cause, delta: i64) {
        if delta == 0 {
            return;
        }
        match self.0.iter_mut().find(|(fq, fc, _)| *fq == q && *fc == cause) {
            Some(f) => f.2 += delta,
            None => self.0.push((q, cause, delta)),
        }
    }
    pub fn get(&self, q: Quantity, cause: Cause) -> i64 {
        self.0.iter().find(|(fq, fc, _)| *fq == q && *fc == cause).map_or(0, |f| f.2)
    }
    pub fn net(&self, q: Quantity) -> i64 {
        self.0.iter().filter(|f| f.0 == q).map(|f| f.2).sum()
    }
    pub fn merge(&mut self, o: &Flows) {
        for &(q, c, d) in &o.0 {
            self.add(q, c, d);
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What modules produce during one stage; the engine collects it after each call.
#[derive(Default)]
pub struct Outbox {
    pub requests: Vec<Request>,
    pub events: Vec<Event>,
    pub flows: Flows,
    /// Requests applied, with what was granted; kept for the day so the inspector can say why a
    /// value changed.
    pub applied: Vec<(Request, u64)>,
    /// Explanations for the province being traced (see [`Ctx::traced`]).
    pub notes: Vec<String>,
}

/// Everything a module may use while it runs: the date, the seed, and ways to talk to the rest of
/// the world without calling anyone.
pub struct Ctx<'a> {
    pub day: u64,
    pub seed: u64,
    pub calendar: &'a Calendar,
    /// The goods tree and the goods the world keeps.
    pub goods: &'a Goods,
    pub slot: Slot,
    pub(crate) out: &'a mut Outbox,
    /// The province being traced, when the inspector replays a day to explain it.
    pub(crate) trace: Option<u32>,
}

impl Ctx<'_> {
    /// Whether the inspector wants to know why `province` changes today. Modules check this before
    /// building a [`Ctx::note`], so an untraced day costs one comparison.
    pub fn traced(&self, province: u32) -> bool {
        self.trace == Some(province)
    }

    /// Explain, in words, a step of the work for the traced province: the inputs and the result.
    pub fn note(&mut self, text: String) {
        self.out.notes.push(text);
    }

    /// Ask the owner of `field` to remove `take` from `entity`. Applied at the end of today;
    /// the grant arrives tomorrow. `tag` comes back with the grant (e.g. which pop asked).
    pub fn request(&mut self, field: FieldId, entity: u32, take: u64, priority: Priority, cause: Cause, tag: u64) {
        debug_assert!(!field.def().tier.per_good(), "per-good fields are asked with ask_good");
        self.out.requests.push(Request { day: self.day, requester: self.slot, field, entity, good: None, take, priority, cause, tag, to: None });
    }

    /// Ask the owner of the per-good `field` for `take` of the goods under `good` in `province`
    /// ("food", however finely the owner keeps it). The good must be one the manifest `asks` for.
    #[allow(clippy::too_many_arguments)]
    pub fn ask_good(&mut self, field: FieldId, province: u32, good: NodeId, take: u64, priority: Priority, cause: Cause, tag: u64) {
        debug_assert!(field.def().tier.per_good());
        let r = Request { day: self.day, requester: self.slot, field, entity: province, good: Some(good), take, priority, cause, tag, to: None };
        self.out.requests.push(r);
    }

    /// Ask the owner of `field` to move `take` from `entity` (of the goods under `good`, for a
    /// per-good field) into `to`, a value of another field it keeps: a province's coins into its
    /// realm's treasury, say. Nothing is made or used up, so no flow is booked. The pair of fields
    /// must be one the manifest lists in `moves`.
    #[allow(clippy::too_many_arguments)]
    pub fn ask_move(&mut self, field: FieldId, entity: u32, good: Option<NodeId>, take: u64, to: Dest, priority: Priority, cause: Cause, tag: u64) {
        debug_assert_eq!(field.def().tier.per_good(), good.is_some(), "per-good fields name a good, others don't");
        let r = Request { day: self.day, requester: self.slot, field, entity, good, take, priority, cause, tag, to: Some(to) };
        self.out.requests.push(r);
    }

    pub fn emit(&mut self, e: Event) {
        self.out.events.push(e);
    }

    /// Report that a conserved quantity was created (positive) or destroyed (negative).
    pub fn flow(&mut self, q: Quantity, cause: Cause, delta: i64) {
        self.out.flows.add(q, cause, delta);
    }

    /// For owners applying requests: record the grant, report the flow (none for a move: what was
    /// taken is still there, elsewhere), and tell the requester tomorrow.
    pub fn grant(&mut self, r: &Request, granted: u64) {
        if let Some(q) = self.goods.quantity_asked(r).filter(|_| r.to.is_none()) {
            self.out.flows.add(q, r.cause, -(granted as i64));
        }
        self.out.applied.push((*r, granted));
        self.out.events.push(Event::Granted { request: *r, granted });
    }
}

/// A module version. Generation modules implement `generate`; runtime modules implement the rest.
pub trait Module: Send {
    fn manifest(&self) -> &'static Manifest;

    /// Generation modules: write the fields this module owns, from the fields it reads.
    fn generate(&self, _w: &mut World) {}

    /// Check the module's parameters against the goods the world keeps, when the world is made or
    /// loaded. A refusal names what is wrong.
    fn check(&self, _goods: &Goods) -> Result<(), String> {
        Ok(())
    }

    /// Runtime modules: set up owned fields at world creation, after generation.
    fn init(&mut self, _w: &mut World, _cx: &mut Ctx) {}

    /// Main logic, on the module's cadence.
    fn tick(&mut self, _w: &mut World, _cx: &mut Ctx) {}

    /// Whether the module has nothing to do on `day`, a day its cadence is due: the engine then
    /// skips its tick and the checks around it. For a daily module whose days are mostly quiet.
    fn idle(&self, _w: &World, _calendar: &Calendar, _day: u64) -> bool {
        false
    }

    /// Owners: apply today's requests for owned fields, in the fixed order given, and grant each.
    fn apply_requests(&mut self, _w: &mut World, _requests: &[Request], _cx: &mut Ctx) {}

    /// Events delivered today (emitted yesterday), and grants for this module's requests.
    fn handle(&mut self, _w: &mut World, _e: &Event, _cx: &mut Ctx) {}

    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() { Ok(()) } else { Err(format!("{:?} keeps no private state", self.manifest().slot)) }
    }

    /// Edits this module carries out itself, because they need what only it knows (its settings, its
    /// private state): `None` for an edit that isn't its own, else whether it fits the world as it
    /// is. Nothing changes yet. The engine asks the module that keeps the fields the edit sets.
    fn check_edit(&self, _w: &World, _edit: &crate::edit::Edit) -> Option<Result<(), String>> {
        None
    }

    /// Carry out an edit [`Module::check_edit`] took as its own and passed, at the start of `day`,
    /// and report every person or good it made or removed.
    fn carry_out(&mut self, _w: &mut World, _edit: &crate::edit::Edit, _day: u64, _calendar: &Calendar) -> Flows {
        Flows::default()
    }

    /// An edit (not a settings change) has changed the world from outside: put right whatever the
    /// module works out from the fields and keeps between days. It may put its own fields right too,
    /// moving nobody and nothing.
    fn edited(&mut self, _w: &mut World, _edit: &crate::edit::Edit) {}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad module configuration: {}", self.0)
    }
}

impl std::error::Error for ConfigError {}

/// A checked configuration: run orders, as indices into the manifest list, and each field's owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub generation: Vec<usize>,
    pub runtime: Vec<usize>,
    pub owner: Vec<Option<usize>>,
}

/// Check a configuration and derive its run order. Fails with a message naming the modules and
/// fields involved.
pub fn plan(manifests: &[&Manifest]) -> Result<Plan, ConfigError> {
    let err = |s: String| Err(ConfigError(s));
    let name = |m: &Manifest| format!("{:?} \"{}\"", m.slot, m.version);
    let lookup = |m: &Manifest, f: &str| fields::id(f).ok_or_else(|| ConfigError(format!("{} names unknown field {f}", name(m))));

    let mut slots = BTreeSet::new();
    let mut owner: Vec<Option<usize>> = vec![None; fields::FIELDS.len()];
    for (i, m) in manifests.iter().enumerate() {
        if !slots.insert(m.slot) {
            return err(format!("two modules fill the {:?} slot", m.slot));
        }
        for f in m.owns {
            let id = lookup(m, f)?;
            if let Some(o) = owner[id.0 as usize] {
                return err(format!("{f} has two owners: {} and {}", name(manifests[o]), name(m)));
            }
            owner[id.0 as usize] = Some(i);
        }
    }
    let generation = |i: usize| manifests[i].cadence == Cadence::Generation;
    for (i, m) in manifests.iter().enumerate() {
        for f in m.reads {
            let id = lookup(m, f)?;
            match owner[id.0 as usize] {
                None if id.def().default == fields::Default::Required => {
                    return err(format!("{} reads {f}, but no active module provides it", name(m)));
                }
                Some(o) if generation(i) && !generation(o) => {
                    return err(format!("{} runs at generation but reads {f}, which {} only sets later", name(m), name(manifests[o])));
                }
                _ => {}
            }
        }
        for f in m.reads_earlier {
            let id = lookup(m, f)?;
            if m.reads.contains(f) {
                return err(format!("{} reads {f} both as it stands and as it stood when the day began", name(m)));
            }
            match owner[id.0 as usize] {
                Some(o) if o == i => return err(format!("{} owns {f}; it reads its own values as they stand", name(m))),
                None if id.def().default == fields::Default::Required => {
                    return err(format!("{} reads {f}, but no active module provides it", name(m)));
                }
                _ if generation(i) => return err(format!("{} runs at generation, before any day begins, but reads {f} as it stood when the day began", name(m))),
                _ => {}
            }
        }
        for f in m.requires {
            if owner[lookup(m, f)?.0 as usize].is_none() {
                return err(format!("{} requires {f}, but no active module owns it", name(m)));
            }
        }
        for f in m.requests {
            match owner[lookup(m, f)?.0 as usize] {
                None => return err(format!("{} sends change requests to {f}, but no active module owns it", name(m))),
                Some(o) if o == i => return err(format!("{} owns {f}; it writes it directly instead of requesting", name(m))),
                Some(o) if generation(o) || generation(i) => {
                    return err(format!("{} requests {f}: requests are for running modules, not generation", name(m)));
                }
                _ => {}
            }
        }
        for &(from, into) in m.moves {
            let (a, b) = (lookup(m, from)?, lookup(m, into)?);
            if a == b {
                return err(format!("{} moves {from} into itself", name(m)));
            }
            if !m.requests.contains(&from) {
                return err(format!("{} moves {from} into {into} but doesn't list {from} among its requests", name(m)));
            }
            if owner[a.0 as usize] != owner[b.0 as usize] {
                return err(format!("{} moves {from} into {into}, but one module must keep both", name(m)));
            }
            let (da, db) = (a.def(), b.def());
            if !(da.conserved && db.conserved && da.tier.per_good() == db.tier.per_good()) {
                return err(format!("{} moves {from} into {into}: both must hold the same kind of counted quantity", name(m)));
            }
        }
    }

    let order = |phase: bool| -> Result<Vec<usize>, ConfigError> {
        let members: Vec<usize> = (0..manifests.len()).filter(|&i| generation(i) == phase).collect();
        // Edge owner -> reader, within the phase.
        let mut after: Vec<Vec<usize>> = vec![Vec::new(); manifests.len()];
        let mut waiting = vec![0usize; manifests.len()];
        for &i in &members {
            let mut from = BTreeSet::new();
            for f in manifests[i].reads {
                if let Some(o) = owner[fields::known(f).0 as usize]
                    && o != i
                    && generation(o) == phase
                {
                    from.insert(o);
                }
            }
            for o in from {
                after[o].push(i);
                waiting[i] += 1;
            }
        }
        let mut ready: BTreeSet<(Slot, usize)> = members.iter().filter(|&&i| waiting[i] == 0).map(|&i| (manifests[i].slot, i)).collect();
        let mut out = Vec::new();
        while let Some(first) = ready.pop_first() {
            out.push(first.1);
            for &j in &after[first.1] {
                waiting[j] -= 1;
                if waiting[j] == 0 {
                    ready.insert((manifests[j].slot, j));
                }
            }
        }
        if out.len() < members.len() {
            let stuck: Vec<String> = members.iter().filter(|i| !out.contains(i)).map(|&i| name(manifests[i])).collect();
            return Err(ConfigError(format!("same-day dependency cycle among {}", stuck.join(", "))));
        }
        Ok(out)
    };
    Ok(Plan { generation: order(true)?, runtime: order(false)?, owner })
}

/// The goods a checked configuration keeps: what the owners of per-good fields name in their
/// manifests (they must agree), or the tree's classes when nobody owns one. Every good a module
/// asks for must be in the tree and kept, whole or in parts.
pub fn goods_kept(manifests: &[&Manifest], plan: &Plan, tree: crate::taxonomy::Taxonomy) -> Result<Goods, ConfigError> {
    let name = |m: &Manifest| format!("{:?} \"{}\"", m.slot, m.version);
    let mut kept: Option<&Manifest> = None;
    for id in FieldId::all().filter(|id| id.def().tier.per_good()) {
        let Some(o) = plan.owner[id.0 as usize] else { continue };
        let m = manifests[o];
        if m.goods.is_empty() {
            return Err(ConfigError(format!("{} owns {}, kept per good, but names no goods to keep", name(m), id.def().name)));
        }
        match kept {
            Some(k) if k.goods != m.goods => {
                return Err(ConfigError(format!("{} keeps goods {:?} but {} keeps {:?}: per-good fields must keep the same goods", name(k), k.goods, name(m), m.goods)));
            }
            _ => kept = Some(m),
        }
    }
    let goods = match kept {
        Some(m) => Goods::new(tree, m.goods).map_err(|e| ConfigError(format!("{}: {e}", name(m))))?,
        None => Goods::classes(tree),
    };
    let keeper = kept.map_or_else(|| "the world".to_string(), name);
    for m in manifests {
        for &asked in m.asks {
            let Some(node) = goods.tree.find(asked).filter(|&n| n != 0) else {
                return Err(ConfigError(format!("{} asks for {asked}, which is not a good or class of goods in the goods tree", name(m))));
            };
            match goods.columns_under(node) {
                None => {
                    let whole = goods.columns.iter().find(|&&c| goods.tree.is_under(node, c)).map_or("", |&c| goods.tree.name(c));
                    return Err(ConfigError(format!("{} asks for {asked}, but {keeper} keeps it only as part of {whole}", name(m))));
                }
                Some(c) if c.is_empty() => return Err(ConfigError(format!("{} asks for {asked}, but {keeper} keeps none", name(m)))),
                Some(_) => {}
            }
        }
    }
    Ok(goods)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(slot: Slot, cadence: Cadence, reads: &'static [&'static str], owns: &'static [&'static str]) -> Manifest {
        Manifest { slot, version: "test", tier: Tier::Province, cadence, reads, reads_earlier: &[], owns, requests: &[], asks: &[], moves: &[], requires: &[], emits: &[], handles: &[], state_schema: "none", goods: &[] }
    }

    #[test]
    fn owners_run_before_readers_and_ties_go_by_slot() {
        let a = m(Slot::Population, Cadence::Monthly, &[], &["pop.size"]);
        let b = m(Slot::Economics, Cadence::Monthly, &["pop.size"], &["province.stock"]);
        let c = m(Slot::Terrain, Cadence::Generation, &[], &["cell.elevation"]);
        let d = m(Slot::Climate, Cadence::Generation, &["cell.elevation"], &["cell.temperature"]);
        let p = plan(&[&d, &b, &a, &c]).unwrap();
        // Economics has the lower slot id but reads what Population owns, so it runs second.
        assert_eq!(p.runtime, vec![2, 1]);
        assert_eq!(p.generation, vec![3, 0]);
        // Independent modules: slot order, whatever order they were listed in.
        let e = m(Slot::Population, Cadence::Monthly, &[], &["pop.size"]);
        let f = m(Slot::Economics, Cadence::Monthly, &[], &["province.stock"]);
        assert_eq!(plan(&[&e, &f]).unwrap().runtime, vec![1, 0]);
    }

    fn fails(ms: &[&Manifest], needle: &str) {
        let e = plan(ms).expect_err("configuration should be refused");
        assert!(e.0.contains(needle), "message was: {e}");
    }

    #[test]
    fn two_owners_are_refused() {
        let a = m(Slot::Terrain, Cadence::Generation, &[], &["cell.elevation"]);
        let b = m(Slot::Climate, Cadence::Generation, &[], &["cell.elevation"]);
        fails(&[&a, &b], "cell.elevation has two owners");
    }

    #[test]
    fn unmet_needs_are_refused() {
        let reads = m(Slot::Climate, Cadence::Generation, &["cell.elevation"], &["cell.temperature"]);
        fails(&[&reads], "reads cell.elevation, but no active module provides it");
        let requires = Manifest { requires: &["province.stock"], ..m(Slot::Population, Cadence::Monthly, &[], &["pop.size"]) };
        fails(&[&requires], "requires province.stock");
        let requests = Manifest { requests: &["province.stock"], ..m(Slot::Population, Cadence::Monthly, &[], &["pop.size"]) };
        fails(&[&requests], "sends change requests to province.stock, but no active module owns it");
        let typo = m(Slot::Climate, Cadence::Generation, &["cell.elevaton"], &[]);
        fails(&[&typo], "unknown field cell.elevaton");
        // A field with a documented default may be read without an owner.
        let ok = m(Slot::Population, Cadence::Monthly, &["province.stock"], &["pop.size"]);
        assert!(plan(&[&ok]).is_ok());
    }

    #[test]
    fn cycles_are_refused() {
        let a = m(Slot::Economics, Cadence::Monthly, &["pop.size"], &["province.stock"]);
        let b = m(Slot::Population, Cadence::Monthly, &["province.stock"], &["pop.size"]);
        fails(&[&a, &b], "same-day dependency cycle among Economics \"test\", Population \"test\"");
    }

    #[test]
    fn the_owner_of_the_stock_chooses_the_goods_kept() {
        let tree = crate::taxonomy::Taxonomy::standard_goods;
        let kept = |ms: &[&Manifest]| goods_kept(ms, &plan(ms).unwrap(), tree()).map(|g| g.names());
        let owner = |goods| Manifest { goods, ..m(Slot::Economics, Cadence::Monthly, &[], &["province.stock"]) };
        assert_eq!(kept(&[&owner(&["food"])]).unwrap(), ["food"]);
        assert_eq!(kept(&[&owner(&["grain", "ore"])]).unwrap(), ["grain", "ore"]);
        // Nobody keeps goods: readers find each class, empty.
        assert_eq!(kept(&[&m(Slot::Population, Cadence::Monthly, &[], &["pop.size"])]).unwrap(), ["food", "raw", "manufactured", "money"]);
        assert!(kept(&[&owner(&[])]).unwrap_err().0.contains("names no goods"));
        assert!(kept(&[&owner(&["spice"])]).unwrap_err().0.contains("Economics \"test\": the goods tree has no spice"));

        // What a module asks for must be kept, whole or in parts.
        let eater = |asks| Manifest { asks, requests: &["province.stock"], ..m(Slot::Population, Cadence::Monthly, &[], &["pop.size"]) };
        assert_eq!(kept(&[&owner(&["grain", "fish"]), &eater(&["food"])]).unwrap(), ["grain", "fish"]);
        assert_eq!(kept(&[&owner(&["food"]), &eater(&["food"])]).unwrap(), ["food"]);
        let refused = |ms: &[&Manifest]| kept(ms).unwrap_err().0;
        assert_eq!(refused(&[&owner(&["ore"]), &eater(&["food"])]), "Population \"test\" asks for food, but Economics \"test\" keeps none");
        assert_eq!(refused(&[&owner(&["food"]), &eater(&["grain"])]), "Population \"test\" asks for grain, but Economics \"test\" keeps it only as part of food");
        assert!(refused(&[&owner(&["food"]), &eater(&["spice"])]).contains("asks for spice, which is not a good"));
        assert!(refused(&[&owner(&["food"]), &eater(&["goods"])]).contains("asks for goods, which is not a good"));
    }

    #[test]
    fn reading_as_the_day_began_adds_no_edge_and_is_checked() {
        // Economics reads Population's field as it stood when the day began, and Population reads
        // Economics' as it stands: no cycle, and Economics (the lower slot) runs first.
        let a = Manifest { reads_earlier: &["pop.size"], ..m(Slot::Economics, Cadence::Monthly, &[], &["province.stock"]) };
        let b = m(Slot::Population, Cadence::Monthly, &["province.stock"], &["pop.size"]);
        assert_eq!(plan(&[&b, &a]).unwrap().runtime, vec![1, 0]);
        let both = Manifest { reads: &["pop.size"], ..a };
        fails(&[&both, &b], "reads pop.size both as it stands and as it stood");
        let own = Manifest { reads_earlier: &["province.stock"], ..m(Slot::Economics, Cadence::Monthly, &[], &["province.stock"]) };
        fails(&[&own], "owns province.stock; it reads its own values as they stand");
        let typo = Manifest { reads_earlier: &["pop.sise"], ..m(Slot::Economics, Cadence::Monthly, &[], &[]) };
        fails(&[&typo], "unknown field pop.sise");
        let unmet = Manifest { reads_earlier: &["province.center"], ..m(Slot::Economics, Cadence::Monthly, &[], &[]) };
        fails(&[&unmet], "reads province.center, but no active module provides it");
        // A field with a documented default may be read without an owner, as with `reads`.
        let ok = Manifest { reads_earlier: &["province.owner"], ..m(Slot::Economics, Cadence::Monthly, &[], &[]) };
        assert!(plan(&[&ok]).is_ok());
        let early = Manifest { reads_earlier: &["province.owner"], ..m(Slot::Terrain, Cadence::Generation, &[], &["cell.elevation"]) };
        fails(&[&early], "runs at generation, before any day begins");
    }

    #[test]
    fn duplicate_slots_and_generation_reading_runtime_fields_are_refused() {
        let a = m(Slot::Terrain, Cadence::Generation, &[], &["cell.elevation"]);
        let b = m(Slot::Terrain, Cadence::Generation, &[], &["cell.temperature"]);
        fails(&[&a, &b], "two modules fill the Terrain slot");
        let run = m(Slot::Economics, Cadence::Monthly, &[], &["province.stock"]);
        let gen_reader = m(Slot::Provinces, Cadence::Generation, &["province.stock"], &[]);
        fails(&[&run, &gen_reader], "runs at generation but reads province.stock");
    }
}
