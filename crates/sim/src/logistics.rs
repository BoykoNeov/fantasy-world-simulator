//! Logistics and support (build step 8): supply lines and foraging, winter, soldiers' pay and
//! sieges in depth, as four parts of Warfare that can each be switched on or off (Warfare
//! "campaigns" is "troops" with all four). With all four off, Warfare is exactly "troops".
//!
//! - **Supply** (`modules.warfare.supply`): each month, a company standing outside its realm's own
//!   land (land it holds that no enemy occupies) is fed from its home province along a supply line
//!   while it stands at most `reach_ways` provinces beyond that land (counted in ways crossed,
//!   the fewest it takes), losing `carry_loss` of the food a day of the way; beyond, it forages where it stands, taking `forage_take` times what it eats. On an enemy's
//!   land foragers take before the people there (the highest priority); elsewhere they share a
//!   shortage alike. Outside its own land a company loses `camp_disease` of its soldiers a month
//!   to camp disease. `army.supply` says how each is fed: 0 at home, 1 along a line, 2 foraging.
//! - **Winter** (`modules.warfare.seasons`): from `start_lat` from the equator, full
//!   `full_span` further, deepest at `deepest` of the year in the north and half a year later in
//!   the south, over half the year (the cosine's warm half is no winter). In the deepest winter
//!   marches take `road_slow` times as long, soldiers outside their own land die at `deaths` a
//!   month, foragers find `forage_loss` less, and no new attack is begun on a province whose winter
//!   is above `no_attack`.
//! - **Pay** (`modules.warfare.pay`), where the world keeps coins: each month a realm pays every
//!   soldier, guards too, `wage` coins from its treasury into the soldier's home province; of the
//!   soldiers left unpaid, `unpaid_desert` desert home. Under Economics v0, which keeps no coins, it
//!   does nothing.
//! - **Sieges** (`modules.warfare.sieges`): a province enemies besiege (`province.besieger`)
//!   raises `raising` of its usual soldiers (none, as shipped), loses `strip` of its food store a
//!   month to the besiegers, and once its food is under `starving_days` of its people's eating,
//!   each day of the siege counts `starving_speed` days. Trade with it is cut by
//!   `modules.markets.trade.siege_stops`. Where the world keeps coins, the realm that takes it by
//!   siege carries `plunder` of its coins into its treasury. Besiegers on enemy land eat before
//!   the people there.
//!
//! - **Massing** (`modules.warfare.massing`), a fifth part, not in "campaigns" yet: a group the
//!   general gathers against a target sets off as soon as the companies already gathered outmatch
//!   the defence, instead of waiting for every member to march in (those still on their way leave
//!   the group, free for the general); a group that hasn't set off `join_days` after it was formed
//!   is broken up and planned again; and no group takes more than `mass_share` of its realm's free
//!   soldiers (the pool and the group's own).
//!
//! The code is in [`crate::warfare`]'s `Troops` (its `impl` here), so the army fields stay with
//! their owner.

use crate::diplomacy;
use crate::fields::{known, Quantity};
use crate::grid::NONE;
use crate::module::Ctx;
use crate::requests::{Cause, Dest, Priority, Request};
use crate::rng::{purpose, round_stochastic, Module as Rng};
use crate::warfare::{allowed_for, Troops};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// Supply lines and foraging (`modules.warfare.supply`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SupplyParams {
    /// Companies at most this many provinces beyond their realm's own land (ways crossed, the
    /// fewest it takes) are fed along a supply line from home; further out they forage.
    pub reach_ways: u32,
    /// Share of the food carried along a supply line lost for each day of the way.
    pub carry_loss: f64,
    /// Foragers take this many times what they eat from the land they stand in.
    pub forage_take: f64,
    /// Share of a company's soldiers outside its own land who die of camp disease a month.
    pub camp_disease: f64,
}

impl Default for SupplyParams {
    fn default() -> Self {
        SupplyParams { reach_ways: 2, carry_loss: 0.02, forage_take: 2.0, camp_disease: 0.01 }
    }
}

