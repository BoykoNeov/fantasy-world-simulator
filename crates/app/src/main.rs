//! The map view: generate a world, run it some years, then watch it live, poke it and ask why.
//!
//! fantasymap [--size small|medium|large|huge] [--seed S] [--n FACE] [--provinces N] [--years Y]
//!            [--drought best|LON,LAT,RADIUS_KM,PERCENT,YEARS] [--days D] [--select best|P] [--explain]
//!            [--speed DAYS_PER_SECOND|max]
//!            [--projection oval|rect] [--view LON,LAT,ZOOM] [--layer NAME=VALUE]... [--good NAME]
//!            [--screenshot FILE.png] [--paint LON,LAT,RADIUS_KM,METRES] [--split best|P] [--undo]
//!            [--open FILE] [--modules SLOT=VERSION,...] [--remake SIZE,SEED,YEARS[,SLOT=VERSION...]]
//!            [--realms settled=W,city=W,frontier=W] [--rulers] [--wars] [--armies] [--treaties]
//!            [--bench --out DIR --tag NAME]
//!
//! Layer names: terrain, temperature, rainfall, population, famine, drought, provinces, prices,
//! trade, balance, realms, unrest (0..1), and coasts, rivers, borders, graticule, roads,
//! realm_borders (0 or 1); `--good` picks the good the price layer shows. `--view` takes degrees and a zoom (1 = planet
//! height fits). `--drought` pokes a drought after the first years (`best`: over the province that
//! grows the most food), `--days` runs that many more days before the window opens, `--select`
//! opens the inspector on a province, `--explain` explains its last change, and `--speed` starts
//! playing. `--paint`, `--split` and `--undo` edit the world through the window once it is open, in
//! the order given, as a player would (raise land along a short stroke; split half a province off
//! and select the new one; take back the last change), for scripted screenshots of an edited map.
//! `--open` shows a saved world instead of making one: a scenario (`.toml`) or a save of its
//! timelines (world options, `--years`, `--drought` and `--days` are then ignored). `--modules`
//! picks module versions, e.g. `economics=off,population=off` (see `sim::config::CHOICES`); the
//! Modules section shows what the combination tester found for the mix running. `--remake` makes a
//! new world through the New world section once the window is open, as a player would (the
//! versions given change those of the world shown). `--realms` switches realms on, with the ways
//! to start weighted as given (`--realms settled=1` is the default way); `--rulers` gives every
//! realm a ruler and heir, `--wars` keeps wars and truces, `--armies` has provinces raise troops,
//! and `--treaties` has realms choose war and make peace themselves (with wars and armies; each
//! turns realms on the default way if `--realms` isn't given), as in `simrun`.
//! GPU choice: set `WGPU_POWER_PREF=low` (integrated) or `high` (discrete) before running.

use app::bench;
use app::armies::ArmyView;
use app::editor::{Action, Clicked, Editor};
use app::realm_tools::{Kept, RealmTools};
use app::gpu::{self, LayerSettings, MapData, MapGpu, MapUniforms};
use app::inspector::{self, group, LogEntry, Names, RealmNames, Source};
use app::layers::{self, ProvinceView};
use app::playback::{Change, Clock, DayState, Frame, FromSim, MapState, Rate, Speed, Status, Worker};
use app::settings_ui::{self, Show};
use app::projection::{Camera, Projection};
use app::wobble::ProvinceLookup;
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use sim::branch::Timelines;
use sim::calendar::Calendar;
use sim::combos::{Table, Verdict};
use sim::config::{Config, CHOICES};
use sim::edit::{Edit, PopValues, SettingChange};
use sim::events::Event;
use sim::fields::{FieldId, Tier};
use sim::grid::NONE;
use sim::module::Slot;
use sim::sim::{Sim, Stage, Trace, NEVER};
use sim::world::{Size, World, WorldParams, RIVER_MIN_FLOW};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{mpsc, Arc};
use std::time::Instant;

/// An edit a scripted run makes through the window, as a player would.
#[derive(Clone)]
enum LiveEdit {
    Raise { lon: f64, lat: f64, radius_km: f64, metres: f32 },
    /// Half of a province (`None`: the one that grows the most food) becomes a new one, which is
    /// then selected.
    Split(Option<u32>),
    /// Take back the last change.
    Undo,
    /// Change a setting while the world runs.
    Set(String, f64),
}

/// Where a drought poke from the command line goes.
#[derive(Clone, Copy)]
enum DroughtAt {
    /// Over the province that grows the most food: 2,000 km radius, 70% of the rain, 3 years.
    Best,
    At { lon: f64, lat: f64, radius_km: f64, percent: f64, years: f64 },
}

struct Options {
    params: WorldParams,
    config: Config,
    size: Size,
    years: i64,
    drought: Option<DroughtAt>,
    days: u64,
    /// `Some(None)`: the province that grows the most food.
    select: Option<Option<u32>>,
    explain: bool,
    speed: Speed,
    projection: Projection,
    view: Option<(f64, f64, f64)>,
    layers: LayerSettings,
    /// The good the price layer shows.
    good: Option<String>,
    screenshot: Option<String>,
    /// A world to open instead of making one.
    open: Option<String>,
    /// Edits made through the window once it is open (scripted runs): see `--paint` and `--split`.
    live_edits: Vec<LiveEdit>,
    /// A new world made through the window once it is open (scripted runs): see `--remake`.
    remake: Option<String>,
    /// Settings changed in the world made (`--set KEY=VALUE`).
    set: Vec<(String, f64)>,
    /// Section headers to open (scripted runs): "settings", "new-world", "new-world-settings", "change-realm" (the editor's controls on the selected province's realm page) and "wars".
    open_sections: Vec<String>,
    bench: bool,
    out_dir: String,
    tag: String,
}

impl Options {
    fn from_args() -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut size = Size::Large;
        let mut seed = 7;
        let (mut n, mut provinces) = (None, None);
        let mut o = Options {
            params: WorldParams::preset(Size::Large, 7),
            config: Config::new(WorldParams::preset(Size::Large, 7)),
            size,
            years: 20,
            drought: None,
            days: 0,
            select: None,
            explain: false,
            speed: Speed::Paused,
            projection: Projection::EqualEarth,
            view: None,
            layers: LayerSettings::default(),
            good: None,
            screenshot: None,
            open: None,
            live_edits: Vec::new(),
            remake: None,
            set: Vec::new(),
            open_sections: Vec::new(),
            bench: false,
            out_dir: "W:/temp/claude/fantasymap-mapview/bench".into(),
            tag: "default".into(),
        };
        let mut i = 0;
        let value = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_else(|| panic!("{} needs a value", args[*i - 1]))
        };
        let numbers = |v: &str, n: usize, what: &str| -> Vec<f64> {
            let v: Vec<f64> = v.split(',').map(|x| x.trim().parse().unwrap_or_else(|_| panic!("{what}"))).collect();
            assert_eq!(v.len(), n, "{what}");
            v
        };
        while i < args.len() {
            match args[i].as_str() {
                "--size" => {
                    size = match value(&mut i).as_str() {
                        "small" => Size::Small,
                        "medium" => Size::Medium,
                        "large" => Size::Large,
                        "huge" => Size::Huge,
                        other => panic!("unknown size {other}"),
                    }
                }
                "--seed" => seed = value(&mut i).parse().expect("--seed"),
                "--n" => n = Some(value(&mut i).parse().expect("--n")),
                "--provinces" => provinces = Some(value(&mut i).parse().expect("--provinces")),
                "--years" => o.years = value(&mut i).parse().expect("--years"),
                "--terrain" => o.config.modules.terrain = value(&mut i).parse().unwrap_or_else(|e| panic!("{e}")),
                "--climate" => o.config.modules.climate = value(&mut i).parse().unwrap_or_else(|e| panic!("{e}")),
                "--modules" => sim::combos::choose(&mut o.config.modules, &value(&mut i)).unwrap_or_else(|e| panic!("{e}")),
                "--drought" => {
                    let v = value(&mut i);
                    o.drought = Some(if v == "best" {
                        DroughtAt::Best
                    } else {
                        let x = numbers(&v, 5, "--drought best|LON,LAT,RADIUS_KM,PERCENT,YEARS");
                        DroughtAt::At { lon: x[0], lat: x[1], radius_km: x[2], percent: x[3], years: x[4] }
                    });
                }
                "--days" => o.days = value(&mut i).parse().expect("--days"),
                "--select" => {
                    let v = value(&mut i);
                    o.select = Some(if v == "best" { None } else { Some(v.parse().expect("--select best|PROVINCE")) });
                }
                "--explain" => o.explain = true,
                "--speed" => {
                    let v = value(&mut i);
                    o.speed = if v == "max" { Speed::Max } else { Speed::DaysPerSecond(v.parse().expect("--speed DAYS_PER_SECOND|max")) };
                }
                "--projection" => {
                    o.projection = match value(&mut i).as_str() {
                        "oval" => Projection::EqualEarth,
                        "rect" => Projection::Rectangle,
                        other => panic!("unknown projection {other} (oval or rect)"),
                    }
                }
                "--view" => {
                    let v = numbers(&value(&mut i), 3, "--view LON,LAT,ZOOM");
                    o.view = Some((v[0], v[1], v[2]));
                }
                "--layer" => {
                    let v = value(&mut i);
                    let (name, x) = v.split_once('=').expect("--layer NAME=VALUE");
                    let x: f32 = x.parse().expect("--layer value");
                    let l = &mut o.layers;
                    match name {
                        "terrain" => l.terrain = x,
                        "temperature" => l.temperature = x,
                        "rainfall" => l.rainfall = x,
                        "population" => l.population = x,
                        "famine" => l.famine = x,
                        "drought" => l.drought = x,
                        "prices" => l.prices = x,
                        "trade" => l.trade = x,
                        "balance" => l.trade_balance = x,
                        "realms" => l.realms = x,
                        "unrest" => l.unrest = x,
                        "realm_borders" => l.realm_borders = x > 0.5,
                        "armies" => l.armies = x > 0.5,
                        "battles" => l.battles = x > 0.5,
                        "roads" => l.roads = x > 0.5,
                        "provinces" => l.provinces = x,
                        "coasts" => l.coasts = x > 0.5,
                        "rivers" => l.rivers = x > 0.5,
                        "borders" => l.borders = x > 0.5,
                        "graticule" => l.graticule = x > 0.5,
                        other => panic!("unknown layer {other}"),
                    }
                }
                "--good" => o.good = Some(value(&mut i)),
                "--screenshot" => o.screenshot = Some(value(&mut i)),
                "--open" => o.open = Some(value(&mut i)),
                "--paint" => {
                    let x = numbers(&value(&mut i), 4, "--paint LON,LAT,RADIUS_KM,METRES");
                    o.live_edits.push(LiveEdit::Raise { lon: x[0], lat: x[1], radius_km: x[2], metres: x[3] as f32 });
                }
                "--undo" => o.live_edits.push(LiveEdit::Undo),
                "--set" | "--live-set" => {
                    let which = args[i].clone();
                    let kv = value(&mut i);
                    let (key, x) = kv.split_once('=').unwrap_or_else(|| panic!("{which} {kv}: expected KEY=VALUE"));
                    let x: f64 = x.parse().unwrap_or_else(|_| panic!("{which} {kv}: the value is a number"));
                    if which == "--set" {
                        o.set.push((key.into(), x));
                    } else {
                        o.live_edits.push(LiveEdit::Set(key.into(), x));
                    }
                }
                "--open-section" => o.open_sections.push(value(&mut i)),
                "--split" => {
                    let v = value(&mut i);
                    o.live_edits.push(LiveEdit::Split(if v == "best" { None } else { Some(v.parse().expect("--split best|PROVINCE")) }));
                }
                "--remake" => o.remake = Some(value(&mut i)),
                "--realms" => {
                    let mut r = sim::realms::RealmParams { settled: 0.0, ..Default::default() };
                    for part in value(&mut i).split(',') {
                        let (way, weight) = part.split_once('=').unwrap_or_else(|| panic!("--realms: {part}: expected way=weight"));
                        let weight: f64 = weight.parse().expect("--realms weight");
                        match way {
                            "settled" => r.settled = weight,
                            "city" => r.city_states = weight,
                            "frontier" => r.frontier = weight,
                            _ => panic!("--realms: no way {way}; expected settled, city or frontier"),
                        }
                    }
                    o.config.modules.realms = Some(r);
                }
                "--rulers" => {
                    o.config.modules.realms.get_or_insert_with(Default::default);
                    o.config.modules.characters = Some(Default::default());
                }
                "--wars" => {
                    o.config.modules.realms.get_or_insert_with(Default::default);
                    o.config.modules.diplomacy = Some(Default::default());
                }
                "--armies" => {
                    o.config.modules.realms.get_or_insert_with(Default::default);
                    o.config.modules.warfare = Some(Default::default());
                }
                "--treaties" => {
                    o.config.modules.realms.get_or_insert_with(Default::default);
                    o.config.modules.warfare.get_or_insert_with(Default::default);
                    o.config.modules.diplomacy.get_or_insert_with(Default::default).treaties = Some(Default::default());
                }
                "--bench" => o.bench = true,
                "--out" => o.out_dir = value(&mut i),
                "--tag" => o.tag = value(&mut i),
                other => panic!("unknown argument {other}"),
            }
            i += 1;
        }
        o.size = size;
        o.params = WorldParams::preset(size, seed);
        if let Some(n) = n {
            o.params.n = n;
        }
        if let Some(p) = provinces {
            o.params.provinces = p;
        }
        o.config.world = o.params.clone();
        for (key, x) in &o.set {
            sim::settings::set(&mut o.config, key, *x).unwrap_or_else(|e| panic!("--set {key}={x}: {e}"));
        }
        o.params = o.config.world.clone();
        o
    }
}

