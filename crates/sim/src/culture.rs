//! Culture "assimilation" (build step 9): peoples spread over regions, and people take up the
//! culture that pulls hardest on them.
//!
//! **Starting cultures.** When the world is made, [`CultureParams::starting`] cultures are spread
//! over the settled land by region, as realms are: starting provinces spread out (the most
//! populous first, then each time the farthest from those chosen), and each culture grows out from
//! its start along the province ways, nearest first by travel days, so mountains and open water
//! make the borders between peoples. Cultures cross realm borders; they don't follow them. Where
//! two cultures meet, a share of each border province's people ([`CultureParams::borderland`]) is
//! of the neighbouring culture: mixed borderlands. Everyone starts as culture 0 (the pops' first
//! value), so the people are given their culture by shift requests to the pops' owner, made while
//! the world is made and applied before its first day ([`Cause::Placed`]).
//!
//! **Each month** every culture pulls on the people of each province: by its share of the
//! province's own people ([`CultureParams::own`]), by its share of the people next door
//! ([`CultureParams::neighbours`], each neighbour counted less the more travel days away it is),
//! and by being the culture of the realm that holds the province ([`CultureParams::ruler`]). The
//! culture that pulls hardest leads; people of every other culture there take it up, a share a
//! month in proportion to how much harder the leader pulls ([`CultureParams::per_year`]). Small
//! groups join the leader at once, so a province holds a few cultures, not dozens. A realm's
//! culture (`realm.culture`) is its capital's main culture. With realms off, there is no ruler's
//! pull.
//!
//! Neighbours pull only from next door (directly linked provinces), and a neighbour's pull fades
//! by half every [`CultureParams::neighbour_half_days`] of travel: this is how "nearby provinces,
//! fading with travel days" is read. The fade keeps a province wholly of one culture, surrounded by
//! its own kind, from quite holding the ruler's pull off: with the defaults its own culture pulls
//! a little less than the ruler's, so even such a province slowly takes up the ruler's culture,
//! faster once its neighbours do.
//!
//! **Splits** ([`SplitParams`], a part that can be switched off): once a year, the provinces where
//! a culture is the main one are put in groups that hang together along the ways. Its biggest group
//! (by people) is the culture's heartland; a group of [`SplitParams::provinces`] or more provinces
//! cut off from it for [`SplitParams::years`] may split off as a new culture, a chance a year,
//! remembering its parent ([`Cause::SplitOff`]).
//!
//! Events: [`Event::Assimilated`] when a province's main culture changed (seen at the next month's
//! start), [`Event::CultureBorn`] when a culture splits off, [`Event::CultureGone`] when the last
//! of a culture is gone. Names come from the world's seed and the culture's number ([`names`]).
//!
//! Nobody is made or lost: people only change culture, so no flow is booked.

use crate::calendar::Calendar;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::fields::{known, Tier};
use crate::grid::{dot, NONE};
use crate::module::{Cadence, Ctx, Flows, Manifest, Module, Slot};
use crate::pops::Pops;
use crate::requests::{Cause, Priority};
use crate::rng::{draw, purpose, round_stochastic, unit, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

/// The most cultures there can be: a pop's culture is a 16-bit number.
pub const MOST: usize = u16::MAX as usize;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CultureParams {
    /// About how many cultures the world starts with (at most one per settled province).
    pub starting: u32,
    /// Share of a border province's people who are of the neighbouring culture at the start.
    pub borderland: f64,
    /// Share of a culture's people in a province who take up the leading culture in a year, when
    /// the leader pulls on them as hard as it can (and in proportion for less).
    pub per_year: f64,
    /// How hard a culture pulls on a province's people for being all of them (and in proportion
    /// for a share): how people hold to their own.
    pub own: f64,
    /// How hard a culture pulls for being all of the people next door, travel days fading it.
    pub neighbours: f64,
    /// A neighbour's pull halves with every this many days of travel to it.
    pub neighbour_half_days: f64,
    /// How hard the culture of the realm holding a province pulls on its people.
    pub ruler: f64,
    /// A group of fewer people than this takes up the leading culture at once.
    pub small_people: u64,
    /// A group less than this share of its province's people takes up the leading culture at once.
    pub small_share: f64,
    /// New cultures from groups cut off from their own; none for no splits. A missing key in a
    /// file means none (not the default), so switching it off survives a settings change.
    #[serde(default)]
    pub splits: Option<SplitParams>,
}

impl Default for CultureParams {
    fn default() -> Self {
        CultureParams {
            starting: 30,
            borderland: 0.15,
            per_year: 0.03,
            own: 1.0,
            neighbours: 0.5,
            neighbour_half_days: 5.0,
            ruler: 1.5,
            small_people: 100,
            small_share: 0.01,
            splits: Some(SplitParams::default()),
        }
    }
}

/// When a group cut off from its culture becomes a culture of its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SplitParams {
    /// Years a group must have been cut off before it may split.
    pub years: f64,
    /// The fewest provinces a group that splits off holds.
    pub provinces: u32,
    /// Each year, the chance that a group cut off long enough splits off.
    pub per_year: f64,
}

