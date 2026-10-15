# app: the map view

The window that shows the planet: egui + wgpu over the `sim` crate. Build steps 2 (the map view), 4
(time controls and the inspector) and 5 (the scenario editor) from the design doc. The simulation code knows nothing about it; the
map only reads the sim's fields.

What exists:

- **Two map shapes, switchable** (`projection.rs`, mirrored in `map.wgsl`):
  - **Oval world map** (Equal Earth, the default). It keeps true areas, so big lands look as big as they
    are. Dragging sideways turns the world under the map, and the point you grab stays under the cursor.
  - **Rectangle** (equirectangular). It repeats sideways like tiles and stretches the poles.
- **Map layers** (`layers.rs`). Per-cell data is held in 6-layer textures, one layer per cube face. Each
  face has a one-cell border copied from its neighbouring faces, so the seams don't show. Per-province
  values are small tables; "+1 year" re-uploads only those.
- **Lines** (`geometry.rs`): coastlines, province borders (dotted, fading in as you zoom), rivers, and a
  30° latitude/longitude grid. Borders are traced along cell sides and smoothed once per shared border,
  with the points where three regions meet held fixed, so neighbours never get gaps or double lines.
- **Parchment look**: paper tint, grain, a vignette, and a double ink rule around the map's edge.
- **Panel**: layer sliders (terrain, temperature, rainfall, population, famine in the past year,
  drought, province colours), line toggles, and a hover inspector.
- **Playback** (`playback.rs`): the simulation runs on its own thread, up to 120 days ahead of the day
  shown, and sends one frame per day. Days on which nothing ran share their data with the day before.
  The time bar plays at a day, a week, a month or a year a second, or as fast as the simulation goes;
  it steps a day or a week, and runs to a chosen year. Space plays and pauses; the right arrow key
  steps a day. Province colours blend between the two days either side of the moment shown. The bar
  shows how many days a second actually go by, how far ahead the simulation is, and its cost per day.
- **Inspector** (click a province; `inspector.rs`): every value of the province and its pops, with the
  module that sets it and the day it last changed; the change requests settled the day before; and
  "why?", which has the simulation run that day again with tracing on, then lists in order what each
  module did and why (the drought reaching the province, the harvest worked out from capacity, luck,
  month and rain, spoilage, the food each pop got, births and deaths) and every value that changed.
  Below the values, its neighbours: each province it borders or faces across the water, the days
  it takes to get there by land and by sea, and how long their shared border is. Click one to go
  to it.
- **Event log**: what happened to the province each day (famines, food shortages, droughts), newest
  first, filterable by source.
- **Drought tool**: start a drought centred on the selected province (how much rain is missing, radius,
  years). It takes effect the next day; the days computed ahead are thrown away, and the simulation
  goes back to the day shown and computes them again.

- **Smooth province colours** (`province_at` in `map.wgsl`, mirrored by `Camera::pick_province`): of
  the four cells around a point, the province with the largest share of the blend wins. Colour edges
  then run through the middle of each cell side, as the border lines do, instead of stepping along
  cell edges, and the hover inspector names exactly the province drawn under the pointer (checked on
  the GPU, pixel by pixel). The lookup is skipped when no province layer is on.
- **Hand-drawn wobble on province borders** (`wobble.rs`): one smooth field, about 4 cells in
  wavelength and under half a cell in reach, moves where province colours are looked up; each border
  line is moved to match, and picking uses the same field. It fades out next to the sea and along big
  rivers, so borders meet the coast exactly and river borders stay on their river.

- **Editing** (`editor.rs`, the Edit section): tools for the land's height (raise, lower, smooth,
  roughen, flatten; lakes are flattened below 0 m) and the sea level; painted climate (temperature,
  rainfall, a biome, or clearing them); rivers drawn from source to mouth (starting on a river
  redirects it; a spring adds water); and provinces (move land into the selected province, make
  it a new province, merge a clicked province into the selected one). Brush tools draw with the left
  button, and right-drag still moves the map; the brush and the stroke are drawn over the map. The
  inspector sets a pop's size, adds a pop and sets the store of any good kept (picked from a list,
  in the good's own unit); it offers only what some module keeps. An edit shows at once on the day
  shown. Undo and redo (Ctrl+Z, Ctrl+Y) take back edits and droughts, on the latest day watched.
- **A changed map is drawn again**: every day the simulation sends says which map it belongs to.
  When the day shown has a new map, its picture is built on another thread (0.5–0.8 s on a large
  world, about 1.2 s on a huge one: `examples/map_rebuild.rs`) while the old one stays up, then the
  picture and the picking swap in the same frame.
- **History and timelines**: the history slider goes to any day from the earliest kept to the
  latest watched, and back. A change made on an earlier day starts a new timeline and keeps the old
  one; the Timelines section plays either, from where it was left. Slow work (taking back an old
  change, opening a scenario, a long jump) shows what is under way and for how long.
- **File**: save every timeline, as a readable scenario if the name ends in `.toml` or else as a
  save that opens at once; open the same world again. The timeline played is saved on the day
  shown.

- **Modules** (step 6): the module versions running, and what the combination tester found for
  that mix (`crates/sim/data/compatibility.toml`): no problems found, known quirks, or unstable,
  with each finding listed. A mix the table doesn't hold gets the worst verdict of its pairs. The
  verdict comes from small test worlds (200 provinces, 20 years, default settings), whatever world
  is shown, so green means "no problems found", not "proven safe".