/// Winter (`modules.warfare.seasons`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeasonParams {
    /// Degrees from the equator where winter begins.
    pub start_lat: f64,
    /// Degrees further where winter is at its full.
    pub full_span: f64,
    /// When in the year winter is deepest in the north, as a share of the year (half a year later
    /// in the south).
    pub deepest: f64,
    /// In the deepest winter, marches take this many times as long.
    pub road_slow: f64,
    /// In the deepest winter, the share of soldiers outside their own land who die a month.
    pub deaths: f64,
    /// In the deepest winter, foragers find this share less.
    pub forage_loss: f64,
    /// No new attack is begun on a province whose winter is above this (0 none, 1 the deepest).
    pub no_attack: f64,
}

impl Default for SeasonParams {
    fn default() -> Self {
        SeasonParams { start_lat: 25.0, full_span: 30.0, deepest: 0.05, road_slow: 2.0, deaths: 0.03, forage_loss: 0.75, no_attack: 0.6 }
    }
}

/// Soldiers' pay, where the world keeps coins (`modules.warfare.pay`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PayParams {
    /// Coins a realm pays each soldier a month, guards too, into the soldier's home province.
    pub wage: f64,
    /// Share of the soldiers left unpaid in a month who desert home.
    pub unpaid_desert: f64,
}

impl Default for PayParams {
    fn default() -> Self {
        PayParams { wage: 1.0, unpaid_desert: 0.25 }
    }
}

/// Several armies acting together (`modules.warfare.massing`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MassingParams {
    /// A group that hasn't set off this many days after it was formed is broken up.
    pub join_days: u32,
    /// No group takes more than this share of its realm's free soldiers.
    pub mass_share: f64,
}

impl Default for MassingParams {
    fn default() -> Self {
        MassingParams { join_days: 60, mass_share: 0.6 }
    }
}

/// Sieges in depth (`modules.warfare.sieges`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SiegeParams {
    /// A besieged province raises this share of the soldiers it would otherwise raise.
    pub raising: f64,
    /// Share of a besieged province's food store the besiegers strip a month.
    pub strip: f64,
    /// A besieged province with food for fewer than this many days of its people's eating is
    /// starving...
    pub starving_days: f64,
    /// ...and each day of the siege counts this many.
    pub starving_speed: f64,
    /// Where the world keeps coins: the share of a province's coins the realm that takes it by
    /// siege carries into its treasury.
    pub plunder: f64,
}

impl Default for SiegeParams {
    fn default() -> Self {
        SiegeParams { raising: 0.0, strip: 0.25, starving_days: 30.0, starving_speed: 3.0, plunder: 0.2 }
    }
}

fn share(x: f64) -> bool {
    (0.0..=1.0).contains(&x)
}

/// Check the parts switched on.
pub(crate) fn check(sup: Option<&SupplyParams>, sea: Option<&SeasonParams>, pay: Option<&PayParams>, sie: Option<&SiegeParams>, mass: Option<&MassingParams>) -> Result<(), String> {
    if let Some(m) = mass
        && !(m.mass_share > 0.0 && m.mass_share <= 1.0)
    {
        return Err("massing: a group takes more than none and at most all of the free soldiers (above 0, up to 1)".into());
    }
    if let Some(s) = sup {
        if !(share(s.carry_loss) && share(s.camp_disease)) {
            return Err("supply: the food lost a day and camp disease go from 0 to 1".into());
        }
        if !(s.forage_take.is_finite() && s.forage_take >= 1.0) {
            return Err("supply: foragers take at least what they eat (1 or more times)".into());
        }
    }
    if let Some(s) = sea {
        if !((0.0..=90.0).contains(&s.start_lat) && s.full_span.is_finite() && s.full_span > 0.0) {
            return Err("winter: it begins from 0 to 90 degrees from the equator, and is full more than 0 degrees further".into());
        }
        if !(share(s.deepest) && share(s.deaths) && share(s.forage_loss) && share(s.no_attack)) {
            return Err("winter: the deepest time of year, deaths, foraging lost and the line for attacks go from 0 to 1".into());
        }
        if !(s.road_slow.is_finite() && s.road_slow >= 1.0) {
            return Err("winter: marches take at least as long (1 or more times)".into());
        }
    }
    if let Some(p) = pay
        && !(p.wage.is_finite() && p.wage >= 0.0 && share(p.unpaid_desert))
    {
        return Err("pay: the wage is zero coins or more, and deserting goes from 0 to 1".into());
    }
    if let Some(s) = sie {
        if !(share(s.raising) && share(s.strip) && share(s.plunder)) {
            return Err("sieges: raising, stripping and plunder go from 0 to 1".into());
        }
        if !(s.starving_days.is_finite() && s.starving_days >= 0.0 && s.starving_speed.is_finite() && s.starving_speed >= 1.0) {
            return Err("sieges: starving is zero days or more, and a starving siege goes at least as fast (1 or more times)".into());
        }
    }
    Ok(())
}

