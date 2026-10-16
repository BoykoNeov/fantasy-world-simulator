//! Headless run: generate a world, simulate some years, report speed and outcomes, optionally write
//! PNG maps.
//!
//! simrun [--size small|medium|large|huge] [--n FACE] [--provinces N] [--seed S] [--years Y]
//!        [--terrain null|noise] [--climate null|latitude] [--out DIR] [--width PX]
//!        [--save FILE] [--load FILE] [--scenario FILE] [--write-scenario FILE]
//!        [--economics food|markets|off] [--realms settled=W,city=W,frontier=W] [--rulers] [--wars]
//!        [--armies]
//!
//! `--load` continues a saved world for `--years` more years (world options are then ignored);
//! `--save` writes the world at the end. `--scenario` opens a scenario (its played timeline) instead,
//! and `--write-scenario` writes the world at the end as one. `--realms` switches realms on, with
//! the ways to start weighted as given (`--realms settled=1` is the default way). `--rulers` gives
//! every realm a ruler and heir (and turns realms on the default way if `--realms` isn't given).
//! `--wars` keeps wars and truces between realms (started and ended only by pokes, from a scenario
//! file, for now), turning realms on in the same way. Under `--economics markets`, realms tax and
//! spend coins, and the realms line says where the coins sit. Realms come with unrest: an unrest
//! line says how many provinces are in uprising and how many realms have broken away. `--armies`
//! has provinces raise soldiers (turning realms on the same way), and a line says how many serve,
//! of which kinds, and how many deserted.

