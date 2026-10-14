# sim: the simulation core

Plain Rust with no graphics or engine dependency. It runs headless, for tests and batch runs. Build
steps 1 (a thin slice you can watch) and 3 (the module contract) from the design doc.

What exists:

- **Planet grid** (`grid.rs`): a cube sphere. Each cell has 4 edge neighbours and 8 diagonal ones; the
  24 cells at the 8 cube corners have only 7 diagonals. Cells also store their area, latitude and
  longitude, and local east/north directions.
- **Randomness** (`rng.rs`): every random number is a hash of (seed, module, day, entity, purpose).
  There is no shared random stream.
- **Calendar** (`data/calendar.ron`, `calendar.rs`): stored as data. The default has 6-day weeks,
  twelve 30-day months, a 4-day Yearsend, four seasons and two moons.
- **Shared fields** (`fields.rs`): every value modules may read, by name, with its tier, unit, bounds
  (the field contract), whether it is a conserved quantity, and what readers get if nobody owns it.
- **The module contract** (`module.rs`): each module fills a slot and declares a manifest: what it
  reads, owns and requests, its cadence, and the events it emits and handles. A configuration is
  checked once at world creation or load: no field has two owners, every request goes to an owner,
  everything read exists, and there are no cycles. The run order is derived from the manifests.
- **Change requests** (`requests.rs`): a module changes a value it doesn't own by asking its owner.
  Requests are applied at the end of the day; when they ask for more than exists, priority classes
  are served in order and the class that runs short shares by the largest-remainder rule. Grants
  reach the requester the next day.
- **Events** (`events.rs`): sent on day N, delivered on day N+1, in a documented priority order.
- **The engine** (`sim.rs`): the daily clock. After every stage it checks that people and food
  changed only by what was reported, and clamps values that leave their bounds; both kinds of
  problem are counted and sent out as events. In strict mode (tests) a module that writes a field
  it doesn't own stops the run.
- **Saves** (`save.rs`): configuration, module versions and state schemas, the command log, and a
  snapshot with every field by name and everything in flight. Loads refuse other format versions,
  other module versions and damaged files with a message.
- **Modules and their versions** (`config.rs` chooses them):

  | Slot | Versions | Runs |
  | --- | --- | --- |
  | Terrain | `noise` (default), `null`: smooth landmasses, same for every seed | at generation |
  | Climate | `latitude` (default), `null`: 15 °C and 800 mm everywhere | at generation |
  | Biomes, Rivers, Fertility, Provinces | one each | at generation |
  | Economics | `food`: monthly harvest and spoilage, owns the food stores; or off | monthly |
  | Population | `basic`: asks for a month's food, then is born or starves on the grant; or off | monthly |

- **PNG export** (`export.rs`): terrain, provinces, population, famine, temperature and rainfall maps,
  in the equirectangular projection (for convenience only).

Splitting food from people in step 3 changed how a given seed plays out: starting stores are now
counted in days of harvest, people learn their share a day after asking, and harvests use their own
random-number ID. Results for the same seed differ from step 2's.

## Running

Tests (they take under a second):

```
cargo test --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p sim
```

Headless run with timing and maps (sizes: small, medium, large, huge):

```
cargo run --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p sim --bin simrun -- --size large --years 100 --out W:\temp\claude\fantasymap-maps
```

Other options are `--seed`, `--n` (cube face size), `--provinces`, `--width` (map width in pixels),
`--terrain null|noise`, `--climate null|latitude`, `--save FILE`, and `--load FILE` (continue a saved
world for `--years` more years).

## Measured speed (Oct 14, 2026, Ryzen 7800X3D, release build)

Large world (face 408, about 1M cells, 5,092 provinces), seed 7, 100 years:

| | Time | Design target |
| --- | --- | --- |
| World generation | 0.4 s | — |
| Average simulated day | 0.12 ms | ~1.5 ms |
| A day when something ran (month starts, the day after, year starts) | 1.1 ms | — |
| An idle day | 0.00004 ms | well under 0.1 ms |
| A century, headless | 4.4 s | under a minute |
| Save / load | 0.10 s / 0.10 s, 32 MB | — |

Step 1 measured 2.5 s a century. The rest goes to the request and event passes and the checks
after every stage. There's still very little simulation, so these figures are a floor, not a
forecast.