/// How deep the winter is, from 0 (none) to 1, at latitude `lat` in degrees (the provinces keep
/// theirs in radians) and `year_part` of the
/// way through the year.
pub fn winter(p: &SeasonParams, lat: f32, year_part: f64) -> f64 {
    let far = ((lat.abs() as f64 - p.start_lat) / p.full_span).clamp(0.0, 1.0);
    if far == 0.0 {
        return 0.0;
    }
    let deepest = if lat >= 0.0 { p.deepest } else { p.deepest + 0.5 };
    far * (std::f64::consts::TAU * (year_part - deepest)).cos().max(0.0)
}

/// The share of the year gone by on `day`.
pub(crate) fn year_part(cal: &crate::calendar::Calendar, day: u64) -> f64 {
    let start = cal.year_start(cal.date(day).year);
    (day - start) as f64 / cal.days_per_year() as f64
}

/// What logistics keeps between days, beside Warfare's own state (saved after it, and only when
/// not empty, so a world with the parts off saves exactly what it did before).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct More {
    /// Food asked for companies fed other than where they stand alike with the people there:
    /// by the request's tag, the companies and their soldiers then.
    pub eating: BTreeMap<u64, Vec<(u32, u64)>>,
    /// Wages asked, by realm << 32 | home province: the companies and their soldiers then.
    pub paying: BTreeMap<u64, Vec<(u32, u64)>>,
    /// Massing: the day each group still gathering was formed, by group number.
    pub formed: BTreeMap<u32, u64>,
}

/// How a request for food was made, in a tag's high bits (the low ones hold a province).
const STANDING: u64 = 0;
/// Foraging, or besieging, on an enemy's land: before the people there.
const ENEMY: u64 = 1;
/// Along a supply line, from home.
const LINE: u64 = 2;
/// Foraging on land nobody owns or a neutral's: alike with the people there.
const FORAGE: u64 = 3;
/// Stores stripped by besiegers (the grant is not waited for).
const STRIP: u64 = 4;

fn tag(how: u64, province: u32) -> u64 {
    how << 32 | province as u64
}

/// How far beyond land realm `r` holds (that no enemy occupies) each province is, through land it
/// may pass (see [`allowed_for`]): the fewest ways crossed, up to `limit`, and the days of the
/// quickest such way, which supplies are carried along. Provinces further, or not reached, are
/// left out.
fn reach(w: &World, r: u32, foes: &[u32], own: &[u32], limit: u32) -> BTreeMap<u32, f64> {
    let n = w.provinces.count;
    // Fewest ways first, then fewest days (thousandths).
    let mut dist: BTreeMap<u32, (u32, u64)> = BTreeMap::new();
    let mut heap = BinaryHeap::new();
    for &p in own {
        if (p as usize) < n {
            dist.insert(p, (0, 0));
            heap.push(Reverse(((0u32, 0u64), p)));
        }
    }
    crate::warfare::work::add(crate::warfare::work::SEARCHES, 1);
    while let Some(Reverse((d, p))) = heap.pop() {
        if d > dist[&p] {
            continue;
        }
        crate::warfare::work::add(crate::warfare::work::SETTLED, 1);
        for l in w.routes.of(p) {
            let Some(days) = l.land_days else { continue };
            let q = l.other(p);
            if !allowed_for(w, r, foes, q) {
                continue;
            }
            let nd = (d.0 + 1, d.1 + ((days as f64 * 1000.0).round() as u64).max(1));
            if nd.0 <= limit && dist.get(&q).is_none_or(|&x| nd < x) {
                dist.insert(q, nd);
                heap.push(Reverse((nd, q)));
            }
        }
    }
    dist.into_iter().map(|(p, d)| (p, d.1 as f64 / 1000.0)).collect()
}

