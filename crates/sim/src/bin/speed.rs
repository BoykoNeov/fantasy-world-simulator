//! The speed table: how long a simulated day takes for each mix of module versions, and where the
//! time goes (see `sim::speed`).
//!
//! speed [--pairs | --map] [--only MIX] [--size small|medium|large] [--n FACE] [--provinces N]
//!       [--seed S] [--young Y] [--busy Y] [--days D] [--rounds R] [--threads T] [--write]
//!
//! By default every accepted mix runs, the map's versions (terrain, climate) at their defaults;
//! `--map` adds their versions too, `--pairs` runs only the combination tester's small set that has
//! every pair of versions together (much quicker), `--only` one mix (with the base mix, which every
//! run measures first). A medium world unless `--size` or `--n`/`--provinces` say otherwise. One
//! worker thread unless `--threads` (one is the fair measure). Measure on a quiet machine.
//!
//! It prints each mix as it goes, then the mixes that got slower than in the shipped table
//! (`crates/sim/data/speed.toml`) by more than a fifth, as multiples of the base mix. `--write`
//! saves the new table there.

use sim::combos::{all_pairs, Mix};
use sim::speed::{base_mix, measure, mixes, today, About, Settings, Table};
use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |name: &str| args.iter().any(|a| a == name);
    let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let num = |name: &str| get(name).map(|v| v.parse::<u64>().unwrap_or_else(|_| fail(&format!("{name}: a whole number"))));

    let mut s = Settings::default();
    match get("--size").as_deref() {
        None | Some("medium") => {}
        Some("small") => (s.n, s.provinces) = (128, 500),
        Some("large") => (s.n, s.provinces) = (408, 5_000),
        Some(other) => fail(&format!("--size {other}: small, medium or large")),
    }
    s.n = num("--n").map_or(s.n, |x| x as usize);
    s.provinces = num("--provinces").map_or(s.provinces, |x| x as usize);
    s.seed = num("--seed").unwrap_or(s.seed);
    s.young_year = num("--young").unwrap_or(s.young_year);
    s.busy_year = num("--busy").unwrap_or(s.busy_year);
    s.days = num("--days").unwrap_or(s.days);
    s.rounds = num("--rounds").map_or(s.rounds, |x| x as usize);
    let threads = num("--threads").unwrap_or(1) as usize;
    rayon::ThreadPoolBuilder::new().num_threads(threads).build_global().unwrap_or_else(|e| fail(&e.to_string()));

    let base = base_mix();
    let list: Vec<Mix> = if let Some(m) = get("--only") {
        let m = Mix::parse(&m).unwrap_or_else(|e| fail(&e));
        if m == base { vec![base] } else { vec![base, m] }
    } else if has("--pairs") {
        let mut v: Vec<Mix> = all_pairs(0).into_iter().filter(|m| *m != base).collect();
        v.insert(0, base);
        v
    } else {
        mixes(has("--map"))
    };
    println!("{} mixes on a world of {} × {} cells a face and {} provinces, {} thread(s): years {} and {}, {} days, best of {}", list.len(), s.n, s.n, s.provinces, threads, s.young_year, s.busy_year, s.days, s.rounds);

    let started = Instant::now();
    let mut rows = Vec::new();
    for (i, m) in list.iter().enumerate() {
        let row = measure(m, &s);
        let ms = |x: &Option<sim::speed::Moment>| x.as_ref().map_or("-".to_string(), |m| format!("{:.2}", m.ms));
        let top = row.busy.as_ref().and_then(|b| b.modules.first()).map_or(String::new(), |t| format!(", most in {} ({:.2} + {:.2} checks)", t.module, t.work_ms, t.checks_ms));
        let left = started.elapsed().as_secs_f64() / (i + 1) as f64 * (list.len() - i - 1) as f64;
        match &row.refused {
            Some(why) => println!("[{}/{}] {}: refused: {why}", i + 1, list.len(), row.versions),
            None => println!("[{}/{}] {}: young {} ms, busy {} ms a day{top}  (about {:.0} min left)", i + 1, list.len(), row.versions, ms(&row.young), ms(&row.busy), left / 60.0),
        }
        rows.push(row);
    }

    let about = About { machine: machine(), threads, commit: commit(), date: today() };
    let table = Table::new(about, s, rows);
    match Table::shipped() {
        Ok(old) if !old.mix.is_empty() => {
            let slower = table.slower_than(&old, 1.2);
            if slower.is_empty() {
                println!("\nNothing got slower than in the shipped table (measured {} on {}).", old.about.date, old.about.machine);
            } else {
                println!("\nSlower than in the shipped table (measured {} on {}), as multiples of the base mix:", old.about.date, old.about.machine);
                for x in &slower {
                    let ms = x.ms.map_or(String::new(), |(a, b)| format!(" ({a:.2} → {b:.2} ms a day)"));
                    println!("  {} {}: {:.2} → {:.2}{ms}", x.versions, x.moment, x.was, x.now);
                }
            }
        }
        Ok(_) => println!("\nThe shipped table has no measures yet to compare with."),
        Err(e) => println!("\n{e}"),
    }
    if has("--write") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data").join("speed.toml");
        std::fs::write(&path, table.text()).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
        println!("Wrote {}", path.display());
    }
}

/// The processor, as the system names it, and its threads.
fn machine() -> String {
    let cpu = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown processor".into());
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    format!("{cpu}, {cores} threads")
}

/// The commit measured, marked when the working tree had changes.
fn commit() -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    let git = |a: &[&str]| std::process::Command::new("git").arg("-C").arg(dir).args(a).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let hash = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    if dirty { format!("{hash} with changes") } else { hash }
}

fn fail(msg: &str) -> ! {
    eprintln!("speed: {msg}");
    std::process::exit(2)
}
