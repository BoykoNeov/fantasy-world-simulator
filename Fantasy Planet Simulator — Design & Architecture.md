# Fantasy Planet Simulator — Design & Architecture

Oct 13, 2026 · @Boy · revised Oct 14, 2026

**What changed in this revision (Oct 14):** the base time step is now one day. Values keep a single owner, and other modules change them through change requests. Determinism is narrowed to a promise that can be kept. The planet is a cube-sphere grid. Module versions are chosen at world creation or load, and a later save → convert → reload path is planned for changing them. A scenario editor that works mid-game is now a core feature. The build order starts with a thin playable slice and adds target stories. The open questions are mostly resolved (see Decisions).

## Vision and principles

A native-desktop, god/observer fantasy planet simulator. It shows a 2D, colorful, layered fantasy map. On it, a planet of ~1M cells develops its geology, climate, nature, peoples, economies, wars, magic, religions and cultures while you watch, poke, edit and fast-forward it.

Principles, in priority order:

1. **Modularity first.** Every domain is a module. Any module can be switched off, or set to a simpler or richer version, without breaking the rest of the world. For now this happens at world creation or load. Later it will also work mid-game through save → convert → reload.
2. **Performance.** ~1M cells and centuries of simulated time must run smoothly on a single desktop machine. That machine may have only an integrated GPU.
3. **Real-world principles, game-scale fidelity.** Economics, logistics, climate and geology follow real mechanisms, tuned for interesting outcomes rather than scientific accuracy. It is a game, not a research tool.
4. **Depth that grows.** Start simple, add complexity module by module, and keep the simple versions available forever.
5. **Beautiful map.** The map is the main interface. It has a hand-drawn fantasy feel, multiple toggleable layers, and stays smooth at every zoom.
6. **Everything is editable.** Any aspect of the world (map, peoples, realms, characters, module settings) can be edited, before a game or mid-game, through the same logged mechanism that makes replay work.

On intellectual property: existing games in this genre inspire mechanics and ideas. Game mechanics and genres are not protected; only their specific expression is (art, names, text, code). Nothing is copied: no assets, names, UI text or code.

Developed with Claude Code. The architecture therefore also favors clear crate boundaries, data-driven configuration, and per-module tests that an agent can run headless.

## Core architecture

Modules never call each other. They read named **fields** on the world, change fields through their owners, and talk through an **event bus**. That one rule is what makes every module swappable.

&#91;embedded content: core architecture · 4 module slots, fields, event bus, renderer, observer\] *(diagram predates the Oct 14 revision; it needs redrawing to show change requests, the command log and the editor)*

The scheduler runs each module on its own cadence. Every module reads fields, submits change requests and exchanges events. The renderer, the inspector and the editor therefore never depend on which module version is active.

**Fields.** The world is a set of typed arrays keyed by entity tier: `cell.elevation`, `cell.temperature`, `cell.rainfall`, `province.population`, `province.stock` (one value per good), `faction.treasury`, `faction.tech_level`. Each field has exactly one **owner** module, and any module may read it. If no active module owns a field, the engine supplies a documented default, so readers always find a value.

**Change requests: how many modules affect one value.** Many values are changed by several domains. Births, war, plague, famine and migration all change population. Food is grown by the economy, eaten by armies and spoiled by floods. So other modules never write a field directly. They submit a **change request** to its owner, for example "remove 3,000 people from province 812, cause: plague". The rules are:

- **Timing within a day.** Modules run in their fixed order. An owner's own main-tick writes are visible to modules later in the same day; that is why run order matters. Requests submitted on day N are applied in **one pass at the end of day N**, owner by owner in a fixed order, after every module has run. Nobody sees another module's request take effect halfway through a day.
- Every owner takes part in that end-of-day pass **on every day it has pending requests**, even if its own main logic runs weekly or monthly. The pass is cheap: it works through a queue and is separate from the owner's main tick. This stops a daily army from eating food that a weekly economy thinks it still has.
- Within one owner, requests are applied in a fixed order (see Determinism).
- If requests ask for more than exists, the owner **rations**. Requests carry a priority class, and classes are served in order. Within the class that runs short, the available amount is split proportionally (largest-remainder rule); later classes get nothing. The amount actually granted is reported back to the requester the next day, as an event addressed to it.
- Every applied request is kept with its cause for the day it was applied. Older history is rebuilt on demand by replay (build step 4) rather than stored. This is what the inspector's "why did this change" view reads.

**Module contract.** Every module declares, in its manifest:

- `reads`: fields it consumes
- `owns`: fields it is the owner of (and therefore applies requests for)
- `requests`: fields it submits change requests to
- `cadence`: how often its main logic runs (daily, weekly, monthly, seasonal, yearly, every N years), in calendar units
- `tier`: the level it works on (cell, province, pop; factions later)
- `requires`: other fields or capabilities that must exist
- `emits` / `handles`: event types it publishes and subscribes to
- `version` and `state_schema`: its version, and the layout of its private state (needed for saves and for future version changes, see below)

The engine validates a configuration at load. It checks that no field has two owners, every `requires` is satisfied, and there are no cycles in same-day dependencies. A bad combination fails at load with a clear message, never mid-game.

**Versions and the null module.** A domain is a slot, and versions fill it:

- Climate v0 paints static biomes.
- Climate v1 computes temperature and rainfall from latitude, elevation and rain shadow.
- Climate v2 simulates wind and moisture transport.

Economics only ever sees `cell.rainfall`; it never knows which version produced it. Every slot ships a **null version** that writes sensible constants. Switching magic off therefore does not break trade, and a half-built module can be tested against stable neighbours.

**Event bus.** Cross-domain consequences travel as typed events (`WarDeclared`, `HarvestFailed`, `VolcanoErupted`, `TechDiscovered`) with a day stamp and payload. Publishers do not know who listens. Removing a listener module simply means nobody reacts, and the world stays consistent.

- **Delivery rule.** An event emitted on day N is delivered on day N+1. Within a day, events are delivered in a fixed order: the event type's declared priority, then the emitting module's place in the run order, then entity id.
- **Handlers re-check their preconditions on delivery,** because the world may have changed since the event was emitted. That is how real day-to-day cause and effect works. The king is assassinated on Tuesday, so on Thursday the treaty handler finds the signer dead and the treaty fails.
- **Same-day conflicts are settled by priority.** Destruction and death come before agreements, and agreements before routine changes. The order is a documented table in the engine config, not something each module decides.