fn main() -> eframe::Result {
    let opts = Options::from_args();
    let scripted = opts.bench || opts.screenshot.is_some();
    let mut native = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_title("Fantasy Planet").with_inner_size([1920.0, 1080.0]).with_resizable(!scripted),
        ..Default::default()
    };
    if opts.bench {
        // Vsync off so frame times measure work, not the monitor.
        native.wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    eframe::run_native("fantasymap", native, Box::new(move |cc| Ok(Box::new(MapApp::new(cc, opts)))))
}

/// Speeds on the time bar, as days a second.
const SPEEDS: [(&str, Speed); 5] = [
    ("1 day/s", Speed::DaysPerSecond(1.0)),
    ("1 week/s", Speed::DaysPerSecond(6.0)),
    ("1 month/s", Speed::DaysPerSecond(30.0)),
    ("1 year/s", Speed::DaysPerSecond(364.0)),
    ("fastest", Speed::Max),
];

/// The drought tool's settings.
struct DroughtTool {
    percent: f64,
    radius_km: f64,
    years: f64,
}

/// The versions running, and what the combination tester's table says about that mix (`None`: not
/// in it). A world keeps its versions, so this is worked out once.
struct ModulesInfo {
    versions: Vec<(String, &'static str)>,
    verdict: Result<Option<(Verdict, Vec<String>)>, String>,
    /// What the tester's runs were: the verdict says nothing about other worlds.
    tested_on: String,
}

/// What the window works out once for each world.
struct WorldInfo {
    /// Per field: which module sets it, in words, and the owning slot.
    owners: Vec<(String, Option<Slot>)>,
    modules: ModulesInfo,
    /// The columns of the per-good fields that hold coins.
    coin_columns: Vec<usize>,
    /// The good the price layer shows first (`good`, or else the first good that is priced).
    price_good: usize,
    /// Whether the world trades: markets with trade on.
    trades: bool,
    /// The troop kinds, where armies are kept: node, name, whether ships.
    troop_kinds: Vec<app::armies::Kind>,
}

impl WorldInfo {
    fn of(sim: &Sim, table: &Result<Table, String>, good: Option<&str>) -> WorldInfo {
        let manifests = sim.manifests();
        let owners = FieldId::all()
            .map(|id| match sim.plan.owner[id.0 as usize] {
                Some(o) => (format!("{} \"{}\"", inspector::slot_name(manifests[o].slot), manifests[o].version), Some(manifests[o].slot)),
                None => ("nobody (a default)".into(), None),
            })
            .collect();
        let mc = &sim.config.modules;
        let versions: Vec<(String, &'static str)> = CHOICES.iter().map(|c| (inspector::slot_name(c.0).to_string(), mc.choice(c.0).expect("a slot with a choice"))).collect();
        let verdict = table.as_ref().map(|t| t.verdict_for(mc)).map_err(|e| e.clone());
        let modules = ModulesInfo { versions, verdict, tested_on: tested_on(table) };
        let goods = &sim.world.goods;
        let coin_columns = mc.markets.as_ref().and_then(|m| goods.columns_under(goods.tree.find(&m.money)?)).unwrap_or_default();
        let price_good = match good {
            Some(name) => goods.column(name).filter(|c| !coin_columns.contains(c)).unwrap_or_else(|| panic!("--good {name}: this world doesn't price it")),
            None => (0..goods.stride()).find(|c| !coin_columns.contains(c)).unwrap_or(0),
        };
        let troop_kinds = mc.warfare.as_ref().map(sim::warfare::kinds).unwrap_or_default();
        WorldInfo { owners, modules, coin_columns, price_good, trades: mc.markets.as_ref().is_some_and(|m| m.trade.is_some()), troop_kinds }
    }
}

/// What the combination tester's runs were: its verdict says nothing about other worlds.
fn tested_on(table: &Result<Table, String>) -> String {
    table.as_ref().map_or(String::new(), |t| {
        let s = &t.settings;
        format!(
            "Only tested on small worlds ({} provinces, face {}), for {} years on {} seeds, with default settings. Larger worlds, longer runs and changed settings were not tested: this is \"no problems found\", not \"proven safe\".",
            s.provinces,
            s.n,
            s.years,
            s.seeds.len()
        )
    })
}

/// A world made on another thread: its timelines, its picture and how long it took, or why it
/// couldn't be made.
type Made = Result<(Timelines, MapData, f64), String>;

/// The New world section: the size, seed, years run first, module versions and settings picked, and
/// a world being made from them.
struct NewWorld {
    size: Size,
    seed: u64,
    years: i64,
    /// The world to make: its world settings, versions and module settings (the seed is `seed`).
    config: Config,
    /// Why the last setting typed was refused.
    refused: Option<String>,
    /// "make it" was pressed once: the next press replaces the world.
    confirm: bool,
    /// A world being made on another thread, since when.
    making: Option<(Instant, mpsc::Receiver<Made>)>,
    /// Why the last world couldn't be made.
    failed: Option<String>,
}

impl NewWorld {
    /// Picks that start as the world shown: `config`, with the settings in force on the day shown.
    fn like(opts: &Options, config: &Config) -> NewWorld {
        let size = [Size::Small, Size::Medium, Size::Large, Size::Huge].into_iter().find(|&z| WorldParams::preset(z, 0).n == config.world.n).unwrap_or(opts.size);
        NewWorld { size, seed: config.world.seed, years: opts.years, config: config.clone(), refused: None, confirm: false, making: None, failed: None }
    }

    /// A world size: the map detail and province count it has.
    fn set_size(&mut self, size: Size) {
        let p = WorldParams::preset(size, 0);
        (self.size, self.config.world.n, self.config.world.provinces) = (size, p.n, p.provinces);
    }

    /// Make the world picked on another thread: generate it, run it its years, and build its
    /// picture. `done` is called when it is ready.
    fn make(&mut self, done: impl Fn() + Send + 'static) {
        let mut config = self.config.clone();
        config.world.seed = self.seed;
        let years = self.years;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let made = Sim::new(&config).map(|mut sim| {
                sim.run_to_year(sim.calendar.def.first_year + years);
                let data = MapData::build(&sim);
                (Timelines::new(sim), data, t0.elapsed().as_secs_f64())
            }).map_err(|e| e.to_string());
            let _ = tx.send(made);
            done();
        });
        (self.making, self.confirm, self.failed) = (Some((Instant::now(), rx)), false, None);
    }
}

struct MapApp {
    opts: Options,
    /// The world on screen: cells and provinces as generated, and the running values of the day
    /// shown. The simulation itself lives on the worker thread.
    geo: World,
    calendar: Calendar,
    /// Per field: which module sets it, in words, and the owning slot.
    owners: Vec<(String, Option<Slot>)>,
    modules: ModulesInfo,
    worker: Worker,
    /// The day shown first, then the days computed after it.
    frames: VecDeque<Frame>,
    /// Bumped by every poke; older frames are stale.
    epoch: u64,
    clock: Clock,
    rate: Rate,
    /// The day state copied into `geo`.
    shown_state: Arc<DayState>,
    /// The map drawn now. After an edit it differs from the shown day's map until the new
    /// picture is built.
    drawn_map: Arc<MapState>,
    /// The picture of a map being built on another thread.
    rebuild: Option<(Arc<MapState>, mpsc::Receiver<MapData>)>,
    /// A built picture, waiting for the next paint callback to put it on the GPU.
    pending_gpu: Option<Arc<MapData>>,
    /// Provinces the GPU's province tables have room for.
    gpu_provinces: usize,
    target: wgpu::TextureFormat,
    repaint: egui::Context,
    status: Status,
    /// What the simulation last refused or left out, and when.
    notice: Option<(String, Instant)>,
    /// Per timeline, the furthest day watched: a change before it starts a new timeline.
    seen: HashMap<u32, u64>,
    /// Per timeline, the day shown when it was left.
    left_at: HashMap<u32, u64>,
    /// A change, jump or switch asked for and not yet answered, and since when.
    waiting: Option<Instant>,
    /// The day on the history slider while it is dragged.
    scrub: Option<u64>,
    editor: Editor,
    /// The file the File section saves to and opens.
    file: String,
    /// The worlds opened so far, as the worker counts them.
    opened: u32,
    /// A pop being edited in the inspector, and the size typed so far.
    pop_edit: Option<(usize, u64)>,
    /// The size of a pop to add, as typed in the inspector.
    new_pop: u64,
    /// The good whose store the inspector sets (its column), and the amount typed so far.
    stock_good: usize,
    stock_edit: Option<u64>,
    /// The columns of the per-good fields that hold coins.
    coin_columns: Vec<usize>,
    /// The good the price layer shows (its column).
    price_good: usize,
    /// What the economy layers show, and the day state and good they were worked out for.
    market: layers::MarketView,
    market_for: Option<(usize, usize)>,
    /// Whether the world trades (markets with trade on): the trade layers are offered.
    trades: bool,
    /// The troop kinds, where armies are kept: node, name, whether ships.
    troop_kinds: Vec<app::armies::Kind>,
    /// What is typed into the editor's controls on the realm and province pages.
    realm_tools: RealmTools,
    /// Armies, battles today and battles of the last month, for the day shown they were worked out
    /// for (by address).
    army_view: Option<(usize, ArmyView)>,
    /// The combination tester's table, shipped with the simulation.
    table: Result<Table, String>,
    /// The New world section: what is picked, and a world being made.
    new_world: NewWorld,
    /// The configuration the world shown was made with (its settings in force are the day's).
    config: Config,
    /// Settings changed in the Settings section and not yet applied.
    live_draft: Option<Config>,
    /// How many settings in force differ from the defaults, for the settings it was worked out for
    /// (by address): working it out goes through every setting.
    changed_count: (usize, usize),
    /// The trade record and map the trade lines on the GPU were built for, and lines waiting for
    /// the next paint callback to upload them.
    trade_lines_for: Option<(usize, usize)>,
    pending_trade: Option<Vec<app::geometry::LineInstance>>,
    /// The drawn map's province borders, ready to be drawn as borders between realms; who owned
    /// what, and the map, when the borders on the GPU were copied; borders waiting for the next
    /// paint callback to upload them.
    realm_borders: Arc<app::geometry::RealmBorders>,
    realm_lines_for: Option<(Vec<u32>, usize)>,
    pending_realm: Option<Vec<app::geometry::LineInstance>>,
    realm_names: RealmNames,
    /// What the province layers show right now (blended between days).
    view: ProvinceView,
    tables_for: Option<(u64, u64, u64)>,
    /// Province tables waiting for the next paint callback to upload them.
    pending_tables: Option<Vec<layers::Rgba>>,
    /// Everything that happened on the days shown so far, oldest first.
    log: Vec<LogEntry>,
    hidden_sources: HashSet<Source>,
    selected: Option<u32>,
    /// A trace asked for (day, province), and the latest answer.
    trace_pending: Option<(u64, u32)>,
    trace: Option<((u64, u32), Result<Trace, String>)>,
    drought: DroughtTool,
    run_to_year: i64,
    adapter: String,
    line_segments: usize,
    gen_s: f64,
    cam: Option<Camera>,
    projection: Projection,
    layers: LayerSettings,
    /// Cell under the pointer, and the province drawn there (NONE at sea).
    hover: Option<(usize, u32)>,
    /// Finds the province drawn under the pointer, as the GPU does.
    provinces: ProvinceLookup,
    // Scripted runs.
    frame: u32,
    last: Option<Instant>,
    last_dt_ms: f64,
    stats: bench::Stats,
    screenshot_sent: bool,
    bench_target: (f64, f64),
}

/// A new world from the command line's options, run for its years, with its drought.
fn make_world(opts: &Options, drought: &mut DroughtTool) -> Sim {
    let mut sim = Sim::new(&opts.config).unwrap_or_else(|e| panic!("{e}"));
    let first = sim.calendar.def.first_year;
    sim.run_to_year(first + opts.years);
    if let Some(at) = opts.drought {
        let (lon, lat) = match at {
            DroughtAt::Best => {
                let (lon, lat) = sim::grid::dir_to_lonlat(sim.world.provinces.center[breadbasket(&sim.world)]);
                (lon.to_degrees(), lat.to_degrees())
            }
            DroughtAt::At { lon, lat, radius_km, percent, years } => {
                *drought = DroughtTool { percent, radius_km, years };
                (lon, lat)
            }
        };
        let e = drought_event(lon, lat, drought, &sim.calendar);
        sim.poke(e);
    }
    sim.run_days(opts.days);
    sim
}

/// The province that grows the most food.
fn breadbasket(w: &World) -> usize {
    let p = &w.provinces;
    (0..p.count).max_by_key(|&i| p.food_capacity[i]).unwrap_or(0)
}

fn drought_event(lon: f64, lat: f64, t: &DroughtTool, cal: &Calendar) -> Event {
    let days = (t.years * cal.days_per_year() as f64).round() as u64;
    Event::Drought { lon, lat, radius_km: t.radius_km, shortfall: (t.percent * 10.0).round() as u64, days }
}

