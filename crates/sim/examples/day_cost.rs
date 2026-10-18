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
//! cargo run --release -p sim --example day_cost -- [seed] [warm] [days] [rounds] [food|markets]

use sim::config::Config;
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
    if std::env::var("SIEGES_ONLY").is_ok()
        && let Some(t) = c.modules.warfare.as_mut()
    {
        (t.supply, t.seasons, t.pay, t.massing) = (None, None, None, None);
    }
    sim::warfare::work::time(std::env::var("TIMING").is_ok());
    let mut sim = Sim::new(&c).unwrap();
    sim.run_days(warm * sim.calendar.days_per_year() as u64);
    sim::warfare::work::take();
    let (mut best, mut print) = (f64::INFINITY, 0);
    for _ in 0..rounds {
        let mut run = sim.fork();
        let t = cpu_ms();
        run.run_days(days);
        let ms = (cpu_ms() - t) / days as f64;
        println!("round: {ms:.3} ms of processor time a day");
        (best, print) = (best.min(ms), run.fingerprint());
    }
    let wars = (0..sim.world.wars.count() as u32).filter(|&k| sim.world.wars.ongoing(k)).count();
    println!("{economics}, seed {seed}, after {warm} years ({wars} wars going on, {} companies): {best:.3} ms a day at best", sim.world.armies.count());
    println!("fingerprint {print}");
    println!("Warfare's work, a round of {days} days on average:");
    for (what, n) in sim::warfare::work::take() {
        println!("  {what}: {:.1}", n / rounds as f64);
    }
}
