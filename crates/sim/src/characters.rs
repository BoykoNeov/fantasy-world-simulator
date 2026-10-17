//! Characters (step 7): rulers and their heirs, who are born, age and die.
//!
//! Characters v1, "rulers" ([`Rulers`]), gives every realm a named ruler with three traits
//! (cautious to warlike, greed, ability). Each month everyone alive may die, more likely the older
//! they are. A ruler with no heir may have a child, who becomes heir, or name a grown kinsman. When
//! a ruler dies the heir takes the throne; a child heir makes a weak succession, and no heir at all
//! a contested one, won by a claimant from outside the line. How each ruler came to the throne is
//! kept on the ruler's own entry (`character.succession`, with the day, `character.crowned`) and
//! sent as a [`crate::events::Event::Succession`], for what reads it later (unrest).
//!
//! Character numbers are never reused: the dead keep their entries, with the day they died, as
//! dissolved realms do. Rulers are not extra people: they are not counted in the people total.
//! A realm made after the start gets a founding ruler at the next month's start; the ruler and
//! heir of a dissolved realm lose their places then, and live out their lives.

use crate::events::{Event, EventKind};
use crate::grid::NONE;
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::rng::{draw, purpose, unit, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};

/// `character.died` and `character.crowned` while it hasn't happened.
pub const NEVER: u64 = u64::MAX;

/// The age, in years, at which an heir rules in their own right, and a ruler may have children.
pub const ADULT: f64 = 16.0;
/// Rulers have children up to this age.
pub const LAST_CHILD: f64 = 50.0;

/// One entry per character, as parallel arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Characters {
    /// The realm they rule, are heir to, or last belonged to (field `character.realm`).
    pub realm: Vec<u32>,
    /// Their father or mother among the characters, [`NONE`] if not one of them.
    pub parent: Vec<u32>,
    /// Their name, as a number [`name`] turns into words. A child may be named after a forebear.
    pub name: Vec<u32>,
    /// The day they were born, as a whole number of days; below 0 for those born before the world's
    /// first day.
    pub born: Vec<f64>,
    /// The day they died, [`NEVER`] while alive.
    pub died: Vec<u64>,
    /// The day they became ruler, [`NEVER`] if they haven't.
    pub crowned: Vec<u64>,
    /// How they came to the throne, a [`Succession`] as a number.
    pub succession: Vec<u8>,
    /// 0 cautious to 100 warlike.
    pub martial: Vec<u8>,
    /// 0 open-handed to 100 greedy.
    pub greed: Vec<u8>,
    /// 0 inept to 100 able.
    pub ability: Vec<u8>,
}

impl Characters {
    pub fn count(&self) -> usize {
        self.died.len()
    }

    pub fn alive(&self, c: u32) -> bool {
        self.died.get(c as usize).is_some_and(|&d| d == NEVER)
    }

    /// Age in years on `day` (or on the day they died).
    pub fn age(&self, c: u32, day: u64, days_per_year: u32) -> f64 {
        let end = self.died[c as usize].min(day);
        (end as f64 - self.born[c as usize]) / days_per_year as f64
    }

    pub(crate) fn push(&mut self, realm: u32, parent: u32, name: u32, born: f64, traits: [u8; 3]) -> u32 {
        let c = self.count() as u32;
        self.realm.push(realm);
        self.parent.push(parent);
        self.name.push(name);
        self.born.push(born);
        self.died.push(NEVER);
        self.crowned.push(NEVER);
        self.succession.push(Succession::Never as u8);
        self.martial.push(traits[0]);
        self.greed.push(traits[1]);
        self.ability.push(traits[2]);
        c
    }
}

/// How a ruler came to the throne.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Succession {
    /// Not a ruler (yet).
    Never = 0,
    /// The realm's first ruler: at the world's start, or when a realm is made later.
    Founded = 1,
    /// A grown heir: a smooth succession.
    Heir = 2,
    /// An heir still a child: a weak succession.
    Child = 3,
    /// No heir: a contested succession, won by a claimant from outside the line.
    Claimant = 4,
}

impl Succession {
    pub fn from_u8(n: u8) -> Succession {
        match n {
            1 => Succession::Founded,
            2 => Succession::Heir,
            3 => Succession::Child,
            4 => Succession::Claimant,
            _ => Succession::Never,
        }
    }

