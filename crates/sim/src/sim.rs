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
use crate::fields::{known, quantity_total, FieldData, FieldId, Tier};
use crate::edit::{Edit, MapChange};
use crate::module::{goods_kept, plan, ConfigError, Ctx, Flows, Manifest, Module, Outbox, Plan, Slot};
use crate::requests::{Cause, Request};
use crate::world::{Fingerprint, World};
use crate::fields::Quantity;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    /// Net change from field contracts clamping a value back into bounds (normally 0). With it,
    /// each year balances exactly: food after = before + produced − spoiled − eaten + food_clamped,
    /// people after = before + births − deaths + people_clamped.
    pub food_clamped: i64,
    pub people_clamped: i64,
    /// Net change from the player's edits, which balance the year in the same way.
    pub food_edited: i64,
    pub people_edited: i64,
}

impl Ledger {
    /// The year's totals from its flows. `food` is the quantity food is counted in; without one
    /// (a goods tree with no food) the food totals are 0.
    pub fn from_flows(f: &Flows, food: Option<Quantity>) -> Ledger {
        let pos = |q, c| f.get(q, c).max(0) as u64;
        let neg = |q, c| (-f.get(q, c)).max(0) as u64;
        let starved = neg(Quantity::People, Cause::Starvation);
        let food_in = |c| food.map_or(0, |q| pos(q, c));
        let food_out = |c| food.map_or(0, |q| neg(q, c));
        let food_net = |c| food.map_or(0, |q| f.get(q, c));
        Ledger {
            produced: food_in(Cause::Harvest),
            spoiled: food_out(Cause::Spoilage),
            eaten: food_out(Cause::Eating),
            births: pos(Quantity::People, Cause::Births),
            deaths: neg(Quantity::People, Cause::Deaths) + starved,
            starved,
            food_clamped: food_net(Cause::ContractClamp),
            people_clamped: f.get(Quantity::People, Cause::ContractClamp),
            food_edited: food_net(Cause::Edit),
            people_edited: f.get(Quantity::People, Cause::Edit),
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
    /// A player's poke. It is an ordinary event: sent on the command's day (by nobody, like the
    /// engine's own events) and delivered the next day, so the world reacts by its own rules.
    Poke { event: Event },
    /// A player's edit: it sets values directly, at the start of the command's day.
    Edit { edit: Edit },
}

/// `last_change` value for "not since the world was made or loaded".
pub const NEVER: u64 = u64::MAX;

/// The parts of a day, as the inspector's trace names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Generate,
    Init,
    /// Yesterday's events delivered.
    Deliver,
    /// A module's main logic.
    Tick,
    /// An owner applied the day's change requests.
    Apply,
    /// A player's edit, at the start of the day.
    Edit,
}

impl Stage {
    /// Stages of a running world: checked for conservation, and tracked for "last changed".
    fn running(self) -> bool {
        !matches!(self, Stage::Generate | Stage::Init)
    }
}

/// Everything that happened to one province during one day, in order: what each module did and
/// why, and which values changed. Built by [`Sim::trace`].
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    pub day: u64,
    pub province: u32,
    pub steps: Vec<TraceStep>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraceStep {
    pub stage: Stage,
    /// The module responsible: the one that explained or asked, or the owner of a changed field.
    /// `None` for the engine.
    pub module: Option<Slot>,
    pub what: Traced,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Traced {
    /// A module's own explanation (see [`Ctx::note`]).
    Note(String),
    /// A value of the province, or of one of its pops (`entity` is then the pop), changed.
    Changed { field: FieldId, entity: u32, before: f64, after: f64 },
    /// A change request about the province was made today; it is applied tonight.
    Requested(Request),
    /// A change request about the province was applied, and this much was granted.
    Granted(Request, u64),
    /// An event about the province was delivered today.
    Delivered(Event),
    /// An event about the province was sent today, for delivery tomorrow.
    Sent(Event),
}

/// The state of the running world at the start of a day: every value that can change after world
/// creation, each module's private state and everything in flight. Fields written at generation (the
/// map) are left out: only edits change them, and the map is put back from the edits' changes.
#[derive(Clone)]
pub struct Checkpoint {
    pub day: u64,
    /// How many of the edits' map changes the map had.
    pub(crate) map_at: usize,
    /// Commands before this one had been applied, if edits.
    pub(crate) edits_done: usize,
    fields: Vec<(FieldId, FieldData)>,
    modules: Vec<Vec<u8>>,
    pending: Vec<Posted>,
    year_flows: Flows,
    history: Vec<YearSummary>,
    problems: Problems,
    applied_today: Vec<(Request, u64)>,
    last_change: Vec<Vec<u64>>,
    /// Last month's trade, for showing it (not saved: checkpoints live in memory).
    trade: Arc<crate::markets::TradeRecord>,
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
    pub(crate) seq: u32,
    /// Flows for the year in progress.
    pub year_flows: Flows,
    pub history: Vec<YearSummary>,
    pub problems: Problems,
    pub commands: Vec<Command>,
    /// Requests applied today, with what each was granted and why it was made.
    pub applied_today: Vec<(Request, u64)>,
    /// Delivered events (not grants). Not part of saves.
    pub event_log: EventLog,
    /// Where the last day's deliveries start in `event_log`.
    pub(crate) log_today: usize,
    /// Stop with a panic when a module writes a field it doesn't own. Slow: for tests.
    pub strict: bool,
    /// Index of the first poke not yet sent (commands are in day order).
    pub(crate) next_command: usize,
    /// Commands before this index have been applied, if edits. Between days, every edit logged for
    /// today or earlier has been.
    pub(crate) edits_done: usize,
    /// What each edit applied so far did to the map, in the order applied: going back undoes them,
    /// and a replay that applies the same edit again reuses its change.
    pub(crate) map_changes: Vec<Arc<MapChange>>,
    /// How many of `map_changes` the map has now.
    pub(crate) map_at: usize,
    /// Edits that no longer fit the world when their day came (for example, after an earlier edit
    /// they relied on was taken back), with the day and why. They are left out.
    pub skipped_edits: Vec<(u64, String)>,
    /// The world at the start of each year run so far, and where it was loaded: what
    /// [`Sim::rewind`] and [`Sim::trace`] start from. Taken before that day's edits.
    pub checkpoints: Vec<Arc<Checkpoint>>,
    /// Per field, the day each entity's value last changed while the world ran ([`NEVER`] if it
    /// hasn't). Empty for fields only generation writes.
    pub last_change: Vec<Vec<u64>>,
    /// What was found so far while a day is replayed with tracing on.
    trace: Option<Trace>,
}

impl Sim {
    /// Check the configuration, generate the world and set up the running modules.
    pub fn new(config: &Config) -> Result<Sim, ConfigError> {
        Sim::new_with(config, config.modules.build())
    }

    /// [`Sim::new`] with modules of the caller's own instead of the configuration's (tests).
    pub(crate) fn new_with(config: &Config, modules: Vec<Box<dyn Module>>) -> Result<Sim, ConfigError> {
        let mut sim = Sim::assemble_with(config, modules)?;
        for &i in &sim.plan.generation.clone() {
            sim.stage(Stage::Generate, &[i], |s| {
                s.modules[i].generate(&mut s.world);
                Vec::new()
            });
        }
        sim.world.rebuild_routes();
        let owned: Vec<bool> = sim.plan.owner.iter().map(Option::is_some).collect();
        sim.world.fill_defaults(&owned);
        for &i in &sim.plan.runtime.clone() {
            // Creating the starting stocks and people is not a flow, so no conservation check.
            sim.stage(Stage::Init, &[i], |s| vec![(i, s.call(i, |m, w, cx| m.init(w, cx)))]);
        }
        sim.arrive();
        Ok(sim)
    }

    /// Modules, plan and calendar for a configuration, with an empty world (shared with loading).
    pub(crate) fn assemble(config: &Config) -> Result<Sim, ConfigError> {
        Sim::assemble_with(config, config.modules.build())
    }

    pub(crate) fn assemble_with(config: &Config, modules: Vec<Box<dyn Module>>) -> Result<Sim, ConfigError> {
        let calendar = Calendar::new(config.calendar.clone()).map_err(ConfigError)?;
        let manifests: Vec<&Manifest> = modules.iter().map(|m| m.manifest()).collect();
        let plan = plan(&manifests)?;
        let tree = crate::taxonomy::Taxonomy::new(&config.goods).map_err(ConfigError)?;
        let goods = goods_kept(&manifests, &plan, tree)?;
        for m in &modules {
            m.check(&goods).map_err(|e| ConfigError(format!("{:?} \"{}\": {e}", m.manifest().slot, m.manifest().version)))?;
        }
        let records = manifests.iter().map(|m| ModuleRecord::of(m)).collect();
        let mut world = World::empty(&config.world);
        world.goods = Arc::new(goods);
        Ok(Sim {
            world,
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
            log_today: 0,
            strict: false,
            next_command: 0,
            edits_done: 0,
            map_changes: Vec::new(),
            map_at: 0,
            skipped_edits: Vec::new(),
            checkpoints: Vec::new(),
            last_change: vec![Vec::new(); crate::fields::FIELDS.len()],
            trace: None,
        })
    }

    /// Events waiting for tomorrow (grants included).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn manifests(&self) -> Vec<&'static Manifest> {
        self.modules.iter().map(|m| m.manifest()).collect()
    }

    /// Run one day, then arrive at the next: apply the edits logged for it. Returns whether anything
    /// ran (false = an idle day).
    pub fn step_day(&mut self) -> bool {
        let ran = self.run_day();
        self.arrive() || ran
    }

    /// Arrive at the start of today: keep a checkpoint if a year starts, then apply the edits logged
    /// for today. Returns whether there were any.
    fn arrive(&mut self) -> bool {
        self.seq = 0;
        // Replays pass year starts that already have one.
        if self.calendar.is_year_start(self.day) && self.checkpoints.last().is_none_or(|c| c.day < self.day) {
            self.checkpoints.push(Arc::new(self.checkpoint()));
        }
        self.apply_todays_edits()
    }

    pub(crate) fn apply_todays_edits(&mut self) -> bool {
        let mut any = false;
        while let Some(c) = self.commands.get(self.edits_done).filter(|c| c.day <= self.day) {
            if let CommandKind::Edit { edit } = &c.kind {
                let (day, edit) = (c.day, edit.clone());
                if let Err(why) = self.apply_edit(&edit) {
                    self.skipped_edits.push((day, format!("{}: {why}", edit.describe())));
                }
                any = true;
            }
            self.edits_done += 1;
        }
        any
    }

    /// Run today, up to the end of the day.
    fn run_day(&mut self) -> bool {
        let mut ran = false;
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
        self.log_today = self.event_log.entries.len();
        if !self.pending.is_empty() {
            self.deliver();
            ran = true;
        }
        // Today's pokes go out after today's deliveries, so they arrive tomorrow like any event.
        // (Today's edits were applied on arriving at the day.)
        while let Some(c) = self.commands.get(self.next_command).filter(|c| c.day <= self.day) {
            if let CommandKind::Poke { event } = &c.kind {
                let event = event.clone();
                self.post(None, event);
                ran = true;
            }
            self.next_command += 1;
        }
        for k in 0..self.plan.runtime.len() {
            let i = self.plan.runtime[k];
            if self.modules[i].manifest().cadence.due(&self.calendar, self.day) {
                self.stage(Stage::Tick, &[i], |s| vec![(i, s.call(i, |m, w, cx| m.tick(w, cx)))]);
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

    /// Poke the world today (the day about to run): the event is sent today and delivered
    /// tomorrow. It goes into the command log, so replays and saves repeat it.
    pub fn poke(&mut self, event: Event) {
        let at = self.commands.partition_point(|c| c.day <= self.day);
        self.commands.insert(at, Command { day: self.day, kind: CommandKind::Poke { event } });
        // Every edit before it is applied.
        self.edits_done = at + 1;
    }

    /// Poke the world on a day still to come (or today); it goes into the command log like any poke.
    pub fn schedule(&mut self, day: u64, event: Event) -> Result<(), String> {
        if day < self.day {
            return Err(format!("{} has passed", self.calendar.format(day)));
        }
        if day == self.day {
            self.poke(event);
        } else {
            let at = self.commands.partition_point(|c| c.day <= day);
            self.commands.insert(at, Command { day, kind: CommandKind::Poke { event } });
        }
        Ok(())
    }

    /// Edit the world today (the day about to run). It takes effect at once, so the world as it
    /// stands shows it, and goes into the command log. Refused, with the reason, if it doesn't fit
    /// the world as it is.
    pub fn edit(&mut self, edit: Edit) -> Result<(), String> {
        let at = self.commands.partition_point(|c| c.day <= self.day);
        debug_assert_eq!(self.edits_done, at, "between days, today's edits are applied");
        self.apply_edit(&edit)?;
        self.commands.insert(at, Command { day: self.day, kind: CommandKind::Edit { edit } });
        self.edits_done = at + 1;
        Ok(())
    }

    /// Memory held by the edits' map changes, bytes (roughly).
    pub fn map_change_bytes(&self) -> usize {
        self.map_changes.iter().map(|c| c.bytes()).sum()
    }

    /// Take back command `index` (an edit or a poke) as if it had never been given: the world goes
    /// back to the last checkpoint before it and runs again to today without it. Edits made later
    /// that relied on it are left out (see [`Sim::skipped_edits`]).
    pub fn withdraw(&mut self, index: usize) -> Result<Command, String> {
        let Some(c) = self.commands.get(index) else { return Err(format!("no command {index}")) };
        if matches!(c.kind, CommandKind::StartSegment { .. }) {
            return Err("the world's configuration can't be taken back".into());
        }
        let day = c.day;
        let mut taken = None;
        self.change_history(day, index, |cs| taken = Some(cs.remove(index)))?;
        Ok(taken.expect("removed above"))
    }

    /// Give a command again (one taken back by [`Sim::withdraw`]), after the others of its day; the
    /// world runs again from the last checkpoint before it.
    pub fn reinstate(&mut self, command: Command) -> Result<(), String> {
        if matches!(command.kind, CommandKind::StartSegment { .. }) {
            return Err("only edits and pokes can be given again".into());
        }
        let day = command.day;
        if day > self.day {
            return Err(format!("{} hasn't come yet", self.calendar.format(day)));
        }
        let at = self.commands.partition_point(|c| c.day <= day);
        self.change_history(day, at, |cs| cs.insert(at, command))
    }

    /// Change the command log at `at` (a command of `day`), then run the world again from the last
    /// checkpoint that hadn't seen that part of the log, back up to today.
    fn change_history(&mut self, day: u64, at: usize, change: impl FnOnce(&mut Vec<Command>)) -> Result<(), String> {
        let today = self.day;
        let k = self
            .checkpoints
            .iter()
            .rposition(|c| c.day < day || (c.day == day && c.edits_done <= at))
            .ok_or_else(|| format!("nothing kept from before {}", self.calendar.format(day)))?;
        change(&mut self.commands);
        self.checkpoints.truncate(k + 1);
        self.replay_to(today, true)
    }

    /// Events delivered on the last day run (grants excluded).
    pub fn delivered_today(&self) -> &[Posted] {
        &self.event_log.entries[self.log_today.min(self.event_log.entries.len())..]
    }

    /// Fields that can change once the world runs: owned by a running module, or by nobody.
    pub fn running_fields(&self) -> impl Iterator<Item = FieldId> + '_ {
        FieldId::all().filter(|&id| !self.plan.owner[id.0 as usize].is_some_and(|o| self.plan.generation.contains(&o)))
    }

    /// Fields only world generation and edits write: the map.
    pub fn map_fields(&self) -> impl Iterator<Item = FieldId> + '_ {
        let running: Vec<FieldId> = self.running_fields().collect();
        FieldId::all().filter(move |id| !running.contains(id))
    }

    /// The world as it stands now, between days.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            day: self.day,
            map_at: self.map_at,
            edits_done: self.edits_done,
            fields: self.running_fields().map(|id| (id, self.world.field(id))).collect(),
            modules: self.modules.iter().map(|m| m.save_state()).collect(),
            pending: self.pending.clone(),
            year_flows: self.year_flows.clone(),
            history: self.history.clone(),
            problems: self.problems,
            applied_today: self.applied_today.clone(),
            last_change: self.last_change.clone(),
            trade: self.world.trade.clone(),
        }
    }