use sim::branch::Timelines;
use sim::config::Config;
use sim::fields::{quantity_total, Quantity};
use sim::sim::Sim;
use sim::world::{Size, WorldParams};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let size = match get("--size").as_deref() {
        Some("small") => Size::Small,
        Some("medium") => Size::Medium,
        None | Some("large") => Size::Large,
        Some("huge") => Size::Huge,
        Some(other) => panic!("unknown size {other}"),
    };
    let seed: u64 = get("--seed").map_or(7, |s| s.parse().expect("--seed"));
    let mut params = WorldParams::preset(size, seed);
    if let Some(n) = get("--n") {
        params.n = n.parse().expect("--n");
    }
    if let Some(p) = get("--provinces") {
        params.provinces = p.parse().expect("--provinces");
    }
    let years: i64 = get("--years").map_or(100, |s| s.parse().expect("--years"));
    let out = get("--out").map(PathBuf::from);
    let width: u32 = get("--width").map_or(2048, |s| s.parse().expect("--width"));
    let mut config = Config::new(params.clone());
    if let Some(v) = get("--terrain") {
        config.modules.terrain = v.parse().unwrap_or_else(|e| panic!("{e}"));
    }
    if let Some(v) = get("--climate") {
        config.modules.climate = v.parse().unwrap_or_else(|e| panic!("{e}"));
    }
    if let Some(v) = get("--economics") {
        config.modules.set_choice(sim::module::Slot::Economics, &v).unwrap_or_else(|e| panic!("--economics: {e}"));
    }
    if let Some(v) = get("--realms") {
        let mut r = sim::realms::RealmParams { settled: 0.0, ..Default::default() };
        for part in v.split(',') {
            let (way, weight) = part.split_once('=').unwrap_or_else(|| panic!("--realms: {part}: expected way=weight"));
            let weight: f64 = weight.parse().expect("--realms weight");
            match way {
                "settled" => r.settled = weight,
                "city" => r.city_states = weight,
                "frontier" => r.frontier = weight,
                _ => panic!("--realms: no way {way}; expected settled, city or frontier"),
            }
        }
        config.modules.realms = Some(r);
    }
    if args.iter().any(|a| a == "--rulers") {
        config.modules.realms.get_or_insert_with(Default::default);
        config.modules.characters = Some(Default::default());
    }
    if args.iter().any(|a| a == "--wars") {
        config.modules.realms.get_or_insert_with(Default::default);
        config.modules.diplomacy = Some(Default::default());
    }
    if args.iter().any(|a| a == "--armies") {
        config.modules.realms.get_or_insert_with(Default::default);
        config.modules.warfare = Some(Default::default());
    }

    let t = Instant::now();
    let mut sim = match (get("--load"), get("--scenario")) {
        (Some(file), _) => {
            let sim = Sim::load_from(file.as_ref()).unwrap_or_else(|e| panic!("{e}"));
            println!("loaded {file} at {}", sim.calendar.format(sim.day));
            sim
        }
        (None, Some(file)) => {
            let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{file}: {e}"));
            let mut t = Timelines::from_scenario_text(&text).unwrap_or_else(|e| panic!("{file}: {e}"));
            let sim = t.list.swap_remove(t.active).sim;
            println!("opened scenario {file} at {}", sim.calendar.format(sim.day));
            sim
        }
        (None, None) => {
            println!("world: {size:?}, face {} ({} cells), target {} provinces, seed {}", params.n, 6 * params.n * params.n, params.provinces, seed);
            Sim::new(&config).unwrap_or_else(|e| panic!("{e}"))
        }
    };
    let gen_s = t.elapsed().as_secs_f64();
    let versions: Vec<String> = sim.manifests().iter().map(|m| format!("{:?} {}", m.slot, m.version)).collect();
    println!("modules: {}", versions.join(", "));
    let w = &sim.world;
    let land = w.elevation.iter().filter(|&&h| h >= 0.0).count();
    let rivers = w.river_flow.iter().zip(&w.elevation).filter(|(f, h)| **f >= sim::world::RIVER_MIN_FLOW && **h >= 0.0).count();
    println!(
        "ready in {gen_s:.2} s: {:.0}% land, {} provinces, {} river cells, {} people",
        100.0 * land as f64 / w.grid.cells() as f64,
        w.provinces.count,
        rivers,
        group(quantity_total(w, Quantity::People))
    );
    print_realms(&sim);
    if let Some(dir) = &out {
        let t = Instant::now();
        let files = sim::export::write_all(&sim, dir, "start_", width).expect("write PNGs");
        println!("wrote {} maps in {:.2} s to {}", files.len(), t.elapsed().as_secs_f64(), dir.display());
    }

    // Time every day, split into days when something ran and idle days.
    let end = sim.calendar.year_start(sim.calendar.date(sim.day).year + years);
    let (mut busy, mut idle) = ((0u64, 0f64), (0u64, 0f64));
    let run_start = Instant::now();
    while sim.day < end {
        let t = Instant::now();
        let ran = sim.step_day();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let slot = if ran { &mut busy } else { &mut idle };
        slot.0 += 1;
        slot.1 += ms;
    }
    let total_s = run_start.elapsed().as_secs_f64();
    let days = busy.0 + idle.0;

    println!("\nyear  population      starved   famine provinces  food stock");
    let mut rows = sim.history.clone();
    rows.push(sim.year_so_far());
    let stride = (rows.len() / 10).max(1);
    for (k, y) in rows.iter().enumerate() {
        if k.is_multiple_of(stride) || k + 1 == rows.len() {
            println!(
                "{:>4}  {:>13}  {:>11}  {:>16}  {:>10}",
                y.year,
                group(y.population),
                group(y.ledger.starved),
                y.famine_provinces,
                group(y.food_stock)
            );
        }
    }
    println!(
        "\nsimulated {years} years ({days} days) in {total_s:.2} s: {:.4} ms per day on average",
        total_s * 1000.0 / days as f64
    );
    println!("  days when something ran:  {:>6}, {:.4} ms each", busy.0, busy.1 / busy.0.max(1) as f64);
    println!("  idle days:                {:>6}, {:.6} ms each", idle.0, idle.1 / idle.0.max(1) as f64);
    println!("  a century at this rate: {:.1} s", total_s * 100.0 / years as f64);
    println!("fingerprint {:016x}", sim.fingerprint());
    let p = sim.problems;
    println!("contract violations {}, conservation breaks {}", p.contract_violations, p.conservation_breaks);
    if let Some(file) = get("--save") {
        let t = Instant::now();
        sim.save_to(file.as_ref()).expect("write save");
        let mb = std::fs::metadata(&file).map_or(0.0, |m| m.len() as f64 / 1e6);
        println!("saved {file} ({mb:.1} MB) in {:.2} s", t.elapsed().as_secs_f64());
    }
    if let Some(file) = get("--write-scenario") {
        std::fs::write(&file, Timelines::new(sim.fork()).scenario_text()).expect("write scenario");
        println!("wrote scenario {file}");
    }

    print_realms(&sim);
    if let Some(dir) = &out {
        let files = sim::export::write_all(&sim, dir, "end_", width).expect("write PNGs");
        println!("wrote {} maps to {}", files.len(), dir.display());
    }
}

