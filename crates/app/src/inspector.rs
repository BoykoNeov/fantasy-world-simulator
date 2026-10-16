//! Words for the inspector: field values, events, change requests, the event log and traces, in
//! plain language.

use sim::calendar::Calendar;
use sim::characters::{Succession, ADULT};
use sim::diplomacy::WarCause;
use sim::events::{Event, Posted};
use sim::fields::{FieldData, FieldId, Goods};
use sim::grid::NONE;
use sim::module::Slot;
use sim::requests::{Cause, Request};
use sim::fields::Tier;
use sim::sim::{Stage, TraceStep, Traced, NEVER};
use sim::world::World;

/// Realm names by number, worked out once per world and extended as realms are made
/// ([`sim::realms::names`] gives each realm the same name whatever comes after it).
#[derive(Clone, Debug, Default)]
pub struct RealmNames {
    seed: u64,
    pub names: Vec<String>,
}

impl RealmNames {
    /// The names of `w`'s realms, worked out again only for another seed or more realms.
    pub fn update(&mut self, w: &World) {
        let count = w.realms.count();
        if self.seed != w.params.seed || self.names.len() < count {
            *self = RealmNames { seed: w.params.seed, names: sim::realms::names(w.params.seed, count) };
        }
    }
}

/// Names for realms and people in the inspector's words.
#[derive(Clone, Copy)]
pub struct Names<'a> {
    pub world: &'a World,
    /// Realm names by number, as many as are known ([`RealmNames`]).
    pub realms: &'a [String],
}

impl Names<'_> {
    /// A realm's name; "nobody" for none.
    pub fn realm(&self, r: u32) -> String {
        match r {
            NONE => "nobody".into(),
            r => self.realms.get(r as usize).cloned().unwrap_or_else(|| format!("realm {r}")),
        }
    }

    /// Whether character `c` has every value the words below read.
    fn known(&self, c: u32) -> bool {
        let ch = &self.world.characters;
        let c = c as usize;
        let lengths = [ch.realm.len(), ch.name.len(), ch.born.len(), ch.crowned.len(), ch.succession.len(), ch.martial.len(), ch.greed.len(), ch.ability.len(), ch.parent.len()];
        c < ch.count() && lengths.iter().all(|&n| c < n)
    }

    /// A person's name, with a number for a ruler who shares an earlier ruler's ("Aldric II").
    pub fn person(&self, c: u32) -> String {
        if self.known(c) { sim::characters::title(self.world, c) } else { format!("character {c}") }
    }
}

/// One person on a realm's page.
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub name: String,
    /// What there is to say about them, a line each.
    pub lines: Vec<String>,
}

/// A realm's page: who it is, who rules it, how it stands with the others.
#[derive(Clone, Debug, PartialEq)]
pub struct RealmPage {
    pub name: String,
    /// Dissolved realms keep their entry, with no land and no capital.
    pub alive: bool,
    pub capital: Option<u32>,
    /// Facts about it, a line each.
    pub facts: Vec<String>,
    pub ruler: Option<Person>,
    pub heir: Option<Person>,
    /// Earlier rulers, latest first.
    pub past_rulers: Vec<String>,
    /// Wars going on, then truces: the other realm, and how they stand.
    pub relations: Vec<(u32, String)>,
}