    /// Put the world back as it was at `c`. The event log keeps only what was delivered before
    /// that day; commands and checkpoints are left as they are.
    pub fn restore(&mut self, c: &Checkpoint) {
        while self.map_at > c.map_at {
            self.map_at -= 1;
            self.map_changes[self.map_at].undo(&mut self.world);
        }
        while self.map_at < c.map_at {
            self.map_changes[self.map_at].redo(&mut self.world);
            self.map_at += 1;
        }
        for (id, data) in &c.fields {
            self.world.set_field(*id, data.clone()).expect("a checkpoint of this world");
        }
        for (m, bytes) in self.modules.iter_mut().zip(&c.modules) {
            m.load_state(bytes).expect("a checkpoint of this world");
        }
        self.day = c.day;
        self.pending = c.pending.clone();
        self.queue.clear();
        self.seq = 0;
        self.year_flows = c.year_flows.clone();
        self.history = c.history.clone();
        self.problems = c.problems;
        self.applied_today = c.applied_today.clone();
        self.last_change = c.last_change.clone();
        self.world.trade = c.trade.clone();
        let day = c.day;
        self.event_log.entries.retain(|p| p.day + 1 < day);
        self.log_today = self.event_log.entries.partition_point(|p| p.day + 2 < day);
        self.next_command = self.commands.partition_point(|c| c.day < day);
        self.edits_done = c.edits_done;
        self.skipped_edits.retain(|s| s.0 < day);
    }

