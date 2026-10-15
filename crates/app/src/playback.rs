//! Playback: the simulation runs on its own thread, a few months ahead of what is shown, and sends
//! one frame per simulated day. The window plays the frames back at the chosen speed and blends the
//! two days either side of the moment shown. The simulation never knows whether anyone watches.
//!
//! A change (a poke, an edit, undo, redo) throws the days computed ahead away: the worker goes back
//! to the day on screen ([`Sim::rewind`]), makes the change, sends that day again so the screen shows
//! it at once, and computes the future again. A change on a day before the furthest one the player
//! has watched starts a new timeline instead, and the old one is kept. Going to another day or
//! another timeline moves through history without changing it ([`Sim::goto`]). Explaining a day
//! ([`Sim::trace`]) runs on the worker too, which then carries on exactly where it was.
//!
//! Every frame says which map it belongs to. Edits can change the map, so the window rebuilds its
//! picture when the map of the day shown changes.

use crate::layers::ProvinceView;
use sim::branch::Timelines;
use sim::edit::Edit;
use sim::events::{Event, Posted};
use sim::fields::{FieldData, FieldId};
use sim::requests::Request;
use sim::routes::Routes;
use sim::sim::{Command, Sim, Trace};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How many days the simulation may run ahead of the day shown. Enough to cover a heavy day at a
/// year a second; a change only costs going back, whatever this is.
pub const LOOKAHEAD: u64 = 120;

/// The map (the fields only world generation and edits write) as frames show it. Every frame with
/// the same map shares one.
pub struct MapState {
    /// How many worlds were opened before, the timeline, and [`Sim::map_version`] in it: the same
    /// key, the same map.
    pub key: (u32, u32, usize, usize),
    pub fields: Vec<(FieldId, FieldData)>,
    /// Province neighbours, worked out from this map.
    pub routes: Arc<Routes>,
}

impl MapState {
    pub fn of(opened: u32, timeline: u32, sim: &Sim) -> Self {
        let (at, last) = sim.map_version();
        MapState { key: (opened, timeline, at, last), fields: sim.map_fields().map(|id| (id, sim.world.field(id))).collect(), routes: sim.world.routes.clone() }
    }
}

/// Everything shown for one day that can change while the world runs. Shared between the frames
/// of days on which nothing ran.
pub struct DayState {
    pub fields: Vec<(FieldId, FieldData)>,
    pub last_change: Vec<Vec<u64>>,
    pub view: ProvinceView,
    /// How dear everything in each province is to trade, for the coins it holds
    /// ([`sim::markets::money_levels`]); `None` when prices don't follow the money.
    pub money_levels: Option<Vec<f64>>,
    /// Last month's trade ([`sim::markets::TradeRecord`]).
    pub trade: Arc<sim::markets::TradeRecord>,
    pub map: Arc<MapState>,
}

impl DayState {
    pub fn of(sim: &Sim, map: Arc<MapState>) -> Self {
        DayState {
            fields: sim.running_fields().map(|id| (id, sim.world.field(id))).collect(),
            last_change: sim.last_change.clone(),
            view: ProvinceView::now(sim),
            money_levels: sim.config.modules.markets.as_ref().and_then(|m| m.money_levels_now(&sim.world)),
            trade: sim.world.trade.clone(),
            map,
        }
    }
}

/// The world at the start of `day`, and what happened on the day before.
#[derive(Clone)]
pub struct Frame {
    pub day: u64,
    /// Frames from before the latest change are stale.
    pub epoch: u64,
    pub state: Arc<DayState>,
    /// Events delivered on the day before (grants excluded).
    pub delivered: Vec<Posted>,
    /// Change requests applied on the day before, with what was granted.
    pub applied: Vec<(Request, u64)>,
    /// How long the simulation took for the day before, ms.
    pub ms: f64,
    /// Sent again after a change, a jump or a switch: it replaces the day on screen, and `history`
    /// is the event log as it now stands.
    pub replaces: Option<Vec<Posted>>,
}