impl MapApp {
    fn new(cc: &eframe::CreationContext<'_>, opts: Options) -> Self {
        let t0 = Instant::now();
        let mut drought = DroughtTool { percent: 70.0, radius_km: 2000.0, years: 3.0 };
        let timelines = match &opts.open {
            Some(file) => Timelines::open(file.as_ref()).unwrap_or_else(|e| panic!("{e}")),
            None => Timelines::new(make_world(&opts, &mut drought)),
        };
        let sim = timelines.sim();
        let gen_s = t0.elapsed().as_secs_f64();
        let data = MapData::build(sim);

        let rs = cc.wgpu_render_state.as_ref().expect("wgpu renderer");
        let gpu = MapGpu::new(&rs.device, &rs.queue, rs.target_format, &data);
        rs.renderer.write().callback_resources.insert(gpu);
        let info = rs.adapter.get_info();
        // The most populous province, for the benchmark zoom.
        let people = sim.world.province_population();
        let top = (0..people.len()).max_by_key(|&p| people[p]).unwrap_or(0);
        let bench_target = sim.world.provinces.center.get(top).map_or((0.0, 0.0), |&d| sim::grid::dir_to_lonlat(d));

        let table = Table::shipped();
        let WorldInfo { owners, modules, coin_columns, price_good, trades, troop_kinds } = WorldInfo::of(sim, &table, opts.good.as_deref());
        let new_world = NewWorld::like(&opts, &Config { modules: (*sim.settings).clone(), ..sim.config.clone() });
        let config = sim.config.clone();
        let frame = Frame::now(sim, 0);
        let first_day = sim.day;
        let selected = opts.select.map(|s| s.unwrap_or(breadbasket(&sim.world) as u32));
        // On the command line, explain the last day the province changed.
        let trace_pending = opts.explain.then(|| {
            let p = selected.unwrap_or(breadbasket(&sim.world) as u32);
            let day = inspector::latest_change(&sim.last_change, sim.running_fields(), &sim.world.pops.province, &sim.world.goods, p);
            (day.unwrap_or(sim.day.saturating_sub(1)), p)
        });
        let ctx = cc.egui_ctx.clone();
        let app = MapApp {
            geo: sim.world.clone(),
            calendar: sim.calendar.clone(),
            owners,
            modules,
            clock: Clock { speed: opts.speed, ..Clock::new(sim.day) },
            run_to_year: sim.calendar.date(sim.day).year + 5,
            log: sim.event_log.entries.iter().map(|p| LogEntry::of_event(p, &sim.world.goods)).collect(),
            shown_state: frame.state.clone(),
            drawn_map: frame.state.map.clone(),
            rebuild: None,
            pending_gpu: None,
            gpu_provinces: data.provinces,
            target: rs.target_format,
            repaint: cc.egui_ctx.clone(),
            status: Status::default(),
            notice: None,
            seen: HashMap::from([(timelines.active_id(), first_day)]),
            left_at: HashMap::new(),
            waiting: None,
            scrub: None,
            editor: Editor::default(),
            file: opts.open.clone().unwrap_or_else(|| "fantasy_world.toml".into()),
            opened: 0,
            pop_edit: None,
            new_pop: 1000,
            stock_good: 0,
            stock_edit: None,
            coin_columns,
            troop_kinds,
            realm_tools: RealmTools::default(),
            army_view: None,
            price_good,
            market: layers::MarketView::default(),
            market_for: None,
            trades,
            table,
            new_world,
            config,
            live_draft: None,
            changed_count: (0, 0),
            trade_lines_for: None,
            pending_trade: None,
            realm_borders: data.realm_borders.clone(),
            realm_lines_for: None,
            pending_realm: None,
            realm_names: RealmNames::default(),
            view: frame.state.view.clone(),
            frames: VecDeque::from([frame]),
            worker: Worker::start_timelines(timelines, move || ctx.request_repaint()),
            epoch: 0,
            rate: Rate::default(),
            tables_for: None,
            pending_tables: None,
            hidden_sources: HashSet::new(),
            selected,
            trace_pending,
            trace: None,
            drought,
            projection: opts.projection,
            layers: opts.layers,
            opts,
            adapter: format!("{} ({:?})", info.name, info.backend),
            line_segments: data.lines.len(),
            gen_s,
            cam: None,
            hover: None,
            provinces: ProvinceLookup { n: data.n, ids: data.province_ids.clone(), warp: data.warp.clone() },
            frame: 0,
            last: None,
            last_dt_ms: 0.0,
            stats: bench::Stats::default(),
            screenshot_sent: false,
            bench_target,
        };
        if let Some((day, p)) = app.trace_pending {
            app.worker.trace(day, p);
        }
        app
    }

    fn day(&self) -> u64 {
        self.frames[0].day
    }

    /// Show a new world in place of this one: its timelines, and its picture (`data`), made in
    /// `gen_s` seconds. The old world's simulation stops, and nothing of it is kept.
    fn adopt(&mut self, timelines: Timelines, data: MapData, gen_s: f64) {
        let sim = timelines.sim();
        let WorldInfo { owners, modules, coin_columns, price_good, trades, troop_kinds } = WorldInfo::of(sim, &self.table, None);
        (self.troop_kinds, self.realm_tools, self.army_view) = (troop_kinds, RealmTools::default(), None);
        let frame = Frame::now(sim, 0);
        (self.owners, self.modules, self.coin_columns, self.price_good, self.trades) = (owners, modules, coin_columns, price_good, trades);
        self.geo = sim.world.clone();
        self.calendar = sim.calendar.clone();
        (self.config, self.live_draft) = (sim.config.clone(), None);
        self.clock = Clock::new(sim.day);
        self.run_to_year = sim.calendar.date(sim.day).year + 5;
        self.log = sim.event_log.entries.iter().map(|p| LogEntry::of_event(p, &sim.world.goods)).collect();
        self.seen = HashMap::from([(timelines.active_id(), sim.day)]);
        self.left_at.clear();
        (self.shown_state, self.drawn_map, self.view) = (frame.state.clone(), frame.state.map.clone(), frame.state.view.clone());
        self.frames = VecDeque::from([frame]);
        (self.epoch, self.opened, self.status, self.rate) = (0, 0, Status::default(), Rate::default());
        (self.waiting, self.scrub, self.rebuild) = (None, None, None);
        (self.selected, self.trace_pending, self.trace, self.hover) = (None, None, None, None);
        // Framed afresh, as a world is when the window opens.
        self.cam = None;
        (self.pop_edit, self.stock_good, self.stock_edit) = (None, 0, None);
        (self.market, self.market_for, self.tables_for, self.trade_lines_for) = (layers::MarketView::default(), None, None, None);
        (self.pending_tables, self.pending_trade) = (None, None);
        (self.realm_borders, self.realm_lines_for, self.pending_realm, self.realm_names) = (data.realm_borders.clone(), None, None, RealmNames::default());
        self.provinces = ProvinceLookup { n: data.n, ids: data.province_ids.clone(), warp: data.warp.clone() };
        (self.line_segments, self.gpu_provinces, self.gen_s) = (data.lines.len(), data.provinces, gen_s);
        self.pending_gpu = Some(Arc::new(data));
        let ctx = self.repaint.clone();
        let old = std::mem::replace(&mut self.worker, Worker::start_timelines(timelines, move || ctx.request_repaint()));
        // The old simulation finishes the day it is on before it stops: not on the window's time.
        std::thread::spawn(move || drop(old));
    }

    /// Take in what the simulation sent.
    fn pump(&mut self) {
        while let Ok(m) = self.worker.rx.try_recv() {
            match m {
                // After a change, a jump or a switch: the day it lands on replaces the day on screen.
                FromSim::Frame(f) if f.epoch == self.epoch && f.replaces.is_some() => {
                    self.log = f.replaces.iter().flatten().map(|p| LogEntry::of_event(p, &self.geo.goods)).collect();
                    self.clock.pos = f.day as f64;
                    self.frames.clear();
                    self.frames.push_back(f);
                    self.waiting = None;
                }
                FromSim::Frame(f) if f.epoch == self.epoch => self.frames.push_back(f),
                FromSim::Frame(_) => {}
                FromSim::Status(s) => {
                    // Another world opened: nothing watched in it yet.
                    if s.opened != self.opened {
                        self.opened = s.opened;
                        self.seen.clear();
                        self.left_at.clear();
                    }
                    if let Some(m) = &s.message {
                        self.notice = Some((m.clone(), Instant::now()));
                    }
                    self.status = s;
                }
                FromSim::Trace(t) => {
                    // An answer says which day and province it explains; a later question may
                    // still be waiting.
                    let key = t.as_ref().map(|t| (t.day, t.province)).ok().or(self.trace_pending);
                    if let Some(key) = key {
                        if self.trace_pending == Some(key) || t.is_err() {
                            self.trace_pending = None;
                        }
                        self.trace = Some((key, t));
                    }
                }
            }
        }
    }

    /// Move the clock on, and with it the day shown; then refresh what the map shows.
    fn play(&mut self, dt: f64) {
        let last = self.frames.back().map_or(0, |f| f.day);
        self.clock.advance(dt, last);
        let day = self.clock.day();
        while self.frames.len() >= 2 && self.frames[1].day <= day {
            self.frames.pop_front();
            self.absorb();
        }
        self.worker.show(self.day());
        self.rate.record(Instant::now(), self.day());

        let today = self.day();
        let seen = self.seen.entry(self.status.active).or_insert(0);
        *seen = (*seen).max(today);

        let front = &self.frames[0];
        if !Arc::ptr_eq(&self.shown_state, &front.state) {
            // The map first: it sets how many provinces there are.
            if !Arc::ptr_eq(&self.shown_state.map, &front.state.map) {
                for (id, data) in &front.state.map.fields {
                    self.geo.set_field(*id, data.clone()).expect("a map of this world");
                }
                self.geo.routes = front.state.map.routes.clone();
            }
            for (id, data) in &front.state.fields {
                self.geo.set_field(*id, data.clone()).expect("the worker's world has the same shape");
            }
            self.shown_state = front.state.clone();
            // Fewer provinces now (an undone split, going back past one, another timeline): a
            // selected province past the end is gone.
            if self.selected.is_some_and(|p| p as usize >= self.geo.provinces.count) {
                (self.selected, self.pop_edit, self.stock_edit) = (None, None, None);
            }
        }
        self.rebuild_map();
        let front = &self.frames[0];
        let t = self.clock.blend();
        // Days on different maps (an edit between them) aren't blended: their provinces differ.
        let next = self.frames.get(1).filter(|n| n.day == front.day + 1 && t > 0.0 && Arc::ptr_eq(&n.state.map, &front.state.map));
        let key = (front.day, next.map_or(0, |_| t.to_bits()), Arc::as_ptr(&front.state) as u64 ^ self.epoch);
        // Prices change once a month: worked out again only for a new day state or another good.
        let market_key = (Arc::as_ptr(&self.shown_state) as usize, self.price_good);
        if self.market_for != Some(market_key) {
            let price = sim::fields::known("province.price");
            let state = &self.shown_state;
            let k = self.geo.goods.stride();
            let coins: Vec<u64> = match self.coin_columns.first() {
                Some(&c) => (0..self.geo.provinces.count).map(|q| self.geo.stock.get(q * k + c).copied().unwrap_or(0)).collect(),
                None => Vec::new(),
            };
            self.market = layers::MarketView {
                price: match state.fields.iter().any(|f| f.0 == price) {
                    true => layers::relative_prices(&self.geo.price, k, self.price_good, &self.geo.province_population(), state.money_levels.as_deref()),
                    false => Vec::new(),
                },
                balance: layers::trade_balance(&state.trade.shipments, &coins, self.geo.provinces.count),
            };
            self.market_for = Some(market_key);
            self.tables_for = None;
        }
        // Trade lines: for each month's trade, on the map drawn, when they are shown.
        let lines_key = (Arc::as_ptr(&self.shown_state.trade) as usize, Arc::as_ptr(&self.drawn_map) as usize);
        if self.layers.trade > 0.0 && self.trade_lines_for != Some(lines_key) && self.geo.provinces.count == self.gpu_provinces {
            let net = layers::net_trade(&self.shown_state.trade.shipments);
            self.pending_trade = Some(app::geometry::instances(&app::geometry::trade_chains(&self.geo, &net)).0);
            self.trade_lines_for = Some(lines_key);
        }
        // Borders between realms: copied again from the province borders whenever who owns what
        // changes, on the map drawn.
        let drawn = Arc::as_ptr(&self.drawn_map) as usize;
        let stale = self.realm_lines_for.as_ref().is_none_or(|(owner, map)| *map != drawn || *owner != self.geo.owner);
        if self.layers.realm_borders && stale && self.geo.provinces.count == self.gpu_provinces {
            self.pending_realm = Some(self.realm_borders.lines(&self.geo.owner));
            self.realm_lines_for = Some((self.geo.owner.clone(), drawn));
        }
        self.realm_names.update(&self.geo);
        if self.tables_for != Some(key) {
            self.view = match next {
                Some(n) => ProvinceView::blend(&front.state.view, &n.state.view, t),
                None => front.state.view.clone(),
            };
            // Until a changed map's picture is ready, the GPU's tables have the old map's size.
            if self.geo.provinces.count == self.gpu_provinces {
                self.pending_tables = Some(layers::province_tables(&self.geo, &self.view, &self.market));
                self.tables_for = Some(key);
            }
        }
    }

