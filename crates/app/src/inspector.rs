//! Words for the inspector: field values, events, change requests, the event log and traces, in
//! plain language.

use sim::calendar::Calendar;
use sim::events::{Event, Posted};
use sim::fields::{FieldData, FieldId, Goods};
use sim::module::Slot;
use sim::requests::{Cause, Request};
use sim::fields::Tier;
use sim::sim::{Stage, TraceStep, Traced, NEVER};

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

/// The last day any of `fields` changed for `province`, one of its goods or one of its pops
/// (`pop_province` gives each pop's province), from the engine's last-changed stamps.
pub fn latest_change(last_change: &[Vec<u64>], fields: impl Iterator<Item = FieldId>, pop_province: &[u32], goods: &Goods, province: u32) -> Option<u64> {
    fields
        .flat_map(|id| {
            let stamps = &last_change[id.0 as usize];
            let mine: Vec<usize> = match id.def().tier {
                Tier::Province => vec![province as usize],
                Tier::ProvinceGood => (0..goods.stride()).map(|c| goods.index(province as usize, c)).collect(),
                Tier::Pop => (0..pop_province.len()).filter(|&k| pop_province[k] == province).collect(),
                Tier::Cell => Vec::new(),
            };
            mine.into_iter().filter_map(move |i| stamps.get(i).copied())
        })
        .filter(|&d| d != NEVER)
        .max()
}

/// A field's name for people: "province.rain_shortfall" -> "rain shortfall".
pub fn field_name(id: FieldId) -> String {
    let name = id.def().name;
    name.split_once('.').map_or(name, |(_, n)| n).replace('_', " ")
}

/// The name of one value of a field: "food stock" for a per-good field's food column, the field's
/// name otherwise.
pub fn value_name(id: FieldId, entity: u32, goods: &Goods) -> String {
    match id.def().tier {
        Tier::ProvinceGood => format!("{} {}", good_name(goods, goods.split(entity as usize).1), field_name(id)),
        _ => field_name(id),
    }
}

/// The name of column `c` of the per-good fields.
pub fn good_name(goods: &Goods, c: usize) -> &str {
    goods.tree.name(goods.columns[c])
}

/// What a request asks for: "food stock" for food asked of the per-good stock, the field's name
/// otherwise.
pub fn asked_name(r: &Request, goods: &Goods) -> String {
    match r.good {
        Some(node) => format!("{} {}", goods.tree.name(node), field_name(r.field)),
        None => field_name(r.field),
    }
}

/// The province a request is about, for requests on province and per-good fields.
pub fn request_province(r: &Request) -> Option<u32> {
    matches!(r.field.def().tier, Tier::Province | Tier::ProvinceGood).then_some(r.entity)
}

/// The province one value of a field belongs to, for province and per-good fields.
pub fn province_of(id: FieldId, entity: u32, goods: &Goods) -> Option<u32> {
    match id.def().tier {
        Tier::Province => Some(entity),
        Tier::ProvinceGood => Some(goods.split(entity as usize).0 as u32),
        Tier::Cell | Tier::Pop => None,
    }
}

/// One value of a per-good field, in its good's unit: "1,234 rations".
pub fn good_value(goods: &Goods, c: usize, amount: u64) -> String {
    match goods.tree.unit(goods.columns[c]) {
        Some(unit) => format!("{} {unit}", group(amount)),
        None => group(amount),
    }
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
        Cause::Edit => "your edit",
    }
}

/// "Population asked for 12,000 from the food stock, for eating".
pub fn request(r: &Request, goods: &Goods) -> String {
    format!("{} asked for {} from the {}, for {}", slot_name(r.requester), group(r.take), asked_name(r, goods), cause(r.cause))
}

pub fn grant(r: &Request, granted: u64, goods: &Goods) -> String {
    let share = if r.take == 0 { 100.0 } else { 100.0 * granted as f64 / r.take as f64 };
    let from = asked_name(r, goods);
    format!("{} was given {} of {} asked from the {from} ({share:.0}%), for {}", slot_name(r.requester), group(granted), group(r.take), cause(r.cause))
}

pub fn event(e: &Event, cal: &Calendar, goods: &Goods) -> String {
    match e {
        Event::Granted { request, granted } => grant(request, *granted, goods),
        Event::Famine { starved, .. } => format!("famine: {} people starved", group(*starved)),
        Event::ContractViolated { field, entity, value, clamped_to, .. } => {
            format!("{} of entity {entity} was {value}, out of bounds; set to {clamped_to}", value_name(*field, *entity, goods))
        }
        Event::ConservationBroken { quantity, expected, actual, by } => {
            format!("{} total is {} but should be {} (during {:?})", quantity.name(goods), group(*actual), group(*expected), by)
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
    pub fn of_event(p: &Posted, goods: &Goods) -> LogEntry {
        let source = match (p.emitter, &p.event) {
            (Some(s), _) => Source::Module(s),
            (None, Event::Drought { .. }) => Source::Player,
            (None, _) => Source::Engine,
        };
        let province = p.event.province().or(match &p.event {
            Event::ContractViolated { field, entity, .. } => province_of(*field, *entity, goods),
            _ => None,
        });
        LogEntry { day: p.day + 1, source, province, what: Logged::Event(p.event.clone()) }
    }

    /// A change request applied on `day` that got less than it asked for.
    pub fn rationed(day: u64, owner: Slot, r: &Request, granted: u64) -> LogEntry {
        let province = request_province(r);
        LogEntry { day, source: Source::Module(owner), province, what: Logged::Rationed(*r, granted) }
    }

    pub fn text(&self, cal: &Calendar, goods: &Goods) -> String {
        match &self.what {
            Logged::Event(e) => event(e, cal, goods),
            Logged::Rationed(r, g) => format!("not enough to go round: {}", grant(r, *g, goods)),
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
        Stage::Edit => "your edits",
    }
}

/// One step of a trace, in words.
pub fn trace_step(s: &TraceStep, cal: &Calendar, goods: &Goods) -> String {
    let who = s.module.map_or("the engine", slot_name);
    match &s.what {
        Traced::Note(n) => format!("{who}: {n}"),
        Traced::Changed { field, entity, before, after } => {
            let name = value_name(*field, *entity, goods);
            let fmt = |v: f64| if v.fract() == 0.0 && v.abs() < 1e18 { group(v.abs() as u64) } else { format!("{v:.2}") };
            let of = if field.def().tier == sim::fields::Tier::Pop { format!(" of pop {entity}") } else { String::new() };
            let delta = after - before;
            let sign = if delta >= 0.0 { "+" } else { "-" };
            format!("{name}{of}: {} to {} ({sign}{}), set by {who}", fmt(*before), fmt(*after), fmt(delta.abs()))
        }
        Traced::Requested(r) => format!("{}; settled tonight", request(r, goods)),
        Traced::Granted(r, g) => format!("{who} settled it: {}", grant(r, *g, goods)),
        Traced::Delivered(e) => format!("arrived: {}", event(e, cal, goods)),
        Traced::Sent(e) => format!("{who} announced: {} (arrives tomorrow)", event(e, cal, goods)),
    }
}
