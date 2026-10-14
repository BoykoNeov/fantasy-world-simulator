//! The engine: a world, its modules, and the daily clock.
//!
//! One day runs in this order:
//!
//! 1. On a year's first day, the year summary closes the year that just ended (an observer, like
//!    the map: it reads fields and changes nothing).
//! 2. Events emitted yesterday are delivered in their fixed order, grants included.
//! 3. Each module whose cadence is due runs its main logic, in the run order derived from the
//!    manifests.
//! 4. Today's change requests are applied, owner by owner in run order.
//!
//! After every stage the engine checks the stage's work: conserved totals must change by exactly
//! what the reported flows and granted requests explain, and values must stay inside their field
//! contracts. With `strict` on (tests), a module that writes a field it doesn't own stops the run.
//! A day on which nothing is due and nothing is queued costs almost nothing.

use crate::calendar::Calendar;
use crate::config::Config;
use crate::events::{delivery_order, Event, EventKind, EventLog, Posted};
use crate::fields::{quantity_total, FieldId, QUANTITIES};
use crate::module::{plan, ConfigError, Ctx, Flows, Manifest, Module, Outbox, Plan, Slot};
use crate::requests::{Cause, Request};
use crate::world::{Fingerprint, World};
use crate::fields::Quantity;
use serde::{Deserialize, Serialize};

/// A year's totals, from the flows modules reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    pub produced: u64,
    pub spoiled: u64,
    pub eaten: u64,
    pub births: u64,
    /// All deaths, starvation included.
    pub deaths: u64,
    pub starved: u64,
}

impl Ledger {
    pub fn from_flows(f: &Flows) -> Ledger {
        let pos = |q, c| f.get(q, c).max(0) as u64;
        let neg = |q, c| (-f.get(q, c)).max(0) as u64;
        let starved = neg(Quantity::People, Cause::Starvation);
        Ledger {
            produced: pos(Quantity::Food, Cause::Harvest),
            spoiled: neg(Quantity::Food, Cause::Spoilage),
            eaten: neg(Quantity::Food, Cause::Eating),
            births: pos(Quantity::People, Cause::Births),
            deaths: neg(Quantity::People, Cause::Deaths) + starved,
            starved,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct YearSummary {
    pub year: i64,
    /// Population at the end of the year.
    pub population: u64,
    pub food_stock: u64,
    pub ledger: Ledger,
    /// Provinces where anyone starved during the year.
    pub famine_provinces: usize,
}

/// Things that went wrong without stopping the world. Each also goes out as an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problems {
    pub contract_violations: u64,
    pub conservation_breaks: u64,
}

/// One module in a configuration, as a save records it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleRecord {
    pub slot: Slot,
    pub version: String,
    pub state_schema: String,
}

impl ModuleRecord {
    pub fn of(m: &Manifest) -> ModuleRecord {
        ModuleRecord { slot: m.slot, version: m.version.into(), state_schema: m.state_schema.into() }
    }
}

/// Everything that changes the world from outside, day-stamped: replay re-applies these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub day: u64,
    pub kind: CommandKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CommandKind {
    /// A configuration starts running here. The first command of every world; later ones will mark
    /// module version changes (save → convert → reload).
    StartSegment { modules: Vec<ModuleRecord> },
}

pub struct Sim {
    pub world: World,
    pub calendar: Calendar,
    pub config: Config,
    pub(crate) modules: Vec<Box<dyn Module>>,
    pub plan: Plan,
    /// The day about to run; day 0 is the first day of the calendar's first year.
    pub day: u64,
    /// Events emitted and not yet delivered. Between days these are exactly yesterday's.
    pub(crate) pending: Vec<Posted>,
    /// Today's change requests, applied at the end of the day; always empty between days.
    queue: Vec<Request>,
    /// Emission counter for today's events.
    seq: u32,
    /// Flows for the year in progress.
    pub year_flows: Flows,
    pub history: Vec<YearSummary>,
    pub problems: Problems,
    pub commands: Vec<Command>,
    /// Requests applied today, with what each was granted and why it was made.
    pub applied_today: Vec<(Request, u64)>,
    /// Delivered events (not grants). Not part of saves.
    pub event_log: EventLog,
    /// Stop with a panic when a module writes a field it doesn't own. Slow: for tests.
    pub strict: bool,
}

impl Sim {
    /// Check the configuration, generate the world and set up the running modules.
    pub fn new(config: &Config) -> Result<Sim, ConfigError> {
        let mut sim = Sim::assemble(config)?;
        for &i in &sim.plan.generation.clone() {
            sim.stage(&[i], false, |s| {
                s.modules[i].generate(&mut s.world);
                Vec::new()
            });
        }
        let owned: Vec<bool> = sim.plan.owner.iter().map(Option::is_some).collect();
        sim.world.fill_defaults(&owned);
        for &i in &sim.plan.runtime.clone() {
            // Creating the starting stocks and people is not a flow, so no conservation check.
            sim.stage(&[i], false, |s| vec![(i, s.call(i, |m, w, cx| m.init(w, cx)))]);
        }
        Ok(sim)
    }