    /// "a child heir: a weak succession".
    pub fn describe(self) -> &'static str {
        match self {
            Succession::Never => "never ruled",
            Succession::Founded => "founded the realm",
            Succession::Heir => "a grown heir: a smooth succession",
            Succession::Child => "a child heir: a weak succession",
            Succession::Claimant => "a claimant, with no heir: a contested succession",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RulerParams {
    /// Each year, the chance that a ruler with no heir, between [`ADULT`] and [`LAST_CHILD`] years
    /// old, has a child, who becomes heir.
    pub births_per_year: f64,
    /// Each year, the chance that a ruler with no heir names a grown kinsman heir.
    pub kin_per_year: f64,
    /// Multiplies everyone's chance of dying ([`death_per_year`]); 1 is as the table has it.
    pub mortality: f64,
}

impl Default for RulerParams {
    fn default() -> Self {
        RulerParams { births_per_year: 0.3, kin_per_year: 0.1, mortality: 1.0 }
    }
}

/// The chance of dying within a year at `age`: a child's risk early on, then a small steady risk
/// (illness, accidents) and one that doubles about every eight years. About 2% a year at 40, 6% at
/// 60, 14% at 70, a third at 80.
pub fn death_per_year(age: f64) -> f64 {
    let q = if age < 5.0 {
        0.03
    } else if age < 15.0 {
        0.008
    } else {
        0.006 + 0.0015 * (0.09 * (age - 20.0)).exp()
    };
    q.min(1.0)
}

pub struct Rulers {
    pub params: RulerParams,
}

static V1: Manifest = Manifest {
    slot: Slot::Characters,
    version: "rulers",
    tier: crate::fields::Tier::Character,
    cadence: Cadence::Monthly,
    reads: &["realm.capital"],
    reads_earlier: &[],
    owns: &[
        "realm.ruler",
        "realm.heir",
        "character.realm",
        "character.parent",
        "character.name",
        "character.born",
        "character.died",
        "character.crowned",
        "character.succession",
        "character.martial",
        "character.greed",
        "character.ability",
    ],
    requests: &[],
    asks: &[],
    moves: &[],
    // Rulers rule realms: without them there is nothing to rule.
    requires: &["realm.capital"],
    emits: &[EventKind::Succession],
    handles: &[],
    state_schema: "none",
    goods: &[],
};

impl Module for Rulers {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    fn check(&self, _goods: &crate::fields::Goods) -> Result<(), String> {
        let p = &self.params;
        for (what, x) in [("births a year", p.births_per_year), ("kin named a year", p.kin_per_year)] {
            if !(0.0..=1.0).contains(&x) {
                return Err(format!("the chance of {what} goes from 0 to 1"));
            }
        }
        if !(p.mortality.is_finite() && p.mortality >= 0.0) {
            return Err("mortality is zero or more".into());
        }
        Ok(())
    }

    /// A founding ruler for every realm, of any age from young to old, most often middle-aged,
    /// with an heir if old enough to have one.
    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        for r in 0..w.realms.count() as u32 {
            if w.realms.alive(r) {
                found(w, cx, r, false);
            }
        }
    }

    /// Deaths, then successions, then heirs for realms that have none.
    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        if w.realms.count() == 0 {
            return;
        }
        let (day, year) = (cx.day, cx.calendar.days_per_year());
        let months = cx.calendar.months_per_year() as f64;
        let ch = &mut w.characters;
        for c in 0..ch.count() as u32 {
            if !ch.alive(c) {
                continue;
            }
            let q = (death_per_year(ch.age(c, day, year)) * self.params.mortality).min(1.0);
            let monthly = 1.0 - (1.0 - q).powf(1.0 / months);
            if unit(cx.seed, Rng::Characters, day, c as u64, purpose("death")) < monthly {
                ch.died[c as usize] = day;
            }
        }
        for r in 0..w.realms.count() as u32 {
            let ru = r as usize;
            if !w.realms.alive(r) {
                // The ruler and heir of a dissolved realm lose their places and live out their lives.
                w.realms.ruler[ru] = NONE;
                w.realms.heir[ru] = NONE;
                continue;
            }
            let heir = w.realms.heir[ru];
            if heir != NONE && !w.characters.alive(heir) {
                w.realms.heir[ru] = NONE;
            }
            let ruler = w.realms.ruler[ru];
            if ruler == NONE {
                found(w, cx, r, true);
            } else if !w.characters.alive(ruler) {
                succeed(w, cx, r, ruler);
            }
            if w.realms.heir[ru] == NONE {
                name_heir(w, cx, r, &self.params);
            }
        }
    }
}

