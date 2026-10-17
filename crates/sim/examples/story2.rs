//! Story 2, the rich river city, as the checks piece of step 7 words it: markets, a world from a
//! seed, no pokes, 150 years. A "rich river city" is a coastal province at a big river's mouth
//! that stays among the richest 2% by coins for 10 years or more. The story passes on a seed if
//! such a province is occupied in a war and changes hands at the peace, and later its coins fall
//! below half their peak while the trade through it halves.
//!
//! Story 1's world (200 provinces) with every step-7 module on: realms with unrest, rulers,
//! armies and Diplomacy "treaties". Prints, for each river-mouth province that was a rich river
//! city: its years among the richest 2%, its peak, the days occupied, the peaces at which it
//! changed hands, and when its coins and trade fell to half; then, for comparison, how many of the
//! richest 2% (any province) changed hands at a peace.
//!
//! A "big river" is one the map draws ([`RIVER_MIN_FLOW`], the line `province.river_km` counts
//! too): the project's own meaning of the words, not tuned for the story. The city counts as taken
//! when it changes hands at a peace to the realm that occupied it in that war, and its decline is
//! looked for from the year it changed hands on.
//!
//! cargo run --release -p sim --example story2 -- [years] [seed ...] [KEY=VALUE ...] [campaigns]
//!
//! `KEY=VALUE` changes a setting (keys as `simrun --settings` lists them), to try other worlds.
//! `campaigns` switches on Warfare "campaigns" (sieges in depth, the default for new worlds);
//! `supply`, `seasons`, `pay`, `sieges` and `massing` switch on one part each. With none, armies
//! are plain "troops". `culture` and `research` switch those modules on (both off without them).
//!
//! The probe (logistics piece 1, reads only, so the world is the same): for every rich river city,
//! month by month while a realm is at war with its owner, what stands in the way of that realm
//! taking it, as the general sees it (see `warfare.rs`, `plan`):
//! - **unreachable**: the realm holds or occupies no province next to it by land, so the general
//!   never makes it a target (a city behind a front that doesn't move);
//! - **room short**: the provinces next to it where an army may gather take at most the gathering
//!   share of their people, and that many soldiers can't outmatch the garrison;
//! - **soldiers short**: the realm hasn't that much strength in all its field companies;
//! - **could**: neither: an attack could be gathered;
//! - **gathering**, **attacking**, **battle**, **besieged**, **held**: a group of the realm gathering
//!   next to it, marching on it, fighting in it, besieging it, or holding it.
//!
//! And: sieges begun, the best share of a siege's days reached, sieges cut off by a peace,
//! recruits the city raised while besieged, battles in it, and the garrison against the room.
//!
//! And each group the generals make (its target read from Warfare's saved state), from its first
//! member joining until it sets off or breaks up: its days waiting while members still march in,
//! while all are there but too weak or too few for the target, or strong enough (waiting for
//! winter or ships); and why it ended: set off, target gone (taken, freed, peace), no front left
//! to gather on, its members all lost, or broken up and planned again with its target still there.

use sim::config::Config;
use sim::diplomacy::{TreatyParams, WarParams};
use sim::events::Event;
use sim::grid::NONE;
use sim::module::Slot;
use sim::sim::Sim;
use sim::warfare::{TroopParams, FIELD};
use sim::world::{World, WorldParams, RIVER_MIN_FLOW};
use std::collections::{BTreeMap, BTreeSet};

fn config(seed: u64, set: &[(String, f64)], parts: &[String]) -> Config {
    let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed, land_fraction: 0.36, radius_km: 6371.0 });
    c.modules.without_realms();
    c.modules.set_choice(Slot::Economics, "markets").unwrap();
    c.modules.realms = Some(Default::default());
    c.modules.characters = Some(Default::default());
    c.modules.diplomacy = Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() });
    c.modules.warfare = Some(Default::default());
    let t = c.modules.warfare.as_mut().unwrap();
    let on = |p: &str| parts.iter().any(|x| x == p || (x == "campaigns" && p == "sieges"));
    if on("supply") {
        t.supply = Some(Default::default());
    }
    if on("seasons") {
        t.seasons = Some(Default::default());
    }
    if on("pay") {
        t.pay = Some(Default::default());
    }
    if on("sieges") {
        t.sieges = Some(Default::default());
    }
    if on("massing") {
        t.massing = Some(Default::default());
    }
    if on("culture") {
        c.modules.culture = Some(Default::default());
    }
    if on("research") {
        c.modules.research = Some(Default::default());
    }
    for (k, v) in set {
        sim::settings::set(&mut c, k, *v).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    c
}