/// Realm `r`'s page on `day`. `coin`: the column of the per-good fields that holds coins, if the
/// world keeps them. Every value is read with a check, since a realm or person made after the
/// window opened may lack values no running module keeps.
pub fn realm_page(n: &Names, r: u32, day: u64, cal: &Calendar, coin: Option<usize>) -> RealmPage {
    let w = n.world;
    let dpy = cal.days_per_year() as f64;
    let capital = w.realms.capital.get(r as usize).copied().filter(|&c| c != NONE);
    let mine = |v: &[u32], p: usize| v.get(p).copied() == Some(r);
    let provinces: Vec<usize> = (0..w.provinces.count).filter(|&p| mine(&w.owner, p)).collect();
    let people = w.province_population();
    let plural = |k: usize| if k == 1 { "" } else { "s" };
    let mut facts = vec![format!(
        "{} province{}, {} people",
        provinces.len(),
        plural(provinces.len()),
        group(provinces.iter().map(|&p| people.get(p).copied().unwrap_or(0)).sum())
    )];
    if let Some(c) = coin
        && let Some(&coins) = w.realm_stock.get(r as usize * w.goods.stride() + c)
    {
        facts.push(format!("treasury: {} coins", group(coins)));
    }
    if let Some(&from) = w.realms.broke_from.get(r as usize).filter(|&&f| f != NONE) {
        facts.push(format!("broke away from {}", n.realm(from)));
    }
    let rising = provinces.iter().filter(|&&p| w.uprising.get(p).is_some_and(|&u| u != NONE)).count();
    if rising > 0 {
        facts.push(format!("{rising} of its provinces in uprising"));
    }
    let lost = provinces.iter().filter(|&&p| w.occupier.get(p).is_some_and(|&o| o != NONE)).count();
    if lost > 0 {
        facts.push(format!("{lost} of its provinces held by enemies"));
    }
    let taken = (0..w.provinces.count).filter(|&p| mine(&w.occupier, p)).count();
    if taken > 0 {
        facts.push(format!("holds {taken} province{} of its enemies", plural(taken)));
    }

    let ch = &w.characters;
    let years = |from: f64, to: f64| ((to - from) / dpy).floor().max(0.0) as u64;
    let ruler_now = w.realms.ruler.get(r as usize).copied().unwrap_or(NONE);
    let person = |c: u32, ruler: bool| -> Option<Person> {
        if c == NONE || !n.known(c) {
            return None;
        }
        let k = c as usize;
        let age = years(ch.born[k], ch.died[k].min(day) as f64);
        let child = !ruler && (age as f64) < ADULT;
        let mut lines = vec![format!("{age} years old{}", if child { ", a child" } else { "" })];
        let traits = sim::characters::traits_in_words(w, c);
        lines.push(if traits.is_empty() { "no marked traits".to_string() } else { traits.join(", ") });
        if ruler && ch.crowned[k] != NEVER {
            let how = Succession::from_u8(ch.succession[k]).describe();
            lines.push(format!("ruler since {} ({} years): {how}", cal.format(ch.crowned[k]), years(ch.crowned[k] as f64, day as f64)));
        }
        if !ruler && ch.parent[k] != NONE && ch.parent[k] == ruler_now {
            lines.push("the ruler's child".into());
        }
        Some(Person { name: n.person(c), lines })
    };
    let ruler = person(ruler_now, true);
    let heir = person(w.realms.heir.get(r as usize).copied().unwrap_or(NONE), false);
    let mut past: Vec<usize> =
        (0..ch.count()).filter(|&k| n.known(k as u32) && ch.realm[k] == r && ch.crowned[k] != NEVER && ch.died[k] != NEVER && ch.died[k] <= day).collect();
    past.sort_by_key(|&k| std::cmp::Reverse((ch.crowned[k], k)));
    let past_rulers = past
        .iter()
        .map(|&k| format!("{}: ruled {} to {}; {}", n.person(k as u32), cal.format(ch.crowned[k]), cal.format(ch.died[k]), Succession::from_u8(ch.succession[k]).describe()))
        .collect();

    // Wars going on, then truces, latest first.
    let wars = &w.wars;
    let day_of = |v: &[u64], k: usize| v.get(k).copied().unwrap_or(NEVER);
    let mut fighting = Vec::new();
    let mut truces = Vec::new();
    let mut truced = std::collections::HashSet::new();
    for k in (0..wars.count()).rev() {
        let (Some(&a), Some(&b)) = (wars.attacker.get(k), wars.defender.get(k)) else { continue };
        let other = if a == r {
            b
        } else if b == r {
            a
        } else {
            continue;
        };
        let began = cal.format(day_of(&wars.began, k));
        let rebellion = wars.cause.get(k).is_some_and(|&c| WarCause::from_u8(c) == WarCause::Rebellion);
        if day_of(&wars.ended, k) == NEVER {
            // A war with a dissolved realm is over, though its entry ends only at the next month.
            if !w.realms.alive(a) || !w.realms.alive(b) {
                continue;
            }
            // In a rebellion's war the rebels are the attacker.
            let text = match (rebellion, a == r) {
                (true, true) => format!("fighting for its freedom from {} since {began}", n.realm(other)),
                (true, false) => format!("at war with its rebels, {}, since {began}", n.realm(other)),
                (false, _) => format!("at war with {} since {began}, declared by {}", n.realm(other), n.realm(a)),
            };
            fighting.push((other, text));
        } else if day_of(&wars.truce_until, k) > day && truced.insert(other) {
            let winner = match wars.winner.get(k).copied().unwrap_or(NONE) {
                NONE => "nobody won".to_string(),
                x => format!("{} won", n.realm(x)),
            };
            let until = cal.format(day_of(&wars.truce_until, k));
            truces.push((other, format!("truce with {} until {until} (the war ended {}: {winner})", n.realm(other), cal.format(day_of(&wars.ended, k)))));
        }
    }
    fighting.extend(truces);
    RealmPage { name: n.realm(r), alive: capital.is_some(), capital, facts, ruler, heir, past_rulers, relations: fighting }
}

