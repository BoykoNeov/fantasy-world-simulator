//! Where a simulated day's time goes, for the speed table ([`crate::speed`]): each module's own
//! work, and the engine's checks around it, stage by stage.
//!
//! Off unless a [`Timing`] is put in [`crate::sim::Sim::timing`]; then the engine reads the clock
//! around each stage and each module call. Not part of the world: not saved, not fingerprinted, not
//! copied by a fork.

use crate::module::Slot;
use crate::sim::Stage;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct Timing {
    /// Time in each module's own code (its tick, its serving of requests, its handling of events),
    /// by slot and stage.
    pub work: BTreeMap<(Slot, Stage), Duration>,
    /// The engine's checks around each stage (conservation sums, copies for the last-changed
    /// stamps, field contracts), by slot and stage. A stage several modules share (delivering
    /// events) shares its checks between them equally.
    pub checks: BTreeMap<(Slot, Stage), Duration>,
    /// The whole of the days run while timing, the engine's work between stages included.
    pub total: Duration,
    pub days: u64,
    /// The stage under way: its kind, when it began, and the module work inside it so far.
    current: Option<(Stage, Instant, Duration)>,
}

impl Timing {
    /// The engine's own time outside every stage: copying fields read as the day began, sorting
    /// events, checkpoints.
    pub fn between(&self) -> Duration {
        let stages: Duration = self.work.values().chain(self.checks.values()).sum();
        self.total.saturating_sub(stages)
    }

    /// Each module's share of the days' time: (slot, its own work, the checks around it).
    pub fn by_module(&self) -> Vec<(Slot, Duration, Duration)> {
        let mut out: BTreeMap<Slot, (Duration, Duration)> = BTreeMap::new();
        for (&(s, _), &d) in &self.work {
            out.entry(s).or_default().0 += d;
        }
        for (&(s, _), &d) in &self.checks {
            out.entry(s).or_default().1 += d;
        }
        out.into_iter().map(|(s, (w, c))| (s, w, c)).collect()
    }

    pub(crate) fn begin_stage(&mut self, kind: Stage) {
        self.current = Some((kind, Instant::now(), Duration::ZERO));
    }

    pub(crate) fn module_call(&mut self, slot: Slot, took: Duration) {
        if let Some((kind, _, inside)) = &mut self.current {
            *inside += took;
            *self.work.entry((slot, *kind)).or_default() += took;
        }
    }

    pub(crate) fn end_stage(&mut self, actors: &[Slot]) {
        let Some((kind, began, inside)) = self.current.take() else { return };
        if actors.is_empty() {
            return;
        }
        let checks = began.elapsed().saturating_sub(inside) / actors.len() as u32;
        for &s in actors {
            *self.checks.entry((s, kind)).or_default() += checks;
        }
    }

    pub(crate) fn day(&mut self, took: Duration) {
        self.total += took;
        self.days += 1;
    }
}
