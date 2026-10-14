# sim: the simulation core

Plain Rust with no graphics or engine dependency. It runs headless, for tests and batch runs. This is
build step 1 from the design doc: a thin slice you can watch.

What exists:

- **Planet grid** (`grid.rs`): a cube sphere. Each cell has 4 edge neighbours and 8 diagonal ones; the
  24 cells at the 8 cube corners have only 7 diagonals. Cells also store their area, latitude and
  longitude, and local east/north directions.
- **Randomness** (`rng.rs`): every random number is a hash of (seed, module, day, entity, purpose).
  There is no shared random stream.
- **Calendar** (`data/calendar.ron`, `calendar.rs`): stored as data. The default has 6-day weeks,
  twelve 30-day months, a 4-day Yearsend, four seasons and two moons.
- **World generation** (`world.rs`): noise terrain → climate from latitude, elevation and distance to
  the sea → biomes → rivers → provinces.
- **Pops** (`pops.rs`): people per province who eat, store food, grow when fed and starve when not.
  People and food are whole numbers, and every month balances exactly. Harvests are seasonal away from
  the equator, and regions have good and bad years.
- **Scheduler** (`sim.rs`): a daily clock, with each system running on its own calendar cadence.
- **PNG export** (`export.rs`): terrain, provinces, population, famine, temperature and rainfall maps.
  They use the equirectangular projection for convenience only; the map view's projection is still an
  open question in the design doc.

## Running

Tests (they take under a second):

```
cargo test --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p sim
```

Headless run with timing and maps (sizes: small, medium, large, huge):

```
cargo run --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p sim --bin simrun -- --size large --years 100 --out W:\temp\claude\fantasymap-maps
```

Other options are `--seed`, `--n` (cube face size), `--provinces`, `--width` (map width in pixels).

## Measured speed (Oct 14, 2026, Ryzen 7800X3D, release build)

Large world (face 408, about 1M cells, 5,092 provinces), seed 7, 100 years:

| | Time | Design target |
| --- | --- | --- |
| World generation | 0.5 s | — |
| Average simulated day | 0.07 ms | ~1.5 ms |
| A day with the monthly pops pass | 1.9 ms | — |
| An idle day | 0.00004 ms | well under 0.1 ms |
| A century, headless | 2.5 s | under a minute |

There's very little simulation yet, so these figures are a floor, not a forecast.