**Determinism and replay.** The same seed, configuration and command log on the same machine and build produce the same world. Results are **not** promised to match across different machines or builds. Three things make the promise hold:

- **Order-independent randomness.** Every random number is computed from `(seed, module, day, entity, purpose)` by a hash. Nothing is drawn from a shared stream. Results therefore don't depend on the order in which parallel work runs, and adding a module doesn't shift anyone else's numbers. Toggling a module still changes the world, and so changes everything downstream; that is expected.
- **Fixed-order totals.** Parallel sums use fixed chunking and combine the chunks in a fixed order, so floating-point totals don't drift between runs.
- **Exact conserved quantities.** Conserved quantities (population, money, goods) are stored as integers or fixed-point values. Conservation checks are then exact rather than approximate. Anything that divides a conserved total proportionally (rationing, border-edit reallocation, splitting into finer categories) uses one shared **largest-remainder rule**: whole units are handed out by share, and leftover units go to the largest fractional remainders, ties broken by entity id. Totals therefore never drift by ±1.

**Saves** hold:

- the seed and world-generation config;
- the **command log** (every player poke, edit and config change, day-stamped);
- periodic snapshots.

Replay re-runs the simulation from the nearest snapshot and re-applies the commands. The internal event log (everything modules emitted) is stored separately. It is used by the chronicle and the inspector's history, and it can be pruned without breaking replay.

**Data-driven content.** Module parameters, biome tables, goods and unit trees, spell lists and tech trees live in TOML/RON files, not code. Balancing and data-only modding therefore do not require a rebuild.

## Tech stack

The simulation lives in its own plain-Rust crate with no engine dependency. The app shell is egui + wgpu, chosen by a renderer trial (Oct 14).

| Layer | Choice | Why |
| --- | --- | --- |
| Simulation core | Plain Rust crate (`sim`) | No GC pauses at 1M cells; `rayon` for parallel array passes; runs headless for tests and batch runs |
| Renderer / UI | **egui + wgpu (via eframe)**, decided Oct 14 after a renderer trial against Bevy (`spikes/REPORT.md`) | Same picture at about the same speed as Bevy; updates only the changed part of a texture, needed no engine defaults switched off, rebuilds ~5× faster, a fifth of the size. The sim doesn't use an engine ECS, so nothing in the sim depended on the choice. Bevy is worth revisiting only if the project ever wants 3D, an asset pipeline or many animated sprites |
| Config and content | TOML / RON | Human-editable, mod-friendly, no rebuild to rebalance |
| Saves | Command log + snapshots, `postcard`, versioned format | Deterministic replay, small files, future save → convert → reload. `postcard` replaced the planned `bincode` in step 3: bincode was abandoned in 2025 and its last release is deliberately a compile error |
| Tests | `cargo test` on `sim` with fixed seeds | Quick per-module tests run a few simulated decades headless in seconds; century-scale and 1000-year runs go to nightly / CI |

The `sim`/`app` split matters for modularity too. The renderer only ever sees field arrays and events, so it never depends on which module version is active.

## Planet geometry

The planet is a **cube sphere**: six square grids folded onto a sphere. Each face is N × N cells. N = 408 gives 6 × 408² ≈ 1M cells, and **N is a setting**, so smaller worlds are cheap.

- **Even cells.** The equi-angular mapping keeps cell sizes much more even than a plain cube or a rectangular map: the largest cell is 1.40× the smallest (measured in the renderer spike), against ~5× for a plain cube mapping. Unlike a rectangular world map, cells near the poles don't shrink, so erosion, diffusion and climate use the same time step everywhere. Grid lines don't imprint on rivers or mountain ranges. Each cell still stores its true area, centre latitude/longitude, and a local east/north direction.
- **Still flat arrays.** Each face is a contiguous 2D array, and passes are cache-friendly parallel sweeps. Neighbours inside a face are index arithmetic. Neighbours across face edges come from a precomputed table. Every cell has 4 edge neighbours, including cells on face edges and corners. The only special spots are the 8 cube corners. There three cells meet instead of four, so each of those three corner cells is missing one diagonal neighbour (7 of 8).
- **Vector fields use local directions.** Wind, ocean currents and plate motion are stored in each cell's local east/north frame, because "grid right" is not "east" on every face. Converting between neighbouring cells goes through the table.
- **Seams are the bug magnet.** Rivers, flows and diffusion crossing a face edge are where errors will hide. A required test routes a river across a seam and checks water conservation, and another runs diffusion across a cube corner.
- **Sim resolution ≠ drawing resolution.** The map looks detailed through shader texture and noise, not through more simulated cells. Detail on screen does not require raising N.

**Projection is a renderer concern, not a simulation concern.** The sim only knows cells on a sphere, and the map view picks a flat projection (see Decisions: an oval Equal Earth map by default, a repeating rectangle as the alternative). There is no globe view.

World generation order for a new planet: tectonic plates → elevation → erosion and rivers → ocean currents (optional) → climate → biomes → resources → initial populations. Each stage is itself a module with versions, so a quick noise-based terrain is as valid as full plate tectonics.

## Simulation hierarchy

Not everything runs at cell level. The world has three tiers, and each module declares which tier it ticks on. This is the single biggest performance lever.

| Tier | Count | Holds | Typical cadence | Example modules |
| --- | --- | --- | --- | --- |
| Cells | ~1M (setting) | Elevation, climate, biome, soil, land use, mana density | Yearly to every 100 years; weather daily or weekly | Geology, climate, weather, biomes, natural world |
| Provinces | ~5k default (setting) | Pops, food, goods, buildings, religion, culture, unrest | Daily (requests), weekly to monthly (main logic) | Economics, city development, religion, culture, disease |
| Factions & characters | Hundreds | Treasury, tech, diplomacy, armies, rulers, mages | Daily to weekly | Warfare, research, diplomacy, magic (high), named characters |

**Provinces are derived from cells.** After world generation, cells are clustered (watershed, or Voronoi seeded by habitable land) into provinces of roughly equal carrying capacity. Provinces cache aggregates of their cells (total farmland, average rainfall, resource counts). These are refreshed only when a cell-level module or an edit marks them dirty. Economics reads the cache, never 1M cells.