fn coins(w: &World, p: usize) -> u64 {
    let g = &w.goods;
    g.columns_under(g.tree.find("money").unwrap()).unwrap().iter().map(|&c| w.stock[g.index(p, c)]).sum()
}

/// Each coastal province where a big river (one the map draws) reaches the sea, with the most
/// water reaching the sea there, in big rivers' worth.
fn mouths(w: &World) -> Vec<(usize, f64)> {
    let mut best = vec![0u64; w.provinces.count];
    for c in 0..w.elevation.len() {
        let p = w.provinces.of_cell[c];
        if p == NONE || w.elevation[c] < 0.0 || w.river_flow[c] < RIVER_MIN_FLOW {
            continue;
        }
        let d = w.downstream[c];
        let to_sea = d == NONE || w.elevation[d as usize] < 0.0;
        if to_sea {
            best[p as usize] = best[p as usize].max(w.river_flow[c]);
        }
    }
    (0..w.provinces.count).filter(|&p| best[p] > 0 && w.provinces.coast_km[p] > 0.0).map(|p| (p, best[p] as f64 / RIVER_MIN_FLOW as f64)).collect()
}

#[derive(Clone, Default)]
struct Track {
    /// Years among the richest 2%, in a row now, and the most in a row.
    run: u32,
    longest: u32,
    /// The highest coins at a year's end, and the year's trade then.
    peak: u64,
    peak_trade: f64,
    occupied_days: u64,
    /// Years it ended as its realm's capital, and as a target of a war.
    capital_years: u32,
    targeted: u32,
    /// Years it changed hands at a peace.
    handed: Vec<i64>,
    /// The first year after a hand-over that its coins were under half the peak, and its trade
    /// under half the trade at the peak.
    poorer: Option<i64>,
    less_trade: Option<i64>,
}

/// What stood in the way of a realm at war with a river city's owner taking it, month by month
/// (see the top of the file).
#[derive(Clone, Default)]
struct Probe {
    unreachable: u32,
    room_short: u32,
    soldiers_short: u32,
    could: u32,
    gathering: u32,
    attacking: u32,
    battle: u32,
    besieged: u32,
    held: u32,
    /// Months a war named it as its target, and of those, months it was unreachable; and those
    /// months by what stood in the way, in the order of [`STATUS`].
    target_months: u32,
    target_unreachable: u32,
    target_status: [u32; 9],
    /// Months reachable: the garrison's strength the general must outmatch (its edge counted)
    /// over what the room to gather can hold (soldiers × the realm's strength a soldier).
    need_over_room: Vec<f64>,
    sieges: u32,
    /// The most of a siege's days reached, as a share of the days it needs.
    best_progress: f64,
    /// Days each siege needed, when it began.
    siege_need: Vec<u32>,
    /// Sieges going on when a peace ended the war.
    cut_by_peace: u32,
    raised_under_siege: u64,
    battles_held: u32,
    battles_lost: u32,
}

/// What stood in the way, in the order `Probe::target_status` counts it.
const STATUS: [&str; 9] = ["unreachable", "room short", "soldiers short", "could", "gathering", "attacking", "battle", "besieged", "held"];

/// The troop kinds' strength and whether ships, by node.
struct Kit {
    p: TroopParams,
    strength: Vec<f64>,
    ship: Vec<bool>,
}

impl Kit {
    fn new(p: &TroopParams) -> Kit {
        let kinds = sim::warfare::kinds(p);
        let nodes = kinds.iter().map(|k| k.0 as usize + 1).max().unwrap_or(0);
        let (mut strength, mut ship) = (vec![0.0; nodes], vec![false; nodes]);
        for (node, name, is_ship, _) in kinds {
            strength[node as usize] = p.kinds[&name].strength;
            ship[node as usize] = is_ship;
        }
        Kit { p: p.clone(), strength, ship }
    }
}