    /// Keep the picture in step with the shown day's map: build a changed map's picture on another
    /// thread, and once it is ready swap it in, picking with it, in the same frame.
    fn rebuild_map(&mut self) {
        let wanted = self.shown_state.map.clone();
        if let Some((map, rx)) = &self.rebuild {
            match rx.try_recv() {
                Ok(data) => {
                    let map = map.clone();
                    self.rebuild = None;
                    if Arc::ptr_eq(&map, &wanted) {
                        self.provinces = ProvinceLookup { n: data.n, ids: data.province_ids.clone(), warp: data.warp.clone() };
                        self.line_segments = data.lines.len();
                        self.gpu_provinces = data.provinces;
                        self.realm_borders = data.realm_borders.clone();
                        self.pending_gpu = Some(Arc::new(data));
                        self.drawn_map = map;
                        // The new picture starts with no trade lines and no realm borders.
                        (self.tables_for, self.trade_lines_for, self.realm_lines_for) = (None, None, None);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.rebuild = None,
            }
        }
        if self.rebuild.is_none() && !Arc::ptr_eq(&wanted, &self.drawn_map) {
            let (world, view) = (self.geo.clone(), self.shown_state.view.clone());
            let (tx, rx) = mpsc::channel();
            let ctx = self.repaint.clone();
            std::thread::spawn(move || {
                let _ = tx.send(MapData::of(&world, &view));
                ctx.request_repaint();
            });
            self.rebuild = Some((wanted, rx));
        }
    }

    /// The front frame was just reached: log what happened the day before it.
    fn absorb(&mut self) {
        let f = &self.frames[0];
        self.log.extend(f.delivered.iter().map(|p| LogEntry::of_event(p, &self.geo.goods)));
        for (r, g) in f.applied.iter().filter(|(r, g)| *g < r.take) {
            let owner = self.owners[r.field.0 as usize].1.unwrap_or(r.requester);
            self.log.push(LogEntry::rationed(f.day - 1, owner, r, *g));
        }
        // Years of shortages add up; the oldest go first.
        if self.log.len() > 400_000 {
            self.log.drain(..100_000);
        }
    }

    /// Poke the world on the day shown. The days computed ahead are thrown away.
    fn poke(&mut self, event: Event) {
        self.change(Change::Poke(event));
    }

    /// Change the world on the day shown. The days computed ahead are thrown away; on a day before
    /// the furthest one watched, a new timeline starts and the old one is kept.
    fn change(&mut self, change: Change) {
        let branch = !self.at_the_edge();
        self.new_epoch();
        self.worker.change(self.epoch, self.day(), change, branch);
    }

    fn live_edit(&mut self, e: LiveEdit) {
        let edit = match e {
            LiveEdit::Raise { lon, lat, radius_km, metres } => {
                Edit::Paint { path: vec![[lon, lat], [lon + 2.0, lat + 1.0]], radius_km, brush: sim::edit::Brush::Raise(metres) }
            }
            LiveEdit::Split(p) => {
                let w = &self.geo;
                let p = p.unwrap_or(breadbasket(w) as u32);
                let cells: Vec<u32> = (0..w.grid.cells() as u32).filter(|&c| w.provinces.of_cell[c as usize] == p).collect();
                self.selected = Some(w.provinces.count as u32);
                Edit::Split { cells: cells[..cells.len() / 2].to_vec() }
            }
            LiveEdit::Undo => return self.change(Change::Undo),
            LiveEdit::Set(key, value) => Edit::Settings { changes: vec![SettingChange { key, value }] },
        };
        self.send_edit(Ok(edit));
    }

    /// Make the change the player asked for: an edit, or a poke.
    fn act(&mut self, a: Action) {
        match a {
            Action::Edit(e) => self.send_edit(Ok(e)),
            Action::Poke(e) => self.poke(e),
        }
    }

    /// What the world keeps of realms, rulers and armies, so the editor offers only what it can take.
    fn kept(&self) -> Kept {
        let kept = |name: &str| self.owners[sim::fields::known(name).0 as usize].1.is_some();
        Kept {
            treasury: self.coin_columns.first().copied().filter(|_| kept("realm.stock")),
            opinions: kept("relation.a_of_b"),
            wars: kept("war.attacker"),
            rulers: kept("realm.ruler"),
            armies: kept("army.size"),
        }
    }

    /// Send an edit, or say why there is none.
    fn send_edit(&mut self, edit: Result<Edit, String>) {
        match edit {
            Ok(e) => self.change(Change::Edit(e)),
            Err(why) => self.notice = Some((why, Instant::now())),
        }
    }

    /// Whether the day shown is the furthest one watched. Undo and redo only apply there: before
    /// it, a change starts a new timeline.
    fn at_the_edge(&self) -> bool {
        self.day() >= self.seen.get(&self.status.active).copied().unwrap_or(0)
    }

    /// Show another day of this timeline.
    fn goto(&mut self, day: u64) {
        self.new_epoch();
        self.worker.goto(self.epoch, day);
    }

    /// Play another timeline, from the day it was left on.
    fn switch(&mut self, id: u32) {
        self.left_at.insert(self.status.active, self.day());
        let day = self.left_at.get(&id).or(self.seen.get(&id)).copied().unwrap_or(0);
        self.new_epoch();
        self.worker.switch(self.epoch, id, day);
    }

    /// Frames from now on answer a new request; the ones computed ahead are dropped.
    fn new_epoch(&mut self) {
        self.epoch += 1;
        self.frames.truncate(1);
        self.clock.pause();
        self.waiting = Some(Instant::now());
    }

    fn explain(&mut self, day: u64, province: u32) {
        self.trace_pending = Some((day, province));
        self.worker.trace(day, province);
    }

    fn time_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(self.calendar.format(self.day()));
            ui.separator();
            let paused = self.clock.speed == Speed::Paused;
            if ui.selectable_label(paused, "⏸ pause").clicked() {
                self.clock.pause();
            }
            for (name, speed) in SPEEDS {
                if ui.selectable_label(self.clock.speed == speed && self.clock.stop_at.is_none(), name).clicked() {
                    self.clock.play(speed);
                }
            }
            ui.separator();
            let today = self.day();
            if ui.button("+1 day").clicked() {
                self.clock.pause();
                self.clock.run_to(today + 1);
            }
            if ui.button("+1 week").clicked() {
                self.clock.pause();
                self.clock.run_to(today + self.calendar.days_per_week() as u64);
            }
            ui.separator();
            ui.label("run to year");
            ui.add(egui::DragValue::new(&mut self.run_to_year).range(self.calendar.date(today).year + 1..=1_000_000));
            if ui.button("go").clicked() {
                self.clock.run_to(self.calendar.year_start(self.run_to_year));
            }
            ui.separator();
            let ahead = self.frames.back().map_or(0, |f| f.day) - today;
            let busy: Vec<f64> = self.frames.iter().map(|f| f.ms).filter(|&m| m > 0.0).collect();
            let ms = if busy.is_empty() { 0.0 } else { busy.iter().sum::<f64>() / self.frames.len().max(1) as f64 };
            ui.small(format!("showing {:.0} days a second · simulation {ahead} days ahead · {ms:.3} ms per simulated day", self.rate.days_per_second()));
        });
        self.history_bar(ui);
        // Keys typed into a text box are not shortcuts.
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (undo, redo) = ui.input(|i| (i.modifiers.command && i.key_pressed(egui::Key::Z), i.modifiers.command && i.key_pressed(egui::Key::Y)));
        let (undo, redo) = (undo && !typing, redo && !typing);
        if self.at_the_edge() && self.waiting.is_none() {
            if undo && self.status.can_undo {
                self.change(Change::Undo);
            }
            if redo && self.status.can_redo {
                self.change(Change::Redo);
            }
        }
        // Space pauses and resumes at a day a second; the right arrow steps a day.
        let (space, right) = ui.input(|i| (i.key_pressed(egui::Key::Space), i.key_pressed(egui::Key::ArrowRight)));
        let (space, right) = (space && !typing, right && !typing);
        if space {
            if self.clock.speed == Speed::Paused { self.clock.play(Speed::DaysPerSecond(1.0)) } else { self.clock.pause() }
        }
        if right {
            self.clock.pause();
            self.clock.run_to(self.day() + 1);
        }
    }

    /// The history slider, from the earliest day kept to the furthest watched; letting go of it goes
    /// there. Then what is under way, and what the simulation refused.
    fn history_bar(&mut self, ui: &mut egui::Ui) {
        let today = self.day();
        let earliest = self.status.earliest.min(today);
        let furthest = self.seen.get(&self.status.active).copied().unwrap_or(today).max(today);
        ui.horizontal(|ui| {
            ui.label("history");
            let mut day = self.scrub.unwrap_or(today);
            let cal = self.calendar.clone();
            let width = (ui.available_width() - 260.0).max(120.0);
            ui.spacing_mut().slider_width = width;
            let r = ui.add(egui::Slider::new(&mut day, earliest..=furthest.max(earliest + 1)).show_value(false));
            if r.dragged() {
                self.scrub = Some(day);
            } else if r.drag_stopped() || (r.changed() && self.scrub.is_none()) {
                self.scrub = None;
                if day != today {
                    self.goto(day);
                }
            }
            ui.label(cal.format(self.scrub.unwrap_or(today)));
            if today < furthest && ui.button("back to the latest").clicked() {
                self.goto(furthest);
            }
        });
        if let Some(since) = self.waiting {
            let what = self.status.busy.clone().unwrap_or_else(|| "working…".into());
            ui.colored_label(egui::Color32::from_rgb(160, 90, 20), format!("{what} {:.1} s", since.elapsed().as_secs_f64()));
        } else if self.rebuild.is_some() {
            ui.small("redrawing the changed map…");
        }
        if let Some((text, at)) = &self.notice {
            if at.elapsed().as_secs() < 8 {
                ui.colored_label(egui::Color32::from_rgb(170, 40, 30), text);
            } else {
                self.notice = None;
            }
        }
    }

    /// Every timeline, and a button to play each one.
    fn timelines(&mut self, ui: &mut egui::Ui) {
        if self.status.timelines.len() < 2 {
            ui.small("Changing a day before the latest one you have watched starts a new timeline; this one is kept.");
            return;
        }
        let mut play = None;
        for t in &self.status.timelines {
            ui.horizontal_wrapped(|ui| {
                let name = match t.parent {
                    None => format!("timeline {}", t.id),
                    Some((p, d)) => format!("timeline {} (left {p} on {})", t.id, self.calendar.format(d)),
                };
                if t.id == self.status.active {
                    ui.strong(format!("▶ {name}"));
                } else if ui.button(name).on_hover_text("play this timeline from where it was left").clicked() {
                    play = Some(t.id);
                }
                ui.small(format!("{} changes", t.commands));
            });
        }
        if let Some(id) = play {
            self.switch(id);
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let w = &self.geo;
        ui.heading("Fantasy Planet");
        // Named by the world itself: an opened one may not be the size the options say.
        let size = [Size::Small, Size::Medium, Size::Large, Size::Huge].into_iter().find(|&z| WorldParams::preset(z, 0).n == w.grid.n);
        ui.label(match size {
            Some(z) => format!("{z:?} world, seed {}", w.params.seed),
            None => format!("world of {} cells, seed {}", w.grid.cells(), w.params.seed),
        });
        ui.label(format!("{} people", group(self.view.people.iter().sum::<f64>().round() as u64)));
        ui.separator();

        self.modules_section(ui);
        self.settings_section(ui);
        self.new_world_section(ui);
        ui.separator();
        if self.kept().wars {
            self.wars_section(ui);
            ui.separator();
        }

        ui.label("Edit");
        let edge = self.at_the_edge();
        ui.horizontal(|ui| {
            let why_not = "only on the latest day you have watched; earlier, a change starts a new timeline";
            let undo = ui
                .add_enabled(self.status.can_undo && edge, egui::Button::new("⟲ undo"))
                .on_hover_text("take back your last change: an edit, or a poke such as a drought or a war declared (Ctrl+Z)")
                .on_disabled_hover_text(why_not);
            let redo = ui.add_enabled(self.status.can_redo && edge, egui::Button::new("⟳ redo")).on_hover_text("give it back (Ctrl+Y)").on_disabled_hover_text(why_not);
            if undo.clicked() {
                self.change(Change::Undo);
            }
            if redo.clicked() {
                self.change(Change::Redo);
            }
        });
        let names = Names { world: &self.geo, realms: &self.realm_names.names };
        let realms = self.realms_on().then_some((&self.geo, &names));
        if let Some(a) = self.editor.panel(ui, self.selected, realms) {
            self.act(a);
        }
        ui.separator();

        ui.label("Timelines");
        self.timelines(ui);
        ui.separator();

        ui.label("File");
        ui.add(egui::TextEdit::singleline(&mut self.file).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.button("save").on_hover_text("every timeline: a readable scenario if the name ends in .toml, else a save that opens at once").clicked() {
                self.worker.save(self.file.clone().into(), self.day());
            }
            if ui.button("open").on_hover_text("the same world saved earlier (another world: start the app with --open)").clicked() {
                self.new_epoch();
                self.worker.open(self.epoch, self.file.clone().into());
            }
        });
        ui.separator();

        ui.label("Map shape");
        ui.radio_value(&mut self.projection, Projection::EqualEarth, "Oval world map (true areas)");
        ui.radio_value(&mut self.projection, Projection::Rectangle, "Rectangle (repeats sideways)");
        ui.separator();

        ui.label("Layers");
        let realms = self.realms_on();
        let armies = self.kept().armies;
        let l = &mut self.layers;
        ui.add(egui::Slider::new(&mut l.terrain, 0.0..=1.0).text("terrain"));
        ui.add(egui::Slider::new(&mut l.temperature, 0.0..=1.0).text("temperature"));
        ui.add(egui::Slider::new(&mut l.rainfall, 0.0..=1.0).text("rainfall"));
        ui.add(egui::Slider::new(&mut l.population, 0.0..=1.0).text("population"));
        ui.add(egui::Slider::new(&mut l.famine, 0.0..=1.0).text("famine, past year"));
        ui.add(egui::Slider::new(&mut l.drought, 0.0..=1.0).text("drought"));
        ui.add(egui::Slider::new(&mut l.provinces, 0.0..=1.0).text("province colours"));
        // Only where some module prices goods.
        if !self.market.price.is_empty() {
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut l.prices, 0.0..=1.0).text("prices of"))
                    .on_hover_text("what the good costs to trade (its local price times the money level) against the middle province: purple cheap, orange dear, from a quarter to four times; clear where nobody lives. Food goods are priced together, so grain, fish and livestock show the same map");
                let goods = &self.geo.goods;
                let mut c = self.price_good;
                egui::ComboBox::from_id_salt("price layer good").selected_text(inspector::good_name(goods, c)).show_ui(ui, |ui| {
                    for k in (0..goods.stride()).filter(|k| !self.coin_columns.contains(k)) {
                        ui.selectable_value(&mut c, k, inspector::good_name(goods, k));
                    }
                });
                self.price_good = c;
            });
        }
        if self.trades {
            ui.add(egui::Slider::new(&mut l.trade, 0.0..=1.0).text("trade, last month"))
                .on_hover_text("a line between each pair of provinces that traded, as thick as the trade was worth (what was sold one way less what came back), with an arrowhead toward the buyer");
            ui.add(egui::Slider::new(&mut l.trade_balance, 0.0..=1.0).text("sold or bought more"))
                .on_hover_text("last month: gold where a province sold more than it bought, rose where it bought more; full colour at a tenth of the coins it holds");
            if self.shown_state.trade.shipments.is_empty() {
                ui.small("No trade recorded yet: it shows once a month's trade has run (a world just opened keeps none).");
            }
        }
        // Only where some module keeps realms.
        if realms {
            ui.add(egui::Slider::new(&mut l.realms, 0.0..=1.0).text("realms"))
                .on_hover_text("each province in the colour of the realm that owns it, striped with an enemy's colour where the enemy occupies it; clear where no realm owns the land");
            ui.add(egui::Slider::new(&mut l.unrest, 0.0..=1.0).text("unrest"))
                .on_hover_text("how restless each province is: clear when calm, then yellow through orange to red as it nears an uprising; deep crimson where it has risen up");
        }
        ui.checkbox(&mut l.coasts, "coastlines");
        ui.checkbox(&mut l.rivers, "rivers");
        ui.checkbox(&mut l.borders, "province borders (zoom in)");
        ui.checkbox(&mut l.graticule, "latitude and longitude lines");
        ui.checkbox(&mut l.roads, "roads and sea lanes").on_hover_text("the ways between neighbouring provinces, seat to seat: roads over land, dashed lanes by sea");
        if realms {
            ui.checkbox(&mut l.realm_borders, "borders between realms").on_hover_text("thick lines where the realms on either side differ, or where a realm's land meets land nobody owns");
        }
        if armies {
            ui.checkbox(&mut l.armies, "armies").on_hover_text("field armies marching, gathering, besieging or fighting, in their realm's colour, sized by soldiers; ships as hulls. Guards and recruits idle at home aren't drawn (the province page lists them)");
            ui.checkbox(&mut l.battles, "battles").on_hover_text("crossed swords where a battle is fought today; a fading mark where one ended in the last month");
        }
        ui.separator();

        if let Some((c, p)) = self.hover {
            self.cell_info(ui, c, p);
            ui.separator();
        }
        ui.small("Click a province to inspect it. Space plays and pauses; the right arrow key steps a day.");
        let w = &self.geo;
        ui.small(format!("{} cells, {} provinces", w.grid.cells(), w.provinces.count));
        ui.small(format!("{} line segments", self.line_segments));
        ui.small(format!("world built and run in {:.1} s", self.gen_s));
        ui.small(format!("frame {:.1} ms", self.last_dt_ms));
        ui.small(format!("GPU: {}", self.adapter));
    }

    /// Every war going on, and the peaces of the last five years; each side a button to its capital.
    fn wars_section(&mut self, ui: &mut egui::Ui) {
        let names = Names { world: &self.geo, realms: &self.realm_names.names };
        let (going, peaces) = inspector::wars(&names, self.day(), &self.calendar, 5 * self.calendar.days_per_year() as u64);
        let open = self.opts.open_sections.iter().any(|s| s == "wars");
        let mut go = None;
        egui::CollapsingHeader::new(format!("Wars ({} going on)", going.len())).default_open(open).show(ui, |ui| {
            if going.is_empty() {
                ui.small("None going on.");
            }
            for g in &going {
                ui.label(egui::RichText::new(&g.title).strong());
                for l in &g.lines {
                    ui.small(l);
                }
                ui.horizontal_wrapped(|ui| {
                    for r in [g.attacker, g.defender] {
                        if let Some(c) = self.geo.realms.capital.get(r as usize).copied().filter(|&c| c != NONE)
                            && ui.small_button(names.realm(r)).on_hover_text("select its capital").clicked()
                        {
                            go = Some(c);
                        }
                    }
                });
            }
            if !peaces.is_empty() {
                egui::CollapsingHeader::new(format!("Peaces of the last five years ({})", peaces.len())).id_salt("recent peaces").show(ui, |ui| {
                    for p in peaces.iter().take(60) {
                        ui.small(p);
                    }
                });
            }
        });
        if let Some(c) = go {
            (self.selected, self.pop_edit, self.stock_edit) = (Some(c), None, None);
        }
    }

    /// The armies and battles of the day shown, worked out once for it.
    fn army_view(&mut self) -> &ArmyView {
        let key = Arc::as_ptr(&self.shown_state) as usize;
        if self.army_view.as_ref().is_none_or(|v| v.0 != key) {
            let w = &self.geo;
            let view = ArmyView {
                markers: app::armies::markers(w, &self.troop_kinds),
                battles: app::armies::battles(w, &self.troop_kinds),
                recent: app::armies::recent_battles(&self.log, self.day(), 30),
            };
            self.army_view = Some((key, view));
        }
        &self.army_view.as_ref().expect("just set").1
    }

    /// The versions running, and the combination tester's verdict on them.
    fn modules_section(&mut self, ui: &mut egui::Ui) {
        let at = Arc::as_ptr(&self.shown_state.settings) as usize;
        if self.changed_count.0 != at {
            let now = self.settings_now();
            self.changed_count = (at, sim::settings::changed(&now, &sim::settings::defaults_for(&now)).len());
        }
        let ModulesInfo { versions, verdict, tested_on } = &self.modules;
        ui.label("Modules");
        let line: Vec<String> = versions.iter().map(|(slot, v)| format!("{slot} {v}")).collect();
        ui.small(line.join(" · "));
        Self::verdict_line(ui, verdict, tested_on, "running");
        Self::changed_note(ui, self.changed_count.1);
    }

    /// The configuration in force on the day shown: the world's, with its settings of that day.
    fn settings_now(&self) -> Config {
        Config { modules: (*self.shown_state.settings).clone(), ..self.config.clone() }
    }

    /// How many settings differ from their defaults: the combination tester ran only the defaults.
    fn changed_line(ui: &mut egui::Ui, c: &Config) {
        Self::changed_note(ui, sim::settings::changed(c, &sim::settings::defaults_for(c)).len());
    }

    fn changed_note(ui: &mut egui::Ui, n: usize) {
        if n > 0 {
            ui.small(format!("{n} setting{} changed from the defaults; the combination tester ran only default settings.", if n == 1 { "" } else { "s" }));
        }
    }

    /// The settings the world runs with on the day shown, to change from that day on.
    fn settings_section(&mut self, ui: &mut egui::Ui) {
        let now = self.settings_now();
        let open = self.opts.open_sections.iter().any(|s| s == "settings");
        let mut apply = None;
        egui::CollapsingHeader::new("Settings").default_open(open).show(ui, |ui| {
            ui.small("What the world runs with on the day shown, each with what it does. A change applies from that day on and goes into the timeline like an edit: undo takes it back, and on an earlier day it starts a new timeline. Settings that shape the map or the start are under New world.");
            let mut edited = self.live_draft.clone().unwrap_or_else(|| now.clone());
            let defaults = sim::settings::defaults_for(&now);
            if let Some(e) = settings_ui::panel(ui, "live", &mut edited, &defaults, Show::Live, open) {
                self.notice = Some((e, Instant::now()));
            }
            let changes: Vec<SettingChange> = sim::settings::changed(&edited, &now)
                .into_iter()
                .filter(|i| i.spec.when == sim::settings::When::Live)
                .map(|i| SettingChange { key: i.key, value: i.value })
                .collect();
            self.live_draft = (!changes.is_empty()).then_some(edited);
            if !changes.is_empty() {
                ui.horizontal(|ui| {
                    if ui.button(format!("apply {} change{}", changes.len(), if changes.len() == 1 { "" } else { "s" })).clicked() {
                        apply = Some(true);
                    }
                    if ui.button("discard").clicked() {
                        apply = Some(false);
                    }
                });
            }
            if apply == Some(true) {
                apply = None;
                self.live_draft = None;
                self.send_edit(Ok(Edit::Settings { changes }));
            }
        });
        if apply == Some(false) {
            self.live_draft = None;
        }
    }

    /// The combination tester's verdict on a set of versions, in colour, with what it found. `id`
    /// tells apart the places it is shown.
    fn verdict_line(ui: &mut egui::Ui, verdict: &Result<Option<(Verdict, Vec<String>)>, String>, tested_on: &str, id: &str) {
        match verdict {
            Ok(Some((v, notes))) => {
                let (color, words) = match v {
                    Verdict::Green => (egui::Color32::from_rgb(70, 130, 60), "no problems found in testing"),
                    Verdict::Yellow => (egui::Color32::from_rgb(170, 130, 30), "known quirks found in testing"),
                    Verdict::Red => (egui::Color32::from_rgb(180, 50, 40), "unstable in testing"),
                    Verdict::Refused => (egui::Color32::from_rgb(180, 50, 40), "refused: these can't run together"),
                };
                ui.colored_label(color, words).on_hover_text(tested_on);
                if !notes.is_empty() {
                    egui::CollapsingHeader::new(format!("what the tester found ({})", notes.len())).id_salt((id, "tester found")).show(ui, |ui| {
                        for n in notes {
                            ui.small(n);
                        }
                    });
                }
            }
            Ok(None) => {
                ui.small("not in the combination tester's table yet");
            }
            Err(e) => {
                ui.small(format!("the combination tester's table can't be read: {e}"));
            }
        }
    }

    /// Make another world: its size, seed and module versions, with what the combination tester
    /// found for those versions, shown as they are picked.
    fn new_world_section(&mut self, ui: &mut egui::Ui) {
        let nw = &mut self.new_world;
        let opened = |what: &str| self.opts.open_sections.iter().any(|s| s == what);
        let open = self.opts.remake.is_some() || opened("new-world");
        let open_settings = opened("new-world-settings");
        egui::CollapsingHeader::new("New world").default_open(open || open_settings).show(ui, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("new world size").selected_text(format!("{:?}", nw.size)).show_ui(ui, |ui| {
                    for z in [Size::Small, Size::Medium, Size::Large, Size::Huge] {
                        if ui.selectable_label(nw.size == z, format!("{z:?}")).clicked() {
                            nw.set_size(z);
                            nw.confirm = false;
                        }
                    }
                });
                ui.label("seed");
                ui.add(egui::DragValue::new(&mut nw.seed));
            });
            ui.horizontal(|ui| {
                ui.label("run");
                ui.add(egui::DragValue::new(&mut nw.years).range(0..=500).suffix(" years"));
                ui.label("before showing it");
            });
            egui::Grid::new("new world modules").num_columns(2).show(ui, |ui| {
                for &(slot, versions) in CHOICES {
                    ui.label(inspector::slot_name(slot));
                    let mut v = nw.config.modules.choice(slot).expect("a slot with a choice");
                    egui::ComboBox::from_id_salt(("new world version", inspector::slot_name(slot))).selected_text(v).show_ui(ui, |ui| {
                        for &x in versions {
                            ui.selectable_value(&mut v, x, x);
                        }
                    });
                    if Some(v) != nw.config.modules.choice(slot) {
                        settings_ui::choose(&mut nw.config.modules, slot, v);
                        nw.confirm = false;
                    }
                    ui.end_row();
                }
            });
            ui.small("Realms need people; rulers, diplomacy and armies need realms; realms choosing war and peace need armies. Picking one switches on what it needs, and switching one off switches off what needs it.");
            egui::CollapsingHeader::new("settings").id_salt("new world settings").default_open(open_settings).show(ui, |ui| {
                ui.small("Every setting of the world to make, with what it does. Those \"fixed once made\" shape the map or the start; the others can also be changed later, while the world runs (Settings, above).");
                let defaults = sim::settings::defaults_for(&nw.config);
                if ui.button("all back to the defaults").clicked() {
                    nw.config.modules = defaults.modules.clone();
                    nw.config.world = defaults.world.clone();
                    nw.confirm = false;
                }
                let before = nw.config.clone();
                nw.refused = settings_ui::panel(ui, "new world", &mut nw.config, &defaults, Show::All, open_settings).or(nw.refused.take().filter(|_| nw.config == before));
                if nw.config != before {
                    nw.confirm = false;
                }
                if let Some(e) = &nw.refused {
                    ui.colored_label(egui::Color32::from_rgb(180, 50, 40), e);
                }
            });
            // What the tester found for these versions, as they are picked.
            let m = &nw.config.modules;
            let verdict = self.table.as_ref().map(|t| t.verdict_for(m)).map_err(|e| e.clone());
            let refused = matches!(verdict, Ok(Some((Verdict::Refused, _))));
            Self::verdict_line(ui, &verdict, &tested_on(&self.table), "new world");
            Self::changed_line(ui, &nw.config);
            if let Some((since, _)) = &nw.making {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("making the world… {:.0} s", since.elapsed().as_secs_f64()));
                });
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            } else if nw.confirm {
                ui.label("This replaces the world shown and all its timelines; anything not saved is lost.");
                ui.horizontal(|ui| {
                    if ui.button("make it").clicked() {
                        let ctx = self.repaint.clone();
                        nw.make(move || ctx.request_repaint());
                    }
                    if ui.button("cancel").clicked() {
                        nw.confirm = false;
                    }
                });
            } else if ui
                .add_enabled(!refused, egui::Button::new("make a new world"))
                .on_disabled_hover_text("these versions can't run together")
                .clicked()
            {
                nw.confirm = true;
            }
            if let Some(e) = &nw.failed {
                ui.colored_label(egui::Color32::from_rgb(180, 50, 40), format!("couldn't make it: {e}"));
            }
        });
    }

    /// Whether some module keeps realms in this world.
    fn realms_on(&self) -> bool {
        self.owners[sim::fields::known("province.owner").0 as usize].1.is_some()
    }

    fn names(&self) -> Names<'_> {
        Names { world: &self.geo, realms: &self.realm_names.names }
    }

    fn cell_info(&self, ui: &mut egui::Ui, c: usize, p: u32) {
        let w = &self.geo;
        let (lon, lat) = (w.grid.lon[c].to_degrees(), w.grid.lat[c].to_degrees());
        ui.label(inspector::lat_lon(lat as f64, lon as f64));
        let e = w.elevation[c];
        ui.label(if e < 0.0 { format!("sea, {:.0} m deep", -e) } else { format!("{}, {:.0} m", w.biome[c].name(), e) });
        ui.label(format!("{:.0} °C, {:.0} mm rain a year", w.temperature[c], w.rainfall[c]));
        if e >= 0.0 && w.river_flow[c] >= RIVER_MIN_FLOW {
            ui.label(format!("river, {:.0} m³/s", w.river_flow[c] as f64 * 1000.0 / (365.25 * 86400.0)));
        }
        if p != NONE {
            let p = p as usize;
            let people = self.view.people[p];
            ui.label(format!("province {p}: {} people, {:.1} per km²", group(people.round() as u64), people / w.provinces.area_km2[p]));
            if let Some(food) = w.stock_under(p, "food") {
                ui.label(format!("food stock {} rations", group(food)));
            }
            if self.layers.prices > 0.0
                && let Some(&x) = self.market.price.get(p).filter(|x| !x.is_nan())
            {
                let times = 4f64.powf(x as f64);
                let name = inspector::good_name(&w.goods, self.price_good);
                ui.label(if times >= 1.0 { format!("{name}: {times:.2}× the middle province's price to trade") } else { format!("{name}: {:.2}× the middle province's price to trade (cheaper)", times) });
            }
            if self.layers.trade_balance > 0.0 || self.layers.trade > 0.0 {
                let (mut sold, mut bought) = (0.0, 0.0);
                for s in &self.shown_state.trade.shipments {
                    if s.from as usize == p {
                        sold += s.coins;
                    } else if s.to as usize == p {
                        bought += s.coins;
                    }
                }
                if sold + bought > 0.0 {
                    ui.label(format!("last month: sold {} coins' worth, bought {}", group(sold.round() as u64), group(bought.round() as u64)));
                }
            }
            if self.realms_on() {
                let names = self.names();
                let at = |v: &[u32]| v.get(p).copied().unwrap_or(NONE);
                ui.label(match at(&w.owner) {
                    NONE => "no realm".to_string(),
                    r => format!("realm: {}", names.realm(r)),
                });
                if at(&w.occupier) != NONE {
                    ui.label(format!("occupied by {}", names.realm(at(&w.occupier))));
                }
                let unrest = w.unrest.get(p).copied().unwrap_or(0);
                if at(&w.uprising) != NONE {
                    ui.label("risen up");
                } else if unrest > 0 {
                    ui.label(format!("unrest {} of {} toward an uprising", group(unrest), group(sim::realms::LINE)));
                }
            }
            if self.view.drought[p] > 0.0 {
                ui.label(format!("drought: {:.0}% of the rain missing", 100.0 * self.view.drought[p]));
            }
            let starved = self.view.starved[p].round() as u64;
            if starved > 0 {
                ui.label(format!("{} starved in the last 12 months", group(starved)));
            }
        }
    }

    /// The goods kept in province `p`: a row each with its store, price and workers, as far as
    /// the world keeps them, and what they cost to trade where prices follow the money. Coins are
    /// held, not made or priced, so only their store shows. `when` shows when a value last
    /// changed, with its "why?".
    fn goods_table(&self, ui: &mut egui::Ui, p: usize, when: &mut dyn FnMut(&mut egui::Ui, FieldId, usize)) {
        let state = &self.shown_state;
        let stock = sim::fields::known("province.stock");
        let price = sim::fields::known("province.price");
        let kept: Vec<FieldId> = FieldId::all().filter(|&id| id.def().tier == Tier::ProvinceGood && state.fields.iter().any(|f| f.0 == id)).collect();
        if kept.is_empty() {
            return;
        }
        let (goods, w) = (&self.geo.goods, &self.geo);
        let level = state.money_levels.as_ref().and_then(|l| l.get(p)).copied();
        ui.label(egui::RichText::new("Goods").strong()).on_hover_text(format!("set by {}", self.owners[stock.0 as usize].0));
        let data: Vec<_> = kept.iter().map(|&id| w.field(id)).collect();
        let to_trade = "the local price times the money level: what trade reckons with. Where people hold more coins than across the world, everything is dearer to trade, so the province buys more and sells less, and the coins flow back out.";
        egui::Grid::new("goods").striped(true).show(ui, |ui| {
            ui.small("good");
            for &id in &kept {
                ui.small(inspector::field_name(id));
                if id == price && level.is_some() {
                    ui.small("to trade").on_hover_text(to_trade);
                }
            }
            ui.small("last changed");
            ui.end_row();
            for c in 0..goods.stride() {
                let i = goods.index(p, c);
                let coin = self.coin_columns.contains(&c);
                ui.label(inspector::good_name(goods, c));
                for (&id, d) in kept.iter().zip(&data) {
                    let shown = id == stock || !coin;
                    ui.label(if shown && !d.is_empty() { inspector::per_good_value(id, goods, c, d.value(i) as u64) } else { String::new() });
                    if let (true, Some(level)) = (id == price, level) {
                        ui.label(if shown && !d.is_empty() { format!("{:.3} coins", d.value(i) / sim::markets::PRICE_SCALE * level) } else { String::new() });
                    }
                }
                // The latest of the good's values to change.
                let last = |id: FieldId| state.last_change.get(id.0 as usize).and_then(|v| v.get(i)).copied().filter(|&d| d != NEVER);
                let id = kept.iter().copied().filter(|&id| id == stock || !coin).max_by_key(|&id| last(id)).unwrap_or(stock);
                when(ui, id, i);
                ui.end_row();
            }
        });
        // How rich the province is in coins against the world, and what that does to its prices.
        if let (Some(level), Some(&coin)) = (level, self.coin_columns.first()) {
            let people = w.province_population();
            let k = goods.stride();
            let coins = |q: usize| w.stock.get(q * k + coin).copied().unwrap_or(0) as f64;
            let (all_coins, all_people) = (0..people.len()).filter(|&q| people[q] > 0).fold((0.0, 0.0), |(c, h), q| (c + coins(q), h + people[q] as f64));
            if people[p] > 0 && all_people > 0.0 {
                let pct = (level - 1.0) * 100.0;
                let effect = if pct.abs() < 0.5 {
                    "the same as its local price".to_string()
                } else {
                    format!("{:.0}% {} than its local price", pct.abs(), if pct > 0.0 { "more" } else { "less" })
                };
                ui.small(format!(
                    "{:.1} coins a person here, {:.1} across the world. Money level ×{level:.2}: to trade, everything here costs {effect}.",
                    coins(p) / people[p] as f64,
                    all_coins / all_people
                ))
                .on_hover_text(to_trade);
            }
        }
    }

    /// Everything about the selected province: its values and where they come from, what happened
    /// to it, why, and the drought tool.
    fn inspector(&mut self, ui: &mut egui::Ui, p: u32) {
        let pi = p as usize;
        ui.horizontal(|ui| {
            ui.heading(format!("Province {p}"));
            if ui.small_button("close").clicked() {
                self.selected = None;
            }
        });
        let today = self.day();
        let mut explain = None;
        if self.realms_on() {
            self.realm_section(ui, p);
            ui.separator();
        }

        ui.label(egui::RichText::new("Values").strong());
        let state = self.shown_state.clone();
        let changed = |id: FieldId, i: usize| state.last_change.get(id.0 as usize).and_then(|v| v.get(i)).copied();
        let mut when = |ui: &mut egui::Ui, id: FieldId, i: usize| match changed(id, i) {
            _ if state.last_change[id.0 as usize].is_empty() && !state.fields.iter().any(|f| f.0 == id) => {
                ui.label("when the world was made");
            }
            None | Some(NEVER) => {
                ui.label("not since the start");
            }
            Some(d) => {
                ui.horizontal(|ui| {
                    ui.label(self.calendar.format(d));
                    if ui.small_button("why?").on_hover_text("Run that day again and explain it").clicked() {
                        explain = Some(d);
                    }
                });
            }
        };
        let names = Names { world: &self.geo, realms: &self.realm_names.names };
        egui::Grid::new("values").striped(true).num_columns(4).show(ui, |ui| {
            ui.small("value");
            ui.small("now");
            ui.small("set by");
            ui.small("last changed");
            ui.end_row();
            for id in FieldId::all() {
                let data = self.geo.field(id);
                match id.def().tier {
                    Tier::Province => {
                        ui.label(inspector::field_name(id));
                        ui.label(inspector::value(id, &data, pi, &names));
                        ui.label(&self.owners[id.0 as usize].0);
                        when(ui, id, pi);
                        ui.end_row();
                    }
                    // In the goods table below; realms and rulers are on the realm's page above;
                    // armies have a section of their own below.
                    Tier::ProvinceGood | Tier::Cell | Tier::Pop | Tier::Realm | Tier::RealmGood | Tier::Army | Tier::Character | Tier::War | Tier::Relation | Tier::RealmTech | Tier::Culture => {}
                }
            }
        });
        self.goods_table(ui, pi, &mut when);
        let pops: Vec<usize> = (0..self.geo.pops.size.len()).filter(|&k| self.geo.pops.province[k] == p).collect();
        let size = sim::fields::known("pop.size");
        // Editing a pop: its size, typed in, set when asked.
        let mut editing = self.pop_edit;
        let mut set_pop = None;
        for &k in &pops {
            let w = &self.geo.pops;
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "pop {k}: {} people (species {}, culture {}, religion {}, class {}), set by {}; size last changed",
                    group(w.size[k]),
                    w.species[k],
                    w.culture[k],
                    w.religion[k],
                    w.class[k],
                    self.owners[size.0 as usize].0
                ));
                when(ui, size, k);
                match &mut editing {
                    Some((j, n)) if *j == k => {
                        ui.add(egui::DragValue::new(n).range(0..=1_000_000_000_000u64).speed(100.0).suffix(" people"));
                        if ui.small_button("set").clicked() {
                            set_pop = Some((k, *n));
                        }
                        if ui.small_button("cancel").clicked() {
                            editing = None;
                        }
                    }
                    _ => {
                        if ui.small_button("edit").on_hover_text("set how many people this pop has").clicked() {
                            editing = Some((k, w.size[k]));
                        }
                    }
                }
            });
        }
        if pops.is_empty() {
            ui.small("Nobody lives here.");
        }
        self.pop_edit = editing;
        let w = &self.geo;
        let to = |k: usize, size: u64| PopValues { size, species: w.pops.species[k], culture: w.pops.culture[k], religion: w.pops.religion[k], class: w.pops.class[k] };
        let mut edit = set_pop.map(|(k, n)| Edit::SetPop { pop: k as u32, to: to(k, n) });
        // Only what some module keeps can be set: with nobody keeping it, it would be a number
        // nothing reads (the world refuses such an edit too).
        let kept = |name: &str| self.owners[sim::fields::known(name).0 as usize].1.is_some();
        if kept("pop.size") {
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::DragValue::new(&mut self.new_pop).range(1..=1_000_000_000u64).speed(100.0).suffix(" people"));
                if ui.small_button("add a pop").clicked() {
                    edit = Some(Edit::AddPop { province: p, values: PopValues::people(self.new_pop) });
                }
            });
        }
        if kept("province.stock") {
            ui.horizontal_wrapped(|ui| {
                let goods = &self.geo.goods;
                let mut c = self.stock_good.min(goods.stride() - 1);
                egui::ComboBox::from_id_salt("good to set").selected_text(inspector::good_name(goods, c)).show_ui(ui, |ui| {
                    for k in 0..goods.stride() {
                        ui.selectable_value(&mut c, k, inspector::good_name(goods, k));
                    }
                });
                if c != self.stock_good {
                    (self.stock_good, self.stock_edit) = (c, None);
                }
                // The store as it is, until a new amount is typed.
                let mut amount = self.stock_edit.unwrap_or_else(|| self.geo.stock.get(goods.index(pi, c)).copied().unwrap_or(0));
                let unit = goods.tree.unit(goods.columns[c]).map(|u| format!(" {u}")).unwrap_or_default();
                if ui.add(egui::DragValue::new(&mut amount).range(0..=1_000_000_000_000u64).speed(1000.0).suffix(unit)).changed() {
                    self.stock_edit = Some(amount);
                }
                if ui.small_button("set the store").clicked() {
                    edit = Some(Edit::SetStock { province: p, good: inspector::good_name(goods, c).to_string(), amount });
                }
            });
        }
        if let Some(e) = edit {
            self.pop_edit = None;
            self.stock_edit = None;
            self.send_edit(Ok(e));
        }
        if self.kept().armies {
            ui.separator();
            let names = Names { world: &self.geo, realms: &self.realm_names.names };
            let (action, march) = self.realm_tools.armies(ui, &self.geo, &names, p, &self.troop_kinds);
            if let Some(a) = action {
                self.act(a);
            }
            if march.is_some() {
                self.editor.march = march;
            }
        }

        // Neighbours and the days it takes to reach them, seat to seat; click one to go there.
        let routes = self.geo.routes.clone();
        let links: Vec<&sim::routes::Link> = routes.of(p).collect();
        ui.separator();
        egui::CollapsingHeader::new(egui::RichText::new(format!("Neighbours ({})", links.len())).strong()).default_open(true).show(ui, |ui| {
            if links.is_empty() {
                ui.small("None: no province shares a border with it or faces it across the water.");
                return;
            }
            let days = |d: Option<f32>| d.map_or("—".to_string(), |d| format!("{d:.1} days"));
            egui::Grid::new("neighbours").striped(true).num_columns(4).show(ui, |ui| {
                ui.small("province");
                ui.small("by land");
                ui.small("by sea");
                ui.small("shared border");
                ui.end_row();
                for l in links {
                    let q = l.other(p);
                    if ui.small_button(format!("{q}")).on_hover_text("select this province").clicked() {
                        (self.selected, self.pop_edit, self.stock_edit) = (Some(q), None, None);
                    }
                    ui.label(days(l.land_days));
                    ui.label(days(l.sea_days));
                    ui.label(if l.border_km > 0.0 { format!("{} km", group(l.border_km.round() as u64)) } else { "across water".into() });
                    ui.end_row();
                }
            });
        });

        // Last month's trade: what went to and came from each neighbour, by good.
        let record = self.shown_state.trade.clone();
        let mut trades: std::collections::BTreeMap<(bool, u32, u16), (u64, f64)> = std::collections::BTreeMap::new();
        for s in record.shipments.iter().filter(|s| s.from == p || s.to == p) {
            let (sold, other) = if s.from == p { (true, s.to) } else { (false, s.from) };
            let t = trades.entry((sold, other, s.column)).or_default();
            (t.0, t.1) = (t.0 + s.units, t.1 + s.coins);
        }
        if !trades.is_empty() {
            ui.separator();
            let title = format!("Trade, settled {} ({})", self.calendar.format(record.day), trades.len());
            egui::CollapsingHeader::new(egui::RichText::new(title).strong()).default_open(false).show(ui, |ui| {
                let goods = &self.geo.goods;
                egui::Grid::new("trade").striped(true).num_columns(5).show(ui, |ui| {
                    for (&(sold, other, column), &(units, coins)) in &trades {
                        ui.label(if sold { "sold" } else { "bought" });
                        ui.label(inspector::good_name(goods, column as usize));
                        ui.label(inspector::good_value(goods, column as usize, units));
                        ui.label(format!("{} {other}", if sold { "to" } else { "from" }));
                        ui.label(format!("for {} coins", group(coins.round() as u64)));
                        ui.end_row();
                    }
                });
            });
        }

        let front = &self.frames[0];
        let goods = &self.geo.goods;
        let settled: Vec<String> =
            front.applied.iter().filter(|(r, _)| inspector::request_province(r) == Some(p)).map(|(r, g)| inspector::grant(r, *g, goods)).collect();
        if !settled.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(format!("Change requests settled on {}", self.calendar.format(today - 1))).strong());
            for s in settled {
                ui.label(s);
            }
        }

        ui.separator();
        // Most days nothing happens to a province; offer the last day something did.
        let latest = inspector::latest_change(&state.last_change, state.fields.iter().map(|f| f.0), &self.geo.pops.province, &self.geo.goods, p);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Why").strong());
            if let Some(d) = latest
                && ui.button(format!("explain the last change, {}", self.calendar.format(d))).clicked()
            {
                explain = Some(d);
            }
            if today > 0 && ui.small_button("explain the day before").clicked() {
                explain = Some(today - 1);
            }
        });
        if let Some((day, q)) = self.trace_pending
            && q == p
        {
            ui.small(format!("running {} again…", self.calendar.format(day)));
        } else if let Some(((day, q), result)) = &self.trace
            && *q == p
        {
            ui.small(format!("What happened to province {p} on {}:", self.calendar.format(*day)));
            match result {
                Err(e) => {
                    ui.label(e);
                }
                Ok(t) if t.steps.is_empty() => {
                    ui.label("Nothing happened to this province that day.");
                }
                Ok(t) => {
                    let mut stage: Option<Stage> = None;
                    for s in &t.steps {
                        if stage != Some(s.stage) {
                            ui.label(egui::RichText::new(inspector::stage(s.stage)).italics());
                            stage = Some(s.stage);
                        }
                        let names = Names { world: &self.geo, realms: &self.realm_names.names };
                        ui.label(format!("• {}", inspector::trace_step(s, &self.calendar, &self.geo.goods, &names)));
                    }
                }
            }
        }

        ui.separator();
        egui::CollapsingHeader::new("Start a drought here").show(ui, |ui| {
            let t = &mut self.drought;
            ui.add(egui::Slider::new(&mut t.percent, 10.0..=100.0).text("% of the rain missing").integer());
            ui.add(egui::Slider::new(&mut t.radius_km, 200.0..=4000.0).text("radius, km").integer());
            ui.add(egui::Slider::new(&mut t.years, 0.25..=10.0).text("years"));
            if ui.button("start it").on_hover_text("Takes effect tomorrow; the days computed ahead are thrown away").clicked() {
                let (lon, lat) = sim::grid::dir_to_lonlat(self.geo.provinces.center[pi]);
                let e = drought_event(lon.to_degrees(), lat.to_degrees(), &self.drought, &self.calendar);
                self.poke(e);
            }
        });

        ui.separator();
        ui.label(egui::RichText::new("What happened here").strong());
        ui.horizontal_wrapped(|ui| {
            let running = |slot: Slot| self.owners.iter().any(|o| o.1 == Some(slot));
            let modules = [Slot::Population, Slot::Economics, Slot::Weather, Slot::Realms, Slot::Characters, Slot::Diplomacy, Slot::Warfare];
            let sources: Vec<Source> = modules.into_iter().filter(|&m| matches!(m, Slot::Population | Slot::Economics | Slot::Weather) || running(m)).map(Source::Module).chain([Source::Player, Source::Engine]).collect();
            for s in sources {
                let mut on = !self.hidden_sources.contains(&s);
                if ui.checkbox(&mut on, s.name()).changed() {
                    if on {
                        self.hidden_sources.remove(&s);
                    } else {
                        self.hidden_sources.insert(s);
                    }
                }
            }
        });
        let mut shown = 0;
        let names = Names { world: &self.geo, realms: &self.realm_names.names };
        for e in self.log.iter().rev() {
            if e.province.is_some_and(|q| q != p) || (e.province.is_none() && e.source != Source::Player) || self.hidden_sources.contains(&e.source) {
                continue;
            }
            ui.label(format!("{} · {} · {}", self.calendar.format(e.day), e.source.name(), e.text(&self.calendar, &self.geo.goods, &names)));
            shown += 1;
            if shown == 150 {
                ui.small("(older entries not shown)");
                break;
            }
        }
        if shown == 0 {
            ui.small("Nothing yet.");
        }
        if let Some(d) = explain {
            self.explain(d, p);
        }
    }

    /// The realm that owns province `p`: who it is, who rules it, how it stands with the others,
    /// and what happened to it. Clicking a realm goes to its capital, and so to its page.
    fn realm_section(&mut self, ui: &mut egui::Ui, p: u32) {
        let w = &self.geo;
        let Some(r) = w.owner.get(p as usize).copied().filter(|&r| r != NONE) else {
            ui.label("No realm owns this province.");
            return;
        };
        let names = Names { world: w, realms: &self.realm_names.names };
        let page = inspector::realm_page(&names, r, self.day(), &self.calendar, self.coin_columns.first().copied(), &self.shown_state.trade);
        let mut go = None;
        ui.horizontal(|ui| {
            let [cr, cg, cb] = layers::realm_color(r, w.params.seed);
            ui.label(egui::RichText::new("■").size(22.0).color(egui::Color32::from_rgb(cr, cg, cb)));
            ui.heading(&page.name);
        });
        ui.horizontal_wrapped(|ui| {
            match page.capital {
                Some(c) if c == p => {
                    ui.label("Its capital is this province.");
                }
                Some(c) => {
                    ui.label("Capital:");
                    if ui.small_button(format!("province {c}")).on_hover_text("select the capital").clicked() {
                        go = Some(c);
                    }
                }
                None => {
                    ui.label("Dissolved: it has no land left.");
                }
            }
        });
        for f in &page.facts {
            ui.label(f);
        }
        let kept = self.kept();
        let open = self.opts.open_sections.iter().any(|s| s == "change-realm");
        let change = self.realm_tools.realm(ui, w, &names, r, &kept, open);
        let person = |ui: &mut egui::Ui, title: &str, who: &Option<inspector::Person>| match who {
            Some(x) => {
                ui.label(egui::RichText::new(format!("{title}: {}", x.name)).strong());
                for l in &x.lines {
                    ui.label(format!("   {l}"));
                }
            }
            None => {
                ui.label(format!("{title}: none"));
            }
        };
        // Rulers only where some module keeps them.
        if self.owners[sim::fields::known("realm.ruler").0 as usize].1.is_some() {
            person(ui, "Ruler", &page.ruler);
            person(ui, "Heir", &page.heir);
        }
        for (other, text) in &page.relations {
            ui.horizontal_wrapped(|ui| {
                ui.label(text);
                let capital = w.realms.capital.get(*other as usize).copied().filter(|&c| c != NONE);
                if let Some(c) = capital
                    && ui.small_button("go").on_hover_text(format!("select {}'s capital", names.realm(*other))).clicked()
                {
                    go = Some(c);
                }
            });
        }
        if !page.past_rulers.is_empty() {
            egui::CollapsingHeader::new(format!("Earlier rulers ({})", page.past_rulers.len())).id_salt(("earlier rulers", r)).show(ui, |ui| {
                for l in &page.past_rulers {
                    ui.label(l);
                }
            });
        }
        let history: Vec<String> = self
            .log
            .iter()
            .rev()
            .filter(|e| matches!(&e.what, inspector::Logged::Event(ev) if inspector::realms_in(ev).contains(&r)))
            .take(100)
            .map(|e| format!("{} · {}", self.calendar.format(e.day), e.text(&self.calendar, &w.goods, &names)))
            .collect();
        egui::CollapsingHeader::new(format!("What happened to {}", page.name)).id_salt(("realm history", r)).show(ui, |ui| {
            if history.is_empty() {
                ui.small("Nothing yet, in the days shown.");
            }
            for h in &history {
                ui.label(h);
            }
        });
        if let Some(c) = go {
            (self.selected, self.pop_edit, self.stock_edit) = (Some(c), None, None);
        }
        if let Some(a) = change {
            self.act(a);
        }
    }

    fn check_screenshot(&mut self, ctx: &egui::Context) {
        let shot = ctx.input(|i| i.raw.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
        let Some(image) = shot else { return };
        let path = match &self.opts.screenshot {
            Some(p) => std::path::PathBuf::from(p),
            None => std::path::Path::new(&self.opts.out_dir).join(format!("fantasymap_{}.png", self.opts.tag)),
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("create output folder");
        }
        let file = std::fs::File::create(&path).expect("png file");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), image.width() as u32, image.height() as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut w| w.write_image_data(image.as_raw())).expect("png write");
        if self.opts.bench {
            let notes = format!("projection {:?}; pixels_per_point {}", self.projection, ctx.pixels_per_point());
            let json = self.stats.to_json(&self.adapter, self.geo.grid.cells(), self.line_segments, (image.width() as u32, image.height() as u32), &notes);
            std::fs::write(path.with_extension("json"), json).expect("json write");
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

struct MapCallback {
    uniforms: MapUniforms,
    copies: u32,
    tables: Option<Vec<layers::Rgba>>,
    trade: Option<Vec<app::geometry::LineInstance>>,
    realm: Option<Vec<app::geometry::LineInstance>>,
    /// A changed map's picture, to replace the one on the GPU.
    rebuild: Option<Arc<MapData>>,
    target: wgpu::TextureFormat,
}

impl egui_wgpu::CallbackTrait for MapCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(data) = &self.rebuild {
            resources.insert(MapGpu::new(device, queue, self.target, data));
        }
        let gpu: &mut MapGpu = resources.get_mut().expect("map resources");
        queue.write_buffer(&gpu.uniforms, 0, bytemuck::bytes_of(&self.uniforms));
        if let Some(t) = &self.tables {
            gpu.upload_tables(queue, t);
        }
        if let Some(t) = &self.trade {
            gpu.upload_trade(device, queue, t);
        }
        if let Some(t) = &self.realm {
            gpu.upload_realm_borders(device, queue, t);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let gpu: &MapGpu = resources.get().expect("map resources");
        gpu.paint(pass, &self.uniforms, self.copies);
    }
}

