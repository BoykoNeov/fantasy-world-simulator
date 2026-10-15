# sim: the simulation core

Plain Rust with no graphics or engine dependency. It runs headless, for tests and batch runs. Build
steps 1 (a thin slice you can watch), 3 (the module contract) and the simulation half of 4 (pokes,
going back to an earlier day, and explaining a day) from the design doc.

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
  | Weather | `droughts`: owns each province's rain shortfall; droughts start with a poke; or off | monthly |

  Harvests shrink in proportion to the missing rain, so a drought cuts the crop.
- **Pokes** (`Sim::poke`): a player's action is an ordinary event, stored in the command log with its
  day. It is sent that day and arrives the next, so replays and saves repeat it. The only poke so
  far is a drought: a centre, a radius, how much rain goes missing, and for how long.
- **Edits** (`edit.rs`, `Sim::edit`): an edit sets values directly instead of letting the world
  react (so far: a pop's size and what it is, a new pop, a province's food store, and reshaping
  provinces: moving land between them, splitting one, merging two). Edits go into the
  command log like pokes and take effect at the start of their day, so the world shown for that day
  already has them. People and food an edit creates or removes are counted as the edit's own cause,
  so every year still balances. `Sim::withdraw` takes a command (an edit or a poke) back:
  the world goes back to the last checkpoint before it and runs again to the same day without it,
  giving exactly the world that never had it. `Sim::reinstate` gives it again. An edit that no
  longer fits when its day comes (it changed a pop that a withdrawn edit had added) is left out and
  listed in `Sim::skipped_edits`.
- **Reshaping provinces** (`edit.rs`): when land changes province, its people and food go with it,
  in proportion to the farmland that moved (by area where there is no farmland), in whole people
  and rations by the largest-remainder rule; people join a pop of their own kind there or make one.
  Province numbers never change: a province merged away stays as an empty placeholder that keeps
  its centre, and a split's new province is numbered after the last. Pops are only ever added, so
  food on its way to a pop still finds it.
- **Painting the land** (`edit.rs`): a brush stroke raises, lowers, smooths, roughens or flattens
  the land along a path (full strength in the brush's middle half, fading to nothing at its edge),
  and the sea can rise or fall everywhere. Flattening below 0 m makes sea, so an inland sea or a
  lake at sea level is painted this way. Afterwards climate, biomes, rivers and farmland are worked
  out again by the world's own module versions; provinces are not regrown. Land that sinks leaves
  its province; new land joins the province next to it, spreading from the old coast; a new island
  that touches no province becomes one. A province whose land all sinks sends its people and food to
  the nearest province with land. Only the downstream links of the rivers are stored now, not the
  order water is added up in (format 4).
- **Painting climate and biomes** (`edit.rs`): an override stroke sets the temperature or the rain
  (blended in at the brush's soft edge) or a land biome. They are stored as map layers the climate
  and biome modules keep over what they work out, through later height edits too, until a stroke
  clears them.
- **Drawing rivers** (`edit.rs`): a river drawn along a path is a riverbed carved from source to
  mouth, falling all the way and lying below its banks, so the water follows it. Starting one on a
  river redirects that river. A spring at the source can add water, so a river can be drawn where
  little rain falls. Lakes are painted with the height brush, below sea level: there are no lakes
  above it yet.
- **Timelines** (`branch.rs`): `Timelines::branch` starts a new timeline from a past day of the one
  being played, and the old one is kept exactly as it was; `Timelines::switch` plays another. A new
  timeline keeps the old one's commands from before that day and the edits logged for it (the day
  as shown already has them), but not that day's pokes nor anything later. Each timeline is a whole
  simulation, so switching is instant; a new one copies the world and shares the checkpoints and
  map changes they have in common, so each costs about one world's worth of memory.
  `Timelines::save` writes every timeline, each as its own save.
- **Scenario files** (`scenario.rs`): every timeline as readable TOML that can be edited by hand:
  the world's settings, module versions and parameters, calendar, and each timeline's commands
  (each a day with an `edit` or a `poke`, including pokes scheduled for days still to come with
  `Sim::schedule`) and the day it reached. Opening one makes the world from its settings and
  replays the commands, giving exactly the world that was written; that takes as long as running
  those days did. A timeline can carry expectations: a range for one of a year's totals (people,
  food stored, harvested, births, deaths, starved, provinces with famine). Checking them runs the
  timeline on to the last year they name. `scenarios/drought_famine.toml` is story 1's first half
  this way: the same world with and without the drought, and what each should show.
