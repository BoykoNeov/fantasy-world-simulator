# app: the map view

The window that shows the planet: egui + wgpu over the `sim` crate. This is build step 2 from the design
doc. The simulation code knows nothing about it; the map only reads the sim's fields.

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
- **Panel**: layer sliders (terrain, temperature, rainfall, population, famine, province colours), line
  toggles, a hover inspector, and buttons that run the simulation 1 or 10 more years.

Not yet: place-name labels (nothing has a name yet) and mipmaps (not needed at current zoom limits).
Province colours still follow cell edges while their borders are smoothed; this shows when zoomed in.

## Running

```
cargo run --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p app --bin fantasymap
```

Drag to pan, scroll to zoom. Options:

- `--size small|medium|large|huge`, `--seed S`, `--n FACE`, `--provinces N`: the world (default: large, seed 7).
- `--terrain null|noise`, `--climate null|latitude`: module versions (default: noise, latitude).
- `--years Y`: how long to run before showing it (default 20).
- `--projection oval|rect`.
- `--view LON,LAT,ZOOM`: start view, in degrees; zoom 1 means the planet's height fits.
- `--layer NAME=VALUE`, repeatable: `terrain`, `temperature`, `rainfall`, `population`, `famine`,
  `provinces` (0..1), and `coasts`, `rivers`, `borders`, `graticule` (0 or 1).
- `--screenshot FILE.png`: open, save a screenshot, close.
- `--bench --out DIR --tag NAME`: a scripted full turn of the world, then a zoom to 8×. It writes a
  screenshot and a JSON file of frame times. Set `WGPU_POWER_PREF=low` (integrated GPU) or `high` first.

Tests, including one that renders cell ids on the GPU and checks them against the CPU picking code
(it skips if there is no GPU):

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
- **8.4 ms is a cap, not the rectangle's real cost.** Something system-wide holds frames to about 120 a
  second (the renderer trial hit the same 8.4 ms floor on the RTX 5090). The rectangle reaches it.
- **The oval costs more than the rectangle**: each pixel's map position needs a few steps of Newton's
  method instead of a straight formula. Zooming costs more than panning because more line segments
  are on screen.
- **The RTX 5090 numbers are not informative.** That day it showed a flat 15.6 ms (about 64 frames a
  second) for this app *and* for the renderer trial's program, so something capped it.