/// The realms an event is about.
pub fn realms_in(e: &Event) -> Vec<u32> {
    match *e {
        Event::Succession { realm, .. } | Event::Uprising { realm, .. } | Event::Calmed { realm, .. } => vec![realm],
        Event::DeclareWar { attacker, defender }
        | Event::WarDeclared { attacker, defender, .. }
        | Event::Peace { attacker, defender, .. }
        | Event::WarRefused { attacker, defender, .. } => vec![attacker, defender],
        Event::ForcePeace { a, b, .. } => vec![a, b],
        Event::Occupied { by, .. } => vec![by],
        Event::Rebellion { realm, from, .. } => vec![realm, from],
        Event::Battle { ref sides, .. } => sides.iter().map(|s| s.0).collect(),
        _ => Vec::new(),
    }
}

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
                // Realms', armies' and characters' values are not a province's.
                Tier::Cell | Tier::Realm | Tier::RealmGood | Tier::Army | Tier::Character | Tier::War | Tier::Relation => Vec::new(),
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
        None if r.field.def().name == "pop.size" => "people".into(),
        None => field_name(r.field),
    }
}

/// The province a request is about, if it names one.
pub fn request_province(r: &Request) -> Option<u32> {
    r.province()
}

/// The province one value of a field belongs to, for province and per-good fields.
pub fn province_of(id: FieldId, entity: u32, goods: &Goods) -> Option<u32> {
    match id.def().tier {
        Tier::Province => Some(entity),
        Tier::ProvinceGood => Some(goods.split(entity as usize).0 as u32),
        Tier::Cell | Tier::Pop | Tier::Realm | Tier::RealmGood | Tier::Army | Tier::Character | Tier::War | Tier::Relation => None,
    }
}

/// One value of a per-good field, in its good's unit: "1,234 rations".
pub fn good_value(goods: &Goods, c: usize, amount: u64) -> String {
    match goods.tree.unit(goods.columns[c]) {
        Some(unit) => format!("{} {unit}", group(amount)),
        None => group(amount),
    }
}

/// One value of a per-good field for column `c`, with its unit: a store in the good's own unit, a
/// price in coins, workers as people.
pub fn per_good_value(id: FieldId, goods: &Goods, c: usize, amount: u64) -> String {
    match id.def().name {
        "province.price" => format!("{:.3} coins", amount as f64 / sim::markets::PRICE_SCALE),
        "province.workers" => format!("{} people", group(amount)),
        _ => good_value(goods, c, amount),
    }
}

