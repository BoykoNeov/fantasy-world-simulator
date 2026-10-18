//! What a simulated day costs on a large world with the default modules (everything on since step
//! 7's checks), as processor time: on a busy machine fairer than the clock, which counts the time
//! other programs take. After `warm` years, `days` are run `rounds` times from the same world and
//! the cheapest round counts. Prints the world's fingerprint at the end, to check that a speed-up
//! changed nothing. Run with RAYON_NUM_THREADS=1 to measure one worker thread.
//!
//! `BARE` (set in the environment) runs without realms, rulers, diplomacy and armies, for a
//! measure of the rest under the same load.
//!
//! `NO_RESEARCH` (set in the environment) runs without Research, which is on by default since
//! techs have effects.
//!
//! Every part of logistics is on by default since Oct 18; `SIEGES_ONLY` (set in the environment)
//! keeps only sieges in depth, the default before. Warfare's work is counted and printed for the
//! measured days, a round's worth: route searches, generals' plans and the like; `TIMING` adds how
//! long each part of its day took (a little slower).
//!
//! `WEAK_DISEASE` (set in the environment) puts camp disease and the deepest winter's deaths back to
//! 1% and 3% a month (new worlds have 8% and 10% since Oct 18), to tell the stronger losses' cost
//! from the other parts'. `NO_PAY` switches soldiers' pay off. `BREAKDOWN` prints where the cheapest
//! round's time went, module by module, by the clock (see `sim::timing`): only meaningful on a quiet
//! machine, as on a busy one the clock counts the time other programs take. `COUNT` runs the days
//! once more and prints the work done in them, which load can't blur: change requests by who asked,
//! the field and the cause, and events by kind.
//!
//! `TURNS` (set in the environment to a list such as `all,weak,sieges,nopay`) compares those mixes
//! of logistics in one process: each world is made and run `warm` years, then they run `days` in
//! turn, `rounds` times each, and each one's cheapest round counts. A busy machine's load changes
//! over minutes; taking turns spreads it over all of them alike. The names: `all` (the default),
//! `weak` (`WEAK_DISEASE`), `sieges` (`SIEGES_ONLY`), `nopay` (`NO_PAY`). A name after `same-`
//! (`same-weak`, `same-nopay`, `same-calm`: no camp disease, winter deaths or pay) is not a world of
//! its own but the first one listed, changed by live settings once it is warm: the same world, so
//! only the day's work differs, not the history.
//!
//! cargo run --release -p sim --example day_cost -- [seed] [warm] [days] [rounds] [food|markets]

use sim::config::Config;
use sim::edit::{Edit, SettingChange};
use sim::module::Slot;
use sim::sim::Sim;
use sim::world::{Size, WorldParams};

/// Processor time this process has used, all its threads, in ms.
#[cfg(windows)]
fn cpu_ms() -> f64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(h: isize, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: the four pointers are to locals that outlive the call.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    (k + u) as f64 / 10_000.0
}

/// Elsewhere, the clock.
#[cfg(not(windows))]
fn cpu_ms() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: u64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(7);
    let warm: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(5);
    let days: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(364);
    let rounds: usize = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(3);
    let economics = args.get(4).map_or("food", |s| s.as_str());
    let mut c = Config::new(WorldParams::preset(Size::Large, seed));
    c.modules.set_choice(Slot::Economics, economics).unwrap();
    // Research is on by default since techs have effects (Oct 18); `NO_RESEARCH` measures without it.
    if std::env::var("NO_RESEARCH").is_ok() {
        c.modules.research = None;
    }
    if std::env::var("BARE").is_ok() {
        for slot in [Slot::Warfare, Slot::Diplomacy, Slot::Characters, Slot::Realms] {
            c.modules.set_choice(slot, "off").unwrap();
        }
    }
    if let Ok(list) = std::env::var("TURNS") {
        return turns(&c, &list, warm, days, rounds);
    }
    let on = |name: &str| std::env::var(name).is_ok();
    logistics(&mut c, on("SIEGES_ONLY"), on("WEAK_DISEASE"), on("NO_PAY"));
    let breakdown = std::env::var("BREAKDOWN").is_ok();
    sim::warfare::work::time(std::env::var("TIMING").is_ok());
    let mut sim = Sim::new(&c).unwrap();
    sim.run_days(warm * sim.calendar.days_per_year() as u64);
    sim::warfare::work::take();
    let (mut best, mut print, mut timing) = (f64::INFINITY, 0, None);
    for _ in 0..rounds {
        let mut run = sim.fork();
        if breakdown {
            run.timing = Some(Box::default());
        }
        let t = cpu_ms();
        run.run_days(days);
        let ms = (cpu_ms() - t) / days as f64;
        println!("round: {ms:.3} ms of processor time a day");
        if ms < best {
            timing = run.timing.take();
        }
        (best, print) = (best.min(ms), run.fingerprint());
    }
    let wars = (0..sim.world.wars.count() as u32).filter(|&k| sim.world.wars.ongoing(k)).count();
    println!("{economics}, seed {seed}, after {warm} years ({wars} wars going on, {} companies): {best:.3} ms a day at best", sim.world.armies.count());
    println!("fingerprint {print}");
    if let Some(t) = timing {
        let per_day = |d: std::time::Duration| d.as_secs_f64() * 1000.0 / t.days.max(1) as f64;
        println!("Where the cheapest round's time went, ms a day by the clock (own work + checks):");
        let mut rows = t.by_module();
        rows.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)));
        for (slot, w, c) in rows {
            println!("  {slot:?}: {:.3} + {:.3}", per_day(w), per_day(c));
        }
        println!("  engine between stages: {:.3}", per_day(t.between()));
    }
    if std::env::var("COUNT").is_ok() {
        let mut run = sim.fork();
        let mut timing = Box::<sim::timing::Timing>::default();
        timing.count = true;
        run.timing = Some(timing);
        run.run_days(days);
        let t = run.timing.take().expect("set above");
        println!("Work done in the {days} days, a day on average:");
        let mut rows: Vec<_> = t.requests.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        for ((slot, field, cause), n) in rows {
            println!("  request {slot:?} {} {cause:?}: {:.1}", field.def().name, *n as f64 / days as f64);
        }
        let mut rows: Vec<_> = t.events.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        for (kind, n) in rows {
            println!("  event {kind:?}: {:.1}", *n as f64 / days as f64);
        }
    }
    println!("Warfare's work, a round of {days} days on average:");
    for (what, n) in sim::warfare::work::take() {
        println!("  {what}: {:.1}", n / rounds as f64);
    }
}