impl Troops {
    /// Whether any part of logistics changes the month's eating.
    pub(crate) fn campaigning(&self) -> bool {
        self.params.supply.is_some() || self.params.seasons.is_some() || self.params.sieges.is_some()
    }

    /// How deep the winter is in province `p` today (0 with winter off).
    pub(crate) fn winter_at(&self, w: &World, cal: &crate::calendar::Calendar, day: u64, p: u32) -> f64 {
        match (&self.params.seasons, w.provinces.lat.get(p as usize)) {
            (Some(s), Some(&lat)) => winter(s, lat.to_degrees(), year_part(cal, day)),
            _ => 0.0,
        }
    }

    /// The month's eating with logistics on, in place of everyone eating where they stand: deaths
    /// on campaign first, then food asked by how each company is fed; and the besieged
    /// provinces' stores stripped.
    pub(crate) fn eat_campaigning(&mut self, w: &mut World, cx: &mut Ctx) {
        let days = cx.calendar.def.months[cx.calendar.date(cx.day).month].days as u64;
        let stock = known("province.stock");
        let food = cx.goods.tree.find("food").expect("checked at configuration: the goods tree has food");
        let enemies = diplomacy::enemies(w);
        let supply = self.params.supply.clone();
        let seasons = self.params.seasons.clone();
        let (cal, day, seed) = (cx.calendar, cx.day, cx.seed);
        let part = year_part(cal, day);
        let winter_at = |w: &World, p: u32| match (&seasons, w.provinces.lat.get(p as usize)) {
            (Some(s), Some(&lat)) => winter(s, lat.to_degrees(), part),
            _ => 0.0,
        };
        self.st.eating.clear();
        self.more.eating.clear();
        // Each realm's own land (held, no enemy in it), for supply lines.
        let mut own: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        if supply.is_some() {
            for q in 0..w.provinces.count {
                if w.owner[q] != NONE && w.occupier[q] == NONE {
                    own.entry(w.owner[q]).or_default().push(q as u32);
                }
            }
        }
        let mut lines: BTreeMap<u32, BTreeMap<u32, f64>> = BTreeMap::new();
        // Extra food asked beyond the soldiers' own: for the carrying, by home, and for foraging,
        // by the tag of the request it goes with.
        let mut carried: BTreeMap<u32, f64> = BTreeMap::new();
        let mut foraged: BTreeMap<u64, f64> = BTreeMap::new();
        let mut asked: BTreeMap<u64, f64> = BTreeMap::new();
        for a in 0..w.armies.count() {
            let size = w.armies.size[a];
            if size == 0 {
                continue;
            }
            let (realm, at, home) = (w.armies.realm[a], w.armies.province[a], w.armies.home[a]);
            let owner = w.owner.get(at as usize).copied().unwrap_or(NONE);
            let at_home_land = owner == realm && w.occupier[at as usize] == NONE;
            let foes = enemies.get(realm as usize).map_or(&[][..], |v| &v[..]);
            let enemy_land = owner != NONE && foes.binary_search(&owner).is_ok();
            // Deaths on campaign, outside its own land: camp disease, and winter.
            let mut size = size;
            if !at_home_land {
                let rate = supply.as_ref().map_or(0.0, |s| s.camp_disease) + seasons.as_ref().map_or(0.0, |s| s.deaths) * winter_at(w, at);
                let dead = round_stochastic(size as f64 * rate * days as f64 / 30.0, seed, Rng::Warfare, day, a as u64, purpose("attrition")).min(size);
                if dead > 0 {
                    w.armies.size[a] -= dead;
                    cx.flow(Quantity::People, Cause::Attrition, -(dead as i64));
                    size -= dead;
                    if cx.traced(at) || cx.traced(home) {
                        cx.note(format!("company {a}: {dead} soldiers died of camp disease and winter, away from their own land"));
                    }
                    self.settle(w, a as u32);
                    if size == 0 {
                        continue;
                    }
                }
            }
            let ration = (size * days) as f64;
            let (how, from, status) = if at_home_land {
                (STANDING, at, 0u8)
            } else if let Some(s) = &supply {
                let line = lines.entry(realm).or_insert_with(|| reach(w, realm, foes, own.get(&realm).map_or(&[][..], |v| &v[..]), s.reach_ways));
                match line.get(&at) {
                    Some(&d) => {
                        // What arrives after `d` days of losing `carry_loss` a day.
                        let kept = (1.0 - s.carry_loss).powf(d).max(1e-6);
                        *carried.entry(home).or_default() += ration * (1.0 / kept - 1.0);
                        (LINE, home, 1)
                    }
                    None => {
                        let how = if enemy_land { ENEMY } else { FORAGE };
                        let found = 1.0 - seasons.as_ref().map_or(0.0, |s| s.forage_loss) * winter_at(w, at);
                        *foraged.entry(tag(how, at)).or_default() += ration * found * (s.forage_take - 1.0);
                        *asked.entry(tag(how, at)).or_default() += ration * found;
                        (how, at, 2)
                    }
                }
            } else if self.params.sieges.is_some() && enemy_land && w.besieger.get(at as usize) == Some(&realm) {
                (ENEMY, at, 0)
            } else {
                (STANDING, at, 0)
            };
            if supply.is_some() {
                w.armies.supply[a] = status;
            }
            if how == STANDING {
                self.st.eating.entry(from).or_default().push((a as u32, size));
            } else {
                self.more.eating.entry(tag(how, from)).or_default().push((a as u32, size));
            }
        }
        for (&at, companies) in &self.st.eating {
            let soldiers: u64 = companies.iter().map(|c| c.1).sum();
            cx.ask_good(stock, at, food, soldiers * days, Priority::Essential, Cause::Eating, at as u64);
        }
        for (&t, companies) in &self.more.eating {
            let (how, at) = (t >> 32, (t & 0xFFFF_FFFF) as u32);
            let soldiers: u64 = companies.iter().map(|c| c.1).sum();
            let take = match asked.get(&t) {
                Some(&x) => x.round() as u64,
                None => soldiers * days,
            };
            let priority = if how == ENEMY { Priority::Critical } else { Priority::Essential };
            cx.ask_good(stock, at, food, take, priority, Cause::Eating, t);
            if let Some(&extra) = foraged.get(&t) {
                let extra = extra.round() as u64;
                if extra > 0 {
                    cx.ask_good(stock, at, food, extra, priority, Cause::Foraging, t);
                }
            }
        }
        for (&home, &extra) in &carried {
            let extra = extra.round() as u64;
            if extra > 0 {
                cx.ask_good(stock, home, food, extra, Priority::Essential, Cause::Spoilage, tag(LINE, home));
            }
        }
        // Besiegers strip the stores of the provinces they besiege on enemy land.
        if let Some(s) = self.params.sieges.as_ref().filter(|s| s.strip > 0.0) {
            let cols = cx.goods.columns_under(food).unwrap_or_default();
            for p in 0..w.provinces.count.min(w.besieger.len()) {
                let by = w.besieger[p];
                if by == NONE || !diplomacy::at_war(w, by, w.owner[p]) {
                    continue;
                }
                let store: u64 = cols.iter().map(|&c| w.stock[cx.goods.index(p, c)]).sum();
                let take = (store as f64 * s.strip).round() as u64;
                if take > 0 {
                    cx.ask_good(stock, p as u32, food, take, Priority::Critical, Cause::Foraging, tag(STRIP, p as u32));
                    if cx.traced(p as u32) {
                        cx.note(format!("realm {by}'s besiegers strip {take} of its {store} food"));
                    }
                }
            }
        }
    }