/// Days a siege of province `p` needs, as Warfare works it out.
fn siege_need(w: &World, k: &Kit, p: usize, people: &[u64]) -> u32 {
    let mut v: Vec<u64> = people.iter().copied().filter(|&x| x > 0).collect();
    let middle = if v.is_empty() {
        1.0
    } else {
        let m = v.len() / 2;
        (*v.select_nth_unstable(m).1).max(1) as f64
    };
    let mut need = k.p.siege_base_days * (1.0 + people[p] as f64 / middle);
    let o = w.owner[p];
    if o != NONE && w.realms.capital.get(o as usize) == Some(&(p as u32)) {
        need *= k.p.capital_siege;
    }
    (need.min(k.p.siege_max_days).round() as u32).max(1)
}

/// The month's look at each river city: for every realm at war with its owner, what stands in the
/// way of its taking it.
fn probe_month(w: &World, k: &Kit, mouth: &[(usize, f64)], probe: &mut [Probe]) {
    let people = w.province_population();
    let enemies = sim::diplomacy::enemies(w);
    let arm = &w.armies;
    let mut standing: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for a in 0..arm.count() {
        if arm.size[a] > 0 && arm.next[a] == NONE && !k.ship[arm.kind[a] as usize] {
            standing.entry(arm.province[a]).or_default().push(a);
        }
    }
    let strength_of = |of: &[usize]| -> f64 { of.iter().map(|&a| arm.size[a] as f64 * k.strength[arm.kind[a] as usize]).sum() };
    for &(p, _) in mouth {
        let o = w.owner[p];
        if o == NONE || !w.realms.alive(o) {
            continue;
        }
        for &x in &enemies[o as usize] {
            let st = &mut probe[p];
            let foes = &enemies[x as usize];
            let targeted = (0..w.wars.count() as u32).any(|j| w.wars.ongoing(j) && w.wars.attacker[j as usize] == x && w.wars.target.get(j as usize) == Some(&(p as u32)));
            st.target_months += targeted as u32;
            let mark = |st: &mut Probe, k: usize| {
                if targeted {
                    st.target_status[k] += 1;
                }
            };
            if w.occupier[p] == x {
                st.held += 1;
                mark(st, 8);
                continue;
            }
            let here = standing.get(&(p as u32)).cloned().unwrap_or_default();
            let (mine, theirs): (Vec<usize>, Vec<usize>) = here.iter().partition(|&&a| arm.realm[a] == x);
            let theirs: Vec<usize> = theirs.into_iter().filter(|&a| foes.binary_search(&arm.realm[a]).is_ok()).collect();
            if mine.iter().any(|&a| arm.role[a] == FIELD) {
                if theirs.is_empty() {
                    st.besieged += 1;
                    mark(st, 7);
                } else {
                    st.battle += 1;
                    mark(st, 6);
                }
                continue;
            }
            if (0..arm.count()).any(|a| arm.realm[a] == x && arm.size[a] > 0 && arm.next[a] != NONE && arm.dest[a] == p as u32 && arm.group[a] != NONE) {
                st.attacking += 1;
                mark(st, 5);
                continue;
            }
            let staging = |q: usize| (w.owner[q] == x && w.occupier[q] == NONE) || w.occupier[q] == x;
            let gather: Vec<u32> = w
                .routes
                .of(p as u32)
                .filter(|l| l.land_days.is_some())
                .map(|l| l.other(p as u32))
                .filter(|&q| staging(q as usize) && !standing.get(&q).is_some_and(|v| v.iter().any(|&a| foes.binary_search(&arm.realm[a]).is_ok())))
                .collect();
            if gather.is_empty() {
                st.unreachable += 1;
                st.target_unreachable += targeted as u32;
                mark(st, 0);
                continue;
            }
            let gathering = (0..arm.count()).any(|a| arm.realm[a] == x && arm.size[a] > 0 && arm.group[a] != NONE && (gather.contains(&arm.province[a]) || gather.contains(&arm.dest[a])));
            let need = k.p.outmatch * (1.0 + k.p.home_edge) * strength_of(&theirs);
            let room: f64 = gather.iter().map(|&q| (k.p.gather_share * people[q as usize] as f64).floor()).sum();
            let field: Vec<usize> = (0..arm.count()).filter(|&a| arm.realm[a] == x && arm.role[a] == FIELD && arm.size[a] > 0 && !k.ship[arm.kind[a] as usize]).collect();
            let (soldiers, power) = (field.iter().map(|&a| arm.size[a]).sum::<u64>(), strength_of(&field));
            let each = if soldiers > 0 { power / soldiers as f64 } else { 1.0 };
            st.need_over_room.push(need / (room * each).max(1.0));
            if gathering {
                st.gathering += 1;
                mark(st, 4);
            } else if room * each < need {
                st.room_short += 1;
                mark(st, 1);
            } else if power < need {
                st.soldiers_short += 1;
                mark(st, 2);
            } else {
                st.could += 1;
                mark(st, 3);
            }
        }
    }
}

