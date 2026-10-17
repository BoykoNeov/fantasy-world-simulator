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
    /// People come to live in a province from outside the pops: soldiers home from an army. Their
    /// sender has already booked them out (with `cause`); the pops' owner books them in on delivery.
    Arrived { province: u32, people: u64, cause: crate::requests::Cause },
    /// Character `ruler` takes realm `realm`'s throne (its capital then was `capital`): after a
    /// ruler's death, or as a new realm's first ruler. Not sent for the rulers a world starts with.
    Succession { realm: u32, capital: u32, ruler: u32, how: crate::characters::Succession },
    /// The player's poke: realm `attacker` declares war on realm `defender`. It breaks a truce.
    DeclareWar { attacker: u32, defender: u32 },
    /// The player's poke: the war between realms `a` and `b` (either way round) ends, won by
    /// `winner`, one of the two, or by nobody ([`crate::grid::NONE`]): then everything goes back.
    ForcePeace { a: u32, b: u32, winner: u32 },
    /// War `war` has begun: `attacker` declared it on `defender`.
    WarDeclared { war: u32, attacker: u32, defender: u32, cause: crate::diplomacy::WarCause },
    /// War `war` has ended. `winner` (or nobody, [`crate::grid::NONE`]) keeps every province of the
    /// other's that it occupies; every other occupation between the two ends. The owner of
    /// provinces hands them over on delivery. `rebellion` when it was a rebellion's war: then the
    /// realm the rebels broke from, winning, takes back all their land.
    Peace { war: u32, attacker: u32, defender: u32, winner: u32, rebellion: bool },
    /// Realm `by` has taken `province` and occupies it, or, when `by` is its owner, freed it. The
    /// sender (warfare) sends it only for enemies at war; the owner of provinces applies it.
    Occupied { province: u32, by: u32 },
    /// A poke about war or peace between `attacker` and `defender` was turned down, and why.
    WarRefused { attacker: u32, defender: u32, why: crate::diplomacy::Refusal },
    /// `province`, of realm `realm`, has risen up: in an uprising of its own (`seat` is
    /// `province`), or joining the one that began in `seat`. It pays its realm nothing until the
    /// uprising dies down.
    Uprising { province: u32, realm: u32, seat: u32 },
    /// The uprising in `province`, of realm `realm`, has died down.
    Calmed { province: u32, realm: u32 },
    /// Provinces in uprising have broken away from realm `from` as the new realm `realm`, with
    /// `capital` its capital and `provinces` provinces. It is at war with `from` from the next day.
    Rebellion { realm: u32, from: u32, capital: u32, provinces: u32 },
    /// A battle in `province` is over: `winner` held the field (or nobody, [`crate::grid::NONE`],
    /// when a peace ended it), `loser` broke first (or nobody), and `killed` soldiers died in it:
    /// `sides` gives each realm that fought and the soldiers it lost, which sum to `killed`. The
    /// owner of provinces adds to the unrest where it was fought.
    Battle { province: u32, winner: u32, loser: u32, killed: u64, sides: Vec<(u32, u64)> },
    /// A month's trade across the border between realms `a` and `b` (`a` the lower number): what
    /// the goods each way were worth, in coins (the price and the carrying; tolls not included).
    /// Sent once a month for each pair of realms that traded; not kept in the event log.
    Trade { a: u32, b: u32, a_to_b: u64, b_to_a: u64 },
    /// The player's poke: character `character`, a ruler or an heir, dies. A ruler's heir, or a
    /// claimant, takes the throne at once, as when a ruler dies of age.
    KillRuler { character: u32 },
    /// The player's poke: `provinces`, all of one realm and not its capital, break away as a new
    /// realm at war with it, as rebels do (the realm then sends [`Event::Rebellion`]).
    Breakaway { provinces: Vec<u32> },
    /// The player's poke: realm `realm`'s field companies standing in `from` march to `to`. Under
    /// orders, the general leaves them alone until they arrive.
    March { realm: u32, from: u32, to: u32 },
    /// A poke of the player's no longer fits the world, and why (the wars' own pokes are refused
    /// with [`Event::WarRefused`]).
    Refused { why: String },
    // The first wave of modules built side by side (Oct 17), a block each.
    // Research.
    /// Realm `realm` (its capital then `capital`) has learned tech `tech` (its column).
    Discovery { realm: u32, tech: u16, capital: u32 },
    // Culture.
    /// A new culture `culture` has split off from `parent` (or began, with no parent:
    /// [`crate::grid::NONE`]) in `provinces` provinces around `hearth`.
    CultureBorn { culture: u32, parent: u32, hearth: u32, provinces: u32 },
    /// The last person of culture `culture` is gone.
    CultureGone { culture: u32 },
    /// Province `province`'s main culture changed from `from` to `to`.
    Assimilated { province: u32, from: u32, to: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EventKind {
    Granted,
    Famine,
    ContractViolated,
    ConservationBroken,
    Drought,
    Arrived,
    Succession,
    DeclareWar,
    ForcePeace,
    WarDeclared,
    Peace,
    Occupied,
    WarRefused,
    Uprising,
    Calmed,
    Rebellion,
    Battle,
    Trade,
    KillRuler,
    Breakaway,
    March,
    Refused,
    Discovery,
    CultureBorn,
    CultureGone,
    Assimilated,
}

impl EventKind {
    /// Same-day conflicts are settled by this table, not by each module: destruction and death
    /// first, then agreements, then routine changes, then the engine's own diagnostics.
    ///
    /// | class | value | kinds |
    /// | --- | --- | --- |
    /// | destruction and death | 0 | Famine, Drought, Succession, Occupied, Battle, KillRuler |
    /// | agreements | 1 | DeclareWar, ForcePeace, WarDeclared, Peace, WarRefused, Rebellion, Breakaway |
    /// | routine changes | 2 | Granted, Arrived, Uprising, Calmed, Trade, March, Refused, Discovery, CultureBorn, CultureGone, Assimilated |
    /// | diagnostics | 3 | ContractViolated, ConservationBroken |
    pub fn priority(self) -> u8 {
        match self {
            // A province taken the day before a peace is handed over (or given back) by it.
            EventKind::Famine | EventKind::Drought | EventKind::Succession | EventKind::Occupied | EventKind::Battle | EventKind::KillRuler => 0,
            EventKind::DeclareWar | EventKind::ForcePeace | EventKind::WarDeclared | EventKind::Peace | EventKind::WarRefused | EventKind::Rebellion | EventKind::Breakaway => 1,
            EventKind::Granted | EventKind::Arrived | EventKind::Uprising | EventKind::Calmed | EventKind::Trade | EventKind::March | EventKind::Refused => 2,
            EventKind::Discovery | EventKind::CultureBorn | EventKind::CultureGone | EventKind::Assimilated => 2,
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
            Event::Arrived { .. } => EventKind::Arrived,
            Event::Succession { .. } => EventKind::Succession,
            Event::DeclareWar { .. } => EventKind::DeclareWar,
            Event::ForcePeace { .. } => EventKind::ForcePeace,
            Event::WarDeclared { .. } => EventKind::WarDeclared,
            Event::Peace { .. } => EventKind::Peace,
            Event::Occupied { .. } => EventKind::Occupied,
            Event::WarRefused { .. } => EventKind::WarRefused,
            Event::Uprising { .. } => EventKind::Uprising,
            Event::Calmed { .. } => EventKind::Calmed,
            Event::Rebellion { .. } => EventKind::Rebellion,
            Event::Battle { .. } => EventKind::Battle,
            Event::Trade { .. } => EventKind::Trade,
            Event::KillRuler { .. } => EventKind::KillRuler,
            Event::Breakaway { .. } => EventKind::Breakaway,
            Event::March { .. } => EventKind::March,
            Event::Refused { .. } => EventKind::Refused,
            Event::Discovery { .. } => EventKind::Discovery,
            Event::CultureBorn { .. } => EventKind::CultureBorn,
            Event::CultureGone { .. } => EventKind::CultureGone,
            Event::Assimilated { .. } => EventKind::Assimilated,
        }
    }

    /// The entity used to order same-priority events.
    pub fn entity(&self) -> u64 {
        match self {
            Event::Granted { request, .. } => request.entity as u64,
            Event::Famine { province, .. } | Event::Arrived { province, .. } => *province as u64,
            Event::ContractViolated { entity, .. } => *entity as u64,
            Event::ConservationBroken { .. } | Event::Drought { .. } => 0,
            Event::Succession { realm, .. } => *realm as u64,
            // Pokes about one pair go by the pair, so the player's order between them holds.
            Event::DeclareWar { attacker: a, defender: b }
            | Event::ForcePeace { a, b, .. }
            | Event::WarRefused { attacker: a, defender: b, .. }
            | Event::Trade { a, b, .. } => {
                pair(*a, *b)
            }
            Event::WarDeclared { war, .. } | Event::Peace { war, .. } => *war as u64,
            Event::Occupied { province, .. } | Event::Uprising { province, .. } | Event::Calmed { province, .. } | Event::Battle { province, .. } => *province as u64,
            Event::Rebellion { realm, .. } | Event::March { realm, .. } => *realm as u64,
            Event::KillRuler { character } => *character as u64,
            Event::Breakaway { provinces } => provinces.first().copied().unwrap_or(0) as u64,
            Event::Refused { .. } => 0,
            Event::Discovery { realm, .. } => *realm as u64,
            Event::CultureBorn { culture, .. } | Event::CultureGone { culture } => *culture as u64,
            Event::Assimilated { province, .. } => *province as u64,
        }
    }

    /// The province this event is about, if it is about one.
    pub fn province(&self) -> Option<u32> {
        match self {
            Event::Famine { province, .. }
            | Event::Arrived { province, .. }
            | Event::Occupied { province, .. }
            | Event::Battle { province, .. }
            | Event::Uprising { province, .. }
            | Event::Calmed { province, .. }
            | Event::March { from: province, .. } => Some(*province),
            Event::Breakaway { provinces } => provinces.first().copied(),
            Event::Succession { capital, .. } | Event::Rebellion { capital, .. } | Event::Discovery { capital, .. } => Some(*capital),
            Event::CultureBorn { hearth, .. } => Some(*hearth),
            Event::Assimilated { province, .. } => Some(*province),
            _ => None,
        }
    }
}

/// Two realms as one number, the same whichever comes first.
fn pair(a: u32, b: u32) -> u64 {
    ((a.min(b) as u64) << 32) | a.max(b) as u64
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
    // Each key once per event, not once per comparison: working out a rank walks the modules.
    events.sort_by_cached_key(|p| (p.event.kind().priority(), p.emitter.map_or(usize::MAX, &rank), p.event.entity(), p.seq));
}

/// Delivered events, kept for the chronicle and the inspector's history. It is not part of a
/// snapshot: replay doesn't need it, and old entries are pruned. Grants are not kept here (the
/// day's applied requests are), nor the monthly trade between realms.
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
                good: Some(1),
                take: 1,
                priority: Priority::Essential,
                cause: Cause::Eating,
                tag: 0,
                to: None,
                shift: None,
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
