//! Words for the inspector: field values, events, change requests, the event log and traces, in
//! plain language.

use sim::calendar::Calendar;
use sim::events::{Event, Posted};
use sim::fields::{FieldData, FieldId};
use sim::module::Slot;
use sim::requests::{Cause, Request};
use sim::sim::{Stage, TraceStep, Traced};

/// 1234567 -> "1,234,567".
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A field's name for people: "province.food_stock" -> "food stock".
pub fn field_name(id: FieldId) -> String {
    let name = id.def().name;
    name.split_once('.').map_or(name, |(_, n)| n).replace('_', " ")
}

/// One value of a field, with its unit.
pub fn value(id: FieldId, data: &FieldData, i: usize) -> String {
    let def = id.def();
    match (def.name, data) {
        ("province.center", FieldData::Dir(v)) => {
            let (lon, lat) = sim::grid::dir_to_lonlat(v[i]);
            lat_lon(lat.to_degrees(), lon.to_degrees())
        }
        ("province.lat", _) => format!("{:.1}°", data.value(i).to_degrees()),
        ("province.rain_shortfall", _) => {
            let v = data.value(i);
            if v == 0.0 { "none: a normal year".into() } else { format!("{:.0}% of the rain missing", v / 10.0) }
        }
        (_, FieldData::U64(_) | FieldData::U32(_) | FieldData::U16(_) | FieldData::U8(_)) => format!("{} {}", group(data.value(i) as u64), def.unit),
        _ => format!("{:.1} {}", data.value(i), def.unit),
    }
}

pub fn lat_lon(lat: f64, lon: f64) -> String {
    format!("{:.1}°{} {:.1}°{}", lat.abs(), if lat >= 0.0 { "N" } else { "S" }, lon.abs(), if lon >= 0.0 { "E" } else { "W" })
}

pub fn slot_name(s: Slot) -> &'static str {
    match s {
        Slot::Terrain => "Terrain",
        Slot::Climate => "Climate",
        Slot::Biomes => "Biomes",
        Slot::Rivers => "Rivers",
        Slot::Fertility => "Farmland",
        Slot::Provinces => "Provinces",
        Slot::Economics => "Economics",
        Slot::Population => "Population",
        Slot::Weather => "Weather",
    }
}

fn cause(c: Cause) -> &'static str {
    match c {
        Cause::Harvest => "harvest",
        Cause::Spoilage => "spoilage",
        Cause::Eating => "eating",
        Cause::Births => "births",
        Cause::Deaths => "deaths",
        Cause::Starvation => "starvation",
        Cause::ContractClamp => "a value pulled back into bounds",
    }
}

/// "Population asked for 12,000 rations from the food stock, for eating".
pub fn request(r: &Request) -> String {
    format!("{} asked for {} from the {}, for {}", slot_name(r.requester), group(r.take), field_name(r.field), cause(r.cause))
}

pub fn grant(r: &Request, granted: u64) -> String {
    let share = if r.take == 0 { 100.0 } else { 100.0 * granted as f64 / r.take as f64 };
    format!("{} was given {} of {} asked from the {} ({share:.0}%), for {}", slot_name(r.requester), group(granted), group(r.take), field_name(r.field), cause(r.cause))
}

pub fn event(e: &Event, cal: &Calendar) -> String {
    match e {
        Event::Granted { request, granted } => grant(request, *granted),
        Event::Famine { starved, .. } => format!("famine: {} people starved", group(*starved)),
        Event::ContractViolated { field, entity, value, clamped_to, .. } => {
            format!("{} of entity {entity} was {value}, out of bounds; set to {clamped_to}", field_name(*field))
        }
        Event::ConservationBroken { quantity, expected, actual, by } => {
            format!("{quantity:?} total is {} but should be {} (during {:?})", group(*actual), group(*expected), by)
        }
        Event::Drought { lon, lat, radius_km, shortfall, days } => format!(
            "a drought begins around {}, {:.0} km across: {:.0}% of the rain missing for {}",
            lat_lon(*lat, *lon),
            2.0 * radius_km,
            *shortfall as f64 / 10.0,
            duration(*days, cal)
        ),
    }
}

