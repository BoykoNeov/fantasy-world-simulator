# app: the map view

The window that shows the planet: egui + wgpu over the `sim` crate. Build steps 2 (the map view), 4
(time controls and the inspector) and 5 (the scenario editor) from the design doc, and the window
pieces of steps 6 and 7. The simulation code knows nothing about it; the
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
  drought, province colours, prices), line toggles, and a hover inspector.
- **Price map** (step 6, where some module prices goods): pick a good, and each province shows
  what it costs to trade there (its local price times the money level) against the middle
  province: purple cheap, orange dear, from a quarter to four times; clear where nobody lives.
  Worked out again only when the day's values or the good change (prices move once a month).
- **Trade** (step 6, markets with trade): "trade, last month" draws a line between each pair of
  provinces that traded, seat to seat, as thick as the trade was worth (what went one way less
  what came back) with an arrowhead toward the buyer; pairs under 3% of the busiest are left out.
  "sold or bought more" tints provinces gold where they sold more than they bought, rose where
  they bought more (full colour at a tenth of the coins held). Both come from the record of last
  month's trade the simulation keeps for showing (`sim::markets::TradeRecord`); a world just
  opened has none until a month has run. The trade lines are a small GPU buffer of their own,
  replaced only when a month's trade comes in. The inspector lists what the province sold and
  bought last month, from and to whom, for how much.
- **Roads and sea lanes** (a line toggle, off by default): every way between neighbouring
  provinces, seat to seat, solid over land and dashed by sea.
- **Nothing waiting for the GPU is lost** when egui throws a pass away to lay it out again (the
  first time a table shows): province tables, trade lines and a new map's picture stay waiting
  for the next pass that is drawn.
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
  Goods have a table of their own: a row per good with its store, local price and workers, and
  when they last changed (with "why?"). Where prices follow the money, a "to trade" column shows
  each price times the province's money level (`sim::markets::money_levels`, the same function
  trade uses), with a line saying how many coins a person it holds against the world. Coins show
  only their store: they are held, not made or priced.
  Below the values, its neighbours: each province it borders or faces across the water, the days
  it takes to get there by land and by sea, and how long their shared border is. Click one to go
  to it.
- **Event log**: what happened to the province each day (famines, food shortages, droughts), newest
  first, filterable by source. The player's pokes (droughts, wars, peace, killing a ruler, breaking
  away, marching) show under "your pokes".
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

