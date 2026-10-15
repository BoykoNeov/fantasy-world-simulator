//! Scenario files written by older builds still load, and make the same world they made then.
//!
//! Each file in `tests/old_scenarios/` was written by the build it is named after, together with a
//! `.expected` file that build printed: for each timeline, its day and every province's people,
//! starved this year, and store (and, with markets, price and workers) of every good kept.
//! Fingerprints can't be compared across builds, since fields added later are hashed too.
//!
//! - `step5_food`: commit bfcfad4, the end of step 5. Economics "food", before the goods tree; an
//!   edit of every kind then, two droughts and a second timeline.
//! - `markets_before_trade`: commit 7593428. Economics "markets" before trade: no trade table.
//! - `trade_before_money`: commit f27d284. Trade before prices followed the money: a trade table
//!   without `money_response` or `money_range`.

use sim::branch::Timelines;
use sim::sim::Sim;

fn line(name: &str, v: impl Iterator<Item = u64>) -> String {
    format!("{name} {}\n", v.map(|x| x.to_string()).collect::<Vec<_>>().join(" "))
}

/// What the old builds printed, worked out the same way.
fn dump(sim: &Sim) -> String {
    let w = &sim.world;
    let mut out = format!("day {}\n", sim.day);
    out += &line("people", w.province_population().into_iter());
    out += &line("starved", w.starved_this_year.iter().copied());
    let k = w.goods.stride();
    let count = w.stock.len() / k;
    // Economics "food" keeps no prices: the fields are there, all zero, as nobody writes them.
    let priced = w.price.iter().any(|&x| x > 0);
    for (c, name) in w.goods.names().iter().enumerate() {
        out += &line(&format!("stock {name}"), (0..count).map(|p| w.stock[p * k + c]));
        if priced {
            out += &line(&format!("price {name}"), (0..count).map(|p| w.price[p * k + c]));
            out += &line(&format!("workers {name}"), (0..count).map(|p| w.workers[p * k + c]));
        }
    }
    out
}

fn check(name: &str) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/old_scenarios/");
    let text = std::fs::read_to_string(format!("{dir}{name}.toml")).unwrap();
    let expected = std::fs::read_to_string(format!("{dir}{name}.expected")).unwrap();
    let t = Timelines::from_scenario_text(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut got = String::new();
    for tl in &t.list {
        got += &format!("timeline {}\n", tl.id);
        got += &dump(&tl.sim);
    }
    // Line by line, so a difference names what differs.
    for (g, e) in got.lines().zip(expected.lines()) {
        assert_eq!(g, e, "{name}");
    }
    assert_eq!(got.lines().count(), expected.lines().count(), "{name}");
}

#[test]
fn a_step_5_scenario_makes_the_same_world() {
    check("step5_food");
}

#[test]
fn a_markets_scenario_from_before_trade_makes_the_same_world() {
    check("markets_before_trade");
}

#[test]
fn a_trade_scenario_from_before_money_levels_makes_the_same_world() {
    check("trade_before_money");
}