**Pops, not people.** Population is aggregated into pops per province. A pop is keyed by culture, religion, class and **species**. Species is a pop attribute from day one, even though only humans exist at first; adding it later would touch every module. Only elites are individual entities: rulers, generals, archmages, prophets. This keeps character drama while the demographic math stays O(provinces). **Named characters get real agency.** They have ambitions, form plots and rivalries, and make decisions (declare war, marry, rebel, patronise scholars). Those decisions act on the world through the normal change requests and events, so politics is driven by people, not only by aggregate numbers.

**Armies and trade** are entities that move across provinces, not cells, and pathfind on the province graph. Cell-level detail is used only for rendering their routes and for terrain modifiers at battle time.

## Time model

The base step is **one day**, discrete and deterministic. "Day by day" means the clock, **not** that every module runs daily. Each module runs its main logic on its declared cadence. A day on which only a few modules are due must cost almost nothing.

**Cadences (defaults, configurable per module).**

| Cadence | Modules |
| --- | --- |
| Daily | Armies (movement, supply, battles), event delivery, change-request application, character actions, weather if enabled |
| Weekly | Markets and prices, unrest |
| Monthly | Harvest accounting, migration, disease spread, culture and religion drift |
| Seasonal / yearly | Climate, biomes, research progress, demography summaries |
| Every 100 years | Geology |

Within one day, modules run in a fixed dependency order derived from their manifests (climate before agriculture before trade before war). A module may sub-step internally where it needs finer time. For example, a tactical battle module may resolve a battle in hours inside one day. The rest of the planet never sees those sub-steps.

**Calendar.** The calendar is custom and stored as data from day one: days per week, weeks per month, months per year, seasons, moons. Modules ask the calendar what season it is, or whether a week starts today. They never hardcode 7 or 365.

**World sizes.** Worlds come in sizes, chosen at creation. Cell count and province count are separate settings, so "few provinces on a detailed map" is possible. Province count drives the cost of most rich modules (economy, politics, culture), so a small-province world can run complex modules fast.

| Preset | Cube face N | Cells | Provinces |
| --- | --- | --- | --- |
| Small | 128 | ~100k | ~500 |
| Medium | 256 | ~400k | ~2k |
| Large | 408 | ~1M | ~5k |
| Huge | 576 | ~2M | ~20k |

**Speed budget (initial targets, to be measured from build step 1).** Speed depends on world size × module complexity. The targets are stated for one reference combination, and every other combination is measured and shown, not promised.

- A century is ~36,500 days. Reference target: **a century in under a minute headless** with the simple module set on a Large world on a mid-range desktop. That is ~1.5 ms per simulated day on average, with heavy yearly and century passes amortised over the days between them.
- Complex module sets will be much slower. A complex set on a Large or Huge world may not reach 1x, which is accepted. The answer for "deep simulation at a watchable speed" is a smaller world, especially fewer provinces.
- The module panel and world-creation screen show the measured ms per simulated day for the chosen size and module set, so the player sees the trade-off before starting.
- An idle day (nothing but request queues and event delivery) should cost well under 0.1 ms. Heavy passes may run on a worker thread ahead of need (see Playback).
- On screen, 365x (a year per second) leaves ~2.7 ms per day for the sim plus streaming states to the renderer. The top on-screen speed is "as fast as the sim goes". Headless **run to year N** is faster because it skips rendering and state streaming.

## Playback: discrete days that feel alive

The simulation thread and the render thread are fully separate. The sim produces a stream of daily world states (as deltas). The renderer plays them back at the chosen speed and **interpolates between states**. The sim never knows whether anyone is watching.

- **Speeds.** Pause, step one day, step one week, 1x (aims for one day per second; with complex module sets the sim may not keep up, which is acceptable — playback slows down and the speed indicator shows the real rate), 7x (a week per second), 30x, 365x (a year per second), max, and run to year N. At 1x armies march and borders slide. At 365x it is a time-lapse; the renderer then samples the latest state each frame and skips the days in between.
- **Lookahead buffer.** The sim runs a few days ahead of playback, so a heavy yearly climate pass or a 100-year geology pass never stutters the view. If the sim is faster than playback it idles; if slower, playback waits and the speed indicator shows it. **Any poke, edit or config change throws away the buffered future days** and the sim recomputes from the current day.
- **Interpolation.** Numeric fields lerp. Categorical fields (owner, biome) switch at the step; the renderer animates the visual transition (border crossfade, color fade). Interpolation is purely visual and never written back.
- **Replay = playback.** A save is a seed + command log + snapshots. Replaying it produces the same state stream, so scrubbing history, rewinding to "the day the empire fell", and branching a new timeline from any day come almost for free.
- **Stepping.** On pause, step forward one day and inspect exactly what changed. This is the primary debugging tool.

## Rendering

The map is composited on the GPU from **independent layer textures**. Each one is sampled straight from a field array and re-uploaded only when its owner marks it dirty. Because the planet is a cube sphere, each layer is a **6-layer 2D array texture**, one layer per cube face. Each face carries a one-cell border copied from its neighbouring faces, so smooth sampling reads real data across face edges and the seams don't show. Hardware cubemap sampling is *not* used: it assumes the plain cube mapping, and our equi-angular cells would come out misplaced. The map shader instead converts every screen pixel through the chosen projection to a direction on the sphere, then does its own direction → (face, equi-angular u, v) maths. That maths mirrors the CPU picking code exactly, and the two are tested against each other.

| Layer | Source fields | Dirty when |
| --- | --- | --- |
| Terrain base | elevation, biome, soil | Geology/biome tick or terrain edit (rare) |
| Hydrology | rivers, lakes, coasts | Erosion tick or terrain edit |
| Climate overlays | temperature, rainfall, season | Monthly / seasonal |
| Political | province owner, culture, religion | Daily (only changed provinces) |
| Economy | trade flow, wealth, population | Weekly |
| Magic / fantastical | mana density, corruption, ley lines | Per magic module cadence |
| Weather / fog / clouds | weather module | Daily, or animated procedurally |

Layers are composited in one shader with per-layer opacity and blend mode, so the user can stack "terrain + political at 40% + trade routes" freely. A political repaint never touches terrain.

**Vector pass.** Borders, rivers, roads, army routes, city icons and labels are drawn as vector geometry on top. They are built on the sphere from the province graph and cell edges, and projected for display. They are rebuilt only when something changes, and re-projected when the projection changes. Labels use a simple placement pass with zoom-dependent culling. The projected map must handle the east–west wrap seam cleanly.