- **The combination tester** (`combos.rs`, `src/bin/combos.rs`): runs mixes of module versions on
  small worlds (face 48, 200 provinces) for 20 years on three seeds, strictly (a module writing a
  value it doesn't own stops its run), each with one drought so the weather has work to do. It also
  checks every scenario in `scenarios/`. The scorer looks at world totals, not single values (a
  province's store or a quiet year's missing rain sits at zero as a matter of course): a crash, a
  value clamped back into bounds, a total its causes don't explain, a value held at a finite upper
  bound for 180 days, or a counted total at zero for 90 days while its owner runs are red; no
  counted total changing for five years is yellow. `data/known_quirks.toml` lists problems a mix
  is known to have, which then show yellow. A mix the engine refuses (people with no food economy)
  is marked refused. Only 32 mixes exist, so all of them run; `--pairs` picks a smaller set that
  still has every pair of versions together, for when the slots grow. The table goes to
  `data/compatibility.toml`, which the window reads. Verdicts depend only on the mix and the seed,
  so the table is the same on every run; speed is printed, not kept in it. Today every mix that
  runs is green.
- **Undo cost**: taking back the latest edit replays at most a year. Taking back an older map edit
  also works out every later map edit again (about 0.2 s per brush stroke on a large world), and
  taking back an old poke replays every year since.
- **Going back** (`sim.rs`): at the start of every year the engine keeps a checkpoint of everything
  that can change once the world runs. `Sim::rewind` returns to the start of any earlier day by
  restoring the checkpoint before it and running forward; the days after are forgotten, so a poke
  made then starts a new future. The map (fields written at generation) is not in checkpoints: only
  edits change it, and each edit's change to the map is kept as the values before and after where
  they differ. Going back undoes those changes, and a replay that meets the same edit makes its
  change again instead of repairing the map from scratch.
- **Why a value changed** (`Sim::trace`): runs a past day again with tracing on for one province,
  then puts the world back exactly. The trace lists, in order, each module's own explanation (the
  drought reaching the province, the harvest worked out from capacity, the year's luck, the month
  and the rain, spoilage, how much food each pop got and what followed), the change requests made
  and granted, events sent and delivered, and every value that changed. Tracing is off unless asked
  for; modules check one number before explaining anything.
- **Last changed**: for every value a running module owns, the engine stamps the day it last
  changed (`Sim::last_change`).
- **Story 1, first half** (`tests/stories.rs`): a three-year drought over the province that grows the
  most food. In that test world, harvests fall to about 30% of normal, the region's stores fall to
  18% of normal, and 22% of its people starve in the worst year (hardly anyone without the drought).
  The famine ends once the rain returns. Every province outside the drought ends exactly as it would
  have without it.

- **PNG export** (`export.rs`): terrain, provinces, population, famine, temperature and rainfall maps,
  in the equirectangular projection (for convenience only).

Provinces grow in all eight directions over land whose roughness varies smoothly from place to
place, and big rivers and ridges are costly to cross. Borders therefore meander and often follow
rivers and mountains, instead of running straight along the grid. `examples/province_stats.rs`
measures the result (large world, seed 7: farmland spread 0.80 against area 0.99; the earlier
four-way growth gave 0.72 against 0.89). This also changed every seed's provinces.

Splitting food from people in step 3 changed how a given seed plays out: starting stores are now
counted in days of harvest, people learn their share a day after asking, and harvests use their own
random-number ID. Results for the same seed differ from step 2's.

## Running

Tests (a few seconds; the story test runs a world twice for four years):

```
cargo test --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p sim
```

The combination tester (about 25 s on all cores; run it at below-normal priority). `--write` saves
the table, `--check` runs again what the table lists and fails if anything came out differently,
`--only economics=off,population=off` runs one mix, and `--out DIR` writes, for each run that
wasn't green, a scenario that replays it up to just after its first problem:

