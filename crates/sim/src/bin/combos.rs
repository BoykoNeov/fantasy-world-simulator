//! The combination tester: run mixes of module versions headless and score them.
//!
//! combos [--all | --pairs] [--only MIX] [--seeds 1,2,3] [--years Y] [--n FACE] [--provinces N]
//!        [--out DIR] [--write | --check]
//!
//! By default every mix there is runs (while there are few; otherwise `--pairs`, a small set that
//! still has every pair of versions together), on each seed, plus every scenario in
//! `crates/sim/scenarios`. `--only economics=off,population=off` runs one mix (the others keep
//! their defaults). `--out DIR` writes a scenario for each run that wasn't green, which makes the run
//! again up to just after its first problem (`simrun --scenario FILE`, or the window's `--open`).
//!
//! `--write` saves the table the window reads (`crates/sim/data/compatibility.toml`). `--check` runs
//! again what that table lists, with its settings, and fails if anything came out differently.

use rayon::prelude::*;
use sim::combos::{all_mixes, all_pairs, check_scenario, quiet_panics, run, Mix, Quirks, RunResult, ScenarioRow, Settings, Table, Verdict};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn scenarios_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |name: &str| args.iter().any(|a| a == name);
    let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let table_path = data().join("compatibility.toml");
    let quirks_text = std::fs::read_to_string(data().join("known_quirks.toml")).unwrap_or_default();
    let quirks = Quirks::parse(&quirks_text).unwrap_or_else(|e| fail(&e));

    let mut settings = Settings::default();
    let mut mixes: Vec<Mix>;
    let mut scenario_names: Option<Vec<String>> = None;
    if has("--check") {
        let text = std::fs::read_to_string(&table_path).unwrap_or_else(|e| fail(&format!("{}: {e}", table_path.display())));
        let old = Table::parse(&text).unwrap_or_else(|e| fail(&e));
        settings = old.settings.clone();
        mixes = old.mix.iter().map(|m| Mix::parse(&m.versions).unwrap_or_else(|e| fail(&e))).collect();
        scenario_names = Some(old.scenario.iter().map(|s| s.name.clone()).collect());
    } else {
        if let Some(s) = get("--seeds") {
            settings.seeds = s.split(',').map(|x| x.trim().parse().unwrap_or_else(|_| fail("--seeds: whole numbers, comma-separated"))).collect();
        }
        if let Some(y) = get("--years") {
            settings.years = y.parse().unwrap_or_else(|_| fail("--years"));
        }
        if let Some(n) = get("--n") {
            settings.n = n.parse().unwrap_or_else(|_| fail("--n"));
        }
        if let Some(p) = get("--provinces") {
            settings.provinces = p.parse().unwrap_or_else(|_| fail("--provinces"));
        }
        mixes = match get("--only") {
            Some(m) => vec![Mix::parse(&m).unwrap_or_else(|e| fail(&e))],
            None if has("--pairs") => all_pairs(0),
            None if has("--all") || all_mixes().len() <= 64 => all_mixes(),
            None => all_pairs(0),
        };
        if has("--write") && get("--only").is_some() {
            fail("--write saves the whole table; it can't be used with --only");
        }
    }
    mixes.sort();

    quiet_panics();
    let jobs: Vec<(Mix, u64)> = mixes.iter().flat_map(|m| settings.seeds.iter().map(move |&s| (m.clone(), s))).collect();
    println!("{} mixes × {} seeds, {} years each, face {}, {} provinces", mixes.len(), settings.seeds.len(), settings.years, settings.n, settings.provinces);
    let t = Instant::now();
    let mut runs: Vec<RunResult> = jobs.par_iter().map(|(m, s)| run(m, *s, &settings)).collect();
    for r in &mut runs {
        quirks.apply(r);
    }
    let scenarios = run_scenarios(scenario_names);
    println!("done in {:.1} s\n", t.elapsed().as_secs_f64());

    print_runs(&runs);
    let table = Table::new(&settings, &runs, scenarios);
    print_pairs_and_scenarios(&table);

    if let Some(dir) = get("--out") {
        write_repros(&runs, &settings, Path::new(&dir));
    }
    let text = table.text();
    if has("--write") {
        std::fs::write(&table_path, &text).unwrap_or_else(|e| fail(&format!("{}: {e}", table_path.display())));
        println!("\nwrote {}", table_path.display());
    }
    if has("--check") {
        let old = std::fs::read_to_string(&table_path).expect("read above");
        if old.replace("\r\n", "\n") != text {
            let new_rows: Vec<&str> = text.lines().collect();
            let differ: Vec<&str> = old.lines().filter(|l| !new_rows.contains(l)).take(10).collect();
            println!("\nthe table is out of date: run `combos --write` (lines that changed, first 10):");
            for l in differ {
                println!("  {l}");
            }
            std::process::exit(1);
        }
        println!("\nthe table is up to date");
    }
    let worst = runs.iter().map(|r| r.verdict()).filter(|&v| v != Verdict::Refused).max().unwrap_or(Verdict::Green);
    println!("\nworst verdict: {worst:?}");
}