**Fantasy aesthetic.** The hand-drawn feel comes from shader work, not art per cell. It includes parchment texture, hillshading from elevation, stylized coast outlines, painterly biome palettes, and optional old-map decorations (compass, cartouche, sea monsters). Palettes are data, so a "dark fantasy" or "storybook" look is a config swap.

**Zoom.** Mipmapped layer textures cover planet-to-region zoom. Below a threshold, a detail pass adds per-cell features (forest glyphs, field patterns, city sprites).

**Picking.** The inspector and the editor brush both need the inverse path: screen point → inverse projection → direction on the sphere → (face, i, j) cell. This is one shared function, tested against the forward path.

## God / observer mode

The player is an observer who can poke the world. Every poke is an ordinary event, so the world reacts to a divine earthquake exactly as it does to a natural one, and modules need no special player code. Pokes are recorded in the command log (see Scenario editor).

**Time controls.** Play/pause, step a day or a week, speed slider, jump to year, scrub history, branch a timeline from any past day.

**Inspector.** Click any cell, province, faction or character to see every field with **provenance**: the value, its owner, the day it last changed, and the change requests applied to it with their causes. This is cheap, because requests already carry causes. For a deeper "why", such as how rainfall turned into this harvest, the inspector **replays that day for that entity with tracing on**. Determinism makes this possible, and nothing extra is stored for 1M cells. "Why is this city starving" is answered by following rainfall → harvest → food stock → unrest.

**Daily event log.** For each province and faction there is an ordered list of what happened each day (events fired, decisions made, battles resolved), filterable by module. It is also the raw material for chronicles and storytelling.

**Poke tools** (each an event type):

- Terrain: raise or sink land, open a volcano, redirect a river
- Climate: cool or warm a region, trigger a drought or a flood
- Life: spawn a plague, a monster, a resource deposit, a new species
- Peoples: found a city, gift a technology, convert a province, inspire a prophet, kill or bless a ruler
- Magic: open a ley line, raise mana, unleash a cataclysm
- Politics: declare war, force peace, split or merge a realm

**Module panel.** It shows active module versions and their parameters:

- **Parameters** can be tweaked at runtime. Each change is logged as a config command.
- **Version changes** (including switching a module on or off) go through the **save → convert → reload** path described under "Changing module versions". This path is planned for later. Until then, versions are chosen at world creation or load.

**Chronicle.** An auto-generated narrative digest of notable events per era, built from the event log. An LLM-written flavor layer is a later addition.

## Scenario editor

An interactive editor for **every aspect of the world**: the map, peoples, realms, characters, module settings and scripted events. It works on a new world or **mid-game**. The editor and the poke tools are one system with two kinds of command.

**Edits and pokes.**

- A **poke** is an in-world event; the world reacts to it by its own rules. Example: "an earthquake hits here".
- An **edit** is an authorial change that sets values directly and bypasses world rules. Example: "this province has 40,000 people, all of faith X".
- Both are **commands**, stamped with the day they apply on and stored in the command log. That is what keeps replay, branching and saves working with edits.

**When edits apply.** Edits are made while paused or running, and they take effect at the start of the day being shown, so it shows them at once. Applying one throws away the lookahead buffer. An edit made while viewing a **past** day starts a new branch of history from that day; the original timeline is kept.

**Undo/redo.** Undo works by reloading the nearest snapshot and replaying the command log without the undone command. A snapshot is taken when an editing session starts, so this is fast. No hand-written reverse operation is needed for each command.

**What can be edited.**

| Area | Edits |
| --- | --- |
| World | Seed, planet size (face N), calendar, generation settings (new world only) |
| Map | Elevation brush (raise, lower, smooth, add noise), sea level, rivers and lakes (draw, redirect), biome and climate overrides, resources and minerals, mana |
| Provinces | Split, merge, redraw borders, rename, set capitals |
| Peoples | Pops (size, culture, faith, class, species), culture and faith definitions |
| Realms | Create or dissolve factions, assign provinces, treasury, techs, relations, treaties, wars |
| Characters | Create, edit traits, kill, assign roles, mark individuals to follow (Lives) |
| Armies | Place, size, composition, orders |
| Modules | Parameters (live); versions and content tables (goods tree, units, tech tree, spells) through the reload path |
| Script | Scheduled pokes at chosen days, and assertions (expected ranges for chosen fields at chosen years) |

**Repair after an edit.** An edit can leave derived data inconsistent, so each kind of edit has a repair step that runs before the next day:

| Edit | Repair |
| --- | --- |
| Elevation / sea level | Recompute rivers, lakes and coasts in the affected region; mark climate and biomes dirty there; refresh province aggregates. If land becomes sea, its people move to the nearest land province, or are removed with an explicit "drowned" record. |
| Biome / climate override | The override is a field the climate owner respects until it is cleared; dependent layers are marked dirty. |
| **Province borders (hardest case)** | Reassign cells, then move pops, stocks, buildings and money between provinces in proportion to each moved cell's share (carrying capacity, or population density where known), keeping totals exact. Then fix ownership, armies inside the changed provinces, and the trade/route graph. |
| Realm ownership | Make diplomacy and war states consistent: no faction at war with itself, and no treaties with dissolved factions. |
| Pops, money, goods | Recorded as **declared external sources or sinks**, so conservation checks do not flag the edit as drift. |

**Scenario files.** A scenario is a TOML file: seed and generation settings (or a world snapshot), module configuration and parameter overrides, the edit log, scheduled pokes, and assertions. The editor saves and loads scenarios, and any mid-game world can be saved as one. Scenarios double as presets for play ("Ice-age start", "Magic-dead world", "Two empires at war") and as test cases for the combination tester.

## Module catalogue

Each domain is a slot with at least a null version, a simple version and a richer one. The versions listed are illustrative; the point is that each slot's readers only see its output fields.