/// One value of a field, with its unit; realms by name.
pub fn value(id: FieldId, data: &FieldData, i: usize, names: &Names) -> String {
    let def = id.def();
    let realm = |v: &[u32]| v.get(i).copied().unwrap_or(NONE);
    match (def.name, data) {
        ("province.center", FieldData::Dir(v)) => {
            let (lon, lat) = sim::grid::dir_to_lonlat(v[i]);
            lat_lon(lat.to_degrees(), lon.to_degrees())
        }
        ("province.owner", FieldData::U32(v)) if realm(v) == NONE => "no realm".into(),
        ("province.owner" | "province.occupier", FieldData::U32(v)) => names.realm(realm(v)),
        ("province.uprising", FieldData::U32(v)) => match realm(v) {
            NONE => "calm".into(),
            seat if seat as usize == i => "risen up".into(),
            seat => format!("joined the uprising begun in province {seat}"),
        },
        ("province.unrest", _) => format!("{} of {} toward an uprising", group(data.value(i) as u64), group(sim::realms::LINE)),
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
        Slot::Realms => "Realms",
        Slot::Characters => "Characters",
        Slot::Diplomacy => "Diplomacy",
        Slot::Warfare => "Warfare",
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
        Cause::Made => "made",
        Cause::MadeInto => "made into other goods",
        Cause::Used => "used up",
        Cause::WornOut => "worn out",
        Cause::TradeIn => "trade, coming in",
        Cause::TradeOut => "trade, going out",
        Cause::Taxes => "taxes",
        Cause::Raised => "raising soldiers",
        Cause::Returned => "soldiers coming home",
        Cause::Spent => "a realm's spending",
        Cause::Deserted => "hungry soldiers deserting home",
        Cause::Equipped => "equipping soldiers",
        Cause::Bounty => "a bounty for soldiers raised",
        Cause::Killed => "soldiers killed in battle or lost at sea",
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

pub fn event(e: &Event, cal: &Calendar, goods: &Goods, names: &Names) -> String {
    let realm = |r: &u32| names.realm(*r);
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
        Event::Arrived { people, cause: c, .. } => format!("{} people arrived: {}", group(*people), cause(*c)),
        Event::Succession { realm: r, ruler, how, .. } => format!("{} has a new ruler, {}: {}", realm(r), names.person(*ruler), how.describe()),
        Event::DeclareWar { attacker, defender } => format!("{} is to declare war on {}", realm(attacker), realm(defender)),
        Event::ForcePeace { a, b, winner } => match *winner {
            NONE => format!("{} and {} are to make peace, nobody winning", realm(a), realm(b)),
            w => format!("{} and {} are to make peace, {} winning", realm(a), realm(b), realm(&w)),
        },
        Event::WarDeclared { attacker, defender, cause, .. } => format!("{} declared war on {}: {}", realm(attacker), realm(defender), cause.describe()),
        // In a rebellion's war the rebels are the attacker.
        Event::Peace { attacker, defender, winner, rebellion: true, .. } => match *winner {
            w if w == *defender => format!("the rebellion of {} is over: {} won and takes back all the rebels' land", realm(attacker), realm(defender)),
            w if w == *attacker => format!("{} has won its freedom from {} and keeps the land it holds", realm(attacker), realm(defender)),
            _ => format!("peace between the rebels of {} and {}: nobody won, the rebels stay free", realm(attacker), realm(defender)),
        },
        Event::Peace { attacker, defender, winner, rebellion: false, .. } => match *winner {
            NONE => format!("peace between {} and {}: nobody won, all occupied land goes back", realm(attacker), realm(defender)),
            w => format!("peace between {} and {}: {} won and keeps the land it holds", realm(attacker), realm(defender), realm(&w)),
        },
        Event::Occupied { by, .. } => format!("{} took the province (or, if it owns it, freed it)", realm(by)),
        Event::WarRefused { attacker, defender, why } => format!("the poke about {} and {} was turned down: {}", realm(attacker), realm(defender), why.describe()),
        Event::Uprising { province, realm: r, seat } if province == seat => format!("the province rose up against {}", realm(r)),
        Event::Uprising { realm: r, seat, .. } => format!("the province joined the uprising against {} begun in province {seat}", realm(r)),
        Event::Calmed { realm: r, .. } => format!("the uprising against {} died down", realm(r)),
        Event::Rebellion { realm: r, from, provinces, .. } => format!("{provinces} provinces broke away from {} as {}", realm(from), realm(r)),
        Event::Battle { winner, loser, killed, .. } => match (*winner, *loser) {
            (NONE, NONE) => format!("a battle here ended with nobody holding the field: {} killed", group(*killed)),
            (w, NONE) => format!("a battle here ended with {} holding the field: {} killed", realm(&w), group(*killed)),
            (NONE, l) => format!("a battle here ended with {} broken and nobody holding the field: {} killed", realm(&l), group(*killed)),
            (w, l) => format!("{} beat {} in battle here: {} killed", realm(&w), realm(&l), group(*killed)),
        },
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

    pub fn text(&self, cal: &Calendar, goods: &Goods, names: &Names) -> String {
        match &self.what {
            Logged::Event(e) => event(e, cal, goods, names),
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
pub fn trace_step(s: &TraceStep, cal: &Calendar, goods: &Goods, names: &Names) -> String {
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
        Traced::Delivered(e) => format!("arrived: {}", event(e, cal, goods, names)),
        Traced::Sent(e) => format!("{who} announced: {} (arrives tomorrow)", event(e, cal, goods, names)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_good_values_have_their_own_units() {
        let goods = Goods::default();
        let id = sim::fields::known;
        assert_eq!(per_good_value(id("province.price"), &goods, 0, 62), "0.062 coins");
        assert_eq!(per_good_value(id("province.workers"), &goods, 0, 1500), format!("{} people", group(1500)));
        assert_eq!(per_good_value(id("province.stock"), &goods, 0, 1500), format!("{} rations", group(1500)));
    }
}
