//! Save files.
//!
//! A save is a short header (a magic tag and the format version, checked before anything else is
//! read) and then, in postcard:
//!
//! - the configuration: world parameters, calendar, and each slot's version with its parameters;
//! - each module's slot, version and private-state schema, so a load with a different build is
//!   refused instead of misread;
//! - the command log, which starts with the configuration's timeline segment;
//! - snapshots. Each holds every shared field by name with its taxonomy depth (and, for a per-good
//!   field, the goods it holds), each module's private state tagged with its schema, and the
//!   engine's state in flight: events waiting for delivery (grants included), the year's flows,
//!   the history of years and the problem counts.
//!
//! The event log is not saved: it is history, and prunable. Saves are taken between days, when no
//! change request is queued.

use crate::config::Config;
use crate::events::Posted;
use crate::fields::{FieldData, FieldId, Goods, FIELDS};
use crate::module::{ConfigError, Flows, Slot};
use crate::sim::{to_bytes, Command, ModuleRecord, Problems, Sim, YearSummary};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"FMAPSAVE";
/// Bump on any change to the layout below, to any saved type, or to the order of slots, fields,
/// causes or events (saved events refer to fields by position).
/// Format 2 (step 4): the Weather slot, the rain shortfall field, drought events and pokes.
/// Format 3 (step 5): edits in the command log, the edit cause, and edits in the year's ledger.
/// Format 4 (step 5): the drainage order is no longer stored (it is rebuilt from the downstream
/// links when rivers are worked out).
/// Format 5 (step 5): climate and biomes painted by the editor, and springs of drawn rivers.
/// Format 6 (step 6): the goods tree in the configuration, the food store kept per good
/// (`province.stock`), and the goods each per-good field holds.
/// Format 7 (step 6): change requests name the good asked for, and a request on a per-good field
/// names the province rather than one stored good.
/// Format 8 (step 6): the edit that sets any good's store.
/// Format 9 (step 6): what each province's land offers (grazing, forest, coast, rivers, uplands,
/// ore), cached from its cells.
/// Format 10 (step 6): Economics "markets": its parameters in the configuration, prices and work
/// per good, and the causes made, made into, used and worn out.
/// Format 11 (step 6): trade in the markets parameters, and the causes trade in and trade out.
/// Format 12 (step 6): trade's prices follow the money (two more trade parameters).
/// Format 13 (step 7): province owners, realms and their stores, armies; four new slots; requests
/// that move what they take (`Request::to`); the causes taxes, raised and returned; the event of
/// people arriving.
/// Format 14 (step 7): Realms "basic": its parameters in the configuration.
/// Format 15 (step 7): Characters "rulers": its parameters in the configuration, the character
/// fields, each realm's ruler and heir, and the succession event.
/// Format 16 (step 7): Diplomacy "wars": its parameters in the configuration, the war fields,
/// occupation, and the events of war and peace.
/// Format 17 (step 7): taxes in the realms parameters, the realms module's private state (each
/// realm's last capital), the cause of a realm's spending, and markets keeping the realms' stores.
/// Format 18 (step 7): unrest in the realms parameters and how long a rebellion's war lasts in the
/// wars', unrest, uprisings and the realm each broke away from, the realms module's private state
/// (each province's hunger this month too), the events of uprisings and rebellions, and a peace
/// saying whether it ended a rebellion.
pub const FORMAT_VERSION: u32 = 18;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveFile {
    pub config: Config,
    pub modules: Vec<ModuleRecord>,
    pub commands: Vec<Command>,
    pub snapshots: Vec<Snapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub day: u64,
    pub fields: Vec<FieldBlob>,
    pub module_state: Vec<ModuleState>,
    pub engine: EngineState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldBlob {
    pub name: String,
    /// Taxonomy depth the values are stored at: the deepest good kept, for a per-good field; 0
    /// for every other field.
    pub depth: u8,
    /// For a per-good field, the goods it holds, in column order; empty otherwise.
    pub goods: Vec<String>,
    pub data: FieldData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModuleState {
    pub slot: Slot,
    pub state_schema: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineState {
    pub pending: Vec<Posted>,
    pub year_flows: Flows,
    pub history: Vec<YearSummary>,
    pub problems: Problems,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError(pub String);

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot load save: {}", self.0)
    }
}

impl std::error::Error for LoadError {}

impl From<ConfigError> for LoadError {
    fn from(e: ConfigError) -> Self {
        LoadError(e.to_string())
    }
}

impl Sim {
    /// The whole simulation as save-file bytes, with one snapshot at the current day.
    pub fn save(&self) -> Vec<u8> {
        let fields = FieldId::all()
            .map(|id| {
                let (depth, goods) = layout(&self.world.goods, id);
                FieldBlob { name: id.def().name.into(), depth, goods, data: self.world.field(id) }
            })
            .collect();
        let module_state = self
            .modules
            .iter()
            .map(|m| ModuleState { slot: m.manifest().slot, state_schema: m.manifest().state_schema.into(), bytes: m.save_state() })
            .collect();
        let engine = EngineState {
            pending: self.pending.clone(),
            year_flows: self.year_flows.clone(),
            history: self.history.clone(),
            problems: self.problems,
        };
        let file = SaveFile {
            config: self.config.clone(),
            modules: self.manifests().iter().map(|m| ModuleRecord::of(m)).collect(),
            commands: self.commands.clone(),
            snapshots: vec![Snapshot { day: self.day, fields, module_state, engine }],
        };
        let mut out = Vec::from(&MAGIC[..]);
        out.extend(FORMAT_VERSION.to_le_bytes());
        out.extend(to_bytes(&file));
        out
    }

    /// Rebuild a simulation from save-file bytes, from its latest snapshot.
    pub fn load(bytes: &[u8]) -> Result<Sim, LoadError> {
        Sim::load_with(bytes, |c| c.modules.build())
    }

    /// [`Sim::load`] with modules of the caller's own, built from the save's configuration (tests).
    pub(crate) fn load_with(bytes: &[u8], build: impl FnOnce(&Config) -> Vec<Box<dyn crate::module::Module>>) -> Result<Sim, LoadError> {
        let err = |s: String| Err(LoadError(s));
        if bytes.len() < 12 || &bytes[..8] != MAGIC {
            return err("not a save file".into());
        }
        let format = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if format != FORMAT_VERSION {
            return err(format!("save format {format}; this build reads format {FORMAT_VERSION}"));
        }
        let file: SaveFile = postcard::from_bytes(&bytes[12..]).map_err(|e| LoadError(format!("damaged save: {e}")))?;
        let mut sim = Sim::assemble_with(&file.config, build(&file.config))?;

        let here: Vec<ModuleRecord> = sim.manifests().iter().map(|m| ModuleRecord::of(m)).collect();
        if here != file.modules {
            let show = |v: &[ModuleRecord]| v.iter().map(|r| format!("{:?} {} ({})", r.slot, r.version, r.state_schema)).collect::<Vec<_>>().join(", ");
            return err(format!("the save's modules are [{}]; this build has [{}]", show(&file.modules), show(&here)));
        }
        let Some(snap) = file.snapshots.into_iter().last() else { return err("no snapshot".into()) };

        if let Some(b) = snap.fields.iter().find(|b| crate::fields::id(&b.name).is_none()) {
            return err(format!("unknown field {}", b.name));
        }
        let mut blobs = snap.fields;
        for def in FIELDS {
            let Some(k) = blobs.iter().position(|b| b.name == def.name) else { return err(format!("missing field {}", def.name)) };
            let b = blobs.swap_remove(k);
            let (depth, goods) = layout(&sim.world.goods, crate::fields::known(def.name));
            if b.depth != depth {
                return err(format!("{} is stored at taxonomy depth {}, this world uses {depth}", def.name, b.depth));
            }
            if b.goods != goods {
                return err(format!("{} holds goods [{}], this world keeps [{}]", def.name, b.goods.join(", "), goods.join(", ")));
            }
            sim.world.set_field(crate::fields::known(def.name), b.data).map_err(LoadError)?;
        }
        sim.world.check_lengths().map_err(LoadError)?;
        sim.world.rebuild_routes();

        for m in sim.modules.iter_mut() {
            let man = m.manifest();
            let Some(s) = snap.module_state.iter().find(|s| s.slot == man.slot) else {
                return err(format!("no state for {:?}", man.slot));
            };
            if s.state_schema != man.state_schema {
                return err(format!("{:?} state is schema {}, this build reads {}", man.slot, s.state_schema, man.state_schema));
            }
            m.load_state(&s.bytes).map_err(LoadError)?;
        }

        sim.day = snap.day;
        sim.pending = snap.engine.pending;
        sim.year_flows = snap.engine.year_flows;
        sim.history = snap.engine.history;
        sim.problems = snap.engine.problems;
        sim.commands = file.commands;
        sim.next_command = sim.commands.partition_point(|c| c.day < sim.day);
        // The snapshot was taken between days, with the day's edits applied.
        sim.edits_done = sim.commands.partition_point(|c| c.day <= sim.day);
        // History before the load isn't kept: going back stops here.
        sim.checkpoints = vec![std::sync::Arc::new(sim.checkpoint())];
        Ok(sim)
    }

}

/// A field's taxonomy depth and goods, as a save records them.
fn layout(goods: &Goods, id: FieldId) -> (u8, Vec<String>) {
    if id.def().tier.per_good() { (goods.depth(), goods.names()) } else { (0, Vec::new()) }
}

impl Sim {
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.save())
    }

    pub fn load_from(path: &Path) -> Result<Sim, LoadError> {
        Sim::load(&std::fs::read(path).map_err(|e| LoadError(format!("{}: {e}", path.display())))?)
    }
}