/// How many realms, how big, and how much settled land nobody owns.
fn print_realms(sim: &Sim) {
    let w = &sim.world;
    let alive: Vec<u32> = (0..w.realms.count() as u32).filter(|&r| w.realms.alive(r)).collect();
    if alive.is_empty() {
        return;
    }
    let people = w.province_population();
    let mut size = vec![0usize; w.realms.count()];
    for &r in w.owner.iter().filter(|&&r| r != sim::grid::NONE) {
        size[r as usize] += 1;
    }
    let mut sizes: Vec<usize> = alive.iter().map(|&r| size[r as usize]).collect();
    sizes.sort_unstable();
    let settled = (0..w.provinces.count).filter(|&p| people[p] > 0).count();
    let unowned = (0..w.provinces.count).filter(|&p| people[p] > 0 && w.owner[p] == sim::grid::NONE).count();
    let names = sim::realms::names(w.params.seed, w.realms.count());
    let biggest = alive.iter().max_by_key(|&&r| (size[r as usize], std::cmp::Reverse(r))).copied().expect("not empty");
    println!(
        "realms: {} (provinces each: smallest {}, middle {}, largest {}); settled provinces nobody owns: {unowned} of {settled}; largest: {}, capital province {}",
        alive.len(),
        sizes[0],
        sizes[sizes.len() / 2],
        sizes[sizes.len() - 1],
        names[biggest as usize],
        w.realms.capital[biggest as usize]
    );
    print_treasuries(sim, &alive, biggest, &names[biggest as usize]);
    print_rulers(sim, biggest, &names[biggest as usize]);
    if sim.config.modules.realms.as_ref().is_some_and(|r| r.unrest.is_some()) {
        let risen = w.uprising.iter().filter(|&&s| s != sim::grid::NONE).count();
        let restless = w.unrest.iter().filter(|&&u| u >= sim::realms::LINE / 2).count();
        let broke = w.realms.broke_from.iter().filter(|&&r| r != sim::grid::NONE).count();
        println!("unrest: {risen} provinces in uprising, {restless} at least half way there; {broke} realms have broken away so far");
    }
    let wars = &w.wars;
    if wars.count() > 0 {
        let on = (0..wars.count() as u32).filter(|&k| wars.ongoing(k)).count();
        let won = (0..wars.count()).filter(|&k| wars.winner[k] != sim::grid::NONE).count();
        println!("wars: {} declared, {on} going on, {won} won, {} ended with nobody winning", wars.count(), wars.count() - on - won);
    }
    print_armies(sim);
}

/// In a world with coins: the treasuries' share of all coins, the largest realm's treasury against
/// its land's coins, and its capital's coins a person against the world's.
fn print_treasuries(sim: &Sim, alive: &[u32], biggest: u32, realm: &str) {
    let w = &sim.world;
    let g = &w.goods;
    let Some(coins) = g.tree.find("money").and_then(|m| g.columns_under(m)).filter(|c| !c.is_empty()) else { return };
    let held = |values: &[u64], row: usize| coins.iter().map(|&c| values.get(g.index(row, c)).copied().unwrap_or(0)).sum::<u64>() as f64;
    let in_provinces: f64 = (0..w.provinces.count).map(|p| held(&w.stock, p)).sum();
    let in_treasuries: f64 = (0..w.realms.count()).map(|r| held(&w.realm_stock, r)).sum();
    if in_provinces + in_treasuries <= 0.0 {
        return;
    }
    let dead: f64 = (0..w.realms.count()).filter(|&r| !alive.contains(&(r as u32))).map(|r| held(&w.realm_stock, r)).sum();
    let people = w.province_population();
    let land: f64 = (0..w.provinces.count).filter(|&p| w.owner[p] == biggest).map(|p| held(&w.stock, p)).sum();
    let all_people: u64 = people.iter().sum();
    let world_each = in_provinces / all_people.max(1) as f64;
    // Each capital's coins a person against the world's.
    let mut rich: Vec<(f64, u32)> = alive
        .iter()
        .filter(|&&r| people[w.realms.capital[r as usize] as usize] > 0)
        .map(|&r| {
            let c = w.realms.capital[r as usize] as usize;
            (held(&w.stock, c) / people[c] as f64 / world_each.max(1e-9), r)
        })
        .collect();
    rich.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (middle, most) = (rich.get(rich.len() / 2).map_or(0.0, |x| x.0), rich.last().map_or(0.0, |x| x.0));
    if let Some(&(_, r)) = rich.last() {
        let land = land_of(w, r);
        let c = w.realms.capital[r as usize] as usize;
        let realm_people: u64 = land.iter().map(|&p| people[p]).sum();
        println!(
            "richest capital: province {c} of realm {r} ({} provinces, {} people), with {} people and {} coins",
            land.len(),
            group(realm_people),
            group(people[c]),
            group(held(&w.stock, c) as u64)
        );
    }
    println!(
        "treasuries: {:.1}% of all coins ({} in dissolved realms); {realm}'s holds {:.1}% of what its provinces hold; capitals have {middle:.2} times the world's coins a person in the middle, {most:.2} at most (the world: {world_each:.2} a person)",
        100.0 * in_treasuries / (in_provinces + in_treasuries),
        group(dead as u64),
        100.0 * held(&w.realm_stock, biggest as usize) / land.max(1.0),
    );
}