impl Default for SplitParams {
    fn default() -> Self {
        SplitParams { years: 60.0, provinces: 3, per_year: 0.05 }
    }
}

/// One entry per culture, as parallel arrays. Numbers are never reused: a culture with nobody left
/// keeps its entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cultures {
    /// The culture it split from, [`crate::grid::NONE`] for a starting culture (field
    /// `culture.parent`).
    pub parent: Vec<u32>,
    /// The day it began (field `culture.born`).
    pub born: Vec<u64>,
    /// The province where it began (field `culture.hearth`).
    pub hearth: Vec<u32>,
    /// The day its last person was gone, `u64::MAX` while it lives (field `culture.died`).
    pub died: Vec<u64>,
}

impl Cultures {
    pub fn count(&self) -> usize {
        self.parent.len()
    }

    /// Whether culture `c` exists and has people (as of its module's last look).
    pub fn alive(&self, c: u32) -> bool {
        self.died.get(c as usize).is_some_and(|&d| d == u64::MAX)
    }

    fn push(&mut self, parent: u32, born: u64, hearth: u32) -> u32 {
        self.parent.push(parent);
        self.born.push(born);
        self.hearth.push(hearth);
        self.died.push(u64::MAX);
        (self.parent.len() - 1) as u32
    }
}

pub struct Assimilation {
    pub params: CultureParams,
    /// Each province's main culture at the last month's start (or as an edit of ours left it),
    /// [`NONE`] where nobody lives: a change is told as [`Event::Assimilated`].
    main: Vec<u32>,
    /// The day each province's group was found cut off from its culture's heartland, `u64::MAX`
    /// while it isn't.
    cut_since: Vec<u64>,
}

impl Assimilation {
    pub fn new(params: CultureParams) -> Assimilation {
        Assimilation { params, main: Vec::new(), cut_since: Vec::new() }
    }

    fn fit(&mut self, n: usize) {
        self.main.resize(n, NONE);
        self.cut_since.resize(n, u64::MAX);
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Culture,
    version: "assimilation",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    // The realms' owners and capitals give the ruler's pull and the realms' cultures; both have
    // defaults (nobody, none), so culture runs with realms off.
    reads: &["pop.province", "pop.culture", "pop.size", "province.cells", "province.center", "province.owner", "realm.capital"],
    reads_earlier: &[],
    owns: &["culture.parent", "culture.born", "culture.hearth", "culture.died", "realm.culture"],
    // People take up another culture by a shift request to the pops' owner.
    requests: &["pop.culture"],
    asks: &[],
    moves: &[],
    requires: &["pop.size"],
    emits: &[EventKind::CultureBorn, EventKind::CultureGone, EventKind::Assimilated],
    handles: &[],
    state_schema: "main-cut-1",
    goods: &[],
};

impl Module for Assimilation {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    fn check(&self, _goods: &crate::fields::Goods) -> Result<(), String> {
        let p = &self.params;
        if p.starting < 1 || p.starting as usize > MOST {
            return Err(format!("the world starts with 1 to {MOST} cultures"));
        }
        if !(0.0..=0.5).contains(&p.borderland) {
            return Err("the mixed share of a borderland goes from 0 to a half".into());
        }
        if !(0.0..=1.0).contains(&p.per_year) {
            return Err("the share taking up the leading culture in a year goes from 0 to 1".into());
        }
        if ![p.own, p.neighbours, p.ruler].iter().all(|x| x.is_finite() && *x >= 0.0) {
            return Err("the pulls of a culture's own people, its neighbours and the ruler are 0 or more".into());
        }
        if !(p.neighbour_half_days.is_finite() && p.neighbour_half_days > 0.0) {
            return Err("a neighbour's pull halves over more than 0 days".into());
        }
        if !(0.0..=1.0).contains(&p.small_share) {
            return Err("a small group's share goes from 0 to 1".into());
        }
        if let Some(s) = &p.splits {
            if !(s.years.is_finite() && s.years > 0.0) {
                return Err("splits: a group is cut off for more than 0 years before it splits".into());
            }
            if s.provinces < 1 {
                return Err("splits: a group that splits off holds a province or more".into());
            }
            if !(0.0..=1.0).contains(&s.per_year) {
                return Err("splits: the chance a year goes from 0 to 1".into());
            }
        }
        Ok(())
    }