    /// Modules, plan and calendar for a configuration, with an empty world (shared with loading).
    pub(crate) fn assemble(config: &Config) -> Result<Sim, ConfigError> {
        let calendar = Calendar::new(config.calendar.clone()).map_err(ConfigError)?;
        let modules = config.modules.build();
        let manifests: Vec<&Manifest> = modules.iter().map(|m| m.manifest()).collect();
        let plan = plan(&manifests)?;
        let records = manifests.iter().map(|m| ModuleRecord::of(m)).collect();
        Ok(Sim {
            world: World::empty(&config.world),
            calendar,
            config: config.clone(),
            modules,
            plan,
            day: 0,
            pending: Vec::new(),
            queue: Vec::new(),
            seq: 0,
            year_flows: Flows::default(),
            history: Vec::new(),
            problems: Problems::default(),
            commands: vec![Command { day: 0, kind: CommandKind::StartSegment { modules: records } }],
            applied_today: Vec::new(),
            event_log: EventLog::default(),
            strict: false,
        })
    }

    /// Events waiting for tomorrow (grants included).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn manifests(&self) -> Vec<&'static Manifest> {
        self.modules.iter().map(|m| m.manifest()).collect()
    }

    /// Run one day. Returns whether anything ran (false = an idle day).
    pub fn step_day(&mut self) -> bool {
        let mut ran = false;
        self.seq = 0;
        if !self.applied_today.is_empty() {
            self.applied_today.clear();
        }
        if self.calendar.is_year_start(self.day) {
            if self.day > 0 {
                let summary = self.year_so_far();
                self.history.push(summary);
            }
            self.year_flows = Flows::default();
            self.event_log.prune(self.day);
            ran = true;
        }
        if !self.pending.is_empty() {
            self.deliver();
            ran = true;
        }
        for k in 0..self.plan.runtime.len() {
            let i = self.plan.runtime[k];
            if self.modules[i].manifest().cadence.due(&self.calendar, self.day) {
                self.stage(&[i], true, |s| vec![(i, s.call(i, |m, w, cx| m.tick(w, cx)))]);
                ran = true;
            }
        }
        if !self.queue.is_empty() {
            self.apply_requests();
            ran = true;
        }
        self.day += 1;
        ran
    }

    pub fn run_days(&mut self, days: u64) {
        for _ in 0..days {
            self.step_day();
        }
    }

    /// Run until the first day of `year` (the year has not started yet).
    pub fn run_to_year(&mut self, year: i64) {
        let target = self.calendar.year_start(year);
        while self.day < target {
            self.step_day();
        }
    }

    /// Place of a slot in the run order (generation modules first).
    fn rank(&self, slot: Slot) -> usize {
        let p = &self.plan;
        p.generation.iter().chain(&p.runtime).position(|&i| self.modules[i].manifest().slot == slot).unwrap_or(usize::MAX)
    }

    fn deliver(&mut self) {
        let mut events = std::mem::take(&mut self.pending);
        delivery_order(&mut events, |s| self.rank(s));
        let runtime = self.plan.runtime.clone();
        self.stage(&runtime, true, |s| {
            let mut outs = Vec::new();
            for p in &events {
                for &i in &runtime {
                    let m = s.modules[i].manifest();
                    let to_me = match &p.event {
                        Event::Granted { request, .. } => request.requester == m.slot,
                        e => m.handles.contains(&e.kind()),
                    };
                    if to_me {
                        outs.push((i, s.call(i, |m, w, cx| m.handle(w, &p.event, cx))));
                    }
                }
            }
            outs
        });
        let log = &mut self.event_log;
        log.entries.extend(events.into_iter().filter(|p| p.event.kind() != EventKind::Granted));
    }

    fn apply_requests(&mut self) {
        let mut queue = std::mem::take(&mut self.queue);
        let owner = |r: &Request| self.plan.owner[r.field.0 as usize].expect("validated: requested fields have owners");
        let owner_rank = |r: &Request| self.plan.runtime.iter().position(|&i| i == owner(r));
        queue.sort_by_key(|r| (owner_rank(r), r.order_key(self.rank(r.requester))));
        let runs: Vec<(usize, Vec<Request>)> =
            queue.chunk_by(|a, b| owner(a) == owner(b)).map(|run| (owner(&run[0]), run.to_vec())).collect();
        for (i, run) in runs {
            self.stage(&[i], true, |s| vec![(i, s.call(i, |m, w, cx| m.apply_requests(w, &run, cx)))]);
        }
    }