fn fail(why: &str) -> ! {
    eprintln!("combos: {why}");
    std::process::exit(2);
}

/// Every scenario in the scenarios folder (or only the ones named), checked in parallel.
fn run_scenarios(only: Option<Vec<String>>) -> Vec<ScenarioRow> {
    let dir = scenarios_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect())
        .unwrap_or_default();
    files.sort();
    let name = |p: &PathBuf| p.file_name().expect("a file").to_string_lossy().into_owned();
    if let Some(only) = &only {
        files.retain(|p| only.contains(&name(p)));
        for n in only {
            if !files.iter().any(|p| name(p) == *n) {
                fail(&format!("the table lists scenario {n}, which is no longer in {}", dir.display()));
            }
        }
    }
    files
        .par_iter()
        .map(|p| {
            let text = std::fs::read_to_string(p).unwrap_or_else(|e| fail(&format!("{}: {e}", p.display())));
            check_scenario(&name(p), &text)
        })
        .collect()
}

fn print_runs(runs: &[RunResult]) {
    println!("{:<80} {:<8} {:>10}  findings", "mix", "seed", "ms/day");
    for r in runs {
        let v = r.verdict();
        let first = r.refused.clone().or_else(|| r.findings.first().map(|f| format!("day {}: {:?}: {}", f.day, f.problem, f.what)));
        println!("{:<80} {:<8} {:>10.4}  {:?} {}", r.mix.label(), r.seed, r.ms_per_day, v, first.unwrap_or_default());
    }
}

fn print_pairs_and_scenarios(t: &Table) {
    let bad: Vec<_> = t.pair.iter().filter(|p| p.verdict != Verdict::Green).collect();
    println!("\npairs: {} green, {} not", t.pair.len() - bad.len(), bad.len());
    for p in bad {
        println!("  {} with {}: {:?}{}", p.a, p.b, p.verdict, p.example.as_ref().map_or(String::new(), |e| format!(" ({e})")));
    }
    if !t.scenario.is_empty() {
        println!("\nscenarios:");
        for s in &t.scenario {
            println!("  {}: {:?}", s.name, s.verdict);
            for f in &s.finding {
                println!("    {:?}: {}", f.problem, f.what);
            }
        }
    }
}

fn write_repros(runs: &[RunResult], settings: &Settings, dir: &Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
    for r in runs {
        if let Some(s) = r.repro(settings) {
            let name = r.mix.label().replace(['=', ' '], "_");
            let path = dir.join(format!("{name}_seed{}.toml", r.seed));
            let text = format!("# {} seed {}: {:?}\n", r.mix.label(), r.seed, r.verdict()) + &toml::to_string(&s).expect("scenarios serialise");
            std::fs::write(&path, text).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
            println!("wrote {}", path.display());
        }
    }
}