/// Logistics' parts as `SIEGES_ONLY`, `WEAK_DISEASE` and `NO_PAY` set them.
fn logistics(c: &mut Config, sieges_only: bool, weak_disease: bool, no_pay: bool) {
    let Some(t) = c.modules.warfare.as_mut() else { return };
    if sieges_only {
        (t.supply, t.seasons, t.pay, t.massing) = (None, None, None, None);
    }
    if weak_disease {
        if let Some(s) = t.supply.as_mut() {
            s.camp_disease = 0.01;
        }
        if let Some(s) = t.seasons.as_mut() {
            s.deaths = 0.03;
        }
    }
    if no_pay {
        t.pay = None;
    }
}

/// `TURNS`: the named mixes of logistics, each warmed up, then run in turn.
fn turns(c: &Config, list: &str, warm: u64, days: u64, rounds: usize) {
    let mut worlds: Vec<(&str, Sim, Vec<f64>)> = Vec::new();
    for name in list.split(',') {
        if let Some(how) = name.strip_prefix("same-") {
            let (_, first, _) = worlds.first().expect("TURNS: same-… after a world to change");
            let mut sim = first.fork();
            let set = |key: &str, value: f64| SettingChange { key: key.into(), value };
            let changes = match how {
                "weak" => vec![set("modules.warfare.supply.camp_disease", 0.01), set("modules.warfare.seasons.deaths", 0.03)],
                "nopay" => vec![set("modules.warfare.pay.wage", 0.0)],
                "calm" => vec![set("modules.warfare.supply.camp_disease", 0.0), set("modules.warfare.seasons.deaths", 0.0), set("modules.warfare.pay.wage", 0.0)],
                other => panic!("TURNS: same-{other} is not same-weak, same-nopay or same-calm"),
            };
            sim.edit(Edit::Settings { changes }).unwrap();
            println!("{name}: the first world, changed");
            worlds.push((name, sim, Vec::new()));
            continue;
        }
        let mut c = c.clone();
        match name {
            "all" => {}
            "weak" => logistics(&mut c, false, true, false),
            "sieges" => logistics(&mut c, true, false, false),
            "nopay" => logistics(&mut c, false, false, true),
            other => panic!("TURNS: {other} is not all, weak, sieges or nopay"),
        }
        let mut sim = Sim::new(&c).unwrap();
        sim.run_days(warm * sim.calendar.days_per_year() as u64);
        let wars = (0..sim.world.wars.count() as u32).filter(|&k| sim.world.wars.ongoing(k)).count();
        println!("{name}: made and run {warm} years ({wars} wars going on, {} companies)", sim.world.armies.count());
        worlds.push((name, sim, Vec::new()));
    }
    for round in 0..rounds {
        for (name, sim, times) in &mut worlds {
            let mut run = sim.fork();
            let t = cpu_ms();
            run.run_days(days);
            let ms = (cpu_ms() - t) / days as f64;
            println!("round {round}, {name}: {ms:.3} ms of processor time a day");
            times.push(ms);
        }
    }
    for (name, _, times) in &mut worlds {
        times.sort_by(f64::total_cmp);
        println!("{name}: {:.3} ms a day at best, {:.3} in the middle", times[0], times[times.len() / 2]);
    }
}