| Slot | Tier | v0 (simple) | v1 | v2 (deep) | v3 (deeper, later) | Key output fields |
| --- | --- | --- | --- | --- | --- | --- |
| Geology | Cell | Noise heightmap | Plates + uplift + erosion | Plate drift over millennia, volcanism, mineral genesis | Ice ages and sea-level cycles, soil formation and exhaustion, aquifers, resource depletion | elevation, rock, minerals, soil |
| Climate | Cell | Static painted biomes | Latitude + elevation + rain shadow | Wind, moisture transport, ocean currents, climate drift | Circulation cells, multi-year oscillations, volcanic winters, feedback from deforestation and irrigation | temperature, rainfall, season |
| Weather | Cell | None (climate averages) | Daily or weekly variance per region | Storm tracks, droughts, floods as events | Fronts and storms tracked across the map, affecting harvests, battles and sea travel | weather\_now, disaster events |
| Natural world | Cell/Province | Biome yields only | Flora/fauna populations, forests regrow | Food webs, migrations, extinctions, monsters | Keystone species, domestication, invasive species, ecological collapse and recovery | wildlife, timber, fish |
| Economics | Province | Food vs population | Goods, prices, local markets | Trade networks, money, credit, guilds, logistics | Production chains (input-output), capital, banking, inflation, trade companies, substitution when a good runs out | food\_stock, goods, prices, wealth |
| City development | Province | Pop threshold = town | Buildings, districts, infrastructure | Urban planning, sanitation, walls, sprawl onto cells | Districts with land value, migration between cities, fires, slums, urban cultures | buildings, infrastructure |
| Research | Faction | Fixed tech per era | Tech tree with costs | Knowledge diffusion, scholars, libraries, lost knowledge | Institutions and patronage, secrecy, independent discovery, paradigm shifts, knowledge loss on collapse | tech\_level, known\_techs |
| Culture | Province/Faction | One culture per faction | Assimilation, drift | Languages, traditions, art, cultural prestige | Language trees, works of art as artifacts, ideas spreading along trade routes | culture\_mix, traits |
| Religion | Province/Faction | None | Faiths spread, conversion | Schisms, prophets, holy sites, divine favor | Theology drift, syncretism, holy wars, gods as entities that answer worship (with magic on) | faith\_mix, holy\_sites |
| Warfare | Faction | Army size vs army size | Units, terrain, morale | Daily movement and battles, supply lines, attrition, sieges, naval | Doctrine and command, unit experience, combined arms, fortification networks, fog of war, mercenaries | armies, battles, control |
| War logistics | Faction/Province | Free movement | Supply range from depots | Roads, rivers, foraging, seasons, carrying capacity | Depot networks, river and sea transport capacity, baggage trains, camp disease, winter quarters | supply, attrition |
| Diplomacy | Faction | Random war/peace | Opinion, treaties | Alliances, vassalage, dynasties, espionage | Coalitions, legitimacy, balance of power, international law, intelligence networks | relations, treaties |
| Characters | Faction | Rulers as stats | Traits, lifespans, succession | Ambitions, rivalries, courts | Dynasties, relationships, plots, grudges remembered across generations | rulers, notable\_people |
| Magic | Cell/Faction | None | Mana field + spells as events | Schools, ley lines, corruption, magical economy and warfare | Magic as physics: a mana cycle like the water cycle, magical pollution, planar rifts, divine entities | mana, ley\_lines, corruption |
| Fantastical world | Cell/Province | None | Monsters and races as data | Dragons, undead, planar rifts, blighted lands | Sapient non-human civilizations with their own version sets, elder beings as agents, ages of myth | creatures, anomalies |
| Disease | Province | None | Plague events | Transmission over trade routes, immunity, medicine | Pathogen evolution, zoonoses, quarantine policy, medicine as research output | health, outbreaks |

Rule for "real-world principles": economics, logistics, climate and warfare must work and feel right with magic and the fantastical set to null. Magic then adds inputs and events on top. It is never a crutch for a domain that doesn't stand on its own.

## Taxonomies and changing module versions

**Taxonomies are data.** Every categorical field is typed by a taxonomy tree. A goods tree might read `goods → food → {grain, fish, livestock, fruit}` and `goods → manufactured → furniture → {chair, table, chest}`. The same pattern covers units (`spearmen → infantry → army strength`), faiths (`sect → faith → religion`), species, techs and resources. Trees live in TOML beside the module parameters.

**Versions declare their depth.**

- Economics v0 ("food") keeps one value per province: the `food` class as a whole.
- Economics v1 ("markets") keeps the eight leaves: grain, fish, livestock, timber, stone, ore, cloth, tools.
- A later version could keep a deeper tree, such as one with kinds of grain.

A version names the nodes it keeps in its manifest. They need not be one whole depth: v0 keeps `food` and nothing of `raw` or `manufactured`. When no module keeps goods, readers find each top-level class, empty.

A reader may read at its own depth or any coarser one; the engine sums children on the fly. Warfare v1 asking for `food` from Economics v2 just gets grain + fish + livestock + fruit.

**Readers must be depth-agnostic.** Rule for module authors: never hardcode leaf names; ask the taxonomy (`all leaves under food`, `parent of chair`). This lets a v3 production chain coexist with a v0 reader. It is valuable now, independent of version changes.

**Now: versions are chosen at world creation or load.** Mid-game version swapping is not supported yet.

**Later: change versions through save → convert → reload.** Changing a module's version mid-game (from the module panel or the editor) will work as follows:

1. Save the world at the current day.
2. Run a **conversion step** on the save. Moving to a coarser version sums leaves into their parents (exact for totals). Moving to a finer version splits each total into children using the taxonomy's default proportions, adjusted by the current world (e.g. climate-driven expected grain/fish/livestock mix). The converted module's private state is initialised from the shared fields.
3. Reload with the new configuration. The new module warms up for a few days.
4. **Check conservation.** Population, money, goods by top-level class, and land area must be exactly equal before and after. A conversion that breaks this is refused with a report, never applied silently.
5. The config change is recorded in the command log as the **start of a new timeline segment**, so replay knows which configuration ran when.

**What the save format needs from day one** so this path isn't blocked later:

- each module's version and parameters;
- each shared field stored with its taxonomy depth;
- each module's private state stored separately from shared fields, tagged with its `state_schema`;
- config changes recorded in the command log as timeline segment boundaries;
- a format version number on every save.

## Lives: followed individuals that ride on the modules

A **Lives** module simulates a small set of named individuals, custom-made or picked out of a pop, without owning any of the world's rules. It has no farming logic and no magic logic. It asks the active modules what a life in this province can be, and rolls that person's year against the fields those modules already write.

**Roles come from the modules, not from Lives.** Every module manifest may declare one or more **roles**. A role states who can hold it (pop class, species, tech or faith prerequisites), a yearly outcome function that reads that module's own fields, the life events it can emit, and what the role contributes back. Examples:

