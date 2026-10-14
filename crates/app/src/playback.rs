//! Playback: the simulation runs on its own thread, a few months ahead of what is shown, and sends
//! one frame per simulated day. The window plays the frames back at the chosen speed and blends the
//! two days either side of the moment shown. The simulation never knows whether anyone watches.
//!
//! A poke throws the days computed ahead away: the worker goes back to the day on screen
//! ([`Sim::rewind`]) and the future is computed again with the poke in it. Explaining a day
//! ([`Sim::trace`]) runs on the worker too, which then carries on exactly where it was.

use crate::layers::ProvinceView;
use sim::events::{Event, Posted};
use sim::fields::{FieldData, FieldId};
use sim::requests::Request;
use sim::sim::{Sim, Trace};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How many days the simulation may run ahead of the day shown. Enough to cover a heavy day at a
/// year a second; a poke only costs going back, whatever this is.
pub const LOOKAHEAD: u64 = 120;

/// Everything shown for one day that can change while the world runs. Shared between the frames
/// of days on which nothing ran.
pub struct DayState {
    pub fields: Vec<(FieldId, FieldData)>,
    pub last_change: Vec<Vec<u64>>,
    pub view: ProvinceView,
}

impl DayState {
    pub fn of(sim: &Sim) -> Self {
        DayState {
            fields: sim.running_fields().map(|id| (id, sim.world.field(id))).collect(),
            last_change: sim.last_change.clone(),
            view: ProvinceView::now(sim),
        }
    }
}

/// The world at the start of `day`, and what happened on the day before.
#[derive(Clone)]
pub struct Frame {
    pub day: u64,
    /// Frames from before the latest poke are stale.
    pub epoch: u64,
    pub state: Arc<DayState>,
    /// Events delivered on the day before (grants excluded).
    pub delivered: Vec<Posted>,
    /// Change requests applied on the day before, with what was granted.
    pub applied: Vec<(Request, u64)>,
    /// How long the simulation took for the day before, ms.
    pub ms: f64,
}

impl Frame {
    /// The world as it stands, as frame 0 of `epoch`.
    pub fn now(sim: &Sim, epoch: u64) -> Self {
        Frame { day: sim.day, epoch, state: Arc::new(DayState::of(sim)), delivered: sim.delivered_today().to_vec(), applied: sim.applied_today.clone(), ms: 0.0 }
    }
}

enum ToSim {
    Poke { epoch: u64, day: u64, event: Event },
    Trace { day: u64, province: u32 },
}

pub enum FromSim {
    Frame(Frame),
    Trace(Result<Trace, String>),
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
        let (tx, commands) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let shown = Arc::new(AtomicU64::new(sim.day));
        let watching = shown.clone();
        let thread = std::thread::Builder::new()
            .name("simulation".into())
            .spawn(move || run(sim, commands, out, watching, wake))
            .expect("start the simulation thread");
        Worker { tx: Some(tx), rx, shown, thread: Some(thread) }
    }

    pub fn show(&self, day: u64) {
        self.shown.store(day, Ordering::Relaxed);
    }

    /// Poke the world at the start of `day` (the day shown); frames from before have older epochs.
    pub fn poke(&self, epoch: u64, day: u64, event: Event) {
        self.send(ToSim::Poke { epoch, day, event });
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

fn run(mut sim: Sim, commands: Receiver<ToSim>, out: Sender<FromSim>, shown: Arc<AtomicU64>, wake: impl Fn()) {
    let mut epoch = 0;
    let mut state = Arc::new(DayState::of(&sim));
    loop {
        let ahead = sim.day >= shown.load(Ordering::Relaxed) + LOOKAHEAD;
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
        match message {
            Some(ToSim::Poke { epoch: e, day, event }) => {
                if day < sim.day {
                    sim.rewind(day).expect("the worker keeps every year since the world began");
                }
                epoch = e;
                sim.poke(event);
                state = Arc::new(DayState::of(&sim));
                continue;
            }
            Some(ToSim::Trace { day, province }) => {
                if out.send(FromSim::Trace(sim.trace(day, province))).is_err() {
                    return;
                }
                wake();
                continue;
            }
            None if ahead => continue,
            None => {}
        }
        let t = Instant::now();
        if sim.step_day() {
            state = Arc::new(DayState::of(&sim));
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let frame = Frame { day: sim.day, epoch, state: state.clone(), delivered: sim.delivered_today().to_vec(), applied: sim.applied_today.clone(), ms };
        if out.send(FromSim::Frame(frame)).is_err() {
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