    /// The starting cultures, by region, asked of the pops' owner as shifts from culture 0 (applied
    /// before the first day).
    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        let n = w.provinces.count;
        let people = w.province_population();
        w.cultures = Cultures::default();
        self.main = vec![NONE; n];
        self.cut_since = vec![u64::MAX; n];
        let region = regions(w, &people, self.params.starting as usize, cx.seed);
        for (c, &start) in region.starts.iter().enumerate() {
            debug_assert_eq!(c, w.cultures.count());
            w.cultures.push(NONE, 0, start);
        }
        let field = known("pop.culture");
        for p in 0..n {
            let c = region.of[p];
            if c == NONE {
                continue;
            }
            // The neighbouring culture with the most people next door, for a borderland.
            let mut other: Option<(u64, u32)> = None;
            for l in w.routes.of(p as u32) {
                let q = l.other(p as u32) as usize;
                let d = region.of[q];
                if d == NONE || d == c || l.days().is_none() {
                    continue;
                }
                let next: u64 = w.routes.of(p as u32).filter(|m| m.days().is_some() && region.of[m.other(p as u32) as usize] == d).map(|m| people[m.other(p as u32) as usize]).sum();
                if other.is_none_or(|(most, o)| next > most || (next == most && d < o)) {
                    other = Some((next, d));
                }
            }
            let mixed = other.map_or(0, |_| (people[p] as f64 * self.params.borderland).round() as u64);
            let other = other.map_or(NONE, |o| o.1);
            if c != 0 && people[p] > mixed {
                cx.ask_shift(field, p as u32, 0, c as u16, people[p] - mixed, Priority::Normal, Cause::Placed, p as u64);
            }
            if other != 0 && other != NONE && mixed > 0 {
                cx.ask_shift(field, p as u32, 0, other as u16, mixed, Priority::Normal, Cause::Placed, p as u64);
            }
            self.main[p] = if mixed > people[p] - mixed || (mixed == people[p] - mixed && other < c) { other } else { c };
        }
        w.realm_culture = realm_cultures(w, &self.main);
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let n = w.provinces.count;
        self.fit(n);
        if w.cultures.count() == 0 {
            return;
        }
        let counts = Counts::of_pops(&w.pops, n);
        let main: Vec<u32> = (0..n).map(|p| counts.main(p)).collect();

        // Cultures with nobody left are gone; one with people again (an edit) lives again.
        let mut total = vec![0u64; w.cultures.count()];
        for (c, x) in counts.cul.iter().zip(&counts.people) {
            if let Some(t) = total.get_mut(*c as usize) {
                *t += x;
            }
        }
        for (c, &t) in total.iter().enumerate() {
            let died = &mut w.cultures.died[c];
            if *died == u64::MAX && t == 0 {
                *died = cx.day;
                cx.emit(Event::CultureGone { culture: c as u32 });
            } else if *died != u64::MAX && t > 0 {
                *died = u64::MAX;
            }
        }

        w.realm_culture = realm_cultures(w, &main);

        // Main cultures that changed since last month.
        for p in 0..n {
            let (was, now) = (self.main[p], main[p]);
            if was != now {
                if was != NONE && now != NONE {
                    cx.emit(Event::Assimilated { province: p as u32, from: was, to: now });
                    if cx.traced(p as u32) {
                        cx.note(format!("the main culture changed from {} to {}", name(cx.seed, was), name(cx.seed, now)));
                    }
                }
                self.cut_since[p] = u64::MAX;
            }
        }
        self.main.clone_from(&main);

        let mut split = vec![false; n];
        if let Some(s) = self.params.splits.clone()
            && cx.calendar.is_year_start(cx.day)
        {
            self.splits(w, cx, &s, &counts, &main, &mut split);
        }
        if self.params.per_year > 0.0 {
            self.assimilate(w, cx, &counts, &split);
        }
    }

    /// Grants for our shifts: nothing to do (the pops' owner has moved the people).
    fn handle(&mut self, _w: &mut World, _e: &Event, _cx: &mut Ctx) {}