impl Frame {
    /// The world as it stands, as frame 0 of `epoch`.
    pub fn now(sim: &Sim, epoch: u64) -> Self {
        Self::with(sim, epoch, Arc::new(DayState::of(sim, Arc::new(MapState::of(0, 0, sim)))))
    }

    fn with(sim: &Sim, epoch: u64, state: Arc<DayState>) -> Self {
        Frame { day: sim.day, epoch, state, delivered: sim.delivered_today().to_vec(), applied: sim.applied_today.clone(), ms: 0.0, replaces: None }
    }
}

/// A change to the world, made on the day shown.
#[derive(Clone, Debug)]
pub enum Change {
    Poke(Event),
    Edit(Edit),
    /// Take back the latest command (an edit or a poke) of the timeline.
    Undo,
    /// Give back the command taken back last.
    Redo,
}

/// One timeline, as the window lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineInfo {
    pub id: u32,
    /// The timeline it left, and the day.
    pub parent: Option<(u32, u64)>,
    /// How many commands it holds (edits and pokes).
    pub commands: usize,
}

/// What the worker tells the window besides frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub timelines: Vec<TimelineInfo>,
    pub active: u32,
    /// The earliest day the timeline played can go back to.
    pub earliest: u64,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Something slow is under way (the words say what); the window says so until it ends.
    pub busy: Option<String>,
    /// A change that was refused, edits left out, or a file saved, in words.
    pub message: Option<String>,
    /// How many worlds were opened since the window started.
    pub opened: u32,
}

enum ToSim {
    Change { epoch: u64, day: u64, change: Change, branch: bool },
    GoTo { epoch: u64, day: u64 },
    Switch { epoch: u64, id: u32, day: u64 },
    Trace { day: u64, province: u32 },
    Save { path: std::path::PathBuf, day: u64 },
    Open { epoch: u64, path: std::path::PathBuf },
}

pub enum FromSim {
    Frame(Frame),
    Trace(Result<Trace, String>),
    Status(Status),
}

/// The simulation thread.
pub struct Worker {
    tx: Option<Sender<ToSim>>,
    pub rx: Receiver<FromSim>,
    /// The day on screen; the worker stays within [`LOOKAHEAD`] days of it.
    shown: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Hand `sim` to a new thread, which starts computing the days after the one it is on.
    /// `wake` is called whenever something new is ready (the window's repaint).
    pub fn start(sim: Sim, wake: impl Fn() + Send + 'static) -> Worker {
        Worker::start_timelines(Timelines::new(sim), wake)
    }

    pub fn start_timelines(timelines: Timelines, wake: impl Fn() + Send + 'static) -> Worker {
        let (tx, commands) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let shown = Arc::new(AtomicU64::new(timelines.sim().day));
        let watching = shown.clone();
        let thread = std::thread::Builder::new()
            .name("simulation".into())
            .spawn(move || run(timelines, commands, out, watching, wake))
            .expect("start the simulation thread");
        Worker { tx: Some(tx), rx, shown, thread: Some(thread) }
    }

    pub fn show(&self, day: u64) {
        self.shown.store(day, Ordering::Relaxed);
    }

    /// Change the world at the start of `day` (the day shown); frames from before have older epochs.
    /// With `branch`, a new timeline leaves the one played at `day` first.
    pub fn change(&self, epoch: u64, day: u64, change: Change, branch: bool) {
        self.send(ToSim::Change { epoch, day, change, branch });
    }

    /// Poke the world at the start of `day` (the day shown).
    pub fn poke(&self, epoch: u64, day: u64, event: Event) {
        self.change(epoch, day, Change::Poke(event), false);
    }

    /// Show `day` of the timeline played, earlier or later, without changing anything.
    pub fn goto(&self, epoch: u64, day: u64) {
        self.send(ToSim::GoTo { epoch, day });
    }

    /// Play timeline `id` from `day`.
    pub fn switch(&self, epoch: u64, id: u32, day: u64) {
        self.send(ToSim::Switch { epoch, id, day });
    }