    /// Restore the latest checkpoint at or before the start of `day`, then run up to it; with
    /// `edits`, apply that day's edits too.
    fn replay_to(&mut self, day: u64, edits: bool) -> Result<(), String> {
        if day > self.day {
            return Err(format!("{} hasn't run yet", self.calendar.format(day)));
        }
        let k = self.checkpoints.partition_point(|c| c.day <= day);
        let Some(c) = k.checked_sub(1).map(|k| self.checkpoints[k].clone()) else {
            return Err(format!("nothing kept from as early as {}", self.calendar.format(day)));
        };
        self.restore(&c);
        while self.day < day {
            self.arrive();
            self.run_day();
        }
        if edits {
            self.arrive();
        }
        Ok(())
    }

    /// Go to the start of any `day` from the earliest kept on, back or forward, keeping every
    /// checkpoint: the command log hasn't changed, so the days after are the same days again. For
    /// moving through history; [`Sim::rewind`] is for changing it.
    pub fn goto(&mut self, day: u64) -> Result<(), String> {
        if day < self.day {
            self.replay_to(day, true)
        } else {
            // A checkpoint ahead saves running the days up to it again. It must be a year before the
            // day, so the event log still holds that year (the famine layer reads it).
            let year = self.calendar.days_per_year() as u64;
            if let Some(c) = self.checkpoints.iter().rev().find(|c| c.day + year <= day && c.day > self.day).cloned() {
                self.restore(&c);
                self.arrive();
            }
            while self.day < day {
                self.step_day();
            }
            Ok(())
        }
    }