| Active module | Roles it offers | Outcome driven by | Events it can emit for a life |
| --- | --- | --- | --- |
| Economics (any version) | Farmer, fisher, merchant, artisan | province harvest, prices, trade volume | Bad harvest, windfall, bankruptcy, joined a guild |
| Warfare | Soldier, officer, mercenary | faction at war, battles in range, supply | Conscripted, wounded, decorated, deserted, killed |
| Research | Scholar, apprentice | faction tech, libraries, patronage | Discovery, lost manuscript, exiled for heresy |
| Religion | Priest, monk, prophet | faith share, holy sites, schisms | Vision, pilgrimage, persecution |
| Magic (if humans can cast) | Mage, hedge witch | mana density, schools, corruption | Spell backfire, recruited by a school |
| Magic (if humans cannot) | Scholar of the arcane, relic hunter | mana density, anomalies nearby | Found an artifact, cursed |
| City development | Builder, official, criminal | buildings, unrest, sanitation | Fire, promotion, arrested |
| Characters | Courtier, heir | ruler traits, court intrigue | Plot, marriage, succession |
| Natural world | Hunter, herder, forester | wildlife, timber, migrations | Mauled, famine year, great hunt |

If a module is off, its roles simply don't exist in that world. Switch Magic to null and the mages become scholars or farmers at the next role change; nothing in Lives needs to know why.

**A life is a sample of its pop.** Each individual belongs to a pop (province, culture, faith, class, species). Their yearly health, wealth and risk are drawn from that pop's distributions, conditioned on their role, skills and traits. An ordinary followed person is a *sample*, not an addition: the pop's numbers already include them, so their births, deaths and migration never change aggregate fields. Their **wallet is a share of their pop's wealth**, not extra money, so it counts in the money total. Age, skills, traits, relationships, memories and the wallet are the only state Lives owns.

**Events reach them through the bus.** Lives subscribes to province- and faction-scoped events (a plague in their province, a battle their army fought, a city fire, a famine) and rolls each followed person against them. This is the same event stream the chronicle reads, so a life story is consistent with the world's history by construction.

**Agency without AI.** A followed individual chooses among available roles, migrations and marriages with a small utility model over their traits and needs (ambition, piety, risk tolerance). The player can also steer a custom individual directly ("become a scholar", "move to the capital"); that is just another poke command.

**Promotion to the Characters tier.** If a followed person becomes a ruler, general, archmage or prophet, they are handed to the Characters module as an elite entity, and Lives keeps following them. From then on they **do** change the world: their decisions act through normal change requests and events, like any character's. The reverse happens on a fall from power.

**Lineage and scale.** Children are sampled from the parents' pop with inherited traits, so a family can be followed for generations. Lives is O(followed individuals), so following a few hundred costs nothing; following thousands is a performance setting, not a design limit.

**Versions.**

- **v0:** a yearly diary from pop statistics only.
- **v1:** roles, skills, relationships, event rolls.
- **v2:** utility-driven choices, lineages, promotion to Characters.
- **v3:** social networks between followed people, reputations, and lives as a source of chronicle narrative.

## Target stories

The plumbing exists so that the world produces stories worth watching. These stories are **acceptance tests**: each is a scenario file with assertions, and the build is not "done" at a step until the stories it unlocks emerge without scripting beyond the initial poke.

1. **Drought → famine → unrest → war.** A multi-year drought in a breadbasket region cuts harvests, food stocks run down, people starve or migrate, unrest rises, and the realm either fractures in rebellion or attacks a neighbour for its grain. The famine half is testable after build step 4; the full chain after step 7.
2. **The rich river city.** A city at a river mouth on a trade route grows rich, becomes the prize of a war, and later declines when the route shifts or a rival port rises. Testable after step 7.
3. **Plague along the trade routes.** A plague lands in a port, spreads along trade routes faster than over land, kills a share of the population, and leaves labour shortages and higher wages behind. Testable after step 10.

## Stability, scenarios and the combination tester

Emergent coupling between independently designed modules will produce runaway growth or collapse somewhere. The goal is not to prevent every bad combination. It is to **detect them automatically, document them, and surface the status in the module panel**.

Defenses, in layers:

1. **Field contracts.** Every numeric field declares bounds and an expected magnitude. Values outside bounds are clamped and emit a `ContractViolated` event with provenance (module, day, inputs). This is a soft failure: logged, never a crash.
2. **Conservation checks per day.** Population, money, goods and land are totalled every day. Because they are stored exactly, any change not explained by a change request or a declared edit source/sink is flagged with the module responsible.
3. **Named negative feedback.** Design rule: every growth loop in a module must name its damper (carrying capacity, diminishing returns, decay, price response). A reviewer or an agent can check the manifest for a loop with no damper.
4. **Scenarios.** Scenarios are authored in the scenario editor (see above) and stored as TOML. Their assertions make them test cases.
5. **Combination tester.** A headless runner that samples module configurations and runs each for N years on several seeds. The full matrix (16 slots × 4–5 versions) is too large. The tester uses:
   - all-pairs coverage (every pair of versions appears together at least once);
   - every shipped scenario and target story;
   - a random sample;
   - a list of **known-risky triples**, since some failures only appear when three modules interact. The list grows whenever a red result needs three modules to reproduce.

   Each run records per-day metrics and performance.
6. **Evaluator.** Scores each run on:
   - contract violations and conservation drift;
   - explosion (a field pinned at its bound) or extinction (pinned at zero) for more than X days;
   - stagnation (nothing changes for Y years);
   - ms per simulated day.

   **Oscillation alone is not a failure.** Boom-and-bust cycles are content. It is flagged only when the swings hit bounds or the period is implausibly short. Output is a **compatibility matrix** per version pair: green, yellow (known quirks) or red (unstable), with the reproducing seed.
7. **Surfacing.** The matrix ships with the game. The module panel shows it when you pick a version ("Economics v2 + Logistics v0: unstable, see issue #42") and refuses combinations marked red unless you override.
8. **Agent loop.** Claude Code runs the tester in CI. A red cell comes with a seed and a day, so the agent can replay, bisect to the day, and read the provenance of the first violation.

Accepted outcome: some combinations stay yellow or red forever. What the player gets is an honest map of what works, and the simple versions always remain a safe fallback.

## Build order