    fn save_state(&self) -> Vec<u8> {
        crate::sim::to_bytes(&(&self.main, &self.cut_since))
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        (self.main, self.cut_since) = postcard::from_bytes(bytes).map_err(|e| format!("Culture state: {e}"))?;
        Ok(())
    }

    fn check_edit(&self, w: &World, edit: &Edit) -> Option<Result<(), String>> {
        match *edit {
            Edit::SetCulture { culture, .. } => Some(if (culture as usize) < w.cultures.count() { Ok(()) } else { Err(format!("no culture {culture}")) }),
            Edit::NewCulture { .. } => Some(if w.cultures.count() < MOST { Ok(()) } else { Err(format!("there are already {MOST} cultures, the most there can be")) }),
            _ => None,
        }
    }

    /// Everyone in the provinces takes the culture (a new one for [`Edit::NewCulture`], born there
    /// in the most populous of them). Nobody is made or lost.
    fn carry_out(&mut self, w: &mut World, edit: &Edit, day: u64, _calendar: &Calendar) -> Flows {
        let (provinces, culture) = match *edit {
            Edit::SetCulture { ref provinces, culture } => {
                // A culture that was gone lives again.
                w.cultures.died[culture as usize] = u64::MAX;
                (provinces, culture)
            }
            Edit::NewCulture { ref provinces } => {
                let people = w.province_population();
                let hearth = *provinces.iter().max_by(|&&a, &&b| people[a as usize].cmp(&people[b as usize]).then(b.cmp(&a))).expect("checked: some provinces");
                (provinces, w.cultures.push(NONE, day, hearth))
            }
            _ => return Flows::default(),
        };
        set_culture(&mut w.pops, w.provinces.count, provinces, culture as u16);
        self.fit(w.provinces.count);
        let people = w.province_population();
        for &p in provinces {
            self.main[p as usize] = if people[p as usize] > 0 { culture } else { NONE };
            self.cut_since[p as usize] = u64::MAX;
        }
        Flows::default()
    }

    /// Provinces may have been added, and realms' capitals moved: their cultures follow.
    fn edited(&mut self, w: &mut World, _edit: &Edit) {
        self.fit(w.provinces.count);
        if w.cultures.count() > 0 {
            let main = main_cultures(w);
            w.realm_culture = realm_cultures(w, &main);
        }
    }
}