fn land_of(w: &sim::world::World, r: u32) -> Vec<usize> {
    (0..w.provinces.count).filter(|&p| w.owner[p] == r).collect()
}

/// How rulers came to the throne so far, how long the reigns that ended lasted, and the largest
/// realm's ruler and heir.
fn print_rulers(sim: &Sim, biggest: u32, realm: &str) {
    use sim::characters::{title, traits_in_words, Succession, NEVER};
    let w = &sim.world;
    let ch = &w.characters;
    if ch.count() == 0 {
        return;
    }
    let year = sim.calendar.days_per_year();
    let mut kinds = [0usize; 5];
    let (mut reigns, mut deaths) = (Vec::new(), Vec::new());
    for c in 0..ch.count() {
        if ch.crowned[c] == NEVER {
            continue;
        }
        kinds[ch.succession[c] as usize] += 1;
        if ch.died[c] != NEVER {
            reigns.push((ch.died[c] - ch.crowned[c]) as f64 / year as f64);
            deaths.push(ch.age(c as u32, sim.day, year));
        }
    }
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    println!(
        "rulers: {} characters; came to the throne by founding {}, as a grown heir {}, as a child heir {}, as a claimant {}; {} reigns ended, lasting {:.1} years on average, the ruler dying at {:.1}",
        ch.count(),
        kinds[Succession::Founded as usize],
        kinds[Succession::Heir as usize],
        kinds[Succession::Child as usize],
        kinds[Succession::Claimant as usize],
        reigns.len(),
        mean(&reigns),
        mean(&deaths)
    );
    let describe = |c: u32| {
        let traits = traits_in_words(w, c);
        let traits = if traits.is_empty() { String::new() } else { format!(", {}", traits.join(", ")) };
        format!("{}, aged {:.0}{traits}", title(w, c), ch.age(c, sim.day, year))
    };
    let (ruler, heir) = (w.realms.ruler[biggest as usize], w.realms.heir[biggest as usize]);
    if ruler != sim::grid::NONE {
        let heir = if heir == sim::grid::NONE { "no heir".to_string() } else { format!("heir {}", describe(heir)) };
        println!("{realm} is ruled by {} ({}); {heir}", describe(ruler), Succession::from_u8(ch.succession[ruler as usize]).describe());
    }
}

/// 1234567 -> "1,234,567".
fn group(v: u64) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// How many serve, of which kinds, against the people; and how many came home in the last year.
fn print_armies(sim: &Sim) {
    let Some(params) = &sim.config.modules.warfare else { return };
    let w = &sim.world;
    let Ok(tree) = sim::taxonomy::Taxonomy::named(&params.tree, "troop tree") else { return };
    let mut by_kind = std::collections::BTreeMap::<String, u64>::new();
    for a in 0..w.armies.count() {
        if w.armies.size[a] > 0 {
            *by_kind.entry(tree.name(w.armies.kind[a]).to_string()).or_default() += w.armies.size[a];
        }
    }
    let soldiers: u64 = by_kind.values().sum();
    let people = quantity_total(w, Quantity::People).max(1);
    let companies = (0..w.armies.count()).filter(|&a| w.armies.size[a] > 0).count();
    let kinds: Vec<String> = by_kind.iter().map(|(k, n)| format!("{k} {}", group(*n))).collect();
    // Raising and going home cancel in the year's flows (booked out by one owner, in by the other),
    // so the arrivals are counted from the event log.
    let since = sim.day.saturating_sub(sim.calendar.days_per_year() as u64);
    let home = |cause| -> u64 {
        let log = sim.event_log.entries.iter().filter(|p| p.day >= since);
        log.filter_map(|p| match p.event {
            sim::events::Event::Arrived { people, cause: c, .. } if c == cause => Some(people),
            _ => None,
        })
        .sum()
    };
    println!(
        "armies: {} soldiers ({:.2}% of the people) in {companies} companies ({}); in the last year {} went home, {} deserted",
        group(soldiers),
        100.0 * soldiers as f64 / people as f64,
        kinds.join(", "),
        group(home(sim::requests::Cause::Returned)),
        group(home(sim::requests::Cause::Deserted)),
    );
}
