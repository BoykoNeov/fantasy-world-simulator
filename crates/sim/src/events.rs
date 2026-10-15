//! The event bus: typed events with a day stamp. Publishers don't know who listens.
//!
//! An event emitted on day N is delivered on day N+1, before any module's main logic runs that day.
//! Delivery order is fixed: the event type's priority (the table in [`EventKind::priority`]), then
//! the emitter's place in the run order (the engine comes last), then the entity, then emission
//! order. Handlers must re-check their preconditions: the world may have changed since the event
//! was sent.

use crate::fields::{FieldId, Quantity};
use crate::module::Slot;
use crate::requests::Request;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// An owner's answer to a change request, delivered to the requester only.
    Granted { request: Request, granted: u64 },
    /// People starved in a province this month.
    Famine { province: u32, starved: u64 },
    /// A value left its field's bounds and was clamped. `by` is the module whose stage wrote it.
    ContractViolated { field: FieldId, entity: u32, value: f64, clamped_to: f64, by: Option<Slot> },
    /// A conserved total changed by an amount no flow or request explains.
    ConservationBroken { quantity: Quantity, expected: u64, actual: u64, by: Vec<Slot> },
    /// A drought begins: for `days`, rain falls short by `shortfall` per mille within `radius_km`
    /// of (`lon`, `lat`) (degrees), fading out towards the edge. Usually a player's poke.
    Drought { lon: f64, lat: f64, radius_km: f64, shortfall: u64, days: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EventKind {
    Granted,
    Famine,
    ContractViolated,
    ConservationBroken,
    Drought,
}

impl EventKind {
    /// Same-day conflicts are settled by this table, not by each module: destruction and death
    /// first, then agreements, then routine changes, then the engine's own diagnostics.
    ///
    /// | class | value | kinds |
    /// | --- | --- | --- |
    /// | destruction and death | 0 | Famine, Drought |
    /// | agreements | 1 | (none yet) |
    /// | routine changes | 2 | Granted |
    /// | diagnostics | 3 | ContractViolated, ConservationBroken |
    pub fn priority(self) -> u8 {
        match self {
            EventKind::Famine | EventKind::Drought => 0,
            EventKind::Granted => 2,
            EventKind::ContractViolated | EventKind::ConservationBroken => 3,
        }
    }
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Event::Granted { .. } => EventKind::Granted,
            Event::Famine { .. } => EventKind::Famine,
            Event::ContractViolated { .. } => EventKind::ContractViolated,
            Event::ConservationBroken { .. } => EventKind::ConservationBroken,
            Event::Drought { .. } => EventKind::Drought,
        }
    }

    /// The entity used to order same-priority events.
    pub fn entity(&self) -> u64 {
        match self {
            Event::Granted { request, .. } => request.entity as u64,
            Event::Famine { province, .. } => *province as u64,
            Event::ContractViolated { entity, .. } => *entity as u64,
            Event::ConservationBroken { .. } | Event::Drought { .. } => 0,
        }
    }

    /// The province this event is about, if it is about one.
    pub fn province(&self) -> Option<u32> {
        match self {
            Event::Famine { province, .. } => Some(*province),
            _ => None,
        }
    }
}

/// An event waiting for delivery, or in the log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Posted {
    /// Day it was emitted; it is delivered the day after.
    pub day: u64,
    /// `None` for the engine.
    pub emitter: Option<Slot>,
    /// Emission order within the day.
    pub seq: u32,
    pub event: Event,
}

/// Sort a day's events into delivery order. `rank` gives each emitting slot's place in the run
/// order.
pub fn delivery_order(events: &mut [Posted], rank: impl Fn(Slot) -> usize) {
    events.sort_by_key(|p| (p.event.kind().priority(), p.emitter.map_or(usize::MAX, &rank), p.event.entity(), p.seq));
}

/// Delivered events, kept for the chronicle and the inspector's history. It is not part of a
/// snapshot: replay doesn't need it, and old entries are pruned. Grants are not kept here (the
/// day's applied requests are).
#[derive(Clone, Debug)]
pub struct EventLog {
    pub entries: Vec<Posted>,
    /// Entries older than this many days are pruned at each year start.
    pub keep_days: u64,
}

impl Default for EventLog {
    fn default() -> Self {
        EventLog { entries: Vec::new(), keep_days: 3 * 364 }
    }
}

impl EventLog {
    pub fn prune(&mut self, today: u64) {
        let oldest = today.saturating_sub(self.keep_days);
        self.entries.retain(|p| p.day >= oldest);
    }

    pub fn count(&self, kind: EventKind) -> usize {
        self.entries.iter().filter(|p| p.event.kind() == kind).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::{Cause, Priority};

    fn posted(emitter: Option<Slot>, seq: u32, event: Event) -> Posted {
        Posted { day: 4, emitter, seq, event }
    }

    #[test]
    fn delivery_goes_by_priority_then_emitter_then_entity_then_order() {
        let grant = |entity| Event::Granted {
            request: Request {
                day: 4,
                requester: Slot::Population,
                field: crate::fields::known("province.stock"),
                entity,
                take: 1,
                priority: Priority::Essential,
                cause: Cause::Eating,
                tag: 0,
            },
            granted: 1,
        };
        let mut v = vec![
            posted(None, 0, Event::ConservationBroken { quantity: Quantity::Goods(0), expected: 1, actual: 2, by: vec![] }),
            posted(Some(Slot::Economics), 1, grant(9)),
            posted(Some(Slot::Economics), 2, grant(3)),
            posted(Some(Slot::Population), 3, Event::Famine { province: 5, starved: 1 }),
            posted(Some(Slot::Economics), 4, Event::Famine { province: 8, starved: 1 }),
            posted(Some(Slot::Population), 5, Event::Famine { province: 5, starved: 2 }),
        ];
        // Run order: Population first, then Economics.
        delivery_order(&mut v, |s| if s == Slot::Population { 0 } else { 1 });
        let seqs: Vec<u32> = v.iter().map(|p| p.seq).collect();
        assert_eq!(seqs, vec![3, 5, 4, 2, 1, 0]);
    }
}