impl eframe::App for MapApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = self.last.map(|l| now - l).unwrap_or_default();
        self.last = Some(now);
        self.last_dt_ms = dt.as_secs_f64() * 1000.0;
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point() as f64;

        self.pump();
        // A new world made: it takes the place of this one.
        if let Some((_, rx)) = &self.new_world.making {
            match rx.try_recv() {
                Ok(Ok((timelines, data, gen_s))) => {
                    self.new_world.making = None;
                    self.adopt(timelines, data, gen_s);
                }
                Ok(Err(e)) => (self.new_world.making, self.new_world.failed) = (None, Some(e)),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => (self.new_world.making, self.new_world.failed) = (None, Some("the world maker stopped".into())),
            }
        }
        self.play(dt.as_secs_f64().min(0.25));

        egui::Panel::top("time").show(ui, |ui| self.time_bar(ui));
        egui::Panel::left("controls").exact_size(270.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui));
        });
        // Only a province the day shown has (a selection can outlive it, or come before it).
        if let Some(p) = self.selected.filter(|&p| (p as usize) < self.geo.provinces.count) {
            egui::Panel::right("inspector").default_size(460.0).show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui, p));
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
            let (w, h) = ((rect.width() as f64 * ppp).round(), (rect.height() as f64 * ppp).round());
            let origin = ((rect.min.x as f64 * ppp).round(), (rect.min.y as f64 * ppp).round());
            let mut cam = self.cam.unwrap_or_else(|| {
                let mut c = Camera::fit(self.projection, w, h);
                if let Some((lon, lat, zoom)) = self.opts.view {
                    c.lon0 = lon.to_radians();
                    c.cy = self.projection.forward(0.0, lat.to_radians()).1;
                    c.scale = c.reference_scale(h) * zoom;
                }
                c
            });
            if cam.projection != self.projection {
                cam = cam.with_projection(self.projection, h);
            }

            self.hover = None;
            if self.opts.bench {
                let fit = Camera::fit(self.projection, w, h);
                let (phase, c) = bench::frame(self.frame, fit, self.bench_target);
                cam = c;
                self.stats.record(phase, dt);
                if phase == bench::Phase::Done && !self.screenshot_sent {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                    self.screenshot_sent = true;
                }
            } else {
                let to_map = |p: egui::Pos2| ((p.x - rect.min.x) as f64 * ppp, (p.y - rect.min.y) as f64 * ppp);
                if let Some(p) = response.hover_pos() {
                    let (x, y) = to_map(p);
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y) as f64;
                    if scroll != 0.0 {
                        cam.zoom_at((scroll * 0.003).exp(), x, y, w, h);
                    }
                    let grid = &self.geo.grid;
                    // The picture may still show a map with more provinces than the day shown has.
                    let count = self.geo.provinces.count as u32;
                    self.hover = cam.pick(grid, x, y, w, h).map(|c| (c, cam.pick_province(&self.provinces, x, y, w, h).filter(|&p| p < count).unwrap_or(NONE)));
                }
                let draws = self.editor.draws();
                let planet_km = self.geo.params.radius_km;
                let primary = egui::PointerButton::Primary;
                // Brush tools draw with the left button; a click is a stroke of one point.
                if draws
                    && (response.dragged_by(primary) || response.clicked())
                    && let Some(p) = response.interact_pointer_pos()
                {
                    let (x, y) = to_map(p);
                    if let Some((lon, lat)) = cam.screen_to_lonlat(x, y, w, h) {
                        self.editor.add_point(lon, lat, planet_km);
                    }
                }
                if draws && (response.drag_stopped_by(primary) || response.clicked()) {
                    if let Some(edit) = self.editor.finish(&self.geo, self.selected) {
                        self.send_edit(edit);
                    }
                } else if response.clicked() {
                    let clicked = self.hover.map(|(_, p)| p).filter(|&p| p != NONE);
                    match self.editor.click(clicked, self.selected) {
                        Some(Clicked::Edit(edit)) => self.send_edit(edit),
                        Some(Clicked::Poke(e)) => self.poke(e),
                        Some(Clicked::Picked) => {}
                        None => {
                            self.selected = clicked;
                            (self.pop_edit, self.stock_edit) = (None, None);
                        }
                    }
                }
                let pans = response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) || (!draws && response.dragged_by(primary));
                if pans
                    && let Some(p) = response.interact_pointer_pos()
                {
                    let d = response.drag_delta();
                    let (_, y) = to_map(p);
                    cam.pan(d.x as f64 * ppp, d.y as f64 * ppp, y - d.y as f64 * ppp, h);
                }
                // Scripted edits, one a frame, once the window is up.
                if self.frame >= 3
                    && self.waiting.is_none()
                    && !self.opts.live_edits.is_empty()
                {
                    let e = self.opts.live_edits.remove(0);
                    self.live_edit(e);
                }
                // A new world from the command line, made as a player would.
                if self.frame >= 3
                    && let Some(text) = self.opts.remake.take()
                {
                    let nw = &mut self.new_world;
                    let parts: Vec<&str> = text.splitn(4, ',').collect();
                    nw.set_size(match parts[0] {
                        "small" => Size::Small,
                        "medium" => Size::Medium,
                        "large" => Size::Large,
                        "huge" => Size::Huge,
                        other => panic!("--remake: unknown size {other}"),
                    });
                    nw.seed = parts.get(1).expect("--remake SIZE,SEED,YEARS[,SLOT=VERSION...]").parse().expect("--remake seed");
                    nw.years = parts.get(2).expect("--remake SIZE,SEED,YEARS[,SLOT=VERSION...]").parse().expect("--remake years");
                    if let Some(m) = parts.get(3) {
                        sim::combos::choose(&mut nw.config.modules, m).unwrap_or_else(|e| panic!("--remake: {e}"));
                    }
                    let ctx = self.repaint.clone();
                    nw.make(move || ctx.request_repaint());
                }
                // Wait for an explanation asked for on the command line, for edits, a new world and
                // the redrawn map, so they are in the picture.
                let settled = self.trace_pending.is_none()
                    && self.waiting.is_none()
                    && self.rebuild.is_none()
                    && self.pending_gpu.is_none()
                    && self.opts.live_edits.is_empty()
                    && self.opts.remake.is_none()
                    && self.new_world.making.is_none();
                if self.opts.screenshot.is_some() && self.frame >= 10 && settled && !self.screenshot_sent {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                    self.screenshot_sent = true;
                }
            }
            self.frame += 1;
            self.cam = Some(cam);

            let hover_province = self.hover.map_or(NONE, |(_, p)| p);
            let uniforms = gpu::uniforms(&cam, self.geo.grid.n, w, h, origin, ppp, &self.layers, hover_province, self.selected.unwrap_or(NONE));
            let (first, last) = cam.visible_copies(w);
            // egui throws a pass away when a layout needs another go (a table shown for the first
            // time): what waits for the GPU stays waiting until a pass that is drawn.
            let (tables, trade, realm, rebuild) = match ctx.will_discard() {
                true => (None, None, None, None),
                false => (self.pending_tables.take(), self.pending_trade.take(), self.pending_realm.take(), self.pending_gpu.take()),
            };
            let callback = MapCallback { uniforms, copies: (last - first + 1) as u32, tables, trade, realm, rebuild, target: self.target };
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));
            // The brush around the pointer, and the stroke being drawn.
            let to_screen = |lon: f64, lat: f64| {
                let (x, y) = cam.lonlat_to_screen(lon, lat, w, h);
                egui::pos2(rect.min.x + (x / ppp) as f32, rect.min.y + (y / ppp) as f32)
            };
            let ink = egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(60, 30, 10, 200));
            if let Some(r_km) = self.editor.brush_radius_km()
                && let Some(p) = response.hover_pos()
                && let Some((lon, lat)) = cam.screen_to_lonlat((p.x - rect.min.x) as f64 * ppp, (p.y - rect.min.y) as f64 * ppp, w, h)
            {
                let r = r_km / self.geo.params.radius_km;
                let edge = to_screen(lon, (lat + r).min(std::f64::consts::FRAC_PI_2));
                let centre = to_screen(lon, lat);
                ui.painter().circle_stroke(centre, centre.distance(edge), ink);
                ui.painter().circle_stroke(centre, centre.distance(edge) * 0.5, egui::Stroke::new(1.0, ink.color.gamma_multiply(0.5)));
            }
            if self.kept().armies && (self.layers.armies || self.layers.battles) {
                let (first, last) = cam.visible_copies(w);
                // The rectangle repeats sideways: everything is drawn on each copy in view.
                let period = (2.0 * self.projection.half_width() * cam.scale / ppp) as f32;
                let half = period * 0.5;
                let t = self.clock.blend();
                let (layers, hover) = (self.layers, response.hover_pos());
                let view = self.army_view().clone();
                let geo = &self.geo;
                let lonlat = |d: sim::grid::V3| sim::grid::dir_to_lonlat(d);
                let ink_c = egui::Color32::from_rgba_unmultiplied(40, 20, 8, 230);
                let mut label: Option<(egui::Pos2, String)> = None;
                for k in first..=last {
                    let dx = egui::vec2(k as f32 * period, 0.0);
                    let at = |d: sim::grid::V3| {
                        let (lon, lat) = lonlat(d);
                        to_screen(lon, lat) + dx
                    };
                    if layers.armies {
                        // Markers standing at one seat sit side by side.
                        let mut at_seat: HashMap<u32, usize> = HashMap::new();
                        for m in &view.markers {
                            let [cr, cg, cb] = layers::realm_color(m.realm, geo.params.seed);
                            let colour = egui::Color32::from_rgb(cr, cg, cb);
                            let r = (3.0 + (m.soldiers as f32).sqrt() / 30.0).clamp(4.0, 15.0);
                            let mut c = at(m.at(geo, t));
                            if m.to == NONE {
                                let n = at_seat.entry(m.from).or_default();
                                c.x += *n as f32 * (r * 2.0 + 2.0);
                                *n += 1;
                            } else if m.dest != NONE && (m.dest as usize) < geo.provinces.count {
                                // A dotted line on to where it is going, broken where it would
                                // cross the map's edge.
                                let from = m.at(geo, t);
                                let to = geo.provinces.center[m.dest as usize];
                                let steps = 24;
                                let points: Vec<egui::Pos2> = (0..=steps).map(|i| at(app::armies::slerp(from, to, i as f64 / steps as f64))).collect();
                                for (i, pair) in points.windows(2).enumerate() {
                                    if i % 2 == 0 && (pair[0].x - pair[1].x).abs() < half {
                                        ui.painter().line_segment([pair[0], pair[1]], egui::Stroke::new(1.2, colour.gamma_multiply(0.8)));
                                    }
                                }
                            }
                            let stroke = egui::Stroke::new(1.2, ink_c);
                            if m.ship {
                                let hull = vec![c + egui::vec2(-r * 1.3, -r * 0.4), c + egui::vec2(r * 1.3, -r * 0.4), c + egui::vec2(r * 0.8, r * 0.6), c + egui::vec2(-r * 0.8, r * 0.6)];
                                ui.painter().add(egui::Shape::convex_polygon(hull, colour, stroke));
                            } else {
                                ui.painter().circle(c, r, colour, stroke);
                            }
                            if let Some(h) = hover
                                && h.distance(c) <= r + 2.0
                            {
                                let names = Names { world: geo, realms: &self.realm_names.names };
                                label = Some((c, app::armies::describe(m, &self.troop_kinds, &names.realm(m.realm))));
                            }
                        }
                    }
                    if layers.battles {
                        let swords = |c: egui::Pos2, alpha: f32| {
                            let s = egui::Stroke::new(2.5, egui::Color32::from_rgba_unmultiplied(170, 20, 20, (255.0 * alpha) as u8));
                            ui.painter().line_segment([c + egui::vec2(-7.0, -7.0), c + egui::vec2(7.0, 7.0)], s);
                            ui.painter().line_segment([c + egui::vec2(-7.0, 7.0), c + egui::vec2(7.0, -7.0)], s);
                        };
                        for &(p, fade) in &view.recent {
                            if !view.battles.contains(&p) && (p as usize) < geo.provinces.count {
                                swords(at(geo.provinces.center[p as usize]) + egui::vec2(0.0, -10.0), 0.7 * (1.0 - fade as f32));
                            }
                        }
                        for &p in &view.battles {
                            if (p as usize) < geo.provinces.count {
                                swords(at(geo.provinces.center[p as usize]) + egui::vec2(0.0, -10.0), 1.0);
                            }
                        }
                    }
                }
                if let Some((c, text)) = label {
                    let font = egui::FontId::proportional(13.0);
                    let galley = ui.painter().layout_no_wrap(text, font, egui::Color32::from_rgb(30, 15, 5));
                    let box_ = egui::Rect::from_min_size(c + egui::vec2(12.0, -8.0), galley.size() + egui::vec2(8.0, 4.0));
                    ui.painter().rect_filled(box_, 3.0, egui::Color32::from_rgba_unmultiplied(245, 235, 210, 235));
                    ui.painter().galley(box_.min + egui::vec2(4.0, 2.0), galley, egui::Color32::BLACK);
                }
            }
            // Provinces the realms tool picked, ringed at their seats.
            if self.editor.tool == app::editor::Tool::Realms {
                for &q in &self.editor.picked {
                    if let Some(&d) = self.geo.provinces.center.get(q as usize) {
                        let (lon, lat) = sim::grid::dir_to_lonlat(d);
                        let c = to_screen(lon, lat);
                        ui.painter().circle_stroke(c, 7.0, egui::Stroke::new(2.5, egui::Color32::from_rgb(200, 40, 30)));
                    }
                }
            }
            if self.editor.stroke.len() > 1 {
                let points: Vec<egui::Pos2> = self.editor.stroke.iter().map(|q| to_screen(q[0].to_radians(), q[1].to_radians())).collect();
                ui.painter().add(egui::Shape::line(points, ink));
            }
        });

        let busy = self.waiting.is_some() || self.rebuild.is_some() || self.notice.is_some();
        if self.opts.bench || self.opts.screenshot.is_some() || self.clock.speed != Speed::Paused || busy {
            ctx.request_repaint();
        }
        if self.opts.bench || self.opts.screenshot.is_some() {
            self.check_screenshot(&ctx);
        }
    }
}