impl Assimilation {
    /// Groups cut off from their culture's heartland, and those that split off this year.
    fn splits(&mut self, w: &mut World, cx: &mut Ctx, s: &SplitParams, counts: &Counts, main: &[u32], split: &mut [bool]) {
        let n = main.len();
        // Provinces of one main culture that hang together along the ways, each group named by its
        // lowest province.
        let mut up: Vec<u32> = (0..n as u32).collect();
        fn find(up: &mut [u32], mut p: u32) -> u32 {
            while up[p as usize] != p {
                up[p as usize] = up[up[p as usize] as usize];
                p = up[p as usize];
            }
            p
        }
        for l in &w.routes.links {
            let (a, b) = (l.a as usize, l.b as usize);
            if a < n && b < n && main[a] != NONE && main[a] == main[b] && l.days().is_some() {
                let (ra, rb) = (find(&mut up, l.a), find(&mut up, l.b));
                if ra != rb {
                    up[ra.max(rb) as usize] = ra.min(rb);
                }
            }
        }
        let root: Vec<u32> = (0..n as u32).map(|p| find(&mut up, p)).collect();
        // Each group's people, and each culture's biggest group (ties: the lower group).
        let mut people = vec![0u64; n];
        for p in 0..n {
            if main[p] != NONE {
                people[root[p] as usize] += counts.total(p);
            }
        }
        let mut heart = vec![NONE; w.cultures.count()];
        for g in 0..n {
            let c = main[g];
            if c == NONE || root[g] != g as u32 || c as usize >= heart.len() {
                continue;
            }
            let h = &mut heart[c as usize];
            if *h == NONE || people[g] > people[*h as usize] {
                *h = g as u32;
            }
        }
        let years = (s.years * cx.calendar.days_per_year() as f64).round() as u64;
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); n];
        for p in 0..n {
            let c = main[p];
            let cut = c != NONE && (c as usize) < heart.len() && heart[c as usize] != root[p];
            if !cut {
                self.cut_since[p] = u64::MAX;
                continue;
            }
            if self.cut_since[p] == u64::MAX {
                self.cut_since[p] = cx.day;
            }
            members[root[p] as usize].push(p as u32);
        }
        for g in 0..n {
            let group = &members[g];
            let long = group.iter().filter(|&&p| self.cut_since[p as usize].saturating_add(years) <= cx.day).count();
            if group.is_empty() || long < s.provinces as usize || w.cultures.count() >= MOST {
                continue;
            }
            if unit(cx.seed, Rng::Culture, cx.day, g as u64, purpose("split")) >= s.per_year {
                continue;
            }
            let parent = main[g];
            let hearth = *group.iter().max_by(|&&a, &&b| counts.total(a as usize).cmp(&counts.total(b as usize)).then(b.cmp(&a))).expect("not empty");
            let born = w.cultures.push(parent, cx.day, hearth);
            cx.emit(Event::CultureBorn { culture: born, parent, hearth, provinces: group.len() as u32 });
            for &p in group {
                let p = p as usize;
                let x = counts.people_of(p, parent);
                if x > 0 {
                    cx.ask_shift(known("pop.culture"), p as u32, parent as u16, born as u16, x, Priority::Normal, Cause::SplitOff, p as u64);
                }
                split[p] = true;
                self.main[p] = born;
                self.cut_since[p] = u64::MAX;
                if cx.traced(p as u32) {
                    cx.note(format!("cut off from the heartland of {} for {} years: its {x} people split off as {}", name(cx.seed, parent), s.years, name(cx.seed, born)));
                }
            }
        }
    }

    /// The month's pull on each province, and the people who take up the leading culture.
    fn assimilate(&self, w: &World, cx: &mut Ctx, counts: &Counts, split: &[bool]) {
        let pr = &self.params;
        let months = cx.calendar.months_per_year() as f64;
        let month_rate = 1.0 - (1.0 - pr.per_year).powf(1.0 / months);
        let field = known("pop.culture");
        // (culture, own people, people next door counted by their fade)
        let mut cand: Vec<(u32, u64, f64)> = Vec::new();
        let n = counts.provinces();
        let totals: Vec<u64> = (0..n).map(|p| counts.total(p)).collect();
        // The one culture of a province that has only one, NONE otherwise.
        let only: Vec<u32> = (0..n).map(|p| counts.only(p)).collect();
        for p in 0..n {
            let total = totals[p];
            if total == 0 || split[p] {
                continue;
            }
            let owner = w.owner.get(p).copied().unwrap_or(NONE);
            let ruler = if owner != NONE && w.realms.alive(owner) { w.realm_culture.get(owner as usize).copied().unwrap_or(NONE) } else { NONE };
            // Most provinces are all of one culture, ruled by it or by nobody, among neighbours all
            // of it: nobody there has anything to take up.
            let c = only[p];
            if c != NONE
                && (ruler == NONE || ruler == c)
                && w.routes.of(p as u32).all(|l| {
                    let q = l.other(p as u32) as usize;
                    l.days().is_none() || q >= n || totals[q] == 0 || only[q] == c
                })
                && !cx.traced(p as u32)
            {
                continue;
            }
            cand.clear();
            cand.extend(counts.of(p).map(|(c, x)| (c as u32, x, 0.0)));
            let mut next_door = 0u64;
            for l in w.routes.of(p as u32) {
                let Some(days) = l.days() else { continue };
                let q = l.other(p as u32) as usize;
                if q >= n || totals[q] == 0 {
                    continue;
                }
                next_door += totals[q];
                let fade = (-(days as f64) / pr.neighbour_half_days).exp2();
                for (c, x) in counts.of(q) {
                    let c = c as u32;
                    match cand.iter_mut().find(|e| e.0 == c) {
                        Some(e) => e.2 += fade * x as f64,
                        None => cand.push((c, 0, fade * x as f64)),
                    }
                }
            }
            if ruler != NONE && (ruler as usize) < MOST && !cand.iter().any(|e| e.0 == ruler) {
                cand.push((ruler, 0, 0.0));
            }
            let pull = |e: &(u32, u64, f64)| {
                let own = pr.own * e.1 as f64 / total as f64;
                let near = if next_door > 0 { pr.neighbours * e.2 / next_door as f64 } else { 0.0 };
                own + near + if e.0 == ruler { pr.ruler } else { 0.0 }
            };
            // The leader: the hardest pull; then the most people here; then the lower number.
            let lead = *cand.iter().max_by(|a, b| pull(a).total_cmp(&pull(b)).then(a.1.cmp(&b.1)).then(b.0.cmp(&a.0))).expect("someone lives here");
            let top = pull(&lead);
            if cx.traced(p as u32) {
                let list: Vec<String> = cand.iter().map(|e| format!("{} {:.2}", name(cx.seed, e.0), pull(e))).collect();
                cx.note(format!("pulls on its {total} people: {}; {} leads", list.join(", "), name(cx.seed, lead.0)));
            }
            if top <= 0.0 {
                continue;
            }
            for e in &cand {
                let (c, x) = (e.0, e.1);
                let gap = top - pull(e);
                if c == lead.0 || x == 0 || gap <= 0.0 {
                    continue;
                }
                let small = x < pr.small_people || (x as f64) < pr.small_share * total as f64;
                let take = if small { x } else { round_stochastic(x as f64 * month_rate * gap / top, cx.seed, Rng::Culture, cx.day, p as u64, purpose("assimilate") ^ c as u64).min(x) };
                if take > 0 {
                    cx.ask_shift(field, p as u32, c as u16, lead.0 as u16, take, Priority::Normal, Cause::Assimilated, p as u64);
                    if cx.traced(p as u32) {
                        cx.note(format!("{take} of its {x} {} take up {}{}", name(cx.seed, c), name(cx.seed, lead.0), if small { " (a small group, all at once)" } else { "" }));
                    }
                }
            }
        }
    }
}