Not yet: place-name labels (nothing has a name yet) and mipmaps (not needed at current zoom limits).

## Running

```
cargo run --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p app --bin fantasymap
```

Drag to pan, scroll to zoom, click a province to inspect it. Options:

- `--size small|medium|large|huge`, `--seed S`, `--n FACE`, `--provinces N`: the world (default: large, seed 7).
- `--terrain null|noise`, `--climate null|latitude`: module versions (default: noise, latitude).
- `--modules SLOT=VERSION,...`: any module versions, e.g. `economics=off,population=off` or
  `weather=off` (slots: terrain, climate, economics, population, weather).
- `--years Y`: how long to run before showing it (default 20).
- `--drought best|LON,LAT,RADIUS_KM,PERCENT,YEARS`: start a drought after those years. `best` centres it
  on the province that grows the most food (2,000 km radius, 70% of the rain, 3 years).
- `--days D`: run D more days before showing it.
- `--select best|P`: open the inspector on a province; `--explain`: explain its last change.
- `--speed DAYS_PER_SECOND|max`: start playing.
- `--projection oval|rect`.
- `--view LON,LAT,ZOOM`: start view, in degrees; zoom 1 means the planet's height fits.
- `--layer NAME=VALUE`, repeatable: `terrain`, `temperature`, `rainfall`, `population`, `famine`,
  `drought`, `provinces` (0..1), and `coasts`, `rivers`, `borders`, `graticule` (0 or 1).
- `--open FILE`: show a saved world (a scenario `.toml`, or a save) instead of making one.
- `--paint LON,LAT,RADIUS_KM,METRES`, `--split best|P`, `--undo`: once the window is open, in the
  order given, raise land along a short stroke, split half a province off (and select the new one),
  or take back the last change, as a player would (for scripted screenshots; `--split best --undo`
  checks that the window copes with provinces going away).
- `--screenshot FILE.png`: open, save a screenshot (after any scripted edits and the redrawn map),
  close.
- `--bench --out DIR --tag NAME`: a scripted full turn of the world, then a zoom to 8×. It writes a
  screenshot and a JSON file of frame times. Set `WGPU_POWER_PREF=low` (integrated GPU) or `high` first.

Tests, including one that renders cell ids on the GPU and checks them against the CPU picking code
(it skips if there is no GPU), and one that drives the simulation thread without a window: it stays
within its lookahead, and a poke after it ran ahead gives exactly the future a fresh world gets; an
edit comes back on the day shown with its new map, undo and redo give the maps before and after,
a change in the past starts a timeline and leaves the old one as it was, going back and forth
gives the same days, and a world saved from the window opens again as it was:

```
cargo test --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p app
```

## Measured speed (Oct 14, 2026, the Ryzen 7800X3D's integrated Radeon, Vulkan)

Large world (about 1M cells, 5,092 provinces, about 315,000 line segments). Window 1920×1080 at 150%
scaling (2880×1620 physical pixels), vsync off, below-normal priority. Three rounds, each running the
renderer trial's egui program and then this app (oval, then rectangle), so all three saw the same
conditions. Milliseconds per frame, median (p50) of the scripted pan and zoom; lower is better. The raw
results are in `bench/results/`.

| | Pan | Zoom |
| --- | --- | --- |
| Renderer trial (egui), same session | 12.5–12.7 | 10.8–11.0 |
| Oval map | 9.2–9.7 | 12.0–12.2 |
| Rectangle | 8.4 | 8.5–8.8 |

- The integrated GPU, the design's hardware floor, draws the full map at 80–120 frames a second.
- **Re-checked after the smooth province colours (Oct 14, later):** old and new shader alternated,
  two rounds each, oval map. Default layers: pan 9.3–9.5 → 10.2 ms, zoom 12.9–13.0 → 12.1–12.3 ms.
  Province colours on: pan 10.4–10.5 → 10.4 ms, zoom 13.4–14.5 → 12.3–13.0 ms. The same, within
  noise. A first version that used small arrays and loops cost 3–4 ms more per frame on this GPU.
- **The border wobble** (same method): with province colours on, pan 9.1–9.6 → 9.9–10.3 ms and zoom
  11.6–11.7 → 12.0–12.7 ms, about 0.7 ms a frame; with no province layer on, nothing.
- Raw results of these two checks are in `bench/results/borders_*.json`: `fill_loops_*` is the first
  version against the one before it, `fill_*` the final smooth colours (`old`/`new`, `def` = default
  layers, `prov` = province colours on), `wobble_*` without (`nw`) and with (`w`) the wobble.
- **8.4 ms is a cap, not the rectangle's real cost.** Something system-wide holds frames to about 120 a
  second (the renderer trial hit the same 8.4 ms floor on the RTX 5090). The rectangle reaches it.
- **The oval costs more than the rectangle**: each pixel's map position needs a few steps of Newton's
  method instead of a straight formula. Zooming costs more than panning because more line segments
  are on screen.
- **The RTX 5090 numbers are not informative.** That day it showed a flat 15.6 ms (about 64 frames a
  second) for this app *and* for the renderer trial's program, so something capped it.
