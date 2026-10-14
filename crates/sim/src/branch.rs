//! Timelines. Going back to a past day and changing something there starts a new timeline from that
//! day; the old one is kept, and the player can switch between them. A new timeline keeps the old
//! one's commands up to the day it leaves, and none after: what the player did later in the old
//! timeline doesn't happen in the new one.
//!
//! Each timeline is a whole simulation of its own, so switching is instant. A new one copies the
//! world and shares the checkpoints and map changes the two have in common, so a timeline costs
//! about one world's worth of memory.

use crate::save::LoadError;
use crate::sim::{CommandKind, Sim};
use std::sync::Arc;

impl Sim {
    /// An exact copy of this simulation, sharing its checkpoints and map changes.
    pub fn fork(&self) -> Sim {
        let mut s = Sim::assemble(&self.config).expect("this simulation's own configuration");
        for (m, from) in s.modules.iter_mut().zip(&self.modules) {
            m.load_state(&from.save_state()).expect("the same module's state");
        }
        s.world = self.world.clone();
        s.day = self.day;
        s.pending = self.pending.clone();
        s.seq = self.seq;
        s.year_flows = self.year_flows.clone();
        s.history = self.history.clone();
        s.problems = self.problems;
        s.commands = self.commands.clone();
        s.applied_today = self.applied_today.clone();
        s.event_log = self.event_log.clone();
        s.log_today = self.log_today;
        s.strict = self.strict;
        s.next_command = self.next_command;
        s.edits_done = self.edits_done;
        s.map_changes = self.map_changes.clone();
        s.map_at = self.map_at;
        s.skipped_edits = self.skipped_edits.clone();
        s.checkpoints = self.checkpoints.clone();
        s.last_change = self.last_change.clone();
        s
    }

    /// A new timeline that leaves this one at the start of `day`: the same world, with the commands
    /// logged before that day and the edits logged for it (the day as shown already has them), but
    /// not that day's pokes nor anything later.
    pub fn branch(&self, day: u64) -> Result<Sim, String> {
        let mut b = self.fork();
        b.rewind(day)?;
        let keep = |c: &crate::sim::Command| c.day < day || (c.day == day && matches!(c.kind, CommandKind::Edit { .. }));
        b.commands.retain(keep);
        // Every command kept up to the day is applied; the day's pokes, which aren't, are gone.
        let before = b.commands.partition_point(|c| c.day < day);
        let through = b.commands.partition_point(|c| c.day <= day);
        b.edits_done = through;
        b.next_command = before;
        for c in b.checkpoints.iter_mut().filter(|c| c.day == day && c.edits_done > before) {
            Arc::make_mut(c).edits_done = through;
        }
        Ok(b)
    }
}

/// One timeline: where it came from, and its simulation.
pub struct Timeline {
    pub id: u32,
    /// The timeline it left, and the day it left it.
    pub parent: Option<(u32, u64)>,
    pub sim: Sim,
}

/// Every timeline of a world, and which one is played.
pub struct Timelines {
    pub list: Vec<Timeline>,
    /// Index into `list`.
    pub active: usize,
}

/// Start of a file holding every timeline.
pub const MAGIC: &[u8; 8] = b"FMAPTIME";

impl Timelines {
    pub fn new(sim: Sim) -> Timelines {
        Timelines { list: vec![Timeline { id: 0, parent: None, sim }], active: 0 }
    }

    pub fn sim(&self) -> &Sim {
        &self.list[self.active].sim
    }

    pub fn sim_mut(&mut self) -> &mut Sim {
        &mut self.list[self.active].sim
    }

    pub fn active_id(&self) -> u32 {
        self.list[self.active].id
    }

    /// Start a new timeline at the start of `day` of the one being played, and play it. The old one
    /// is kept as it was. Returns the new timeline's id.
    pub fn branch(&mut self, day: u64) -> Result<u32, String> {
        let sim = self.sim().branch(day)?;
        let id = self.list.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        self.list.push(Timeline { id, parent: Some((self.active_id(), day)), sim });
        self.active = self.list.len() - 1;
        Ok(id)
    }

    /// Play timeline `id`, from where it was left.
    pub fn switch(&mut self, id: u32) -> Result<(), String> {
        self.active = self.list.iter().position(|t| t.id == id).ok_or_else(|| format!("no timeline {id}"))?;
        Ok(())
    }

    /// Every timeline, each as a save of its own (see [`Sim::save`]): its history before the day it
    /// was saved isn't kept.
    pub fn save(&self) -> Vec<u8> {
        let mut out = Vec::from(&MAGIC[..]);
        let mut put = |x: u64| out.extend(x.to_le_bytes());
        put(self.list.len() as u64);
        put(self.active as u64);
        for t in &self.list {
            let bytes = t.sim.save();
            let (parent, day) = t.parent.map_or((u64::MAX, 0), |(p, d)| (p as u64, d));
            for x in [t.id as u64, parent, day, bytes.len() as u64] {
                out.extend(x.to_le_bytes());
            }
            out.extend(bytes);
        }
        out
    }

    pub fn load(bytes: &[u8]) -> Result<Timelines, LoadError> {
        if bytes.len() < 8 || &bytes[..8] != MAGIC {
            return Err(LoadError("not a timelines file".into()));
        }
        let mut r = Reader { bytes, at: 8 };
        let (count, active) = (r.u64()?, r.u64()? as usize);
        let mut list = Vec::new();
        for _ in 0..count {
            let (id, parent, day, len) = (r.u64()? as u32, r.u64()?, r.u64()?, r.u64()? as usize);
            let sim = Sim::load(r.take(len)?)?;
            list.push(Timeline { id, parent: (parent != u64::MAX).then_some((parent as u32, day)), sim });
        }
        if active >= list.len() {
            return Err(LoadError("damaged timelines file: no such timeline to play".into()));
        }
        Ok(Timelines { list, active })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], LoadError> {
        let b = self.bytes.get(self.at..self.at.saturating_add(n)).ok_or_else(|| LoadError("damaged timelines file".into()))?;
        self.at += n;
        Ok(b)
    }

    fn u64(&mut self) -> Result<u64, LoadError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }
}