/// The day's look at sieges of river cities: (city, besieger) → the most siege days reached.
fn sieges_today(w: &World, mouth: &[(usize, f64)]) -> BTreeMap<(usize, u32), u32> {
    let arm = &w.armies;
    let is_mouth: BTreeSet<u32> = mouth.iter().map(|m| m.0 as u32).collect();
    let mut out = BTreeMap::new();
    for a in 0..arm.count() {
        let p = arm.province[a];
        if arm.size[a] > 0 && arm.next[a] == NONE && arm.role[a] == FIELD && arm.siege_days[a] > 0 && is_mouth.contains(&p) && w.owner[p as usize] != arm.realm[a] {
            let e = out.entry((p as usize, arm.realm[a])).or_insert(0);
            *e = (*e).max(arm.siege_days[a]);
        }
    }
    out
}

/// The soldiers of each river city's owner from that city.
fn garrison_raised(w: &World, p: usize) -> u64 {
    let arm = &w.armies;
    let o = w.owner[p];
    (0..arm.count()).filter(|&a| arm.home[a] == p as u32 && arm.realm[a] == o).map(|a| arm.size[a]).sum()
}

/// Why a group of the general's ended: set off for its target, or broke up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum End {
    /// Set off for its target.
    Launched,
    /// Its target was taken, freed, or no longer an enemy's (the war ended).
    TargetGone,
    /// No province of its realm next to the target was left to gather in.
    FrontLost,
    /// Its companies were all killed, sent home or deserted.
    MembersLost,
    /// Broken up with its target still there, its soldiers too weak for the enemies there now (or
    /// too few to besiege it): the general couldn't find enough, and planned again.
    ReplannedWeak,
    /// Broken up with its target still there and its soldiers strong enough: planned again.
    ReplannedOther,
    /// Still gathering when the run ended.
    Running,
}

/// A group's life, from its first member joining to its end.
#[derive(Clone)]
struct Life {
    realm: u32,
    target: u32,
    start: u64,
    /// Days some member was still marching in to gather; days all were there but too weak for the
    /// enemies in the target (or too few to besiege it); days all were there and strong enough.
    marching: u32,
    short: u32,
    enough: u32,
    end: Option<(u64, End)>,
    /// Its members the day before.
    members: Vec<usize>,
}

/// The groups in the world's Warfare state now (a save file, read).
fn groups_now(sim: &Sim) -> BTreeMap<u32, sim::warfare::GroupView> {
    let bytes = sim.save();
    let file: sim::save::SaveFile = postcard::from_bytes(&bytes[12..]).expect("a save");
    let st = file.snapshots[0].module_state.iter().find(|m| m.slot == Slot::Warfare).expect("armies are on");
    sim::warfare::groups_in_state(&st.bytes).expect("Warfare's state").into_iter().map(|g| (g.number, g)).collect()
}