Start with a thin slice you can watch, and pull the framework out of real modules rather than designing it up front. Each step produces something you can look at. Resist magic until the mundane world works.

1. **Thin slice, headless.** Done Oct 14 (`crates/sim`, see its README for measured speed). The PNG export is equirectangular for convenience only; it does not settle the projection question.
   - Cube-sphere grid with seam and corner neighbour tables.
   - Noise terrain, simple latitude/elevation climate, biomes.
   - Province clustering; pops that grow and starve.
   - A minimal daily scheduler with hashed randomness, and a calendar as data.
   - Headless PNG export (one projection) for eyeballing.
   - Tests: same seed gives same world, water conserved across a seam, speed per simulated day measured from here on.
2. **Map view.** Done Oct 14 (`crates/app`, see its README for how to run it and measured speed).
   - Oval Equal Earth map (default) or repeating rectangle, switchable; dragging sideways turns the world under the map.
   - Per-face array textures with a one-cell border, so face seams don't show.
   - Smoothed coastlines and province borders (one curve per shared border, junctions pinned), rivers, and a 30° graticule.
   - Parchment look, layer sliders (terrain, temperature, rainfall, population, famine, province colours), a hover inspector, and "+1 / +10 years" buttons that re-upload only the per-province tables.
   - Picking is tested against the shader on the GPU.
   - Left for later: place-name labels (nothing has names yet) and mipmaps (not needed at current zoom limits). (Since Oct 14: province colours follow the same curve as the border lines instead of stepping along cell edges, provinces grow into irregular shapes that follow rivers and ridges, and borders get a gentle hand-drawn wobble.)
3. **Extract the module contract.** Done Oct 14 (`crates/sim`, see its README).
   - Manifests for every module, generation stages included; the run order is derived from them, and bad configurations (two owners, unmet needs, cycles) are refused at load with a message.
   - Change requests with rationing (largest-remainder rule), grants delivered the next day; the pops were split into Economics v0 ("food": harvests and stores) and Population v0, so a real module asks another for food.
   - Event delivery on day N+1 in a documented priority order; field contracts (clamp and report); exact conservation checked after every stage and blamed on the module whose stage broke it.
   - Save format with all the fields listed under "What the save format needs", in `postcard`; a save taken while requests are in flight resumes bit for bit.
   - Null and simple versions of terrain and climate, chosen at world creation (`--terrain`, `--climate`).
   - Splitting food from people changed how a given seed plays out, so results differ from step 2 for the same seed.
4. **Time controls and inspector.** Done Oct 14 (`crates/app` and `crates/sim`, see their READMEs).
   - The simulation runs on its own thread, up to 120 days ahead of the day shown. Speeds: pause, a day, a week, a month or a year a second, and as fast as the simulation goes; step a day or a week; run to a year. Province values blend between days.
   - Going back: a checkpoint at the start of every year, and `rewind` to any earlier day by replaying from it. A poke throws the days computed ahead away this way.
   - The inspector: every value of a province with the module that sets it and the day it last changed; the change requests settled that day; "why?", which replays that day with tracing on and lists each module's own explanation and every change; and the province's daily event log, filterable by source.
   - Story 1's famine half needed two things the step didn't list: a way to start a drought (a poke, stored in the command log) and harvests that fall when rain does (Weather v0 "droughts" owns a per-province rain shortfall, and the harvest reads it). Both are in. The story passes as a test (`crates/sim/tests/stories.rs`).
   - Left for later then, done in step 5: scrubbing back through history in the window, and branching a new timeline from a past day.
5. **Scenario editor v1.** Done Oct 14 (`crates/sim` and `crates/app`, see their READMEs).
   - Map painting with repair, province split/merge/redraw with pop reallocation, pop edits.
   - Command log, undo via snapshot replay, branching from a past day, scenario save/load.
   - Early on purpose: the editor is also the main tool for building test scenarios.
   - Edits take effect on the day shown. The map is not in the yearly checkpoints: each edit's change to it is kept as before-and-after values where they differ, undone and redone in place. Province numbers never change (a merged province stays as an empty placeholder) and pops are only ever added, so anything in flight still finds its target.
   - Painting covers height, sea level, climate and biome overrides, and drawn or redirected rivers; lakes are painted below sea level, as there are no lakes above it yet. Resources, minerals and mana wait for the modules that use them.
   - Each timeline is a whole simulation of its own; a new one shares the checkpoints and map changes it has in common with the one it left. Scenario files are readable TOML whose commands are replayed on load. Assertions in scenarios wait for the combination tester (step 6).
   - The history slider also covers step 4's leftover "scrubbing back through history".
