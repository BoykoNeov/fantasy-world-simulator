//! Armies and battles on the map, worked out from the army fields of the day shown (the window
//! never sees Warfare's private state). Pure: the window draws what these give, and the tests run
//! them without a GPU.
//!
//! Only field companies away from rest are drawn: marching, gathering, besieging or fighting.
//! Guards (every province's peacetime force) and recruits standing idle at home (in their home
//! province, in no group) are left out, so the map shows what moves and fights (the user's choice,
//! Oct 17: in a busy year of a large world, 60–85% of field companies are recruits at home).

use crate::inspector::{LogEntry, Logged};
use sim::events::Event;
use sim::grid::{dot, normalize, NONE, V3};
use sim::warfare::FIELD;
use sim::world::World;
use std::collections::BTreeMap;

/// A troop kind as the window knows it: its node in the troop tree, its name, whether it is a
/// ship, and its speed (the share of a way's days it marches in a day). From
/// [`sim::warfare::kinds`].
pub type Kind = (u16, String, bool, f64);

/// What is drawn for the day shown: the markers, the provinces where a battle is fought, and those
/// where one ended in the last month with how far it has faded.
#[derive(Clone, Debug, Default)]
pub struct ArmyView {
    pub markers: Vec<Marker>,
    pub battles: Vec<u32>,
    pub recent: Vec<(u32, f64)>,
}

/// One realm's field soldiers in one place: standing in a province, or on a way between two.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub realm: u32,
    pub ship: bool,
    pub soldiers: u64,
    /// The province it stands in, or left.
    pub from: u32,
    /// The province it is on its way to next, [`NONE`] when standing.
    pub to: u32,
    /// Where it is going in the end, [`NONE`] when standing.
    pub dest: u32,
    /// Days until it reaches `to`, and the way's days at its pace (at least the days left).
    pub days_left: f64,
    pub total: f64,
    /// Soldiers by troop kind (node), in node order.
    pub kinds: Vec<(u16, u64)>,
}

impl Marker {
    /// The share of the way done, `t` (0 to 1) into the day shown: 0 standing.
    pub fn done(&self, t: f64) -> f64 {
        if self.to == NONE || self.total <= 0.0 { 0.0 } else { (1.0 - (self.days_left - t) / self.total).clamp(0.0, 1.0) }
    }

    /// Where it is, `t` into the day shown: along the great circle between the two seats.
    pub fn at(&self, w: &World, t: f64) -> V3 {
        let a = w.provinces.center[self.from as usize];
        if self.to == NONE {
            return a;
        }
        slerp(a, w.provinces.center[self.to as usize], self.done(t))
    }
}

/// The point `f` (0 to 1) of the way from `a` to `b` along the great circle.
pub fn slerp(a: V3, b: V3, f: f64) -> V3 {
    let angle = dot(a, b).clamp(-1.0, 1.0).acos();
    if angle < 1e-9 {
        return a;
    }
    let (sa, sb) = (((1.0 - f) * angle).sin() / angle.sin(), (f * angle).sin() / angle.sin());
    normalize(std::array::from_fn(|i| a[i] * sa + b[i] * sb))
}