/// Each province's people by culture, from the pops: `of(p)` lists its cultures that have people.
struct Counts {
    first: Vec<u32>,
    end: Vec<u32>,
    cul: Vec<u16>,
    people: Vec<u64>,
}

impl Counts {
    fn of_pops(pops: &Pops, n: usize) -> Counts {
        let mut first = vec![0u32; n + 1];
        for (k, &p) in pops.province.iter().enumerate() {
            if pops.size[k] > 0 && (p as usize) < n {
                first[p as usize + 1] += 1;
            }
        }
        for p in 0..n {
            first[p + 1] += first[p];
        }
        let rows = first[n] as usize;
        let (mut cul, mut people) = (vec![0u16; rows], vec![0u64; rows]);
        let mut end: Vec<u32> = first[..n].to_vec();
        for (k, &p) in pops.province.iter().enumerate() {
            let (p, x) = (p as usize, pops.size[k]);
            if x == 0 || p >= n {
                continue;
            }
            let (from, to) = (first[p] as usize, end[p] as usize);
            match cul[from..to].iter().position(|&c| c == pops.culture[k]) {
                Some(i) => people[from + i] += x,
                None => {
                    cul[to] = pops.culture[k];
                    people[to] = x;
                    end[p] += 1;
                }
            }
        }
        first.truncate(n);
        Counts { first, end, cul, people }
    }

    fn of(&self, p: usize) -> impl Iterator<Item = (u16, u64)> + '_ {
        let (a, b) = (self.first[p] as usize, self.end[p] as usize);
        self.cul[a..b].iter().copied().zip(self.people[a..b].iter().copied())
    }

    fn provinces(&self) -> usize {
        self.first.len()
    }

    fn total(&self, p: usize) -> u64 {
        self.of(p).map(|e| e.1).sum()
    }

    /// The province's one culture, if it has exactly one; [`NONE`] otherwise.
    fn only(&self, p: usize) -> u32 {
        if self.end[p] == self.first[p] + 1 { self.cul[self.first[p] as usize] as u32 } else { NONE }
    }

    fn people_of(&self, p: usize, c: u32) -> u64 {
        self.of(p).filter(|e| e.0 as u32 == c).map(|e| e.1).sum()
    }

    /// The culture with the most people, the lower number of equals; [`NONE`] where nobody lives.
    fn main(&self, p: usize) -> u32 {
        self.of(p).max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map_or(NONE, |e| e.0 as u32)
    }
}

impl Counts {
    fn of_world(w: &World) -> Counts {
        Counts::of_pops(&w.pops, w.provinces.count)
    }
}

/// Each province's main culture: the one with the most people (the lower number of equals),
/// [`NONE`] where nobody lives.
pub fn main_cultures(w: &World) -> Vec<u32> {
    let c = Counts::of_world(w);
    (0..c.provinces()).map(|p| c.main(p)).collect()
}

/// Each province's people by culture, most first (the lower number of equals); empty where nobody
/// lives. For showing: the window's culture map and province page.
pub fn mix(w: &World) -> Vec<Vec<(u32, u64)>> {
    let c = Counts::of_world(w);
    (0..c.provinces())
        .map(|p| {
            let mut v: Vec<(u32, u64)> = c.of(p).map(|(k, x)| (k as u32, x)).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            v
        })
        .collect()
}