/// A trait drawn for character `c`: from 0 to 100, most often near the middle.
fn fresh(seed: u64, c: u32, which: &str) -> u8 {
    let u = |k: u64| unit(seed, Rng::Characters, k, c as u64, purpose(which));
    ((u(0) + u(1)) * 50.0).round() as u8
}

/// The three traits of the character about to be made (number `c`); a child's lie halfway between
/// its parent's and fresh ones.
fn traits(w: &World, seed: u64, c: u32, parent: Option<u32>) -> [u8; 3] {
    let ch = &w.characters;
    let fresh = [fresh(seed, c, "martial"), fresh(seed, c, "greed"), fresh(seed, c, "ability")];
    match parent {
        None => fresh,
        Some(p) => {
            let p = p as usize;
            let from = [ch.martial[p], ch.greed[p], ch.ability[p]];
            std::array::from_fn(|k| (fresh[k] as u16 + from[k] as u16).div_ceil(2) as u8)
        }
    }
}

/// A new character of realm `r`, `age` years old today, with `parent` (or none).
fn make(w: &mut World, cx: &Ctx, r: u32, age: f64, parent: Option<u32>) -> u32 {
    let c = w.characters.count() as u32;
    let traits = traits(w, cx.seed, c, parent);
    // A child is often named after its parent or grandparent.
    let named = |k: u32| w.characters.name[k as usize];
    let u = unit(cx.seed, Rng::Characters, 0, c as u64, purpose("named after"));
    let name = match parent {
        Some(p) if u < 0.3 => named(p),
        Some(p) if u < 0.45 && w.characters.parent[p as usize] != NONE => named(w.characters.parent[p as usize]),
        _ => c,
    };
    let born = cx.day as f64 - (age * cx.calendar.days_per_year() as f64).round();
    w.characters.push(r, parent.unwrap_or(NONE), name, born, traits)
}

/// Put `c` on realm `r`'s throne today.
fn crown(w: &mut World, cx: &mut Ctx, r: u32, c: u32, how: Succession, announce: bool) {
    w.realms.ruler[r as usize] = c;
    w.characters.crowned[c as usize] = cx.day;
    w.characters.succession[c as usize] = how as u8;
    let capital = w.realms.capital[r as usize];
    if announce {
        cx.emit(Event::Succession { realm: r, capital, ruler: c, how });
    }
    if cx.traced(capital) {
        let age = w.characters.age(c, cx.day, cx.calendar.days_per_year());
        cx.note(format!("the capital's realm has a new ruler, character {c}, aged {age:.0}: {}", how.describe()));
    }
}

/// A founding ruler for realm `r`, from 18 to 65 years old, and an heir if the ruler is old enough
/// to have a child: 18 to 35 years younger.
fn found(w: &mut World, cx: &mut Ctx, r: u32, announce: bool) {
    let (seed, day) = (cx.seed, cx.day);
    let u = |k: u64, what: &str| unit(seed, Rng::Characters, day, r as u64 * 4 + k, purpose(what));
    let age = 18.0 + 47.0 * (u(0, "founder age") + u(1, "founder age")) / 2.0;
    let c = make(w, cx, r, age, None);
    crown(w, cx, r, c, Succession::Founded, announce);
    let child = age - (18.0 + 17.0 * u(2, "first child"));
    if child >= 0.0 {
        w.realms.heir[r as usize] = make(w, cx, r, child, Some(c));
    }
}

/// Ruler `dead` of realm `r` has died: the heir takes the throne, or, with none, a claimant.
fn succeed(w: &mut World, cx: &mut Ctx, r: u32, dead: u32) {
    let year = cx.calendar.days_per_year();
    let heir = w.realms.heir[r as usize];
    let (c, how) = if heir != NONE {
        w.realms.heir[r as usize] = NONE;
        let how = if w.characters.age(heir, cx.day, year) < ADULT { Succession::Child } else { Succession::Heir };
        (heir, how)
    } else {
        let age = 20.0 + 30.0 * unit(cx.seed, Rng::Characters, cx.day, r as u64, purpose("claimant age"));
        (make(w, cx, r, age, None), Succession::Claimant)
    };
    if cx.traced(w.realms.capital[r as usize]) {
        let age = w.characters.age(dead, cx.day, year);
        cx.note(format!("the ruler of the capital's realm, character {dead}, died aged {age:.0}"));
    }
    crown(w, cx, r, c, how, true);
}

