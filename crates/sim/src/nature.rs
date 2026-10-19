//! Natural world "regrowth" (build step 10): each province's wild woodland, fish and game, used up
//! where people crowd and work them and growing back where they don't. Under markets, the woodland
//! and fish left set how much timber and fish the land can give.
//!
//! **Three measures a province**, each kept as the share gone (in millionths; 0 is untouched, so a
//! world without this module reads untouched land everywhere): `province.woods_lost` (of its wild
//! woodland, `province.forest_km2`), `province.fish_lost` (of the fish in its coastal and river
//! waters) and `province.game_lost` (of its wild game). Each is a part that can be switched off
//! ([`ForestParams`], [`FishParams`], [`WildlifeParams`]); a part that is off leaves its measure
//! untouched. A file that names the module but not a part has that part off, as culture's splits.
//!
//! **Each month** (at its start, as the economy; a month counts for its share of the year's days,
//! so a short month moves the wild less), from how crowded the province is (its people a km² against
//! [`NatureParams::crowd_full`], at most 1):
//! - *Woods.* Crowded land is cleared: the share aimed at is [`ForestParams::most_cleared`] ×
//!   crowding. Below it, clearing closes a share of the gap a year ([`ForestParams::clearing`]);
//!   above it (people have left), the land grows back toward it, about two-thirds of the way in
//!   [`ForestParams::regrow_years`]: steady regrowth, so even fully cleared land comes back. Under
//!   markets each woodcutter fells [`ForestParams::felled_km2`] a year on top, which then grows
//!   back like any cleared land.
//! - *Fish.* The province's shore is its coast and [`FishParams::river_share`] of its river. The
//!   fishing done is its fishers against [`FishParams::fishers_per_km`] a km of shore (under
//!   markets, the workers on fish as the day began; under Economics v0, which has no fishers,
//!   crowding × [`NatureParams::crowd_effort`]). The stock breeds back fastest at half full
//!   ([`FishParams::breeding`]), some swims in from the open sea ([`FishParams::inflow`] of what is
//!   missing, so no fishery dies for good), and the catch takes [`FishParams::catch`] × the
//!   fishing a year of what is there. Fully fished, the defaults settle near a quarter left; half
//!   fished, near half.
//! - *Game.* The room for it is the woodland standing on forest land and what crowding leaves
//!   uncleared on open land. It breeds toward that room ([`WildlifeParams::breeding`], plus
//!   [`WildlifeParams::inflow`] from the wilds around), falls back to it when the room shrinks, and
//!   is hunted where people crowd ([`WildlifeParams::hunting`] × crowding).
//! - *Droughts* (Weather's missing rain) slow the regrowth of woods and game, by the share of rain
//!   missing × [`NatureParams::drought`]; nothing dies of them.
//!
//! **Lived in before the start.** When the world is made, the months of
//! [`NatureParams::lived_in_years`] are run with the starting people, so the world doesn't begin
//! with untouched forests. Nobody knows the fishers yet then (markets sets them after), so the
//! fishing then is crowding × "fishing without markets" under both economies, and nobody fells.
//!
//! **What it changes.** Under markets, the woodland and fish left scale what timber's and fish's
//! land formulas take from `province.forest_km2`, `province.coast_km` and `province.river_km`
//! (markets reads the fields), so a fished-out shore gives less and fishers drift to other work.
//! Under Economics v0 nothing reads them: they are tracked and shown only. Game feeds nothing yet.
//!
//! **Events,** told once and not again until the measure has crossed back by the gap between
//! [`NatureParams::low`] and [`NatureParams::back`], so they don't flicker: WoodsCleared when the
//! woodland standing falls below `low`, WoodsRegrown when it is back above `back`; FisheryCollapsed
//! and FisheryRecovered the same for fish. Which have been told is the module's only state, a byte
//! a province. Provinces whose measures were already low when the world was made start as told.
//!
//! The model is the same every time (no randomness); nobody is made or lost (no flows, no
//! requests).