/// The strength of enemies of realm `r` standing in province `t`, land troops, and of companies `of`.
fn strength_at(w: &World, k: &Kit, r: u32, t: u32, enemies: &[Vec<u32>]) -> f64 {
    let arm = &w.armies;
    let foes = &enemies[r as usize];
    (0..arm.count())
        .filter(|&a| arm.size[a] > 0 && arm.next[a] == NONE && arm.province[a] == t && !k.ship[arm.kind[a] as usize] && foes.binary_search(&arm.realm[a]).is_ok())
        .map(|a| arm.size[a] as f64 * k.strength[arm.kind[a] as usize])
        .sum()
}

/// A day's look at the general's groups: new ones (their targets read from Warfare's state), each
/// one's day, and the ones that ended.
fn groups_day(sim: &Sim, k: &Kit, lives: &mut BTreeMap<u32, Life>) {
    let w = &sim.world;
    let arm = &w.armies;
    let mut now: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for a in 0..arm.count() {
        if arm.size[a] > 0 && arm.group[a] != NONE {
            now.entry(arm.group[a]).or_default().push(a);
        }
    }
    if now.keys().any(|g| !lives.contains_key(g)) {
        let views = groups_now(sim);
        for g in now.keys() {
            if !lives.contains_key(g)
                && let Some(v) = views.get(g)
            {
                lives.insert(*g, Life { realm: v.realm, target: v.target, start: sim.day, marching: 0, short: 0, enough: 0, end: None, members: Vec::new() });
            }
        }
    }
    let enemies = sim::diplomacy::enemies(w);
    let people = w.province_population();
    for (g, life) in lives.iter_mut().filter(|(_, l)| l.end.is_none()) {
        let (r, t) = (life.realm, life.target);
        let Some(members) = now.get(g) else {
            // Gone without setting off: why.
            let (o, occ) = (w.owner[t as usize], w.occupier[t as usize]);
            let foes = enemies.get(r as usize).cloned().unwrap_or_default();
            let valid = if o == r { occ != NONE || strength_at(w, k, r, t, &enemies) > 0.0 } else { o != NONE && foes.binary_search(&o).is_ok() && occ != r };
            let staging = |q: u32| (w.owner[q as usize] == r && w.occupier[q as usize] == NONE) || w.occupier[q as usize] == r;
            let front = w.routes.of(t).any(|l| staging(l.other(t)));
            let lost = life.members.iter().all(|&a| arm.size[a] == 0 || arm.group[a] != *g);
            let why = if !valid {
                End::TargetGone
            } else if !front {
                End::FrontLost
            } else if lost && life.members.iter().all(|&a| arm.size[a] == 0) {
                End::MembersLost
            } else {
                let defending = o == r;
                let need = k.p.outmatch * if defending { 1.0 } else { 1.0 + k.p.home_edge } * strength_at(w, k, r, t, &enemies);
                let least = ((k.p.siege_min_share * people[t as usize] as f64).ceil() as u64).max(1);
                let land: Vec<usize> = life.members.iter().copied().filter(|&a| !k.ship[arm.kind[a] as usize]).collect();
                let have: f64 = land.iter().map(|&a| arm.size[a] as f64 * k.strength[arm.kind[a] as usize]).sum();
                let soldiers: u64 = land.iter().map(|&a| arm.size[a]).sum();
                if have < need || soldiers < least { End::ReplannedWeak } else { End::ReplannedOther }
            };
            life.end = Some((sim.day, why));
            continue;
        };
        life.members = members.clone();
        if members.iter().any(|&a| arm.next[a] != NONE && arm.dest[a] == t) {
            life.end = Some((sim.day, End::Launched));
            continue;
        }
        if members.iter().any(|&a| arm.next[a] != NONE) {
            life.marching += 1;
            continue;
        }
        let defending = w.owner[t as usize] == r;
        let need = k.p.outmatch * if defending { 1.0 } else { 1.0 + k.p.home_edge } * strength_at(w, k, r, t, &enemies);
        let least = ((k.p.siege_min_share * people[t as usize] as f64).ceil() as u64).max(1);
        let land: Vec<usize> = members.iter().copied().filter(|&a| !k.ship[arm.kind[a] as usize]).collect();
        let have: f64 = land.iter().map(|&a| arm.size[a] as f64 * k.strength[arm.kind[a] as usize]).sum();
        let soldiers: u64 = land.iter().map(|&a| arm.size[a]).sum();
        if have < need || soldiers < least {
            life.short += 1;
        } else {
            life.enough += 1;
        }
    }
}