/// Each realm's culture: its capital's main culture; [`NONE`] for a dissolved realm or an empty
/// capital.
fn realm_cultures(w: &World, main: &[u32]) -> Vec<u32> {
    (0..w.realms.count()).map(|r| w.realms.capital[r]).map(|c| if c == NONE { NONE } else { main.get(c as usize).copied().unwrap_or(NONE) }).collect()
}

/// For unrest from foreign rule: each province's share of people not of its realm's culture, in
/// thousandths (0 where no living realm holds it, or its realm has no culture). A realm's culture is
/// its capital's main culture, worked out here from the pops, so a module that reads only the
/// pops, the owners and the capitals can use it (no same-day loop with the culture module, which
/// reads those). With Culture off everyone is culture 0, and it is 0 everywhere.
pub fn foreign_share(w: &World) -> Vec<u64> {
    let counts = Counts::of_world(w);
    let n = counts.provinces();
    let realm: Vec<u32> = (0..w.realms.count())
        .map(|r| w.realms.capital[r])
        .map(|c| if c == NONE || c as usize >= n { NONE } else { counts.main(c as usize) })
        .collect();
    (0..n)
        .map(|p| {
            let owner = w.owner.get(p).copied().unwrap_or(NONE);
            let ruling = if owner == NONE { NONE } else { realm.get(owner as usize).copied().unwrap_or(NONE) };
            let total = counts.total(p);
            if ruling == NONE || total == 0 {
                return 0;
            }
            let foreign = total - counts.people_of(p, ruling);
            ((foreign as u128 * 1000 + total as u128 / 2) / total as u128) as u64
        })
        .collect()
}

/// Everyone in `provinces` takes culture `to`: each pop of another culture joins the province's pop
/// of the same kind with `to` (the lowest numbered), or takes `to` itself if there is none. Pops
/// are never removed or reordered.
fn set_culture(pops: &mut Pops, n: usize, provinces: &[u32], to: u16) {
    let mut wanted = vec![false; n];
    provinces.iter().for_each(|&p| wanted[p as usize] = true);
    let mut of: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, &p) in pops.province.iter().enumerate() {
        if wanted.get(p as usize) == Some(&true) {
            of[p as usize].push(k);
        }
    }
    let mut done = vec![false; n];
    for &p in provinces {
        let p = p as usize;
        if std::mem::replace(&mut done[p], true) {
            continue;
        }
        let mut list = std::mem::take(&mut of[p]);
        for i in 0..list.len() {
            let k = list[i];
            if pops.culture[k] != to && pops.size[k] > 0 {
                let size = pops.size[k];
                crate::requests::shift_part(pops, &mut list, "pop.culture", k, size, to);
            }
        }
    }
}

/// Where each starting culture begins, and each province's culture ([`NONE`] for unsettled land).
struct Regions {
    starts: Vec<u32>,
    of: Vec<u32>,
}

/// `k` cultures (at most one per settled province) spread over the settled land: spread-out starts,
/// each growing along the ways nearest first by travel days. Settled land none reaches (an island
/// with no way to it) takes the culture whose start is nearest as the crow flies.
fn regions(w: &World, people: &[u64], k: usize, seed: u64) -> Regions {
    let n = w.provinces.count;
    let settled: Vec<u32> = (0..n as u32).filter(|&p| w.provinces.cells[p as usize] > 0 && people[p as usize] > 0).collect();
    let mut of = vec![NONE; n];
    if settled.is_empty() {
        return Regions { starts: Vec::new(), of };
    }
    let starts = spread(w, &settled, k.min(settled.len()).min(MOST), people, seed);
    let mut dist = vec![u64::MAX; n];
    let mut heap: BinaryHeap<Reverse<(u64, u32, u32)>> = BinaryHeap::new();
    for (c, &s) in starts.iter().enumerate() {
        heap.push(Reverse((0, c as u32, s)));
    }
    while let Some(Reverse((d, c, p))) = heap.pop() {
        let pu = p as usize;
        if dist[pu] != u64::MAX {
            continue;
        }
        dist[pu] = d;
        of[pu] = c;
        for l in w.routes.of(p) {
            let q = l.other(p);
            if let (u64::MAX, Some(days)) = (dist[q as usize], l.days()) {
                heap.push(Reverse((d + ((days as f64 * 1000.0).round() as u64).max(1), c, q)));
            }
        }
    }
    let center = |p: u32| w.provinces.center[p as usize];
    for &p in &settled {
        if of[p as usize] == NONE {
            let nearest = (0..starts.len()).max_by(|&a, &b| dot(center(p), center(starts[a])).total_cmp(&dot(center(p), center(starts[b]))).then(b.cmp(&a)));
            of[p as usize] = nearest.map_or(NONE, |c| c as u32);
        }
    }
    // Only settled land has a culture.
    for p in 0..n {
        if people[p] == 0 || w.provinces.cells[p] == 0 {
            of[p] = NONE;
        }
    }
    Regions { starts, of }
}