    /// The earliest day going back can reach.
    pub fn earliest_day(&self) -> u64 {
        self.checkpoints.first().map_or(self.day, |c| c.day)
    }

    /// Which map the world has: the same pair means the same map, within one timeline.
    pub fn map_version(&self) -> (usize, usize) {
        (self.map_at, self.map_at.checked_sub(1).map_or(0, |i| Arc::as_ptr(&self.map_changes[i]) as usize))
    }

    /// Go back to the start of an earlier `day` and forget the days after it, so a poke made now
    /// starts a new future. Commands already logged for later days stay and run again.
    pub fn rewind(&mut self, day: u64) -> Result<(), String> {
        self.replay_to(day, true)?;
        self.checkpoints.retain(|c| c.day <= day);
        Ok(())
    }

    /// Run `day` again with tracing on for `province`, then come back to exactly where the world
    /// was. Determinism makes the replayed day the day that ran; nothing extra is stored to explain
    /// it.
    pub fn trace(&mut self, day: u64, province: u32) -> Result<Trace, String> {
        if day >= self.day {
            return Err(format!("{} hasn't run yet", self.calendar.format(day)));
        }
        if province as usize >= self.world.provinces.count {
            return Err(format!("no province {province}"));
        }
        let now = self.checkpoint();
        let (log, log_today) = (self.event_log.entries.clone(), self.log_today);
        let result = self.replay_to(day, false).map(|()| {
            self.trace = Some(Trace { day, province, steps: Vec::new() });
            self.arrive();
            self.run_day();
            self.trace.take().expect("set above")
        });
        self.restore(&now);
        self.event_log.entries = log;
        self.log_today = log_today;
        result
    }