/// The groups' lives summed up, for those aimed at `targets` (all groups if `None`).
fn groups_report(what: &str, lives: &BTreeMap<u32, Life>, end: u64, targets: Option<&BTreeSet<u32>>) {
    let these: Vec<&Life> = lives.values().filter(|l| targets.is_none_or(|t| t.contains(&l.target))).collect();
    let mut ends: BTreeMap<End, usize> = BTreeMap::new();
    let mut days: Vec<u64> = Vec::new();
    let (mut marching, mut short, mut enough) = (0u64, 0u64, 0u64);
    let mut short_at_replan = 0;
    for l in &these {
        let (stop, why) = l.end.unwrap_or((end, End::Running));
        *ends.entry(why).or_default() += 1;
        days.push(stop - l.start);
        marching += l.marching as u64;
        short += l.short as u64;
        enough += l.enough as u64;
        short_at_replan += (matches!(why, End::ReplannedWeak | End::ReplannedOther) && l.short > 0) as usize;
    }
    days.sort_unstable();
    let median = days.get(days.len() / 2).copied().unwrap_or(0);
    let mean = if days.is_empty() { 0.0 } else { days.iter().sum::<u64>() as f64 / days.len() as f64 };
    println!(
        "  groups {what}: {} groups; ended {ends:?}; days to the end: median {median}, mean {mean:.0}; days waiting: members still marching in {marching}, all there but too weak or too few {short}, all there and strong enough {enough}; replanned after waiting too weak {short_at_replan}",
        these.len()
    );
}