    /// A month's food came for companies fed along a supply line or foraging (`r.tag`): each gets
    /// the same share of a full month's food. As where they stand: deaths, the hungry desert.
    pub(crate) fn fed_campaigning(&mut self, w: &mut World, cx: &mut Ctx, r: &Request, granted: u64) {
        let Some(companies) = self.more.eating.remove(&r.tag) else { return };
        let days = cx.calendar.def.months[cx.calendar.date(r.day).month].days as u64;
        let full: u64 = companies.iter().map(|c| c.1).sum::<u64>() * days;
        let fed = if full == 0 { 1.0 } else { (granted as f64 / full as f64).min(1.0) };
        self.feed(w, cx, r, companies, fed);
    }

    /// The month's wages: every company's soldiers, by realm and home province.
    pub(crate) fn pay(&mut self, w: &World, cx: &mut Ctx) {
        let Some(p) = self.params.pay.clone() else { return };
        self.more.paying.clear();
        if !self.coins || p.wage <= 0.0 {
            return;
        }
        for a in 0..w.armies.count() {
            let (size, realm) = (w.armies.size[a], w.armies.realm[a]);
            if size > 0 && w.realms.alive(realm) {
                self.more.paying.entry((realm as u64) << 32 | w.armies.home[a] as u64).or_default().push((a as u32, size));
            }
        }
        let money = cx.goods.tree.find(&self.params.money).expect("checked when the world was made");
        for (&key, companies) in &self.more.paying {
            let (realm, home) = ((key >> 32) as u32, (key & 0xFFFF_FFFF) as u32);
            let soldiers: u64 = companies.iter().map(|c| c.1).sum();
            let due = (p.wage * soldiers as f64).round() as u64;
            if due > 0 {
                let to = Dest { field: known("province.stock"), entity: home };
                cx.ask_move(known("realm.stock"), realm, Some(money), due, to, Priority::Normal, Cause::Wages, key);
            }
        }
    }