use crate::calendar::Calendar;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::fields::{Goods, Tier};
use crate::module::{Cadence, Ctx, Flows, Manifest, Module, Slot};
use crate::world::World;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// A measure's whole, in its field: shares are kept in millionths.
pub const WHOLE: f64 = 1_000_000.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NatureParams {
    /// People a km² at which a province counts as fully crowded (crowding 1).
    pub crowd_full: f64,
    /// Without markets (no fishers known): the fishing done, as a share of full, for each unit of
    /// crowding. Also the fishing while the world is lived in before the start.
    pub crowd_effort: f64,
    /// Years the world was lived in before the start, with its starting people.
    pub lived_in_years: f64,
    /// A measure below this share left is told as cleared or collapsed.
    pub low: f64,
    /// Once told, a measure above this share left is told as back.
    pub back: f64,
    /// How much a drought slows regrowth: the share of rain missing × this.
    pub drought: f64,
    /// The good whose workers fell woodland (a name in the goods tree).
    pub woodcutters: String,
    /// The good whose workers fish (a name in the goods tree).
    pub fishers: String,
    /// The woodland part; none: woodland untouched. A missing key in a file means none (not the
    /// default), as culture's splits.
    #[serde(default)]
    pub forests: Option<ForestParams>,
    /// The fish part; none: fish untouched.
    #[serde(default)]
    pub fish: Option<FishParams>,
    /// The game part; none: game untouched.
    #[serde(default)]
    pub wildlife: Option<WildlifeParams>,
}