    /// Call one module with a fresh context.
    fn call(&mut self, i: usize, f: impl FnOnce(&mut dyn Module, &mut World, &mut Ctx)) -> Outbox {
        let mut out = Outbox::default();
        let slot = self.modules[i].manifest().slot;
        let mut cx = Ctx { day: self.day, seed: self.world.params.seed, calendar: &self.calendar, slot, out: &mut out };
        f(self.modules[i].as_mut(), &mut self.world, &mut cx);
        out
    }

    /// Run a stage for `actors` and check its work: ownership (strict only), conservation (when
    /// `conserve`), then field contracts. Then take in what the modules sent.
    fn stage(&mut self, actors: &[usize], conserve: bool, body: impl FnOnce(&mut Self) -> Vec<(usize, Outbox)>) {
        let owned_by_actors = |s: &Self, id: FieldId| s.plan.owner[id.0 as usize].is_some_and(|o| actors.contains(&o));
        let guard: Vec<(FieldId, u64)> = if self.strict {
            FieldId::all().filter(|&id| !owned_by_actors(self, id)).map(|id| (id, self.field_hash(id))).collect()
        } else {
            Vec::new()
        };
        let before: Vec<u64> = if conserve { QUANTITIES.iter().map(|&q| quantity_total(&self.world, q)).collect() } else { Vec::new() };

        let outs = body(self);

        for (id, h) in guard {
            if self.field_hash(id) != h {
                let who: Vec<Slot> = actors.iter().map(|&i| self.modules[i].manifest().slot).collect();
                panic!("day {}: {who:?} changed {}, which it does not own", self.day, id.def().name);
            }
        }
        if conserve {
            for (k, &q) in QUANTITIES.iter().enumerate() {
                let net: i64 = outs.iter().map(|(_, o)| o.flows.net(q)).sum();
                let expected = before[k] as i128 + net as i128;
                let actual = quantity_total(&self.world, q);
                if expected != actual as i128 {
                    self.problems.conservation_breaks += 1;
                    let by = actors.iter().map(|&i| self.modules[i].manifest().slot).collect();
                    self.post(None, Event::ConservationBroken { quantity: q, expected: expected.max(0) as u64, actual, by });
                }
            }
        }
        let owned: Vec<FieldId> = FieldId::all().filter(|&id| owned_by_actors(self, id)).collect();
        for id in owned {
            self.enforce_contract(id, actors);
        }
        for (i, out) in outs {
            let slot = self.modules[i].manifest().slot;
            self.queue.extend(out.requests);
            self.year_flows.merge(&out.flows);
            self.applied_today.extend(out.applied);
            for e in out.events {
                self.post(Some(slot), e);
            }
        }
    }

    /// Clamp a field into its bounds, reporting what moved. Clamping a conserved field is itself a
    /// flow, so the totals stay explained.
    fn enforce_contract(&mut self, id: FieldId, actors: &[usize]) {
        let def = id.def();
        if def.min == f64::NEG_INFINITY && def.max == f64::INFINITY {
            return;
        }
        let mut data = self.world.field(id);
        let before = def.conserved.map(|_| data.total());
        let moved = data.clamp(def.min, def.max);
        if moved.is_empty() {
            return;
        }
        if let (Some(q), Some(b)) = (def.conserved, before) {
            self.year_flows.add(q, Cause::ContractClamp, data.total() as i64 - b as i64);
        }
        self.world.set_field(id, data).expect("clamping keeps type and length");
        self.problems.contract_violations += moved.len() as u64;
        let by = (actors.len() == 1).then(|| self.modules[actors[0]].manifest().slot);
        // A few examples are enough to find the cause; the count is in `problems`.
        for (entity, value, clamped_to) in moved.into_iter().take(8) {
            self.post(None, Event::ContractViolated { field: id, entity, value, clamped_to, by });
        }
    }

    fn post(&mut self, emitter: Option<Slot>, event: Event) {
        self.pending.push(Posted { day: self.day, emitter, seq: self.seq, event });
        self.seq += 1;
    }

    fn field_hash(&self, id: FieldId) -> u64 {
        let mut h = Fingerprint::default();
        self.world.field(id).fingerprint(&mut h);
        h.0
    }

    /// Summary of the year that contains the last day run, as it stands now.
    pub fn year_so_far(&self) -> YearSummary {
        YearSummary {
            year: self.calendar.date(self.day.saturating_sub(1)).year,
            population: quantity_total(&self.world, Quantity::People),
            food_stock: quantity_total(&self.world, Quantity::Food),
            ledger: Ledger::from_flows(&self.year_flows),
            famine_provinces: self.world.starved_this_year.iter().filter(|&&s| s > 0).count(),
        }
    }