6. **Economics v1 and taxonomies.** Goods tree, prices, local markets, roads, trade layer. The combination tester and evaluator start here, once two slots have two versions each. Planned Oct 15, in this order:
   - **The tester comes first.** Done Oct 15 (`crates/sim/src/combos.rs`, the `combos` program; see the sim README). Every mix that runs is green so far; story 1's famine half is a shipped scenario with expectations. Terrain and climate already have two versions each, and weather, population and economics can be switched off, so it can start now and guard every later piece. It runs headless, covering every pair of versions, the shipped scenarios and a random sample, on small worlds and several seeds. The scorer checks clamped values, conservation drift, values stuck at a bound or at zero, stagnation and ms per day, and writes a green/yellow/red table with the seed and day that reproduce each problem. Scenario files gain assertions (expected ranges at chosen years). The window shows the table's verdict when versions are picked for a new world; the full module panel stays in step 12.
   - **Goods as data.** Done Oct 15 (`crates/sim/src/taxonomy.rs`, `crates/sim/data/goods.toml`; see the sim README). A goods tree in TOML: food → grain, fish, livestock; raw → timber, stone, ore; manufactured → cloth, tools. Shared fields gain values per province *and per good*. Totals, bounds, saves, checkpoints, last-changed stamps and the inspector all handle them, and conservation is checked per top-level class. Economics v0 ("food") behaves exactly as before: same fingerprints, same tester table.
     - The food store became `province.stock`, which holds one value per province for each good the economics version keeps. v0 keeps one, food as a whole, so the stored numbers are unchanged.
     - The tree is part of the configuration (save format 6). Scenario files without one get the shipped tree, so old scenarios still load.
     - Moving land between provinces, or sinking it, already moves every good kept. Left for the "edits" piece: setting any good from the inspector, and refusing an edit to a field nobody owns.
     - Tested with a stand-in economy that keeps grain, fish and ore. Speed is unchanged: about 0.25 ms per simulated day on a large world, against the 1.5 ms budget.
   - **Readers ask the taxonomy.** Done Oct 15 (`serve_goods` in `crates/sim/src/requests.rs`; tests in `crates/sim/src/goods_tests.rs`). Population asks for "food" through the tree, not for a named field, so it works with either economics version. Reading a class sums its goods (tested with a deeper toy tree too).
     - A change request on stored goods names a node of the tree and a province. The owner serves it from every good it keeps under that node as one pool, rationed by the usual rule, and takes each grant from those goods in proportion to what each holds. One request still gets one grant.
     - Modules list the goods they ask for. A world whose economy keeps nothing to serve them, or keeps the good only inside a bigger whole, is refused when it is made, with a message.
     - Economics v0 gives the same worlds: same yearly tables, same tester table. Saves are format 7, because a request now records the good asked for. Fingerprints taken while a grant is in flight differ by that record alone.
   - **Edits move every good.** Split, merge, redraw and sinking provinces move all goods, and the inspector sets any good. An edit that needs a field nobody owns is refused with a message.
   - **Province neighbours and roads.** A province graph with a travel cost per shared border (slope, big rivers, sea crossings between coasts). It is rebuilt whenever the map changes, and stays right through edits, undo and timelines. Roads are fixed by the land in v1; roads that grow with traffic can be a later version.
   - **Money is real.** Coins are a conserved quantity like people and food: held per province, moved by buying and selling, never created or lost except by a declared cause.
   - **Economics v1, "markets".** Each province makes goods from its land and people, eats and uses them, and keeps a local price per good that rises with shortage and falls with plenty (the price response is the named damper).
   - **Trade.** Goods move between neighbouring provinces along roads when the price gap beats the travel cost, paid for in coins. Trade is inside the economics module, so it needs no change requests.
   - **In the window:** a trade and price map layer, goods, prices and coins in the inspector, and choosing module versions when making a world, with the combination tester's verdict shown as they are chosen (piece 1 shows it only for the mix the window was started with).
   - **Kept guarantees.** Old scenario files still load. Story 1's famine half still passes on v0. On v1, a drought region buys food in and its food prices rise. Speed is re-measured against the 1.5 ms per day budget.
7. **Factions and diplomacy.** Realms, rulers, borders, war/peace. Editor gains realm and character edits. Target stories 1 (full) and 2 pass.
8. **Warfare and logistics.** Daily armies, supply, battles, sieges. Battle inspector. Editor gains army placement.
9. **Research, culture, religion.** Spread, drift, conversion, tech diffusion. Lives v1: followed individuals riding on the roles the existing modules offer.
10. **Natural world and disease.** Wildlife, forests, plagues over trade routes. Target story 3 passes.
11. **Magic and the fantastical.** Mana field, ley lines, spells as events, monsters, cataclysms, all as optional modules.
12. **Polish and version changes.** Module version changes via save → convert → reload; chronicle; module panel UX; replay scrubbing; save compatibility across releases; data-only mod loading.

After step 5 the game is already a watchable, editable living map; everything after is depth.

## Decisions

- **Time:** the base step is one day; modules run their main logic on their own cadence. An event on day N takes effect on day N+1, with a documented priority order for same-day conflicts.
- **Writing values:** one owner per field; everyone else submits change requests, which are applied daily and rationed when they exceed what exists.
- **Determinism:** same seed, config and command log on the same machine and build give the same world. Not promised across machines.
- **Module versions:** chosen at world creation or load. Mid-game changes are planned for later via save → convert → reload, and the save format supports this from day one.
- **Scenario editor:** a core feature that edits every aspect of the world, including mid-game, through logged commands.
- **Grid:** cube sphere with equi-angular mapping; face size is a setting (408 ≈ 1M cells); projection lives in the renderer.
- **Province count:** a setting, default ~5k.
- **Species:** a pop attribute from day one; humans only at first.
- **Calendar:** custom and stored as data from day one.
- **Mods:** data-only (TOML/RON) first; scripted modules maybe later.
- **LLM chronicle:** later, not v1.
- **Hardware floor:** a laptop with integrated graphics.
- **Renderer:** egui + wgpu via eframe, decided Oct 14 from the renderer trial (`spikes/REPORT.md`). Caveat kept on record: Bevy crashed on the integrated GPU with border lines on, and the cause was never found (it may have been the trial's own Bevy setup).
- **Map view:** flat map only; no globe view.
- **Map projection:** both, switchable (decided Oct 14). The default is Equal Earth, an oval world map that keeps true areas; panning sideways turns the world under it. The alternative is the plain rectangle (equirectangular), which repeats sideways and stretches the poles.
- **1x speed:** aims for one simulated day per second. With complex module sets the sim may not keep up, and that is acceptable.
- **World sizes:** Small / Medium / Large / Huge presets, with cell count and province count as separate settings. Speed depends on world size × module complexity; smaller worlds (especially fewer provinces) are how to run complex modules fast.
- **Named characters:** get substantial agency (ambitions, plots, decisions that act on the world), not just aggregate politics.
- **Planets:** one planet per save.
- **Rationing:** when requests ask for more than exists, priority classes are served in order and the class that runs short shares proportionally (decided Oct 14). Strict priority alone was rejected because it can starve whole modules out.
- **Editor scope for step 5** (decided Oct 14): branching keeps both timelines, with a switcher; a new branch does not carry over the old timeline's later pokes and edits. Map painting covers height, sea level, biome and climate overrides, and drawing or redirecting rivers and lakes; resources, minerals and mana wait for the modules that use them. Scenarios are readable TOML files whose edits are replayed on load. The window goes back in time with a history slider.
- **Economics scope for step 6** (decided Oct 15): money exists as counted coins; v1 tracks eight named goods under food, raw and manufactured; roads are fixed by the land for now; the combination tester writes a results table from the command line, and the window shows its verdict when versions are picked.
- **Edits take effect at once** (decided Oct 14): an edit logged for a day applies at the start of that day, so the world shown for the day the player is looking at already has it. Painting while paused would otherwise show nothing until a day passed.
- **Day by day:** the clock ticks daily and each module runs its main logic on its own cadence (decided Oct 14). Running every module every day would make slow modules such as climate cost far more for no visible gain.

## Open questions

None right now.
