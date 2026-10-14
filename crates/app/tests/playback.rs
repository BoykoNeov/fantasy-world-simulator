//! The simulation thread, without a window: it runs ahead of the day shown, a poke throws the days
//! ahead away and gives the same future as poking a fresh world, and explaining a day leaves it
//! where it was.

use app::playback::{Frame, FromSim, Worker, LOOKAHEAD};
use sim::config::Config;
use sim::events::Event;
use sim::sim::Sim;
use sim::world::WorldParams;
use std::time::Duration;

fn world() -> Sim {
    Sim::new(&Config::new(WorldParams { n: 24, provinces: 40, seed: 9, land_fraction: 0.4, radius_km: 6371.0 })).unwrap()
}

fn next(w: &Worker) -> FromSim {
    w.rx.recv_timeout(Duration::from_secs(20)).expect("the worker answers")
}

fn frame(w: &Worker) -> Frame {
    loop {
        if let FromSim::Frame(f) = next(w) {
            return f;
        }
    }
}

#[test]
fn the_worker_runs_ahead_rewinds_for_a_poke_and_explains_days() {
    let mut sim = world();
    sim.run_to_year(2);
    let start = sim.day;
    let worker = Worker::start(sim, || {});

    // It runs ahead of the day shown, and no further.
    let frames: Vec<Frame> = (0..LOOKAHEAD).map(|_| frame(&worker)).collect();
    assert_eq!(frames.iter().map(|f| f.day).collect::<Vec<_>>(), (start + 1..=start + LOOKAHEAD).collect::<Vec<_>>());
    assert!(worker.rx.recv_timeout(Duration::from_millis(200)).is_err(), "ran past the lookahead");

    // Show day start + 30 and poke there: the next frames are the new future.
    let day = start + 30;
    worker.show(day);
    let d = Event::Drought { lon: 0.0, lat: 20.0, radius_km: 4000.0, shortfall: 900, days: 400 };
    worker.poke(1, day, d.clone());
    let mut after = frame(&worker);
    while after.epoch != 1 {
        after = frame(&worker);
    }
    assert_eq!(after.day, day + 1);
    let mut later = after;
    while later.day < day + 40 {
        later = frame(&worker);
        assert_eq!(later.epoch, 1);
    }

    let mut fresh = world();
    fresh.run_to_year(2);
    fresh.run_days(30);
    fresh.poke(d);
    fresh.run_days(40);
    let want = Frame::now(&fresh, 1);
    assert_eq!(later.state.fields, want.state.fields, "the future after a poke differs from a fresh world's");
    assert_eq!(later.state.view, want.state.view);
    assert!(later.state.view.drought.iter().any(|&s| s > 0.5), "the drought should show");

    // Explaining a past day answers, and the worker carries on where it was.
    worker.trace(day + 1, 0);
    let traced = loop {
        if let FromSim::Trace(t) = next(&worker) {
            break t;
        }
    };
    assert_eq!(traced.unwrap().day, day + 1);
    // Waiting for the answer skipped the frames sent before it; move the day shown on.
    worker.show(day + 100);
    let mut f = frame(&worker);
    while f.day < day + 150 {
        f = frame(&worker);
    }
    fresh.run_days(day + 150 - fresh.day);
    assert_eq!(f.state.fields, Frame::now(&fresh, 1).state.fields);
}