/// The markers of the day shown, grouped by realm, place, whether ships, and (on the move) where
/// they go and how far along: companies sent together share a marker.
pub fn markers(w: &World, kinds: &[Kind]) -> Vec<Marker> {
    let arm = &w.armies;
    let kind = |node: u16| kinds.iter().find(|k| k.0 == node);
    let speed = |node: u16| kind(node).map_or(1.0, |k| k.3);
    // A group marches at its slowest member's pace.
    let mut slowest: BTreeMap<u32, f64> = BTreeMap::new();
    for a in 0..arm.count() {
        if arm.size[a] > 0 && arm.group[a] != NONE {
            let s = slowest.entry(arm.group[a]).or_insert(f64::INFINITY);
            *s = s.min(speed(arm.kind[a]));
        }
    }
    type Key = (u32, u32, u32, bool, u32, u64);
    let mut out: BTreeMap<Key, Marker> = BTreeMap::new();
    for a in 0..arm.count() {
        let (size, from, to) = (arm.size[a], arm.province[a], arm.next[a]);
        if size == 0 || arm.role[a] != FIELD || (to == NONE && from == arm.home[a] && arm.group[a] == NONE) {
            continue;
        }
        let ship = kind(arm.kind[a]).is_some_and(|k| k.2);
        let days_left = if to == NONE { 0.0 } else { arm.days_left[a] as f64 };
        let total = if to == NONE {
            0.0
        } else {
            let pace = if arm.group[a] != NONE { slowest.get(&arm.group[a]).copied().unwrap_or(1.0) } else { speed(arm.kind[a]) };
            let way = w.routes.between(from, to);
            // Ships go by sea; land troops by land where there is a land way, else by sea.
            let days = way.and_then(|l| if ship { l.sea_days } else { l.land_days.or(l.sea_days) }).unwrap_or(0.0) as f64;
            (days / pace.max(1e-9)).max(days_left)
        };
        let dest = if to == NONE { NONE } else { arm.dest[a] };
        let m = out.entry((arm.realm[a], from, to, ship, dest, days_left.to_bits())).or_insert_with(|| Marker {
            realm: arm.realm[a],
            ship,
            soldiers: 0,
            from,
            to,
            dest,
            days_left,
            total,
            kinds: Vec::new(),
        });
        m.soldiers += size;
        m.total = m.total.max(total);
        match m.kinds.iter_mut().find(|k| k.0 == arm.kind[a]) {
            Some(k) => k.1 += size,
            None => {
                m.kinds.push((arm.kind[a], size));
                m.kinds.sort_unstable();
            }
        }
    }
    out.into_values().collect()
}

/// Provinces where a battle is fought on the day shown, by Warfare's own rule
/// ([`sim::warfare::battle_provinces`]).
pub fn battles(w: &World, kinds: &[Kind]) -> Vec<u32> {
    sim::warfare::battle_provinces(w, |n| kinds.iter().any(|k| k.0 == n && k.2))
}

/// Battles that ended in the `days` before `today` (and not after it: going back in history
/// leaves later days in the log), with how far each has faded, 0 just ended to 1 gone. One per
/// province, the latest.
pub fn recent_battles(log: &[LogEntry], today: u64, days: u64) -> Vec<(u32, f64)> {
    let mut out: BTreeMap<u32, f64> = BTreeMap::new();
    for e in log.iter().rev() {
        if e.day > today {
            continue;
        }
        if today - e.day >= days {
            break;
        }
        if let Logged::Event(Event::Battle { province, .. }) = e.what {
            out.entry(province).or_insert((today - e.day) as f64 / days as f64);
        }
    }
    out.into_iter().collect()
}