    /// Bit-exact hash of the whole simulation state: every field, the day, everything in flight and
    /// the year's totals. (Not the event log: it is history, and prunable.)
    pub fn fingerprint(&self) -> u64 {
        let mut h = Fingerprint::default();
        self.world.fingerprint(&mut h);
        h.u64s(&[self.day]);
        h.bytes(&to_bytes(&self.pending));
        h.bytes(&to_bytes(&self.year_flows));
        h.bytes(&to_bytes(&self.history));
        for m in &self.modules {
            h.bytes(&m.save_state());
        }
        h.0
    }
}

pub(crate) fn to_bytes<T: Serialize>(v: &T) -> Vec<u8> {
    postcard::to_stdvec(v).expect("in-memory serialisation")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WorldParams;

    fn small() -> Sim {
        Sim::new(&Config::new(WorldParams { n: 24, provinces: 40, seed: 11, land_fraction: 0.4, radius_km: 6371.0 })).unwrap()
    }

    fn slot_index(sim: &Sim, slot: Slot) -> usize {
        sim.modules.iter().position(|m| m.manifest().slot == slot).unwrap()
    }

    /// Takes the Economics slot's place and misbehaves in a chosen way.
    struct Rogue {
        manifest: &'static Manifest,
        bad: fn(&mut World),
    }

    impl Module for Rogue {
        fn manifest(&self) -> &'static Manifest {
            self.manifest
        }
        fn tick(&mut self, w: &mut World, _cx: &mut Ctx) {
            (self.bad)(w);
        }
    }

    fn with_rogue(bad: fn(&mut World)) -> Sim {
        let mut sim = small();
        let i = slot_index(&sim, Slot::Economics);
        let manifest = sim.modules[i].manifest();
        sim.modules[i] = Box::new(Rogue { manifest, bad });
        sim
    }

    #[test]
    fn food_from_nowhere_is_caught_and_blamed() {
        let mut sim = with_rogue(|w| w.food_stock[0] += 1);
        // Day 0 starts a month, so the rogue runs; the report is delivered (and logged) on day 1.
        sim.run_days(2);
        assert_eq!(sim.problems.conservation_breaks, 1);
        let blamed = sim.event_log.entries.iter().find_map(|p| match &p.event {
            Event::ConservationBroken { quantity, expected, actual, by } => Some((*quantity, *actual - *expected, by.clone())),
            _ => None,
        });
        assert_eq!(blamed, Some((Quantity::Food, 1, vec![Slot::Economics])));
    }

    #[test]
    #[should_panic(expected = "[Economics] changed pop.size, which it does not own")]
    fn writing_someone_elses_field_stops_a_strict_run() {
        let mut sim = with_rogue(|w| w.pops.size[0] += 1);
        sim.strict = true;
        sim.run_days(32);
    }

    #[test]
    fn several_pops_share_a_short_store_by_the_largest_remainder_rule() {
        let mut sim = small();
        sim.strict = true;
        let p = sim.world.pops.province[0];
        // Two more pops in the first pop's province.
        sim.world.pops.push(p, 1000);
        sim.world.pops.push(p, 2000);
        let n = sim.world.pops.size.len();
        let tags = [0, n - 2, n - 1];
        // Up to the next month start, then make that province's store short.
        sim.run_days(sim.calendar.def.months[0].days as u64);
        assert!(sim.calendar.is_month_start(sim.day));
        let month = sim.calendar.def.months[sim.calendar.date(sim.day).month].days as u64;
        let wants: Vec<_> = tags.iter().map(|&k| (crate::requests::Priority::Essential, sim.world.pops.size[k] * month, k as u64)).collect();
        sim.world.food_stock[p as usize] = 12_345;
        sim.step_day();

        // The harvest and spoilage ran that morning; what was left is shared by the rule and used up.
        let grants: Vec<(u64, u64)> = sim.applied_today.iter().filter(|(r, _)| r.entity == p).map(|(r, g)| (r.tag, *g)).collect();
        let shared: u64 = grants.iter().map(|g| g.1).sum();
        assert!(shared < wants.iter().map(|w| w.1).sum::<u64>(), "the store was not short");
        assert_eq!(sim.world.food_stock[p as usize], 0);
        assert_eq!(grants.iter().map(|g| g.0).collect::<Vec<_>>(), tags.iter().map(|&k| k as u64).collect::<Vec<_>>());
        assert_eq!(grants.iter().map(|g| g.1).collect::<Vec<_>>(), crate::requests::ration(shared, &wants));
        assert_eq!(sim.problems, Problems::default());
    }
}