fn run(seed: u64, years: i64, set: &[(String, f64)], parts: &[String]) -> bool {
    let mut sim = Sim::new(&config(seed, set, parts)).unwrap();
    let kit = Kit::new(sim.settings.warfare.as_ref().expect("armies are on"));
    let start = sim.calendar.date(0).year;
    let n = sim.world.provinces.count;
    let mouth = mouths(&sim.world);
    let is_mouth: Vec<bool> = (0..n).map(|p| mouth.iter().any(|m| m.0 == p)).collect();
    let mut t = vec![Track::default(); n];
    let mut year_trade = vec![0.0f64; n];
    let mut last_trade = 0;
    let (mut rich_handed, mut handed_all) = (0usize, 0usize);
    let mut rich = vec![false; n];
    let mut probe = vec![Probe::default(); n];
    let mut last_sieges: BTreeMap<(usize, u32), u32> = BTreeMap::new();
    let mut last_garrison: BTreeMap<usize, u64> = BTreeMap::new();
    let mut lives: BTreeMap<u32, Life> = BTreeMap::new();
    for year in 1..=years {
        let end = sim.calendar.year_start(start + year);
        while sim.day < end {
            let (owners, occupiers) = (sim.world.owner.clone(), sim.world.occupier.clone());
            sim.step_day();
            let w = &sim.world;
            if w.trade.day != last_trade && !w.trade.shipments.is_empty() {
                last_trade = w.trade.day;
                for s in &w.trade.shipments {
                    year_trade[s.from as usize] += s.coins;
                    year_trade[s.to as usize] += s.coins;
                }
            }
            for p in 0..n {
                if is_mouth[p] && w.occupier[p] != NONE {
                    t[p].occupied_days += 1;
                }
            }
            // The probe: sieges begun, their progress and their end, recruits raised under siege,
            // battles in the city, and once a month what stands in the way.
            let today = sieges_today(w, &mouth);
            for (&(p, x), &days) in &today {
                let need = siege_need(w, &kit, p, &w.province_population());
                let pr = &mut probe[p];
                if !last_sieges.contains_key(&(p, x)) {
                    pr.sieges += 1;
                    pr.siege_need.push(need);
                }
                pr.best_progress = pr.best_progress.max(days as f64 / need as f64);
                let now = garrison_raised(w, p);
                if let Some(&was) = last_garrison.get(&p)
                    && now > was
                {
                    pr.raised_under_siege += now - was;
                }
            }
            last_garrison = today.keys().map(|&(p, _)| (p, garrison_raised(w, p))).collect();
            for e in sim.delivered_today() {
                match e.event {
                    Event::Peace { attacker, defender, .. } => {
                        for &(p, x) in last_sieges.keys() {
                            if (x == attacker && owners[p] == defender) || (x == defender && owners[p] == attacker) {
                                probe[p].cut_by_peace += 1;
                            }
                        }
                    }
                    Event::Battle { province, winner, loser, .. } if is_mouth[province as usize] => {
                        let o = owners[province as usize];
                        if winner == o {
                            probe[province as usize].battles_held += 1;
                        } else if loser == o {
                            probe[province as usize].battles_lost += 1;
                        }
                    }
                    _ => {}
                }
            }
            last_sieges = today;
            groups_day(&sim, &kit, &mut lives);
            if sim.calendar.is_month_start(sim.day) {
                probe_month(w, &kit, &mouth, &mut probe);
            }
            for e in sim.delivered_today() {
                if let Event::WarDeclared { war, .. } = e.event
                    && let Some(&q) = w.wars.target.get(war as usize)
                    && (q as usize) < n
                {
                    t[q as usize].targeted += 1;
                }
            }
            if sim.delivered_today().iter().any(|e| matches!(e.event, Event::Peace { .. })) {
                for p in 0..n {
                    if owners[p] != NONE && w.owner[p] != NONE && w.owner[p] != owners[p] {
                        handed_all += 1;
                        if rich[p] {
                            rich_handed += 1;
                        }
                        // Taken in the war whose peace hands it over: the new owner occupied it.
                        if t[p].longest >= 10 && occupiers[p] == w.owner[p] {
                            t[p].handed.push(year);
                        }
                        if t[p].longest >= 10 && is_mouth[p] {
                            let peaces: Vec<String> = sim.delivered_today().iter().filter_map(|e| match e.event {
                                Event::Peace { attacker, defender, winner, rebellion, .. } => Some(format!("{attacker} v {defender} won by {winner} rebellion {rebellion}")),
                                _ => None,
                            }).collect();
                            println!("  year {year}: rich river city {p} went from realm {} to {} (occupied by {} the day before); peaces: {}", owners[p], w.owner[p], occupiers[p] as i64, peaces.join("; "));
                        }
                    }
                }
            }
        }
        let w = &sim.world;
        let worth: Vec<u64> = (0..n).map(|p| coins(w, p)).collect();
        let mut sorted = worth.clone();
        sorted.sort_unstable();
        let line = sorted[n - n.div_ceil(50)];
        for p in 0..n {
            rich[p] = worth[p] > 0 && worth[p] >= line;
            let k = &mut t[p];
            k.run = if rich[p] { k.run + 1 } else { 0 };
            k.longest = k.longest.max(k.run);
            if w.owner[p] != NONE && w.realms.capital[w.owner[p] as usize] == p as u32 {
                k.capital_years += 1;
            }
            if worth[p] > k.peak {
                k.peak = worth[p];
                k.peak_trade = year_trade[p];
            }
            if !k.handed.is_empty() {
                if k.poorer.is_none() && worth[p] * 2 < k.peak {
                    k.poorer = Some(year);
                }
                if k.less_trade.is_none() && year_trade[p] * 2.0 < k.peak_trade {
                    k.less_trade = Some(year);
                }
            }
        }
        year_trade.iter_mut().for_each(|x| *x = 0.0);
    }
    println!("seed {seed}: {} river-mouth provinces on the coast", mouth.len());
    let mut passed = false;
    let mut totals: BTreeMap<&str, u64> = BTreeMap::new();
    let mut target_totals: BTreeMap<&str, u64> = BTreeMap::new();
    for &(p, flow) in &mouth {
        let k = &t[p];
        if k.longest < 10 {
            continue;
        }
        let pass = !k.handed.is_empty() && k.poorer.is_some() && k.less_trade.is_some();
        passed |= pass;
        println!(
            "  province {p} (river {flow:.1}× a big river's water): {} years in a row among the richest 2%, peak {} coins, a capital {} years, a war's target {} times, occupied {} days, changed hands at a peace in years {:?}, coins under half the peak from year {:?}, trade under half from year {:?}{}",
            k.longest,
            k.peak,
            k.capital_years,
            k.targeted,
            k.occupied_days,
            k.handed,
            k.poorer,
            k.less_trade,
            if pass { ": PASSES" } else { "" }
        );
        let b = &probe[p];
        let mut ratios = b.need_over_room.clone();
        ratios.sort_by(f64::total_cmp);
        let median = ratios.get(ratios.len() / 2).copied().unwrap_or(f64::NAN);
        println!(
            "    probe, months an enemy was at war with its owner: unreachable {} (of {} months a war's target: {} unreachable), room short {}, soldiers short {}, could {}, gathering {}, attacking {}, battle {}, besieged {}, held {}; garrison to beat ÷ room to gather: median {median:.2}, {} of {} months over 1",
            b.unreachable, b.target_months, b.target_unreachable, b.room_short, b.soldiers_short, b.could, b.gathering, b.attacking, b.battle, b.besieged, b.held,
            ratios.iter().filter(|&&r| r > 1.0).count(), ratios.len()
        );
        println!(
            "    sieges begun {} (days needed {:?}), best progress {:.0}%, cut off by a peace {}, recruits raised while besieged {}, battles in it: held {} lost {}",
            b.sieges, b.siege_need, 100.0 * b.best_progress, b.cut_by_peace, b.raised_under_siege, b.battles_held, b.battles_lost
        );
        for (k, v) in [("unreachable", b.unreachable), ("room short", b.room_short), ("soldiers short", b.soldiers_short), ("could", b.could), ("gathering", b.gathering), ("attacking", b.attacking), ("battle", b.battle), ("besieged", b.besieged), ("held", b.held)] {
            *totals.entry(k).or_insert(0) += v as u64;
        }
        for (k, name) in STATUS.iter().enumerate() {
            *target_totals.entry(name).or_insert(0) += b.target_status[k] as u64;
        }
        *totals.entry("sieges begun").or_insert(0) += b.sieges as u64;
        *totals.entry("sieges cut off by a peace").or_insert(0) += b.cut_by_peace as u64;
        *totals.entry("recruits raised while besieged").or_insert(0) += b.raised_under_siege;
        *totals.entry("battles held").or_insert(0) += b.battles_held as u64;
        *totals.entry("battles lost").or_insert(0) += b.battles_lost as u64;
    }
    println!("  probe, all rich river cities together: {totals:?}");
    println!("  probe, months a war named a rich river city its target, by what stood in the way: {target_totals:?}");
    let rich_cities: BTreeSet<u32> = mouth.iter().filter(|m| t[m.0].longest >= 10).map(|m| m.0 as u32).collect();
    groups_report("aimed at rich river cities", &lives, sim.day, Some(&rich_cities));
    groups_report("aimed anywhere", &lives, sim.day, None);
    let cities = mouth.iter().filter(|m| t[m.0].longest >= 10).count();
    println!("  {cities} rich river cities; story 2 {}", if passed { "passes" } else { "does not pass" });
    println!("  for comparison: {handed_all} provinces changed hands at a peace, {rich_handed} of them among the richest 2% at the time");
    let p = sim.problems;
    println!("  problems: {} contract violations, {} conservation breaks", p.contract_violations, p.conservation_breaks);
    passed
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let years: i64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(150);
    let seeds: Vec<u64> = args.iter().skip(1).filter_map(|a| a.parse().ok()).collect();
    let seeds = if seeds.is_empty() { vec![21, 7, 1] } else { seeds };
    let set: Vec<(String, f64)> = args.iter().filter_map(|a| a.split_once('=')).map(|(k, v)| (k.to_string(), v.parse().expect("a number"))).collect();
    let parts: Vec<String> = args.iter().filter(|a| ["campaigns", "supply", "seasons", "pay", "sieges", "massing", "culture", "research"].contains(&a.as_str())).cloned().collect();
    let passed = seeds.iter().filter(|&&s| run(s, years, &set, &parts)).count();
    println!("story 2 passes on {passed} of {} seeds", seeds.len());
}