/// Words for a marker: "Dreinmark: 1,200 levies, 300 cavalry, besieging" and the like.
pub fn describe(m: &Marker, kinds: &[Kind], realm: &str) -> String {
    let name = |node: u16| kinds.iter().find(|k| k.0 == node).map_or(format!("kind {node}"), |k| k.1.replace('_', " "));
    let of: Vec<String> = m.kinds.iter().map(|&(k, n)| format!("{} {}", crate::inspector::group(n), name(k))).collect();
    let doing = if m.to == NONE { format!("in province {}", m.from) } else { format!("marching to province {}", m.dest) };
    format!("{realm}: {}, {doing}", of.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::config::Config;
    use sim::diplomacy::WarParams;
    use sim::sim::Sim;
    use sim::warfare::GUARD;
    use sim::world::WorldParams;

    fn world() -> (Sim, Vec<Kind>) {
        let mut c = Config::new(WorldParams { n: 48, provinces: 200, seed: 5, land_fraction: 0.4, radius_km: 6371.0 });
        c.modules.without_realms();
        c.modules.realms = Some(sim::realms::RealmParams { unrest: None, ..Default::default() });
        c.modules.diplomacy = Some(WarParams::default());
        c.modules.warfare = Some(Default::default());
        let kinds = sim::warfare::kinds(c.modules.warfare.as_ref().unwrap());
        let mut sim = Sim::new(&c).unwrap();
        sim.run_days(2);
        (sim, kinds)
    }

    #[test]
    fn markers_leave_out_guards_idle_recruits_and_empty_companies_and_group_the_rest() {
        let (mut sim, kinds) = world();
        let w = &mut sim.world;
        let (r, p) = (0..w.realms.count() as u32).find_map(|r| Some((r, w.realms.capital.get(r as usize).copied().filter(|&c| c != NONE)?))).unwrap();
        let q = w.routes.of(p).find(|l| l.land_days.is_some()).map(|l| l.other(p)).unwrap();
        let levies = kinds.iter().find(|k| k.1 == "levies").unwrap().0;
        let cavalry = kinds.iter().find(|k| k.1 == "cavalry").unwrap().0;
        let base = w.armies.count();
        w.armies.push(r, p, levies, GUARD, 500); // a guard
        w.armies.push(r, p, levies, FIELD, 400); // idle at home
        w.armies.push(r, p, levies, FIELD, 0); // empty
        let away = w.armies.push(r, p, levies, FIELD, 300); // standing away from home
        let other = w.armies.push(r, p, cavalry, FIELD, 200); // with it: one marker
        w.armies.province[away as usize] = q;
        w.armies.province[other as usize] = q;
        // On the way from p to q, two days left.
        let moving = w.armies.push(r, p, levies, FIELD, 100);
        (w.armies.next[moving as usize], w.armies.dest[moving as usize], w.armies.days_left[moving as usize]) = (q, q, 2.0);
        let all = markers(w, &kinds);
        assert_eq!(w.armies.count(), base + 6);
        let standing = all.iter().find(|m| m.realm == r && m.from == q && m.to == NONE).expect("the companies away from home");
        assert_eq!((standing.soldiers, standing.kinds.len()), (500, 2));
        let walking = all.iter().find(|m| m.realm == r && m.from == p && m.to == q).expect("the company on its way");
        assert_eq!(walking.soldiers, 100);
        assert!(!all.iter().any(|m| m.realm == r && m.from == p && m.to == NONE), "guards and idle recruits at home are not drawn");

        // Along the way by the days left: the way's days at the levies' speed.
        let link = w.routes.between(p, q).unwrap();
        let speed = kinds.iter().find(|k| k.0 == levies).unwrap().3;
        let total = (link.land_days.unwrap() as f64 / speed).max(2.0);
        assert!((walking.total - total).abs() < 1e-6);
        assert!((walking.done(0.0) - (1.0 - 2.0 / total)).abs() < 1e-9);
        assert!((walking.done(0.5) - (1.0 - 1.5 / total)).abs() < 1e-9, "it glides through the day");
        let at = |f: f64| slerp(w.provinces.center[p as usize], w.provinces.center[q as usize], f);
        assert!(dot(at(0.0), w.provinces.center[p as usize]) > 1.0 - 1e-12);
        assert!(dot(at(1.0), w.provinces.center[q as usize]) > 1.0 - 1e-12);
        let mut ends = walking.clone();
        ends.days_left = total;
        assert_eq!(ends.done(0.0), 0.0);
        ends.days_left = 0.0;
        assert_eq!(ends.done(0.7), 1.0, "never past the end");
    }

    #[test]
    fn a_battle_is_found_where_enemies_stand_together_and_not_between_realms_at_peace() {
        let (mut sim, kinds) = world();
        sim.run_days(40);
        let alive: Vec<u32> = (0..sim.world.realms.count() as u32).filter(|&r| sim.world.realms.alive(r)).collect();
        let (a, b) = (alive[0], alive[1]);
        let p = sim.world.realms.capital[a as usize];
        let levies = kinds.iter().find(|k| k.1 == "levies").unwrap().0;
        // b's soldiers placed by hand in a's capital, where a's guards stand.
        let with_b = |w: &World| {
            let mut w = w.clone();
            let k = w.armies.push(b, w.realms.capital[b as usize], levies, FIELD, 1000);
            w.armies.province[k as usize] = p;
            w
        };
        assert!((0..sim.world.armies.count()).any(|k| sim.world.armies.province[k] == p && sim.world.armies.realm[k] == a && sim.world.armies.size[k] > 0), "a's guards");
        assert!(!battles(&with_b(&sim.world), &kinds).contains(&p), "at peace: no battle");
        sim.poke(Event::DeclareWar { attacker: a, defender: b });
        sim.run_days(2);
        assert!(sim.world.wars.current(a, b).is_some());
        assert!(battles(&with_b(&sim.world), &kinds).contains(&p), "at war: a battle");
    }
}