- **Realms** (step 7, window part 1; only where realms are kept, `--realms`/`--rulers`/`--wars`):
  - "realms" colours each province by the realm that owns it, a hue from the realm's number
    (`layers::realm_color`), so a realm keeps its colour while others break away or end. Land
    nobody owns stays clear. Where an enemy occupies a province, diagonal stripes of the
    occupier's colour run over it, fixed to the screen (not yet seen on screen: nothing occupies
    land until armies come; the colours behind them are tested). Neighbouring realms can get close
    colours; the borders between them tell them apart.
  - "borders between realms" (on by default; a world without realms has none) draws a thick line
    wherever the realms on either side of a province border differ, or a realm meets land nobody
    owns. The lines are the province borders themselves, wobble and all, flattened once per map
    with the two provinces each runs between (`geometry::RealmBorders`); when who owns what
    changes, the ones between different owners are copied into a buffer of their own, so nothing
    is worked out again. They don't fade when zoomed out, and rivers and coasts draw over them.
  - "unrest" shades provinces by how restless they are: clear when calm, yellow through orange to
    red toward an uprising, deep crimson where one has risen up.
  - Realm numbers are never blended between days: realm colours, occupiers and unrest show the
    day on screen.
  - The inspector opens with the selected province's realm: its colour and name, its capital
    (click to go there, and so to its page), its provinces and people, its treasury and the tolls it
    collected last month (under markets, from the last month's trade record: none shown in a world
    just opened until a month's trade runs), the realm it broke away from, provinces in uprising
    and held by enemies; its ruler
    (age, traits, since when and how they came to the throne) and heir; its wars and truces (with
    a button to go to the other realm); earlier rulers; and what happened to it (successions, wars,
    peace, uprisings, breakaways). The province's own values say "occupied by" a realm by name,
    "calm" or "risen up", and unrest as "N of 1,000 toward an uprising".
  - Realms and rulers are named everywhere the inspector and the event log speak of them
    (`inspector::Names`), and the log can filter by Realms, Characters and Diplomacy. The monthly
    trade between two realms ("X sold N coins' worth of goods to Y last month, and bought M coins'
    worth") has its words, but isn't kept in the event log (hundreds a month).
  - Hovering says which realm owns a province, who occupies it, and how restless it is.
  - The Modules section lists the realms, rulers, diplomacy and armies versions running, with the
    combination tester's verdict for the mix (they joined its choices at step 7's checks).
- **Armies, battles and wars** (step 7's window part 2; `armies.rs`, only where armies are kept):
  - "armies" draws field armies marching, gathering, besieging or fighting: one marker per realm
    and place (a province's seat, or along the way it is on), in the realm's colour, larger with
    more soldiers, ships as hulls; several realms at one seat sit side by side. Guards and recruits
    standing idle at home aren't drawn (in a busy year on a large world they are 60–85% of field
    companies; the province page lists them). On the march a marker glides along the way by the
    days of it left (a group at its slowest member's pace; ships by sea, land troops by land where
    a land way exists), smoothly between days, with a dotted line on to where it is going.
    Hovering a marker names its realm, its soldiers by kind, and where it is or is going.
  - "battles": crossed swords where a battle is fought on the day shown (by Warfare's own rule,
    `sim::warfare::battle_provinces`: land soldiers of realms at war standing together), and a
    mark fading over a month where one ended (only battles before the day shown).
  - Worked out once for each day shown, from the army fields it is sent; drawn over the map by the
    window's painter, on each copy of the repeating rectangle.
  - The Wars section of the side panel: every war going on (who against whom, why and since when,
    what it was declared over, soldiers and battles each side lost, provinces each holds of the
    other), each side a button to its capital, and the peaces of the last five years. The realm
    page's wars say what each side has lost.
- **Sieges, supply, winter and battles** (Oct 18; Warfare's parts, each shown only where it runs):
  - "winter" (the winter part) shades provinces pale ice to deeper blue by how deep the winter is
    on the day shown, worked out by Warfare's own rule (`sim::logistics::winter`) with the settings
    in force that day, so it shows exactly where marches slow, soldiers away from home die of
    cold, foragers find less and no new attack begins. Hovering says how deep, as a share of the
    deepest.
  - A besieged province says by whom on its page, on hover, and in its values (no longer a raw
    number where nobody besieges it); a besieging company says for how many days.
  - With supply lines on, a company on the province page and an army marker's hover say whether it
    is fed from home along a supply line or foraging (a marker by its worst-off company).
  - Battles: where one is fought on the day shown, the province page lists each side with its
    soldiers by kind (`armies::battle_sides`, by Warfare's own rule); "Battles fought here" lists
    the earlier ones from the log. A battle's report now gives each side's dead ("Dreinmark beat
    Tholia in battle here: 3,100 killed (Dreinmark lost 900, Tholia 2,200)").
- **The battle inspector and soldiers lost** (Oct 18; only where armies are kept):
  - A battle going on: the province page says since when it is fought, and for each side, besides
    its soldiers by kind, what it brought (those there when it began and those who joined since),
    what it has lost in all and on its last day, how many more it can lose before it breaks (or
    that it broke and falls back), and its strength (soldiers times what each counts for, by kind
    and by what its realm knows), with the edge for fighting on its own land. Read from Warfare's
    record of the battles going on (`sim::warfare::BattleRecord`, sent with each day shown, so
    going back shows the battles of that day); a world just opened has none until its next day of
    battle, and then shows only the soldiers by kind.
  - Earlier battles' reports also say how many soldiers each side brought and when the battle
    began ("Dreinmark lost 900 of 4,000, Tholia 2,200 of 3,500; it began 12 Thaw 31").
  - The Wars section lists the battles of the last three years (the days the log keeps), latest
    first, each with a button to its province; and "Soldiers lost": every realm's soldiers killed
    in battle (or lost at sea), dead of camp disease and cold, starved and deserted, since the
    world began, this year so far, and year by year as a table (from the years' summaries).
  - A realm's page says what it has lost since it began, by the same four.
- **Cultures** (Oct 18; only where Culture runs):
  - "cultures" colours each province by its main culture (the one most of its people belong to),
    a hue from the culture's number (`layers::culture_color`, spread round the wheel otherwise than
    realms'), paler the more mixed the province: faint where the main culture holds under half the
    people, full where it is all there is. Worked out once for each day shown, only while the layer
    is on (`layers::SocietyView`, from `sim::culture::mix`); never blended between days.
  - The province page lists its cultures by share ("Rayir 82%, Khetu 15%, 2 more 3%"), and pops
    name their culture. Hovering says the same.
  - The realm page gives the realm's culture (its capital's) and the share of its people of other
    cultures, which is what stirs unrest from foreign rule.
  - The Cultures section of the side panel: every living culture, most people first, with its
    people and share of the world, how many provinces it is the main culture of, and whether it
    was there from the start or split off from another (and when); a button goes to the province
    where it began.
  - Cultures are named everywhere (`sim::culture::names`, from the seed), the event log's births,
    splits, ends and changes of a province's main culture included.
- **Research** (Oct 18; only where Research runs):
  - "techs known" shades each province by how many techs its realm knows: pale for the least
    learned living realm, deep blue-green for the most learned (stretched between the two, since
    most realms know nearly as many as each other); clear where no realm owns the land. Hovering
    says how many.
  - The realm page says how many of the techs it knows, what it is studying (study banked against
    the tech's cost), and what its techs do (harvests, goods made, soldiers' and ships' strength
    in per cent; sieges shorter and unrest rising more slowly), from the research tree in force;
    "Techs known" lists them by field with the day each was learned.
  - Techs are named in the log ("Rovaerane learned iron working").
- **Faiths** (Oct 18; only where Religion runs):
  - "faiths" colours each province by its main faith, paler the more mixed, as the culture map
    (`layers::faith_color`, a third spread of hues so faiths, cultures and realms seldom look
    alike).
  - Faiths are named everywhere (`sim::religion::names`): the log's new faiths, breakaways, ends and
    changes of a province's main faith, the pops on the province page, the province's faiths by
    share on its page and on hover. The realm page gives the realm's faith (its capital's) and the
    share of its people following other faiths, which stirs unrest from foreign rule.
  - The Faiths section of the side panel: every living faith, most followers first, with its share
    of the world, how many provinces it is the main faith of, and how it began, with a button to
    the province where it began (the same code as the Cultures section).
- **Wild land** (Oct 18; only where Natural world runs): "wild" with a picker for woods, fish or
  game (only the parts that run): its own colour (green, blue, olive) where untouched, through pale
  to bare brown where it is all gone; clear where a province has none of it (no woodland, or no
  coast or river for fish). Hovering says what is left of each; the province page gives each as
  "36% of its wild woodland cleared" (it read in millionths).
- **Plagues** (Oct 18; only where Disease runs, on by default since Oct 19):
  - "ill with plague": pale rose where the first few fall ill (1 in 10,000) to deep red where a
    tenth are, on a log scale, since a plague starts at about 1 in 2,000; watch it play to see a
    plague spread. "plague dead": each province's plague dead since the world began against its
    people (now and those dead), ash grey at 1 in 10,000 to near black at a quarter, so the marks
    of old plagues stay (the simulation keeps the count for it: `province.plague_dead_total`, the
    user's choice; the yearly count clears each January).
  - Plagues are named ("the Red Fever", `sim::disease::name`) in the log, the province's values and
    on hover; the province page opens with the plague going on there, if any, and the share ill,
    and a "start a plague here" button (a poke; refused where nobody lives). Sick and immune read
    as percentages.
- **Followed people** (Oct 19; only where Lives runs, off by default): a "People followed"
  section lists everyone followed, what they are now ("a master smith, 34", "a soldier of
  Tholia, 22", "died of plague aged 51") and their story, latest first, with a button to where
  they are. A small gold diamond marks each living one on the map (at home, or with their company);
  hover it for who it is (layer "followed people", `--layer people=0`). The province page names
  those followed there and has a "follow someone here" button (an edit, `Edit::Follow`; refused
  where nobody lives). `--open-section people` opens the section.
- **Editing realms, rulers and armies** (step 7's editor; `editor.rs`, `realm_tools.rs`; only
  what the world keeps is offered):
  - The "realms" tool in the Edit section: click provinces to pick them (again to put one back;
    they are ringed on the map), then give them to a realm picked from a list, to the selected
    province's realm or to nobody, make them a new realm at peace, or have them break away at war
    with their realm.
  - "Change …" on the realm page: set the treasury (under markets); set what it and another realm
    think of each other (Diplomacy "treaties"); declare war on a realm, and make peace with each
    enemy, won by either or by nobody; merge it into another realm, or dissolve it. Its ruler and
    heir: set their traits, or kill them. Crown a new ruler of a chosen age and traits.
  - "Armies here" on the province page: the companies standing in it or coming in (kind, soldiers,
    realm, guards or field, home, where they are going); change one's soldiers or kind; place
    soldiers of a realm and kind (in peace, soldiers standing at home go home at the next month's
    start); and "send X's field companies here to…", then a click on the map where they are to go.
  - Every change goes through the command log as other edits and pokes do: undo, redo, going back,
    timelines and scenario files. A poke the world turns down says why in the event log.
- **Modules** (step 6): the module versions running, and what the combination tester found for
  that mix (`crates/sim/data/compatibility.toml`): no problems found, known quirks, or unstable,
  with each finding listed. A mix the table doesn't hold gets the worst verdict of its pairs. The
  verdict comes from small test worlds (200 provinces, 20 years, default settings), whatever world
  is shown, so green means "no problems found", not "proven safe".
- **Settings** (Oct 17): the settings the world runs with on the day shown that may change while
  it runs, module by module, each with its value, range, unit and what it does in words, and a
  button back to its default. Changes are gathered and applied together ("apply N changes") from
  the day shown on, as an edit: undo takes them back, and on an earlier day they start a new
  timeline. The Modules section says how many settings differ from the defaults, since the
  combination tester ran only the defaults.
- **New world** (step 6): pick a size, a seed, the years to run before showing it, and a version
  for each module, realms, rulers, diplomacy ("wars" or "treaties") and armies included (all on
  by default since step 7's checks; picking one switches on what it needs, and switching one off
  switches off what needs it, as realms with people); what the combination tester found for those versions
  shows as they are picked, and a mix it refuses can't be made. Its "settings" holds every
  setting of the world to make, as the Settings section does, those that shape the map or the
  start ("fixed once made") too, and the parts that switch on and off (trade, unrest, realms
  choosing war and peace). It starts from the world shown, with its settings of the day shown. Making one asks first, since it replaces the world shown and
  all its timelines; the world is generated and run on another thread while the old one stays up.
  If the simulation refuses it anyway, the window says why and keeps the old world.

Not yet: names written on the map (realms have names now, but only the inspector shows them) and mipmaps (not needed at current zoom limits).

## Running

```
cargo run --release --manifest-path M:\claud_projects\fantasymap\Cargo.toml -p app --bin fantasymap
```

Drag to pan, scroll to zoom, click a province to inspect it. Options:

- `--size small|medium|large|huge`, `--seed S`, `--n FACE`, `--provinces N`: the world (default: large, seed 7).
- `--terrain null|noise`, `--climate null|latitude`: module versions (default: noise, latitude).
- `--modules SLOT=VERSION,...`: any module versions, e.g. `weather=off` or
  `realms=off,characters=off,diplomacy=off,warfare=off` (slots: terrain, climate, economics,
  population, weather, realms, characters, diplomacy, warfare). Realms, rulers, realms choosing
  war and peace, and armies are on by default since step 7's checks.
- `--realms settled=W,city=W,frontier=W`, `--rulers`, `--wars`, `--armies`, `--treaties`: realms
  with the ways to start weighted as given, and the others as in `simrun` (`--wars`: wars only by
  pokes and rebels).
- `--years Y`: how long to run before showing it (default 20).
- `--drought best|LON,LAT,RADIUS_KM,PERCENT,YEARS`: start a drought after those years. `best` centres it
  on the province that grows the most food (2,000 km radius, 70% of the rain, 3 years).
- `--days D`: run D more days before showing it.
- `--select best|P`: open the inspector on a province; `--explain`: explain its last change.
- `--speed DAYS_PER_SECOND|max`: start playing.
- `--projection oval|rect`.
- `--view LON,LAT,ZOOM`: start view, in degrees; zoom 1 means the planet's height fits.
- `--layer NAME=VALUE`, repeatable: `terrain`, `temperature`, `rainfall`, `population`, `famine`,
  `drought`, `provinces`, `prices`, `trade`, `balance`, `realms`, `unrest`, `cultures`, `faiths`, `techs`, `winter`, `wild`, `sick`,
  `plague_dead` (0..1), and `coasts`,
  `rivers`, `borders`, `graticule`, `roads`, `realm_borders`, `armies`, `battles` (0 or 1).
- `--good NAME`: the good the price map shows (default: the first, grain).
- `--wild woods|fish|game`: what the wild land map shows (default woods).
- `--plague best|P`: a plague poked after the first years, with `--drought` (`best`: the province
  with the most people); it needs Disease (on by default since Oct 19).
- `--open FILE`: show a saved world (a scenario `.toml`, or a save) instead of making one.
- `--remake SIZE,SEED,YEARS[,SLOT=VERSION,...]`: once the window is open, make a new world through
  the New world section, as a player would (versions not given stay as they are).
- `--paint LON,LAT,RADIUS_KM,METRES`, `--split best|P`, `--undo`: once the window is open, in the
  order given, raise land along a short stroke, split half a province off (and select the new one),
  or take back the last change, as a player would (for scripted screenshots; `--split best --undo`
  checks that the window copes with provinces going away).
- `--part KEY=0|1`: switch a part of a module off or on in the world made, before `--set` (keys as
  `simrun --settings` lists the parts, e.g. `--part modules.warfare.seasons=1`).
- `--set KEY=VALUE`: change a setting of the world made (repeatable; keys as `simrun --settings`
  lists them). `--live-set KEY=VALUE`: once the window is open, change a setting while the world
  runs, as a player would. `--open-section settings|new-world|new-world-settings|change-realm|wars|battles|soldiers-lost|cultures|faiths|battle`: open that
  section (for scripted screenshots).
- `--screenshot FILE.png`: open, save a screenshot (after any scripted edits and the redrawn map),
  close.
- `--bench --out DIR --tag NAME`: a scripted full turn of the world, then a zoom to 8×. It writes a
  screenshot and a JSON file of frame times. Set `WGPU_POWER_PREF=low` (integrated GPU) or `high` first.

Tests, including one that renders cell ids on the GPU and checks them against the CPU picking code
(it skips if there is no GPU), and one that drives the simulation thread without a window: it stays
within its lookahead, and a poke after it ran ahead gives exactly the future a fresh world gets; an
edit comes back on the day shown with its new map, undo and redo give the maps before and after,
a change in the past starts a timeline and leaves the old one as it was, going back and forth
gives the same days, and a world saved from the window opens again as it was. One builds every
realm's page and every log line from the window's copy of the world after 50 years of successions,
breakaways and a war, given only what the simulation sends each day: everyone is there and named,
and no "none" shows as a raw number. Another does the same for techs and cultures after 60 years
with Research, Culture and Religion on (no tech, culture or faith by its bare number on realm pages,
in the Cultures and Faiths sections, province pages or the log), and checks the culture, faith and
techs layers have a value for every province with people or an owner, and the wild land layer for
every province with some of it. Another pokes a plague with Disease on: plagues are named in the
log and the province values, and the plague layers show who is ill and who died.

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
- **Re-checked after the step-6 window piece (Oct 15):** one round, oval map, integrated GPU.
  Default layers: pan 8.7 ms, zoom 11.3 ms (median), within the earlier numbers. With the markets
  economy and both trade layers on (lines and tint): pan 9.7 ms, zoom 12.0 ms, about 1 ms more.
  The static line buffer grew from about 315,000 to 385,000 segments with the roads and sea lanes,
  which are drawn only when switched on. Raw results: `bench/results/step6_window_*.json`.
- **Re-checked after step 7's window part 1 (Oct 16):** three rounds, oval map, integrated GPU,
  large world (seed 7) with realms, rulers and wars after 40 years under Economics v0. Realm layers
  off: pan 9.1–9.2 ms, zoom 11.6 ms (median). Realm colours and borders between realms on: pan
  11.0–11.7 ms, zoom 13.1–13.6 ms, about 2 ms more, still 75–90 frames a second. One round of
  each was disturbed by other work on the machine (frames of 100–400 ms, `off_2` and `on_1`) and
  is left out of those ranges. Raw results: `bench/results/step7_window_*.json`.
- **Re-checked after step 7's window part 2 (Oct 17):** one round each, oval map, integrated GPU,
  large world (seed 7) under Economics v0 with realms, rulers, armies and "treaties" after 6 years
  (14 wars going on), realm colours at 0.45. Armies and battles off: pan 10.2 ms, zoom 12.6 ms
  (median); on: pan 9.7 ms, zoom 12.3 ms. No measurable cost. Raw results:
  `bench/results/step7_armies_*.json`.
- **After the first wave's window (Oct 18): not yet measured.** The culture, techs and winter
  layers add three table reads per pixel whenever a province layer is on. The bench run that day
  was disturbed by other work on the machine (25–130 ms a frame even for the default layers, about
  9 ms before), so it waits for a quiet machine.
- **The RTX 5090 numbers are not informative.** That day it showed a flat 15.6 ms (about 64 frames a
  second) for this app *and* for the renderer trial's program, so something capped it.