/// "3 years", "1 year and 40 days", "12 days".
pub fn duration(days: u64, cal: &Calendar) -> String {
    let dpy = cal.days_per_year() as u64;
    let (y, d) = (days / dpy, days % dpy);
    let plural = |n: u64, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    match (y, d) {
        (0, d) => plural(d, "day"),
        (y, 0) => plural(y, "year"),
        (y, d) => format!("{} and {}", plural(y, "year"), plural(d, "day")),
    }
}

/// Who an event log entry comes from, for filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Module(Slot),
    /// The player's pokes.
    Player,
    /// The engine's own checks.
    Engine,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Module(s) => slot_name(s),
            Source::Player => "your pokes",
            Source::Engine => "engine checks",
        }
    }
}

/// One line of the daily event log.
#[derive(Clone, Debug)]
pub struct LogEntry {
    /// The day it happened (events: the day they were delivered).
    pub day: u64,
    pub source: Source,
    /// The province it is about; `None` for the whole world.
    pub province: Option<u32>,
    pub what: Logged,
}

#[derive(Clone, Debug)]
pub enum Logged {
    Event(Event),
    /// An owner had less than was asked, and shared it out.
    Rationed(Request, u64),
}

impl LogEntry {
    pub fn of_event(p: &Posted) -> LogEntry {
        let source = match (p.emitter, &p.event) {
            (Some(s), _) => Source::Module(s),
            (None, Event::Drought { .. }) => Source::Player,
            (None, _) => Source::Engine,
        };
        let province = p.event.province().or(match &p.event {
            Event::ContractViolated { field, entity, .. } if field.def().tier == sim::fields::Tier::Province => Some(*entity),
            _ => None,
        });
        LogEntry { day: p.day + 1, source, province, what: Logged::Event(p.event.clone()) }
    }

    /// A change request applied on `day` that got less than it asked for.
    pub fn rationed(day: u64, owner: Slot, r: &Request, granted: u64) -> LogEntry {
        let province = (r.field.def().tier == sim::fields::Tier::Province).then_some(r.entity);
        LogEntry { day, source: Source::Module(owner), province, what: Logged::Rationed(*r, granted) }
    }

    pub fn text(&self, cal: &Calendar) -> String {
        match &self.what {
            Logged::Event(e) => event(e, cal),
            Logged::Rationed(r, g) => format!("not enough food: {}", grant(r, *g)),
        }
    }
}

pub fn stage(s: Stage) -> &'static str {
    match s {
        Stage::Generate => "world creation",
        Stage::Init => "starting values",
        Stage::Deliver => "yesterday's events and answers arrive",
        Stage::Tick => "modules do their work",
        Stage::Apply => "change requests are settled",
    }
}

/// One step of a trace, in words.
pub fn trace_step(s: &TraceStep, cal: &Calendar) -> String {
    let who = s.module.map_or("the engine", slot_name);
    match &s.what {
        Traced::Note(n) => format!("{who}: {n}"),
        Traced::Changed { field, entity, before, after } => {
            let name = field_name(*field);
            let fmt = |v: f64| if v.fract() == 0.0 && v.abs() < 1e18 { group(v.abs() as u64) } else { format!("{v:.2}") };
            let of = if field.def().tier == sim::fields::Tier::Pop { format!(" of pop {entity}") } else { String::new() };
            let delta = after - before;
            let sign = if delta >= 0.0 { "+" } else { "-" };
            format!("{name}{of}: {} to {} ({sign}{}), set by {who}", fmt(*before), fmt(*after), fmt(delta.abs()))
        }
        Traced::Requested(r) => format!("{}; settled tonight", request(r)),
        Traced::Granted(r, g) => format!("{who} settled it: {}", grant(r, *g)),
        Traced::Delivered(e) => format!("arrived: {}", event(e, cal)),
        Traced::Sent(e) => format!("{who} announced: {} (arrives tomorrow)", event(e, cal)),
    }
}