impl Default for NatureParams {
    fn default() -> Self {
        NatureParams {
            crowd_full: 4.0,
            crowd_effort: 0.5,
            lived_in_years: 200.0,
            low: 0.2,
            back: 0.5,
            drought: 1.0,
            woodcutters: "timber".into(),
            fishers: "fish".into(),
            forests: Some(ForestParams::default()),
            fish: Some(FishParams::default()),
            wildlife: Some(WildlifeParams::default()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ForestParams {
    /// Share of the woodland cleared in a fully crowded province (and in proportion for less).
    pub most_cleared: f64,
    /// Share of the gap to that a year that clearing closes.
    pub clearing: f64,
    /// Years for cleared land left alone to grow back about two-thirds of the way (a time
    /// constant: nearly all of it in three times as many).
    pub regrow_years: f64,
    /// Km² of woodland a woodcutter fells a year (under markets).
    pub felled_km2: f64,
}

impl Default for ForestParams {
    fn default() -> Self {
        ForestParams { most_cleared: 0.8, clearing: 0.1, regrow_years: 60.0, felled_km2: 0.01 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FishParams {
    /// How fast the stock breeds back, a year, fastest at half full.
    pub breeding: f64,
    /// Share of the missing fish that swim in from the open sea a year.
    pub inflow: f64,
    /// Share of the stock a year the catch takes when fully fished (and in proportion).
    pub catch: f64,
    /// Fishers a km of shore that fish it fully.
    pub fishers_per_km: f64,
    /// A km of river counts as this many km of shore.
    pub river_share: f64,
}

impl Default for FishParams {
    fn default() -> Self {
        FishParams { breeding: 0.3, inflow: 0.02, catch: 0.3, fishers_per_km: 8.0, river_share: 0.67 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WildlifeParams {
    /// How fast game breeds back toward the room for it, a year.
    pub breeding: f64,
    /// Share of the missing game a year that comes in from the wilds around.
    pub inflow: f64,
    /// Share of the game a year hunted in a fully crowded province (and in proportion).
    pub hunting: f64,
    /// Share of the people who live by hunting where the game is untouched (and in proportion for
    /// less): only followed people's jobs read it.
    pub hunters: f64,
}

impl Default for WildlifeParams {
    fn default() -> Self {
        WildlifeParams { breeding: 0.2, inflow: 0.02, hunting: 0.2, hunters: 0.02 }
    }
}

/// How crowded a province is: its people a km² against `full`, from 0 to 1 (0 with no land).
pub fn crowding(people: f64, area_km2: f64, full: f64) -> f64 {
    if area_km2 > 0.0 && full > 0.0 { (people / area_km2 / full).clamp(0.0, 1.0) } else { 0.0 }
}

/// A month of the woods: `lost` the share cleared, `aim` the share crowding clears, `felled` the
/// share of the woodland woodcutters fell a year, `grow` how fast regrowth goes (1, less in a
/// drought), `dt` the month as a share of the year. The new share cleared.
pub fn woods_step(f: &ForestParams, lost: f64, aim: f64, felled: f64, grow: f64, dt: f64) -> f64 {
    let r = Rates::new(&NatureParams { forests: Some(f.clone()), ..NatureParams::default() }, dt);
    r.woods(f, lost, aim, felled, grow)
}

/// A month of the fish: `stock` the share left, `work` the fishing done (1 fully fished). The new
/// share left: the catch first, then breeding and the inflow from the open sea.
pub fn fish_step(f: &FishParams, stock: f64, work: f64, dt: f64) -> f64 {
    Rates::new(&NatureParams::default(), dt).fish(f, stock, work)
}

/// A month of the game: `game` the share left, `room` the share the land has room for, `crowding`
/// for the hunting, `grow` as for the woods. The new share left.
pub fn game_step(g: &WildlifeParams, game: f64, room: f64, crowding: f64, grow: f64, dt: f64) -> f64 {
    let r = Rates::new(&NatureParams { wildlife: Some(g.clone()), ..NatureParams::default() }, dt);
    r.game(g, game, room, crowding, grow)
}

/// What every province's month shares, worked out once a month: the month as a share of the year,
/// and the factors of clearing, of regrowth without a drought, and of game dying back.
#[derive(Clone, Copy, Debug)]
pub struct Rates {
    dt: f64,
    /// The share of the gap to the aim that clearing closes in the month.
    clear: f64,
    /// What is left of the land cleared beyond the aim after the month's regrowth, without a
    /// drought.
    regrow: f64,
    /// What is left of game beyond its room after the month.
    back: f64,
}

impl Rates {
    pub fn new(p: &NatureParams, dt: f64) -> Rates {
        let (clear, regrow) = p.forests.as_ref().map_or((0.0, 1.0), |f| (1.0 - (1.0 - f.clearing.clamp(0.0, 1.0)).powf(dt), (-dt / f.regrow_years).exp()));
        let back = p.wildlife.as_ref().map_or(1.0, |g| (-(g.breeding + g.inflow) * dt).exp());
        Rates { dt, clear, regrow, back }
    }

    fn woods(&self, f: &ForestParams, lost: f64, aim: f64, felled: f64, grow: f64) -> f64 {
        let l = if lost < aim {
            lost + (aim - lost) * self.clear
        } else {
            let regrow = if grow == 1.0 { self.regrow } else { (-grow * self.dt / f.regrow_years).exp() };
            aim + (lost - aim) * regrow
        };
        // Felling takes from what stands, so it never takes more than there is.
        let standing = if felled == 0.0 { 1.0 - l } else { (1.0 - l) * (-felled * self.dt).exp() };
        (1.0 - standing).clamp(0.0, 1.0)
    }

    fn fish(&self, f: &FishParams, stock: f64, work: f64) -> f64 {
        let take = f.catch * work;
        let s = if take == 0.0 { stock } else { stock * (-take * self.dt).exp() };
        let s = s - (1.0 - s) * (-(f.breeding * s + f.inflow) * self.dt).exp_m1();
        s.clamp(0.0, 1.0)
    }

    fn game(&self, g: &WildlifeParams, game: f64, room: f64, crowding: f64, grow: f64) -> f64 {
        let x = if game < room {
            // Breeding (fastest at half the room) and the inflow fill what is missing.
            let rate = (g.breeding * game / room + g.inflow) * grow;
            game - (room - game) * (-rate * self.dt).exp_m1()
        } else {
            // More game than room (land was cleared): it dies back toward the room.
            room + (game - room) * self.back
        };
        let hunted = g.hunting * crowding;
        (if hunted == 0.0 { x } else { x * (-hunted * self.dt).exp() }).clamp(0.0, 1.0)
    }
}

/// The three measures of a province, each as the share gone (0 untouched, 1 all gone).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wild {
    pub woods: f64,
    pub fish: f64,
    pub game: f64,
}

impl Wild {
    fn of(w: &World, p: usize) -> Wild {
        let share = |v: &[u64]| v.get(p).copied().unwrap_or(0).min(WHOLE as u64) as f64 / WHOLE;
        Wild { woods: share(&w.woods_lost), fish: share(&w.fish_lost), game: share(&w.game_lost) }
    }
}

/// What a month's work in a province needs to know of its land and people.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Land {
    pub people: f64,
    pub area_km2: f64,
    pub forest_km2: f64,
    pub coast_km: f64,
    pub river_km: f64,
    /// Share of its rain missing (a drought), 0 to 1.
    pub dry: f64,
}

/// The woodcutters and fishers at work, where they are known (under markets).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hands {
    pub woodcutters: Option<f64>,
    pub fishers: Option<f64>,
}

/// How a month came out, for the inspector's notes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Why {
    pub crowding: f64,
    /// The share of the woodland crowding clears.
    pub aim: f64,
    /// The share of the woodland woodcutters fell a year.
    pub felled: f64,
    /// The km of shore and the fishing done (1 fully fished).
    pub shore: f64,
    pub work: f64,
    /// The share of game the land has room for.
    pub room: f64,
    /// How fast woods and game grow back (1, less in a drought).
    pub grow: f64,
}

impl NatureParams {
    /// One month of a province's wild: the measures after it, and why.
    pub fn month(&self, wild: Wild, land: &Land, hands: Hands, dt: f64) -> (Wild, Why) {
        self.month_with(&Rates::new(self, dt), wild, land, hands)
    }

    /// [`NatureParams::month`], with the month's shared factors worked out already.
    pub fn month_with(&self, rates: &Rates, wild: Wild, land: &Land, hands: Hands) -> (Wild, Why) {
        let mut out = wild;
        let crowding = crowding(land.people, land.area_km2, self.crowd_full);
        let most = self.forests.as_ref().map_or(1.0, |f| f.most_cleared);
        let grow = (1.0 - self.drought * land.dry).clamp(0.0, 1.0);
        let mut why = Why { crowding, aim: most * crowding, grow, ..Why::default() };
        if let Some(f) = &self.forests
            && land.forest_km2 > 0.0
        {
            why.felled = hands.woodcutters.map_or(0.0, |n| f.felled_km2 * n / land.forest_km2);
            out.woods = rates.woods(f, wild.woods, why.aim, why.felled, grow);
        }
        if let Some(f) = &self.fish {
            why.shore = land.coast_km + f.river_share * land.river_km;
            if why.shore > 0.0 {
                why.work = match hands.fishers {
                    Some(n) => n / (why.shore * f.fishers_per_km),
                    None => crowding * self.crowd_effort,
                };
                out.fish = 1.0 - rates.fish(f, 1.0 - wild.fish, why.work);
            }
        }
        if let Some(g) = &self.wildlife
            && land.area_km2 > 0.0
        {
            let forest = land.forest_km2.clamp(0.0, land.area_km2);
            why.room = ((forest * (1.0 - out.woods) + (land.area_km2 - forest) * (1.0 - why.aim)) / land.area_km2).clamp(0.0, 1.0);
            out.game = 1.0 - rates.game(g, 1.0 - wild.game, why.room, crowding, grow);
        }
        (out, why)
    }

    /// A province's wild after [`NatureParams::lived_in_years`] of months (each its own length)
    /// with the land and people it has, nobody known to fish or fell: where the world starts.
    pub fn lived_in(&self, land: &Land, calendar: &Calendar) -> Wild {
        let mpy = calendar.months_per_year().max(1);
        let months = (self.lived_in_years * mpy as f64).round() as usize;
        let rates: Vec<Rates> = (0..mpy).map(|m| Rates::new(self, month_part(calendar, m))).collect();
        let mut wild = Wild::default();
        for k in 0..months {
            let (next, _) = self.month_with(&rates[k % mpy], wild, land, Hands::default());
            let still = (next.woods - wild.woods).abs() < 1e-12 && (next.fish - wild.fish).abs() < 1e-12 && (next.game - wild.game).abs() < 1e-12;
            wild = next;
            // Settled: the rest of the years change nothing worth a millionth.
            if still {
                break;
            }
        }
        wild
    }
}

/// Month `m` as a share of the year (months may differ in length).
pub fn month_part(calendar: &Calendar, m: usize) -> f64 {
    calendar.def.months.get(m).map_or(0.0, |x| x.days as f64) / calendar.days_per_year().max(1) as f64
}

/// Which warnings a province has been given: its woods cleared, its fishery collapsed.
const TOLD_WOODS: u8 = 1;
const TOLD_FISH: u8 = 2;
/// The warnings a province's month gives (not saved).
const WOODS_CLEARED: u8 = 1;
const WOODS_REGROWN: u8 = 2;
const FISH_COLLAPSED: u8 = 4;
const FISH_RECOVERED: u8 = 8;

pub struct Regrowth {
    pub params: NatureParams,
    /// A byte a province: which warnings it has been given (and not yet taken back).
    told: Vec<u8>,
}

impl Regrowth {
    pub fn new(params: NatureParams) -> Regrowth {
        Regrowth { params, told: Vec::new() }
    }

    /// Which warnings each province has been given (for tests and the window).
    pub fn told(&self) -> &[u8] {
        &self.told
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Nature,
    version: "regrowth",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    // Weather's missing rain has a default (none), so the module runs with weather off.
    reads: &["pop.province", "pop.size", "province.area_km2", "province.forest_km2", "province.coast_km", "province.river_km", "province.rain_shortfall"],
    // Markets' workers as the day began: markets reads what this module keeps the same day, so it
    // runs after it. Under Economics v0 nobody keeps workers, and there are none to read.
    reads_earlier: &["province.workers"],
    owns: &["province.woods_lost", "province.fish_lost", "province.game_lost"],
    requests: &[],
    asks: &[],
    moves: &[],
    requires: &["pop.size"],
    emits: &[EventKind::WoodsCleared, EventKind::WoodsRegrown, EventKind::FisheryCollapsed, EventKind::FisheryRecovered],
    handles: &[],
    state_schema: "told-1",
    goods: &[],
};

fn share(x: f64) -> bool {
    (0.0..=1.0).contains(&x)
}

fn rate(x: f64) -> bool {
    x.is_finite() && x >= 0.0
}

fn millionths(x: f64) -> u64 {
    (x.clamp(0.0, 1.0) * WHOLE).round() as u64
}

fn land_of(w: &World, people: &[u64], p: usize) -> Land {
    let pr = &w.provinces;
    Land {
        people: people[p] as f64,
        area_km2: pr.area_km2[p],
        forest_km2: pr.forest_km2[p],
        coast_km: pr.coast_km[p],
        river_km: pr.river_km[p],
        dry: w.rain_shortfall.get(p).copied().unwrap_or(0).min(1000) as f64 / 1000.0,
    }
}

impl Module for Regrowth {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    /// Followed people may hunt, where game is kept: fewer where it is hunted out.
    fn jobs(&self) -> Option<crate::lives::Jobs> {
        let hunters = self.params.wildlife.as_ref()?.hunters;
        Some(crate::lives::Jobs {
            reads: &["pop.province", "pop.size", "province.game_lost"],
            offer: std::sync::Arc::new(move |w: &World, p: usize, out: &mut Vec<crate::lives::Job>| {
                let people: u64 = (0..w.pops.size.len()).filter(|&j| w.pops.province[j] as usize == p).map(|j| w.pops.size[j]).sum();
                let left = 1.0 - w.game_lost.get(p).copied().unwrap_or(0) as f64 / 1_000_000.0;
                out.push(crate::lives::Job { name: "hunter".into(), holders: people as f64 * hunters * left.max(0.0) });
            }),
        })
    }

    fn check(&self, goods: &Goods) -> Result<(), String> {
        let p = &self.params;
        if !(p.crowd_full.is_finite() && p.crowd_full > 0.0) {
            return Err("a province is fully crowded at more than 0 people a km²".into());
        }
        if !rate(p.crowd_effort) {
            return Err("the fishing without markets is 0 or more".into());
        }
        if !(p.lived_in_years.is_finite() && (0.0..=10_000.0).contains(&p.lived_in_years)) {
            return Err("the world was lived in for 0 to 10,000 years before the start".into());
        }
        if !(share(p.low) && share(p.back) && share(p.drought)) {
            return Err("the warning lines and how much droughts slow regrowth go from 0 to 1".into());
        }
        if p.low > p.back {
            return Err(format!("the woods and fish are told back above {} but warned of below {}: \"back above\" must be at least \"warned below\"", p.back, p.low));
        }
        for (what, name) in [("woodcutters", &p.woodcutters), ("fishers", &p.fishers)] {
            if goods.tree.find(name).is_none_or(|n| n == 0) {
                return Err(format!("the {what} work at {name}, which is not a good in the goods tree"));
            }
        }
        if let Some(f) = &p.forests {
            if !(share(f.most_cleared) && share(f.clearing)) {
                return Err("forests: the most cleared and the clearing speed go from 0 to 1".into());
            }
            if !(f.regrow_years.is_finite() && f.regrow_years > 0.0) {
                return Err("forests: woodland grows back over more than 0 years".into());
            }
            if !rate(f.felled_km2) {
                return Err("forests: a woodcutter fells 0 km² a year or more".into());
            }
        }
        if let Some(f) = &p.fish {
            if !(rate(f.breeding) && rate(f.inflow) && rate(f.catch) && rate(f.river_share)) {
                return Err("fish: breeding, the inflow, the catch and rivers as shore are 0 or more".into());
            }
            if !(f.fishers_per_km.is_finite() && f.fishers_per_km > 0.0) {
                return Err("fish: a km of shore is fully fished by more than 0 fishers".into());
            }
        }
        if let Some(g) = &p.wildlife
            && !(rate(g.breeding) && rate(g.inflow) && rate(g.hunting))
        {
            return Err("wildlife: breeding, the inflow and hunting are 0 or more".into());
        }
        if let Some(g) = &p.wildlife
            && !(0.0..=1.0).contains(&g.hunters)
        {
            return Err("wildlife: the hunters' share goes from 0 to 1".into());
        }
        Ok(())
    }

    /// Everything untouched, then the years lived in before the start, with the starting people.
    /// Provinces already low are told so without an event.
    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        let n = w.provinces.count;
        (w.woods_lost, w.fish_lost, w.game_lost) = (vec![0; n], vec![0; n], vec![0; n]);
        self.told = vec![0; n];
        let people = w.province_population();
        let lived: Vec<Option<Wild>> = {
            let (w, calendar, params): (&World, &Calendar, &NatureParams) = (w, cx.calendar, &self.params);
            (0..n).into_par_iter().map(|p| Some(land_of(w, &people, p)).filter(|l| l.area_km2 > 0.0).map(|l| params.lived_in(&l, calendar))).collect()
        };
        for (p, wild) in lived.into_iter().enumerate() {
            if let Some(wild) = wild {
                (w.woods_lost[p], w.fish_lost[p], w.game_lost[p]) = (millionths(wild.woods), millionths(wild.fish), millionths(wild.game));
            }
        }
        for p in 0..n {
            let wild = Wild::of(w, p);
            let land = land_of(w, &people, p);
            if self.params.forests.is_some() && land.forest_km2 > 0.0 && 1.0 - wild.woods < self.params.low {
                self.told[p] |= TOLD_WOODS;
            }
            if let Some(f) = &self.params.fish
                && 1.0 - wild.fish < self.params.low
                && land.coast_km + f.river_share * land.river_km > 0.0
            {
                self.told[p] |= TOLD_FISH;
            }
        }
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let n = w.provinces.count;
        self.told.resize(n, 0);
        let p = &self.params;
        let dt = month_part(cx.calendar, cx.calendar.date(cx.day).month);
        let people = w.province_population();
        let goods = cx.goods;
        let (cut, fishing) = (goods.column(&p.woodcutters), goods.column(&p.fishers));
        let earlier = w.earlier.clone();
        let workers: &[u64] = if cut.is_some() || fishing.is_some() { earlier.u64s("province.workers") } else { &[] };
        let at = |i: usize, c: Option<usize>| c.map(|c| workers.get(goods.index(i, c)).copied().unwrap_or(0) as f64);
        let rates = Rates::new(p, dt);
        if let Some(traced) = (0..n).find(|&i| cx.traced(i as u32)) {
            let land = land_of(w, &people, traced);
            let hands = Hands { woodcutters: at(traced, cut), fishers: at(traced, fishing) };
            let (now, why) = p.month_with(&rates, Wild::of(w, traced), &land, hands);
            notes(cx, p, &land, hands, &now, &why);
        }
        // Each province's month on its own, side by side (the same whatever the threads), with the
        // warnings it gives; then written and told in province order.
        let next: Vec<([u64; 3], u8)> = {
            let w: &World = w;
            self.told
                .par_iter_mut()
                .enumerate()
                .with_min_len(256)
                .map(|(i, told)| {
                    let was = [w.woods_lost[i], w.fish_lost[i], w.game_lost[i]];
                    let land = land_of(w, &people, i);
                    if land.area_km2 <= 0.0 {
                        return (was, 0);
                    }
                    let shore = p.fish.as_ref().map_or(0.0, |f| land.coast_km + f.river_share * land.river_km);
                    let wooded = p.forests.is_some() && land.forest_km2 > 0.0;
                    // Nobody there and nothing gone: nothing changes.
                    let now = if land.people == 0.0 && was == [0; 3] {
                        was
                    } else {
                        let (now, _) = p.month_with(&rates, Wild::of(w, i), &land, Hands { woodcutters: at(i, cut), fishers: at(i, fishing) });
                        [
                            if wooded { millionths(now.woods) } else { was[0] },
                            if shore > 0.0 { millionths(now.fish) } else { was[1] },
                            if p.wildlife.is_some() { millionths(now.game) } else { was[2] },
                        ]
                    };
                    let left = |x: u64| 1.0 - x.min(WHOLE as u64) as f64 / WHOLE;
                    let mut events = 0;
                    if wooded {
                        let standing = left(now[0]);
                        if *told & TOLD_WOODS == 0 && standing < p.low {
                            (*told, events) = (*told | TOLD_WOODS, events | WOODS_CLEARED);
                        } else if *told & TOLD_WOODS != 0 && standing > p.back {
                            (*told, events) = (*told & !TOLD_WOODS, events | WOODS_REGROWN);
                        }
                    }
                    if shore > 0.0 {
                        let fish = left(now[1]);
                        if *told & TOLD_FISH == 0 && fish < p.low {
                            (*told, events) = (*told | TOLD_FISH, events | FISH_COLLAPSED);
                        } else if *told & TOLD_FISH != 0 && fish > p.back {
                            (*told, events) = (*told & !TOLD_FISH, events | FISH_RECOVERED);
                        }
                    }
                    (now, events)
                })
                .collect()
        };
        for (i, ([woods, fish, game], events)) in next.into_iter().enumerate() {
            (w.woods_lost[i], w.fish_lost[i], w.game_lost[i]) = (woods, fish, game);
            if events == 0 {
                continue;
            }
            let province = i as u32;
            if events & WOODS_CLEARED != 0 {
                cx.emit(Event::WoodsCleared { province, left: (WHOLE as u64).saturating_sub(woods) });
            }
            if events & WOODS_REGROWN != 0 {
                cx.emit(Event::WoodsRegrown { province });
            }
            if events & FISH_COLLAPSED != 0 {
                cx.emit(Event::FisheryCollapsed { province });
            }
            if events & FISH_RECOVERED != 0 {
                cx.emit(Event::FisheryRecovered { province });
            }
        }
    }

    fn save_state(&self) -> Vec<u8> {
        crate::sim::to_bytes(&self.told)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.told = postcard::from_bytes(bytes).map_err(|e| format!("Natural world state: {e}"))?;
        Ok(())
    }

    fn check_edit(&self, _w: &World, edit: &Edit) -> Option<Result<(), String>> {
        let Edit::SetNature { woods, fish, game, .. } = *edit else { return None };
        let p = &self.params;
        for (what, x, on) in [("woodland", woods, p.forests.is_some()), ("fish", fish, p.fish.is_some()), ("game", game, p.wildlife.is_some())] {
            let Some(x) = x else { continue };
            if !share(x) {
                return Some(Err(format!("the {what} left is a share from 0 to 1, not {x}")));
            }
            if !on {
                return Some(Err(format!("this world doesn't keep its {what}: that part of the natural world is off")));
            }
        }
        Some(Ok(()))
    }

    /// The shares left the edit names, in each of its provinces (planting refills the province's
    /// wild woodland, restocking its waters; neither adds woodland or shore the map hasn't).
    fn carry_out(&mut self, w: &mut World, edit: &Edit, _day: u64, _calendar: &Calendar) -> Flows {
        if let Edit::SetNature { ref provinces, woods, fish, game } = *edit {
            for &p in provinces {
                let p = p as usize;
                for (v, x) in [(&mut w.woods_lost, woods), (&mut w.fish_lost, fish), (&mut w.game_lost, game)] {
                    if let (Some(x), Some(slot)) = (x, v.get_mut(p)) {
                        *slot = millionths(1.0 - x);
                    }
                }
            }
        }
        Flows::default()
    }

    /// Provinces may have been added (a split): they start with nothing told.
    fn edited(&mut self, w: &mut World, _edit: &Edit) {
        self.told.resize(w.provinces.count, 0);
    }
}

fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

/// The inspector's notes for a province's month.
fn notes(cx: &mut Ctx, p: &NatureParams, land: &Land, hands: Hands, now: &Wild, why: &Why) {
    let density = if land.area_km2 > 0.0 { land.people / land.area_km2 } else { 0.0 };
    let drought = if why.grow < 1.0 { format!("; a drought slows regrowth to {} of its speed", pct(why.grow)) } else { String::new() };
    if p.forests.is_some() && land.forest_km2 > 0.0 {
        let felling = match hands.woodcutters {
            Some(n) if n > 0.0 => format!("; {n:.0} woodcutters fell {} of it a year", pct(why.felled)),
            _ => String::new(),
        };
        cx.note(format!(
            "woodland {} cleared of {:.0} km²: {density:.2} people a km², crowding {:.2} → aim {}{felling}{drought}",
            pct(now.woods),
            land.forest_km2,
            why.crowding,
            pct(why.aim)
        ));
    }
    if p.fish.is_some() && why.shore > 0.0 {
        let who = match hands.fishers {
            Some(n) => format!("{n:.0} fishers"),
            None => format!("crowding {:.2} × {} (no fishers known)", why.crowding, p.crowd_effort),
        };
        cx.note(format!("fish {} left: {:.0} km of shore, {who}: fished {:.2} of fully", pct(1.0 - now.fish), why.shore, why.work));
    }
    if p.wildlife.is_some() && land.area_km2 > 0.0 {
        cx.note(format!("game {} left: room for {}, hunted at crowding {:.2}{drought}", pct(1.0 - now.game), pct(why.room), why.crowding));
    }
}