/// `k` starting provinces among `cands`, spread out: the most populous first, then each time the
/// one farthest from those chosen, favouring the more populous and shuffled a little (as realms
/// start).
fn spread(w: &World, cands: &[u32], k: usize, people: &[u64], seed: u64) -> Vec<u32> {
    let most = cands.iter().map(|&p| people[p as usize]).max().unwrap_or(1).max(1) as f64;
    let weight: Vec<f64> = cands
        .iter()
        .map(|&p| {
            let jitter = 0.7 + 0.6 * unit(seed, Rng::Culture, 0, p as u64, purpose("spread"));
            (people[p as usize] as f64 / most).powf(0.25) * jitter
        })
        .collect();
    let center = |p: u32| w.provinces.center[p as usize];
    let angle = |a: u32, b: u32| dot(center(a), center(b)).clamp(-1.0, 1.0).acos();
    let first = (0..cands.len()).max_by(|&a, &b| people[cands[a] as usize].cmp(&people[cands[b] as usize]).then(b.cmp(&a))).expect("not empty");
    let mut out = vec![cands[first]];
    let mut nearest: Vec<f64> = cands.iter().map(|&p| angle(p, cands[first])).collect();
    while out.len() < k {
        let next = (0..cands.len()).max_by(|&a, &b| (nearest[a] * weight[a]).total_cmp(&(nearest[b] * weight[b])).then(b.cmp(&a))).expect("not empty");
        if nearest[next] <= 0.0 {
            break;
        }
        out.push(cands[next]);
        for (i, &p) in cands.iter().enumerate() {
            nearest[i] = nearest[i].min(angle(p, cands[next]));
        }
    }
    out
}

const ONSETS: &[&str] = &["", "b", "c", "d", "f", "g", "h", "k", "l", "m", "n", "p", "r", "s", "sh", "t", "v", "w", "y", "z", "kh", "dr", "gw"];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "a", "o", "ai", "ea", "ou"];
const CODAS: &[&str] = &["", "", "n", "r", "l", "s", "m", "k", "th", "nd"];
const ENDINGS: &[&str] = &["", "an", "ish", "ese", "ic", "i", "ar", "ene", "ani", "ot"];

/// Culture names, the first `count` cultures', from the world's seed alone: two or three
/// syllables, often with a people's ending ("Dravish", "Kalani"). No two are the same.
pub fn names(seed: u64, count: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(count);
    for c in 0..count as u64 {
        for attempt in 0.. {
            let mut k = 0u64;
            let mut pick = |list: &[&'static str]| {
                k += 1;
                list[(draw(seed, Rng::Culture, attempt, c, purpose("name") ^ k) % list.len() as u64) as usize]
            };
            let syllables = 2 + (pick(&["", "", "", "x"]) == "x") as usize;
            let mut name = String::new();
            for s in 0..syllables {
                name.push_str(pick(if s == 0 { ONSETS } else { &ONSETS[1..] }));
                name.push_str(pick(VOWELS));
                if s + 1 == syllables {
                    name.push_str(pick(CODAS));
                }
            }
            let ending = pick(ENDINGS);
            let vowel = |c: Option<char>| c.is_some_and(|c| "aeiou".contains(c));
            if !ending.is_empty() && vowel(name.chars().last()) != vowel(ending.chars().next()) {
                name.push_str(ending);
            }
            if name.len() >= 4 && seen.insert(name.clone()) {
                let mut cs = name.chars();
                let first = cs.next().expect("not empty").to_ascii_uppercase();
                out.push(std::iter::once(first).chain(cs).collect());
                break;
            }
        }
    }
    out
}

/// One culture's name (for notes); its number for one past what [`names`] makes cheaply.
fn name(seed: u64, c: u32) -> String {
    if c == NONE {
        return "nobody".into();
    }
    names(seed, c as usize + 1).pop().unwrap_or_else(|| format!("culture {c}"))
}