    /// Wages came, in full or not: of the soldiers left unpaid, a share desert home.
    pub(crate) fn paid(&mut self, w: &mut World, cx: &mut Ctx, r: &Request, granted: u64) {
        let Some(companies) = self.more.paying.remove(&r.tag) else { return };
        let Some(p) = self.params.pay.clone() else { return };
        let unpaid = if r.take == 0 { 0.0 } else { 1.0 - granted as f64 / r.take as f64 };
        if unpaid <= 0.0 {
            return;
        }
        for (a, then) in companies {
            let size = w.armies.size[a as usize];
            let leave = round_stochastic(then.min(size) as f64 * unpaid * p.unpaid_desert, cx.seed, Rng::Warfare, r.day, a as u64, purpose("unpaid")).min(size);
            if leave > 0 {
                let home = w.armies.home[a as usize];
                if cx.traced(home) || cx.traced(w.armies.province[a as usize]) {
                    cx.note(format!("company {a}: {:.0}% of its wages unpaid, {leave} soldiers desert home", 100.0 * unpaid));
                }
                self.go_home(w, cx, a, leave, Cause::Deserted);
            }
        }
    }

    /// Siege days a besieged province's siege counts today: `starving_speed` once its food is
    /// under `starving_days` of its people's eating (`people`), else one.
    pub(crate) fn siege_step(&self, w: &World, cx: &Ctx, p: u32, people: u64) -> u32 {
        let Some(s) = &self.params.sieges else { return 1 };
        let Some(food) = cx.goods.tree.find("food") else { return 1 };
        let store: u64 = cx.goods.columns_under(food).unwrap_or_default().iter().map(|&c| w.stock.get(cx.goods.index(p as usize, c)).copied().unwrap_or(0)).sum();
        if people > 0 && (store as f64) < s.starving_days * people as f64 {
            round_stochastic(s.starving_speed, cx.seed, Rng::Warfare, cx.day, p as u64, purpose("starving")) as u32
        } else {
            1
        }
    }

    /// Realm `by` took province `p` by siege: it carries off a share of the coins there.
    pub(crate) fn plunder(&self, w: &World, cx: &mut Ctx, p: u32, by: u32) {
        let Some(s) = self.params.sieges.as_ref().filter(|s| s.plunder > 0.0) else { return };
        if !self.coins || !w.realms.alive(by) {
            return;
        }
        let Some(money) = cx.goods.tree.find(&self.params.money) else { return };
        let coins: u64 = cx.goods.columns_under(money).unwrap_or_default().iter().map(|&c| w.stock.get(cx.goods.index(p as usize, c)).copied().unwrap_or(0)).sum();
        let take = (coins as f64 * s.plunder).floor() as u64;
        if take > 0 {
            let to = Dest { field: known("realm.stock"), entity: by };
            cx.ask_move(known("province.stock"), p, Some(money), take, to, Priority::Normal, Cause::Plunder, p as u64);
            if cx.traced(p) {
                cx.note(format!("realm {by} carries off {take} of its {coins} coins"));
            }
        }
    }

}