/// Realm `r` has no heir: its ruler may have a child this month, or name a grown kinsman (a
/// brother, sister or cousin: a child of the ruler's parent, if the ruler's parent is known).
fn name_heir(w: &mut World, cx: &mut Ctx, r: u32, params: &RulerParams) {
    let ruler = w.realms.ruler[r as usize];
    if ruler == NONE {
        return;
    }
    let months = cx.calendar.months_per_year() as f64;
    let (seed, day) = (cx.seed, cx.day);
    let u = |what: &str| unit(seed, Rng::Characters, day, r as u64, purpose(what));
    let age = w.characters.age(ruler, cx.day, cx.calendar.days_per_year());
    let heir = if (ADULT..=LAST_CHILD).contains(&age) && u("birth") < params.births_per_year / months {
        Some(make(w, cx, r, 0.0, Some(ruler)))
    } else if u("kin") < params.kin_per_year / months {
        let kin_age = ADULT + 24.0 * u("kin age");
        let parent = w.characters.parent[ruler as usize];
        Some(make(w, cx, r, kin_age, (parent != NONE).then_some(parent)))
    } else {
        None
    };
    if let Some(h) = heir {
        w.realms.heir[r as usize] = h;
    }
}

const ONSETS: &[&str] = &["", "b", "br", "c", "d", "dr", "f", "g", "gr", "h", "k", "l", "m", "n", "r", "s", "t", "th", "v", "w"];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "a", "e", "o"];
const MIDDLES: &[&str] = &["", "", "l", "r", "n", "d", "s", "th", "m", "ld", "rn", "st", "dr", "lw"];
const ENDINGS: &[&str] = &[
    "ric", "wen", "mund", "bert", "a", "an", "or", "ys", "red", "ard", "win", "gar", "wyn", "eth", "ia", "o", "us", "ald", "helm", "ora", "ina", "is", "dred", "ith",
];

/// A character's name, from the world's seed and the name number alone: a first syllable and an
/// ending ("Aldric", "Merwen", "Dorana"). Two names may come out alike, rarely.
pub fn name(seed: u64, number: u32) -> String {
    let mut k = 0u64;
    let mut pick = |list: &[&'static str]| {
        k += 1;
        list[(draw(seed, Rng::Characters, 0, number as u64, purpose("name") ^ k) % list.len() as u64) as usize]
    };
    let vowel = |c: Option<char>| c.is_some_and(|c| "aeiou".contains(c));
    loop {
        let mut out = String::new();
        out.push_str(pick(ONSETS));
        out.push_str(pick(VOWELS));
        out.push_str(pick(MIDDLES));
        let ending = pick(ENDINGS);
        // No two vowels meet ("Maa"), and a name has four letters or more.
        if vowel(out.chars().last()) && vowel(ending.chars().next()) {
            continue;
        }
        out.push_str(ending);
        if out.len() >= 4 {
            let mut cs = out.chars();
            let first = cs.next().expect("not empty").to_ascii_uppercase();
            return std::iter::once(first).chain(cs).collect();
        }
    }
}

/// A ruler's name with a number if an earlier ruler of the same realm had the same name
/// ("Aldric II"); anyone else's name alone.
pub fn title(w: &World, c: u32) -> String {
    let ch = &w.characters;
    let base = name(w.params.seed, ch.name[c as usize]);
    let crowned = ch.crowned[c as usize];
    if crowned == NEVER {
        return base;
    }
    let before = (0..ch.count())
        .filter(|&k| ch.realm[k] == ch.realm[c as usize] && ch.name[k] == ch.name[c as usize] && ch.crowned[k] != NEVER && (ch.crowned[k], k) < (crowned, c as usize))
        .count();
    if before == 0 {
        return base;
    }
    const NUMERALS: &[&str] = &["II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"];
    match NUMERALS.get(before - 1) {
        Some(n) => format!("{base} {n}"),
        None => format!("{base} the {}th", before + 1),
    }
}

/// A character's traits in words: "warlike", "greedy", "able"; or "cautious", "inept"; none for
/// middling ones.
pub fn traits_in_words(w: &World, c: u32) -> Vec<&'static str> {
    let ch = &w.characters;
    let c = c as usize;
    let mut out = Vec::new();
    match ch.martial[c] {
        67.. => out.push("warlike"),
        ..=33 => out.push("cautious"),
        _ => {}
    }
    if ch.greed[c] >= 67 {
        out.push("greedy");
    }
    match ch.ability[c] {
        67.. => out.push("able"),
        ..=33 => out.push("inept"),
        _ => {}
    }
    out
}
