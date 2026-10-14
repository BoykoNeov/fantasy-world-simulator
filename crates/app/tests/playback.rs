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
    // The day shown comes again first, then the new future.
    assert_eq!(landed(&worker, 1).day, day);
    let after = frame(&worker);
    assert_eq!((after.day, after.epoch), (day + 1, 1));
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

/// The frame that answers request `epoch`: the day it landed on, sent again.
fn landed(w: &Worker, epoch: u64) -> Frame {
    loop {
        if let FromSim::Frame(f) = next(w)
            && f.epoch == epoch
            && f.replaces.is_some()
        {
            return f;
        }
    }
}

/// Fields equal bit for bit (the "nothing painted" NaNs included).
fn same(a: &[(sim::fields::FieldId, sim::fields::FieldData)], b: &[(sim::fields::FieldId, sim::fields::FieldData)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.0 == y.0 && x.1.len() == y.1.len() && x.1.differences(&y.1).is_empty())
}

/// The next status report.
fn status(w: &Worker) -> app::playback::Status {
    loop {
        if let FromSim::Status(s) = next(w) {
            return s;
        }
    }
}

#[test]
fn an_edit_shows_on_the_day_shown_with_its_map() {
    use app::playback::Change;
    use sim::edit::Edit;
    let mut sim = world();
    sim.run_days(200);
    let start = sim.day;
    let p = (0..sim.world.provinces.count).max_by_key(|&i| sim.world.provinces.cells[i]).unwrap() as u32;
    let cells: Vec<u32> = (0..sim.world.grid.cells() as u32).filter(|&c| sim.world.provinces.of_cell[c as usize] == p).collect();
    let count = sim.world.provinces.count;
    let worker = Worker::start(sim, || {});
    let _ = status(&worker);
    let first = frame(&worker);
    let old_map = first.state.map.clone();
    for _ in 0..50 {
        frame(&worker);
    }

    // Split a province on the day shown: that day comes back with the new map.
    worker.show(start + 10);
    worker.change(1, start + 10, Change::Edit(Edit::Split { cells: cells[..cells.len() / 2].to_vec() }), false);
    let f = landed(&worker, 1);
    assert_eq!(f.day, start + 10);
    assert_ne!(f.state.map.key, old_map.key);
    let provinces = f.state.map.fields.iter().find(|(id, _)| id.def().name == "province.area_km2").unwrap();
    assert_eq!(provinces.1.len(), count + 1);
    assert_eq!(f.state.view.people.len(), count + 1);
    let s = status(&worker);
    assert!(s.can_undo && !s.can_redo && s.message.is_none(), "{s:?}");
    // The next days share the new map.
    let n = frame(&worker);
    assert!(std::sync::Arc::ptr_eq(&n.state.map, &f.state.map));

    // Undo gives the map before; redo gives it back.
    worker.change(2, start + 10, Change::Undo, false);
    let u = landed(&worker, 2);
    assert_eq!(u.state.view.people.len(), count);
    assert!(same(&u.state.map.fields, &old_map.fields), "the map before the split");
    let s = loop {
        let s = status(&worker);
        if s.busy.is_none() {
            break s;
        }
    };
    assert!(s.can_redo && !s.can_undo, "{s:?}");
    worker.change(3, start + 10, Change::Redo, false);
    let r = landed(&worker, 3);
    assert!(same(&r.state.map.fields, &f.state.map.fields), "the map after the split");
    assert_eq!(r.state.fields, f.state.fields);
}

#[test]
fn a_change_in_the_past_starts_a_timeline_and_going_back_and_forth_repeats_days() {
    use app::playback::Change;
    use sim::edit::Edit;
    let mut sim = world();
    sim.run_days(100);
    let start = sim.day;
    let worker = Worker::start(sim, || {});
    let _ = status(&worker);
    let mut f = frame(&worker);
    while f.day < start + 100 {
        f = frame(&worker);
    }
    let later = f;

    // Go back 60 days and forward again: the same day.
    worker.show(start + 40);
    worker.goto(1, start + 40);
    assert_eq!(landed(&worker, 1).day, start + 40);
    let _ = status(&worker);
    worker.goto(2, start + 100);
    let again = landed(&worker, 2);
    assert_eq!(again.state.fields, later.state.fields);
    let _ = status(&worker);

    // A change at a past day starts timeline 1; timeline 0 keeps its log.
    worker.show(start + 40);
    worker.change(3, start + 40, Change::Edit(Edit::SetFoodStock { province: 0, rations: 1 }), true);
    let b = landed(&worker, 3);
    assert_eq!(b.day, start + 40);
    let s = status(&worker);
    assert_eq!((s.active, s.timelines.len()), (1, 2), "{s:?}");
    assert_eq!(s.timelines[0].commands, 0);
    assert_eq!(s.timelines[1].commands, 1);
    assert_eq!(s.timelines[1].parent, Some((0, start + 40)));

    // Back to timeline 0 at the day it showed: no edit there.
    worker.switch(4, 0, start + 100);
    let back = landed(&worker, 4);
    assert_eq!(back.state.fields, later.state.fields);
    assert_eq!(status(&worker).active, 0);
}