    /// Write every timeline to `path`: a scenario if it ends in `.toml`, else saves. The one played
    /// is saved on `day` (the day shown), not on the day the simulation has run ahead to.
    pub fn save(&self, path: std::path::PathBuf, day: u64) {
        self.send(ToSim::Save { path, day });
    }

    /// Open the world in `path` in place of this one. It must be the same world (size, seed,
    /// modules, calendar), since the window was made for it.
    pub fn open(&self, epoch: u64, path: std::path::PathBuf) {
        self.send(ToSim::Open { epoch, path });
    }

    /// Explain what happened to `province` on `day`; the answer comes back as [`FromSim::Trace`].
    pub fn trace(&self, day: u64, province: u32) {
        self.send(ToSim::Trace { day, province });
    }

    fn send(&self, m: ToSim) {
        if let Some(tx) = &self.tx {
            // A worker that died has already reported its panic.
            let _ = tx.send(m);
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx = None; // the worker stops when its command channel closes
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The worker's side: the timelines, the map frames share, and what can be given back per timeline.
struct Run {
    t: Timelines,
    map: Arc<MapState>,
    /// Per timeline, commands taken back, the latest last.
    redo: HashMap<u32, Vec<Command>>,
    out: Sender<FromSim>,
    opened: u32,
}

impl Run {
    fn sim(&mut self) -> &mut Sim {
        self.t.sim_mut()
    }

    /// The day's state, with the map frames share (made again only when the map changed).
    fn state(&mut self) -> Arc<DayState> {
        let id = self.t.active_id();
        let (at, last) = self.t.sim().map_version();
        if self.map.key != (self.opened, id, at, last) {
            self.map = Arc::new(MapState::of(self.opened, id, self.t.sim()));
        }
        Arc::new(DayState::of(self.t.sim(), self.map.clone()))
    }

    fn status(&mut self, busy: Option<String>, message: Option<String>) -> bool {
        let t = &self.t;
        let sim = t.sim();
        let edits = |s: &Sim| s.commands.len() - 1;
        let status = Status {
            timelines: t.list.iter().map(|x| TimelineInfo { id: x.id, parent: x.parent, commands: edits(&x.sim) }).collect(),
            active: t.active_id(),
            earliest: sim.earliest_day(),
            can_undo: edits(sim) > 0,
            can_redo: self.redo.get(&t.active_id()).is_some_and(|r| !r.is_empty()),
            busy,
            message,
            opened: self.opened,
        };
        self.out.send(FromSim::Status(status)).is_ok()
    }

    /// Say what is slow before doing it.
    fn busy(&mut self, what: &str) -> bool {
        self.status(Some(what.into()), None)
    }

    /// Make a change on `day`. Returns a message for the player if something was refused.
    fn change(&mut self, day: u64, change: Change, branch: bool) -> Option<String> {
        if branch {
            if let Err(e) = self.t.branch(day) {
                return Some(e);
            }
        } else if day < self.sim().day
            && let Err(e) = self.sim().rewind(day)
        {
            return Some(e);
        }
        let id = self.t.active_id();
        let skipped = self.sim().skipped_edits.len();
        let result = match change {
            Change::Poke(e) => {
                self.sim().poke(e);
                Ok(())
            }
            Change::Edit(e) => self.sim().edit(e),
            Change::Undo => {
                let last = self.sim().commands.len() - 1;
                if last == 0 {
                    return Some("nothing to take back".into());
                }
                self.busy("taking back the last change…");
                self.t.sim_mut().withdraw(last).map(|c| self.redo.entry(id).or_default().push(c))
            }
            Change::Redo => match self.redo.get_mut(&id).and_then(Vec::pop) {
                None => return Some("nothing to give back".into()),
                Some(c) => {
                    self.busy("giving the change back…");
                    self.sim().reinstate(c)
                }
            },
        };
        let all = &self.t.sim().skipped_edits;
        let left_out = &all[skipped.min(all.len())..];
        match result {
            Err(e) => Some(e),
            Ok(()) if !left_out.is_empty() => Some(format!("left out, no longer fitting: {}", left_out.iter().map(|s| s.1.clone()).collect::<Vec<_>>().join("; "))),
            Ok(()) => None,
        }
    }
}

fn run(t: Timelines, commands: Receiver<ToSim>, out: Sender<FromSim>, shown: Arc<AtomicU64>, wake: impl Fn()) {
    let map = Arc::new(MapState::of(0, t.active_id(), t.sim()));
    let mut r = Run { t, map, redo: HashMap::new(), out, opened: 0 };
    let mut epoch = 0;
    let mut state = r.state();
    if !r.status(None, None) {
        return;
    }
    loop {
        let ahead = r.t.sim().day >= shown.load(Ordering::Relaxed) + LOOKAHEAD;
        let message = if ahead {
            match commands.recv_timeout(Duration::from_millis(5)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match commands.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };
        // Changes, jumps and switches send the day they land on again, replacing the day on screen.
        let moved = match message {
            Some(ToSim::Change { epoch: e, day, change, branch }) => {
                if matches!(change, Change::Poke(_) | Change::Edit(_)) {
                    // A new change: what was taken back can't be given back any more.
                    let id = if branch { None } else { Some(r.t.active_id()) };
                    if let Some(id) = id {
                        r.redo.remove(&id);
                    }
                }
                let message = r.change(day, change, branch);
                Some((e, message))
            }
            Some(ToSim::GoTo { epoch: e, day }) => {
                let far = day.abs_diff(r.t.sim().day) > 3 * 364;
                if far && !r.busy("going to another day…") {
                    return;
                }
                Some((e, r.sim().goto(day).err()))
            }
            Some(ToSim::Switch { epoch: e, id, day }) => {
                let message = r.t.switch(id).and_then(|()| r.sim().goto(day)).err();
                Some((e, message))
            }
            Some(ToSim::Save { path, day }) => {
                if !r.busy("saving…") {
                    return;
                }
                // A copy on the day shown takes the played timeline's place while it is written.
                let mut shown = r.t.sim().fork();
                let moved = shown.goto(day);
                std::mem::swap(r.t.sim_mut(), &mut shown);
                let written = moved.and_then(|()| r.t.write(&path));
                std::mem::swap(r.t.sim_mut(), &mut shown);
                let said = match written {
                    Ok(()) => format!("saved {}", path.display()),
                    Err(e) => e,
                };
                if !r.status(None, Some(said)) {
                    return;
                }
                wake();
                continue;
            }
            Some(ToSim::Open { epoch: e, path }) => {
                if !r.busy("opening (a scenario replays every day it holds)…") {
                    return;
                }
                let message = match Timelines::open(&path) {
                    Err(e) => Some(e),
                    Ok(t) if t.sim().config != r.t.sim().config => {
                        Some(format!("{} is another world; start the app with --open to see it", path.display()))
                    }
                    Ok(t) => {
                        r.t = t;
                        r.redo.clear();
                        r.opened += 1;
                        None
                    }
                };
                Some((e, message))
            }
            Some(ToSim::Trace { day, province }) => {
                if r.out.send(FromSim::Trace(r.t.sim_mut().trace(day, province))).is_err() {
                    return;
                }
                wake();
                continue;
            }
            None if ahead => continue,
            None => None,
        };
        if let Some((e, message)) = moved {
            epoch = e;
            state = r.state();
            let sim = r.t.sim();
            let frame = Frame { replaces: Some(sim.event_log.entries.clone()), ..Frame::with(sim, epoch, state.clone()) };
            if r.out.send(FromSim::Frame(frame)).is_err() || !r.status(None, message) {
                return;
            }
            wake();
            continue;
        }
        let t = Instant::now();
        if r.sim().step_day() {
            state = r.state();
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let frame = Frame { ms, ..Frame::with(r.t.sim(), epoch, state.clone()) };
        if r.out.send(FromSim::Frame(frame)).is_err() {
            return;
        }
        wake();
    }
}

/// How fast days go by on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Speed {
    Paused,
    DaysPerSecond(f64),
    /// As fast as the simulation goes: always the newest day there is.
    Max,
}

/// The playback clock: where on the time line the screen is, in days. Pure, so it is tested
/// without a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// The moment shown: day `floor(pos)`, blended `fract(pos)` of the way to the next.
    pub pos: f64,
    pub speed: Speed,
    /// Pause on reaching this day (stepping, "run to year").
    pub stop_at: Option<u64>,
}

impl Clock {
    pub fn new(day: u64) -> Self {
        Clock { pos: day as f64, speed: Speed::Paused, stop_at: None }
    }

    pub fn day(&self) -> u64 {
        self.pos.floor() as u64
    }

    /// How far towards the next day the moment shown is, 0..1.
    pub fn blend(&self) -> f64 {
        self.pos.fract()
    }

    /// Move on by `dt` seconds, when days up to `last` have been computed. If the simulation is
    /// behind, the clock waits at `last`.
    pub fn advance(&mut self, dt: f64, last: u64) {
        let target = match self.speed {
            Speed::Paused => self.pos,
            Speed::DaysPerSecond(r) => self.pos + r * dt,
            Speed::Max => last as f64,
        };
        let mut pos = target.min(last as f64);
        if let Some(stop) = self.stop_at
            && pos >= stop as f64
        {
            pos = stop as f64;
            self.pause();
        }
        self.pos = pos.max(self.pos);
    }

    /// Stop on the day being shown.
    pub fn pause(&mut self) {
        self.speed = Speed::Paused;
        self.stop_at = None;
        self.pos = self.pos.floor();
    }

    /// Go as fast as possible to `day`, then pause there.
    pub fn run_to(&mut self, day: u64) {
        if day > self.day() {
            self.speed = Speed::Max;
            self.stop_at = Some(day);
        }
    }

    pub fn play(&mut self, speed: Speed) {
        self.speed = speed;
        self.stop_at = None;
    }
}

/// Measures how many days a second actually go by on screen.
#[derive(Default)]
pub struct Rate {
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    pub fn record(&mut self, now: Instant, day: u64) {
        self.samples.push_back((now, day));
        while self.samples.front().is_some_and(|&(t, _)| now - t > Duration::from_secs(1)) {
            self.samples.pop_front();
        }
    }

    pub fn days_per_second(&self) -> f64 {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, d0)), Some(&(t1, d1))) if t1 > t0 => (d1 - d0) as f64 / (t1 - t0).as_secs_f64(),
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_plays_waits_steps_and_runs_to_a_day() {
        let mut c = Clock::new(10);
        c.advance(1.0, 50);
        assert_eq!(c.pos, 10.0, "paused");

        c.play(Speed::DaysPerSecond(7.0));
        c.advance(0.5, 50);
        assert_eq!((c.day(), c.blend()), (13, 0.5));
        // The simulation is behind: the clock waits for it, and never goes back.
        c.advance(10.0, 20);
        assert_eq!(c.pos, 20.0);
        c.advance(0.1, 18);
        assert_eq!(c.pos, 20.0);

        // Max shows the newest day there is.
        c.play(Speed::Max);
        c.advance(0.01, 80);
        assert_eq!(c.day(), 80);

        // Pausing drops the blend; stepping a week stops after seven days, waiting if need be.
        c.play(Speed::DaysPerSecond(1.0));
        c.advance(0.4, 100);
        c.pause();
        assert_eq!(c.pos, 80.0);
        c.run_to(87);
        c.advance(0.01, 84);
        assert_eq!((c.day(), c.speed), (84, Speed::Max));
        c.advance(0.01, 200);
        assert_eq!((c.pos, c.speed, c.stop_at), (87.0, Speed::Paused, None));
        // Running to a day already passed does nothing.
        c.run_to(50);
        assert_eq!(c.speed, Speed::Paused);
    }
}