```
cargo run --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p sim --bin combos -- --check
```

Headless run with timing and maps (sizes: small, medium, large, huge):

```
cargo run --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p sim --bin simrun -- --size large --years 100 --out W:\temp\claude\fantasymap-maps
```

Other options are `--seed`, `--n` (cube face size), `--provinces`, `--width` (map width in pixels),
`--terrain null|noise`, `--climate null|latitude`, `--save FILE`, `--load FILE` (continue a saved
world for `--years` more years), `--scenario FILE` (open a scenario's played timeline and continue it)
and `--write-scenario FILE`.

## Measured speed (Oct 14, 2026, Ryzen 7800X3D, release build)

Large world (face 408, about 1M cells, 5,092 provinces), seed 7, 100 years:

| | Time | Design target |
| --- | --- | --- |
| World generation | 0.7 s (0.4 s before provinces grew in eight directions) | — |
| Average simulated day | 0.12 ms | ~1.5 ms |
| A day when something ran (month starts, the day after, year starts) | 1.1 ms | — |
| An idle day | 0.00004 ms | well under 0.1 ms |
| A century, headless | 4.4 s | under a minute |
| Save / load | 0.10 s / 0.10 s, 32 MB | — |

World generation was re-timed after the province change while another project's tests kept the
machine fully busy: 0.7 s at best, 1.9 and 5.0 s in busier runs. The province step itself took about
0.3 s. The century figure was measured before that change, which only affects generation; under the
same load a century took 5.6–12 s. Worth re-measuring on a quiet machine.

After step 4 (weather, pokes, checkpoints, last-changed stamps), 20 years on the small and large
worlds give the same yearly results as before, and an idle day still costs 0.00006 ms. Days when
something ran took 2.1 ms on the large world (3.4 ms just before the change, same session): the
machine was busy, so these are not comparable with the 1.1 ms above.

**Going back** (Oct 14, below-normal priority, seed 7). The worst case is a day near a year's end,
since the replay starts from that year's first day:

| | Large (5,092 provinces) | Huge (20,066 provinces) |
| --- | --- | --- |
| Explain (trace) a year's last day | 47–49 ms | 164–170 ms |
| Rewind to a year's last day | 46 ms | 242 ms |
| One yearly checkpoint | 0.58 MB (0.22 values + 0.36 last-changed stamps) | 2.3 MB |
| Checkpoints kept for a century / a millennium | 58 MB / 0.58 GB | 0.23 GB / 2.3 GB |

That is fast enough that checkpoints more often than yearly aren't needed. Memory is the cost:
checkpoints are never dropped, so a 1,000-year run on a huge world holds about 2.3 GB of them.
Most of it is the last-changed stamps (8 bytes for every province and pop value). Not trimmed yet;
storing stamps as 4-byte day numbers, or keeping only every tenth year beyond the last few, would
cut it when long runs need it.

**Edits** (Oct 14, below-normal priority, large world, seed 7; `examples/edit_timing.rs`):

| Edit | Time | Kept for going back |
| --- | --- | --- |
| Brush stroke, 20 points, 150 km brush | 178 ms | 0.03 MB |
| The sea rises 50 m (every height changes) | 174 ms | 43 MB |
| Split a province | 26 ms | 0.4 MB |
| Take the split back (replays 100 days, reusing the stroke's and the sea's changes) | 28 ms | — |
| Go back 250 days past all three / run them again | 11 ms / 49–115 ms | — |

Going back puts each map change's values back in place, so its cost follows the cells that
changed (a first version copied whole layers: 226 ms to take the split back). Painting while paused,
as the window does it (go back to the day shown, take the stroke, run 120 days ahead), costs the
same at the 200th stroke of a day as at the 50th: a stroke 168–174 ms, taking it back 35–38 ms,
explaining the day 51–56 ms. The 200 strokes kept 1.9 MB of map changes.

Step 1 measured 2.5 s a century. The rest goes to the request and event passes and the checks
after every stage. There's still very little simulation, so these figures are a floor, not a
forecast.