    /// Apply one edit at the start of today, as its own stage: check it, change the map (or make
    /// again the change the same edit made last time), then the running world. Then check that
    /// people and food changed only by what the edit declared, clamp values into their contracts,
    /// and note what changed.
    fn apply_edit(&mut self, edit: &Edit) -> Result<(), String> {
        for &f in edit.sets() {
            if self.plan.owner[known(f).0 as usize].is_none() {
                let what = match f {
                    "pop.size" => "people",
                    "province.stock" => "stored goods",
                    other => other,
                };
                return Err(format!("no module keeps {what} in this world, so there are none to set"));
            }
        }
        edit.check(&self.world)?;
        let quantities = self.world.quantities();
        let before: Vec<u64> = quantities.iter().map(|&q| quantity_total(&self.world, q)).collect();
        let running: Vec<FieldId> = self.running_fields().collect();
        let watched: Vec<(FieldId, FieldData)> = running.iter().map(|&id| (id, self.world.field(id))).collect();
        let map = edit.changes_map().then(|| {
            match self.map_changes.get(self.map_at) {
                Some(c) if c.edit == *edit => c.redo(&mut self.world),
                _ => {
                    self.map_changes.truncate(self.map_at);
                    let old = self.map_fields().map(|id| (id, self.world.field(id))).collect();
                    // Everything generation derives from the land's height, except provinces.
                    let (modules, plan) = (&self.modules, &self.plan);
                    let mut regenerate = |w: &mut World| {
                        for &i in &plan.generation {
                            if !matches!(modules[i].manifest().slot, Slot::Terrain | Slot::Provinces) {
                                modules[i].generate(w);
                            }
                        }
                    };
                    edit.change_map(&mut self.world, &mut regenerate);
                    self.map_changes.push(Arc::new(MapChange::between(edit.clone(), old, &mut self.world)));
                }
            }
            self.map_at += 1;
            self.map_at - 1
        });
        let flows = edit.change_running(&mut self.world, map.map(|i| &*self.map_changes[i]));
        self.world.fit_realm_stores();

        for (k, &q) in quantities.iter().enumerate() {
            let expected = before[k] as i128 + flows.net(q) as i128;
            let actual = quantity_total(&self.world, q);
            if expected != actual as i128 {
                self.problems.conservation_breaks += 1;
                self.post(None, Event::ConservationBroken { quantity: q, expected: expected.max(0) as u64, actual, by: Vec::new() });
            }
        }
        for &id in &running {
            self.enforce_contract(id, &[]);
        }
        // A map edit is about the provinces whose land or values it changed (cheaper to find from
        // its change than from the edit, which would measure a brush over the whole planet).
        let traced = self.trace.as_ref().map(|t| t.province);
        if let Some(p) = traced
            && match map {
                Some(i) => self.map_changes[i].provinces(&self.world).contains(&p),
                None => edit.provinces(&self.world).contains(&p),
            }
            && let Some(t) = &mut self.trace
        {
            t.steps.push(TraceStep { stage: Stage::Edit, module: None, what: Traced::Note(edit.describe()) });
        }
        for (id, old) in watched {
            self.record_changes(Stage::Edit, id, &old);
        }
        self.year_flows.merge(&flows);
        Ok(())
    }

    /// Place of a slot in the run order (generation modules first).
    fn rank(&self, slot: Slot) -> usize {
        let p = &self.plan;
        p.generation.iter().chain(&p.runtime).position(|&i| self.modules[i].manifest().slot == slot).unwrap_or(usize::MAX)
    }

    fn deliver(&mut self) {
        let mut events = std::mem::take(&mut self.pending);
        delivery_order(&mut events, |s| self.rank(s));
        if let Some(t) = &mut self.trace {
            for p in events.iter().filter(|p| p.event.province() == Some(t.province)) {
                t.steps.push(TraceStep { stage: Stage::Deliver, module: p.emitter, what: Traced::Delivered(p.event.clone()) });
            }
        }
        let runtime = self.plan.runtime.clone();
        self.stage(Stage::Deliver, &runtime, |s| {
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
            self.stage(Stage::Apply, &[i], |s| vec![(i, s.call(i, |m, w, cx| m.apply_requests(w, &run, cx)))]);
        }
    }

    /// Call one module with a fresh context.
    fn call(&mut self, i: usize, f: impl FnOnce(&mut dyn Module, &mut World, &mut Ctx)) -> Outbox {
        let mut out = Outbox::default();
        let slot = self.modules[i].manifest().slot;
        let trace = self.trace.as_ref().map(|t| t.province);
        let goods = self.world.goods.clone();
        let mut cx = Ctx { day: self.day, seed: self.world.params.seed, calendar: &self.calendar, goods: &goods, slot, out: &mut out, trace };
        f(self.modules[i].as_mut(), &mut self.world, &mut cx);
        out
    }

    /// Run a stage for `actors` and check its work: ownership (strict only), then, once the world
    /// runs, conservation; then field contracts. Then note what changed, and take in what the
    /// modules sent.
    fn stage(&mut self, kind: Stage, actors: &[usize], body: impl FnOnce(&mut Self) -> Vec<(usize, Outbox)>) {
        let conserve = kind.running();
        let owned_by_actors = |s: &Self, id: FieldId| s.plan.owner[id.0 as usize].is_some_and(|o| actors.contains(&o));
        let guard: Vec<(FieldId, u64)> = if self.strict {
            FieldId::all().filter(|&id| !owned_by_actors(self, id)).map(|id| (id, self.field_hash(id))).collect()
        } else {
            Vec::new()
        };
        let quantities = if conserve { self.world.quantities() } else { Vec::new() };
        let before: Vec<u64> = quantities.iter().map(|&q| quantity_total(&self.world, q)).collect();
        // Only the actors can change anything (strict mode checks it), so only their fields are watched.
        let watched: Vec<(FieldId, FieldData)> =
            if conserve { FieldId::all().filter(|&id| owned_by_actors(self, id)).map(|id| (id, self.world.field(id))).collect() } else { Vec::new() };

        let outs = body(self);

        for (id, h) in guard {
            if self.field_hash(id) != h {
                let who: Vec<Slot> = actors.iter().map(|&i| self.modules[i].manifest().slot).collect();
                panic!("day {}: {who:?} changed {}, which it does not own", self.day, id.def().name);
            }
        }
        self.world.fit_realm_stores();
        if conserve {
            for (k, &q) in quantities.iter().enumerate() {
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
        for (id, old) in watched {
            self.record_changes(kind, id, &old);
        }
        for (i, out) in outs {
            let slot = self.modules[i].manifest().slot;
            if let Some(t) = &mut self.trace {
                let p = t.province;
                let about = |r: &Request| r.province() == Some(p);
                let step = |what| TraceStep { stage: kind, module: Some(slot), what };
                t.steps.extend(out.notes.into_iter().map(|n| step(Traced::Note(n))));
                t.steps.extend(out.requests.iter().filter(|r| about(r)).map(|r| step(Traced::Requested(*r))));
                t.steps.extend(out.applied.iter().filter(|(r, _)| about(r)).map(|&(r, g)| step(Traced::Granted(r, g))));
                t.steps.extend(out.events.iter().filter(|e| e.province() == Some(p)).map(|e| step(Traced::Sent(e.clone()))));
            }
            self.queue.extend(out.requests);
            self.year_flows.merge(&out.flows);
            self.applied_today.extend(out.applied);
            for e in out.events {
                self.post(Some(slot), e);
            }
        }
    }

    /// Stamp today on every value of `id` that differs from `old`, and trace the traced
    /// province's changes.
    fn record_changes(&mut self, kind: Stage, id: FieldId, old: &FieldData) {
        let new = self.world.field(id);
        let changed = old.differences(&new);
        let stamps = &mut self.last_change[id.0 as usize];
        stamps.resize(new.len(), NEVER);
        for &e in &changed {
            stamps[e as usize] = self.day;
        }
        let Some(t) = &mut self.trace else { return };
        let mine = |e: u32| match id.def().tier {
            Tier::Province => e == t.province,
            Tier::ProvinceGood => self.world.goods.split(e as usize).0 == t.province as usize,
            Tier::Pop => self.world.pops.province.get(e as usize) == Some(&t.province),
            Tier::Cell => self.world.provinces.of_cell.get(e as usize) == Some(&t.province),
            Tier::Army => self.world.armies.province.get(e as usize) == Some(&t.province),
            // A realm's own values are not a province's.
            Tier::Realm | Tier::RealmGood => false,
        };
        // An edit is the player's doing, not the owner's.
        let module = if kind == Stage::Edit { None } else { self.plan.owner[id.0 as usize].map(|o| self.modules[o].manifest().slot) };
        for e in changed.into_iter().filter(|&e| mine(e)) {
            let what = Traced::Changed { field: id, entity: e, before: old.value(e as usize), after: new.value(e as usize) };
            t.steps.push(TraceStep { stage: kind, module, what });
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
        let goods = self.world.goods.clone();
        let before = if def.conserved { goods.totals(id, &data) } else { Vec::new() };
        let moved = data.clamp(def.min, def.max);
        if moved.is_empty() {
            return;
        }
        // Each quantity the field holds (each class of goods) balances on its own.
        if def.conserved {
            for ((q, b), (_, a)) in before.into_iter().zip(goods.totals(id, &data)) {
                self.year_flows.add(q, Cause::ContractClamp, a as i64 - b as i64);
            }
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
        let food = self.food();
        YearSummary {
            year: self.calendar.date(self.day.saturating_sub(1)).year,
            population: quantity_total(&self.world, Quantity::People),
            food_stock: food.map_or(0, |q| quantity_total(&self.world, q)),
            ledger: Ledger::from_flows(&self.year_flows, food),
            famine_provinces: self.world.starved_this_year.iter().filter(|&&s| s > 0).count(),
        }
    }

    /// The quantity food is counted in: the goods tree's "food" class, if it has one.
    pub fn food(&self) -> Option<Quantity> {
        let t = &self.world.goods.tree;
        t.find("food").and_then(|n| t.class_of(n)).map(|c| Quantity::Goods(c as u8))
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
        let mut sim = with_rogue(|w| w.stock[0] += 1);
        // Day 0 starts a month, so the rogue runs; the report is delivered (and logged) on day 1.
        sim.run_days(2);
        assert_eq!(sim.problems.conservation_breaks, 1);
        let blamed = sim.event_log.entries.iter().find_map(|p| match &p.event {
            Event::ConservationBroken { quantity, expected, actual, by } => Some((*quantity, *actual - *expected, by.clone())),
            _ => None,
        });
        assert_eq!(blamed, Some((Quantity::Goods(0), 1, vec![Slot::Economics])));
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
        sim.world.stock[p as usize] = 12_345;
        sim.step_day();

        // The harvest and spoilage ran that morning; what was left is shared by the rule and used up.
        let grants: Vec<(u64, u64)> = sim.applied_today.iter().filter(|(r, _)| r.entity == p).map(|(r, g)| (r.tag, *g)).collect();
        let shared: u64 = grants.iter().map(|g| g.1).sum();
        assert!(shared < wants.iter().map(|w| w.1).sum::<u64>(), "the store was not short");
        assert_eq!(sim.world.stock[p as usize], 0);
        assert_eq!(grants.iter().map(|g| g.0).collect::<Vec<_>>(), tags.iter().map(|&k| k as u64).collect::<Vec<_>>());
        assert_eq!(grants.iter().map(|g| g.1).collect::<Vec<_>>(), crate::requests::ration(shared, &wants));
        assert_eq!(sim.problems, Problems::default());
    }
}
