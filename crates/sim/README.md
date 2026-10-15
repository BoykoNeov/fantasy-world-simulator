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
  reach the requester the next day. A request on stored goods names a node of the goods tree in a
  province ("food in province 12"), never one store: the owner serves it from all the goods it
  keeps under that node, as one pool (`serve_goods`), so the requester works whichever goods the
  economics version keeps. Modules list the goods they ask for in their manifest, and a
  configuration whose goods can't serve one is refused.
- **Events** (`events.rs`): sent on day N, delivered on day N+1, in a documented priority order.
- **The engine** (`sim.rs`): the daily clock. After every stage it checks that people and each
  class of goods changed only by what was reported, and clamps values that leave their bounds; both kinds of
  problem are counted and sent out as events. In strict mode (tests) a module that writes a field
  it doesn't own stops the run.
- **Saves** (`save.rs`): configuration, module versions and state schemas, the command log, and a
  snapshot with every field by name (a per-good field also names the goods it holds) and
  everything in flight. Format 6 (step 6) added the goods tree and the per-good store; format 7
  the good a change request asks for; format 8 the edit that sets any good; format 9 the land facts;
  format 10 the markets economy (its parameters, prices, workers and four new causes: made, made
  into other goods, used up, worn out); format 11 trade (its parameters, and the causes trade in and
  trade out); format 12 trade's prices following the money. Loads refuse other format versions,
  other module versions and damaged files with a message.
- **Modules and their versions** (`config.rs` chooses them):

  | Slot | Versions | Runs |
  | --- | --- | --- |
  | Terrain | `noise` (default), `null`: smooth landmasses, same for every seed | at generation |
  | Climate | `latitude` (default), `null`: 15 °C and 800 mm everywhere | at generation |
  | Biomes, Rivers, Fertility, Provinces | one each | at generation |
  | Economics | `food` (default): monthly harvest and spoilage, keeps one store per province of the "food" class; `markets`: the eight goods and coins, made by workers from the land, used up and priced (see below); or off | monthly |
  | Population | `basic`: asks for a month's food (the tree's "food", however it is kept), then is born or starves on the grant; or off | monthly |
  | Weather | `droughts`: owns each province's rain shortfall; droughts start with a poke; or off | monthly |

  Harvests shrink in proportion to the missing rain, so a drought cuts the crop. Markets needs
  people to do its work: with Population off it is refused when the world is made.
- **Goods** (`taxonomy.rs`, `data/goods.toml`, `fields.rs`): the goods tree is data: food (grain,
  fish, livestock), raw (timber, stone, ore), manufactured (cloth, tools) and money (coin, counted
  in coins; raw goods are counted in cartloads, cloth in bolts, tools in sets). Coins are stored and counted like any good: a class of their own, so they are never
  made or lost unannounced, and edits set and move them like goods. It is part of the
  configuration, so saves and scenarios carry it; a scenario without one gets the shipped tree.
  Stored goods are one field, `province.stock`, with a value per province *per good kept*. The
  economics version chooses which nodes of the tree it keeps (Economics v0 keeps one: "food" as a
  whole); with no economics, readers find each class, empty. People and each top-level class of
  goods are counted separately and must balance on their own, so goods can't turn into other goods
  unannounced. A clamp is booked to the class it hit. Economics v0 gives exactly the same worlds as
  before goods had a tree (same fingerprints). Readers ask the tree, not a store: a class reads as
  the sum of its goods (`World::stock_under`), and a good kept only inside a bigger whole (grain,
  when food is kept whole) can't be read on its own.
- **Land facts** (`provinces.rs`): each province caches what its land offers, worked out with its
  other aggregates at generation and after province and height edits: food better from herds than
  fields (`province.grazing_capacity`, by biome), woodland, shore, big rivers, hills and mountains
  (500 m and up), and land with ore in reach (mostly uplands, more where the province lies on one
  of the smooth veins of ore a few provinces across). The markets economy reads these, never cells.
- **Economics v1, "markets"** (`markets.rs`, `data/markets.toml`): keeps grain, fish, livestock,
  timber, stone, ore, cloth, tools and coins, and owns each province's price per good
  (`province.price`, thousandths of a coin) and workers per good (`province.workers`). Once a
  month, in each province:
  1. *Prices* follow the stores: price = base × (stores wanted / stores held), within 1/20 and 20
     times the base, moving half way each month. Food goods are priced by their stores together,
     since people eat any of them; stores wanted reach to the next harvest far from the equator.
  2. *Work* follows what pays: each good is worth its price, less its inputs' cost, for what one
     more worker would make of it. Each worker on a piece of land adds less than the one before,
     so work spreads until one more worker is worth the same everywhere; nobody takes work worth
     less than resting (20 coins a year; a worker's year at base prices is worth about 200).
     Workers move a fifth of the way each month. This is the damper on every good: one piling up
     gets cheap and loses its workers.
  3. *Making*: grain comes with the harvest, and grain and livestock with the rain and the year's
     luck, as in v0. Tools (one per worker) make workers half as productive again, and wear out.
     Tools take a cartload of ore and of timber each.
  4. *Using*: people use up timber, stone and cloth (less when they are dear); food is eaten through
     Population's requests, served from all three food goods. Stores rot or wear.

  5. *Trade*, after every province's month: along each way between neighbours, a good goes to the
     end where it is dearer when the gap in price beats the cost of carrying it there. A cart
     doubles grain's price in about five days; a day at sea costs a tenth of a day by land, and the
     cheaper way is taken; cloth and tools are worth a lot for their weight. The buyer pays the
     seller in coins: the seller's price and the carrying. Each way closes half the gap (beyond the
     carrying) a month: the damper on trade. Where several ways serve one store, each closes only
     its part. A province sells at most half a store and spends at most half its coins in a month,
     so a poor province can't buy much. Food is traded as food, taken from the seller's grain, fish
     and livestock as it holds them. Every trade is worked out from the stores as the month's work
     left them, then scaled to what sellers can spare and buyers can pay, so the order of the ways
     doesn't matter. Goods arrive the same month (nothing is on the road). Each year's flows count
     what came in and went out by trade (the two cancel). Asked why a province changed, the month's
     first day says what it bought and sold, from and to whom, at what prices. The month's trade
     is kept for showing it (`markets::TradeRecord`, `World::trade`): each good sent along each
     way and what was paid for it. It sits beside the fields, so it is not saved, not in the
     fingerprint, and read by no module; checkpoints in memory carry it, so going back shows the
     trade of that time, and a world just opened has none until a month's trade runs. On the
     large world it holds 25,000–32,000 shipments, 0.8–1 MB in each yearly checkpoint (about a
     third of what markets' checkpoints already cost); `examples/market_stats.rs` prints it.

  Coins (the goods under the money class, found through the tree) are never made, used or priced:
  each person starts with 20, held by the province, and trade moves them. How each good is made,
  used, priced and carried is data, saved with the world; bad parameters are refused when the world
  is made. A configuration without the trade table (scenario files written before trade) runs with
  no trade, as it did.
  Work and prices are settled before day one, so the first month starves nobody. Provinces work
  side by side on all cores; each one's random draws are its own, so the result never depends on
  how they are shared out. Shortage of anything but food and tools hurts nobody yet (planned:
  "everything matters" later). Before trade, a small world settled near the old economy's
  population over a century (136–146 million against 142–152 on seed 1), with famines of the same
  size. Trade cuts starvation at first: over 15 years about a third on the small world (18.5 million
  starved against 28.2 million on seed 1) and a fifth on the large one (19.2 against 24.0 million on
  seed 7), with fewer famine provinces every year. Bad harvests strike whole regions, and goods
  cross one border a month, so famines remain. On the small world provinces are about 1,000 km
  across, weeks apart by cart, so grain goes mostly by sea there.

  **Prices follow the money.** Without it, coins pool: they flow to the provinces that sell food
  and nothing brings them back, so buying provinces run dry and stop buying. On the large world
  2,770 of about 5,000 provinces fell under a coin a person within a century, trade shrank to a
  fifth, and trade stopped cutting starvation. So, to trade, everything in a province is dearer by
  its coins a person against the world's (`money_response` 1: twice the coins, twice the price),
  within a third and three times (`money_range`). Where coins pile up goods are dear, so the
  province buys more and sells less, and coins flow back: the price–specie flow of real metal
  money. The level is set at the start of the month's trade and holds through it, since it
  follows wealth over months, not one month's buying (a starving province paying for food doesn't
  want food less). Stored prices leave it out (inside a province everything scales with it, so
  nothing there changes); the window shows it beside them, from the function trade uses
  (`markets::money_levels`, against the average over inhabited provinces).

  Over a century on the large world (seed 7): starved per 1,000 people a year 9.44 without trade,
  9.18 with trade alone, 8.86 with prices following money; on average 129.5, 133.4 and 135.9
  million people. At year 100, 209 provinces are under a coin a person (2,770 without the money
  effect) and the richest tenth hold 37% of the coins (65%); trade keeps its volume. The first 15
  years starve 24.0, 19.2 and 19.9 million (the large world was run on one seed). On the small
  world the order is the same on seeds 1, 2 and 3: 9.05, 8.45, 8.15; 9.13, 8.47, 7.88; 9.26, 8.89,
  8.12 per 1,000 a year, with the most people under the money effect each time. On seed 1 a
  stronger response (2) did a little worse, and a wider range (5) about the same. Two provinces alike but for coins even them out
  month by month, never overshooting, with each good going one way only (a unit test).
  `examples/market_stats.rs` prints a run year by year, with what was traded and where the coins
  are (`--no-trade` turns trade off, `--money 1,3` sets the response and range).
- **Pokes** (`Sim::poke`): a player's action is an ordinary event, stored in the command log with its
  day. It is sent that day and arrives the next, so replays and saves repeat it. The only poke so
  far is a drought: a centre, a radius, how much rain goes missing, and for how long.
- **Edits** (`edit.rs`, `Sim::edit`): an edit sets values directly instead of letting the world
  react (so far: a pop's size and what it is, a new pop, a province's store of any good the world
  keeps on its own, and reshaping provinces: moving land between them, splitting one, merging two).
  An edit that sets something no active module keeps (people with Population off, stored goods
  with Economics off) is refused, as is setting a class kept as several goods ("food is kept as
  grain and fish; set each of those"). `SetFoodStock`, the older food-only edit, still loads from
  scenario files and means setting the "food" store. Edits go into the
  command log like pokes and take effect at the start of their day, so the world shown for that day
  already has them. People and food an edit creates or removes are counted as the edit's own cause,
  so every year still balances. `Sim::withdraw` takes a command (an edit or a poke) back:
  the world goes back to the last checkpoint before it and runs again to the same day without it,
  giving exactly the world that never had it. `Sim::reinstate` gives it again. An edit that no
  longer fits when its day comes (it changed a pop that a withdrawn edit had added) is left out and
  listed in `Sim::skipped_edits`.
- **Reshaping provinces** (`edit.rs`): when land changes province, its people and every good it
  stores go with it, in proportion to the farmland that moved (by area where there is no
  farmland), in whole people and units by the largest-remainder rule; people join a pop of their own kind there or make one.
  Province numbers never change: a province merged away stays as an empty placeholder that keeps
  its centre, and a split's new province is numbered after the last. Pops are only ever added, so
  food on its way to a pop still finds it.
- **Province neighbours and roads** (`routes.rs`, `World::routes`): which provinces border each
  other and how many days it takes to travel between them. They are neighbours by land when they
  share a cell side, and by sea when their coasts face each other across water (lakes below sea
  level count as water). Ships go at most 400 km out from their own shore, so straits and inland
  seas are crossed but open oceans aren't. A pair can be both. A way runs from one province's seat
  (its own cell nearest its centre) to the other's. Over land, a laden cart covers 30 km a day on
  the flat, each kilometre of climbing or descending costs another day, and getting onto or off a
  big river costs half a day (so crossing one costs a day). The best crossing of the shared border
  counts. By sea, a ship covers 100 km a day, plus a day in port at each end. Each way costs the
  same in both directions. Roads are fixed by the land for now. The list is worked out from the
  map, not saved: at world creation, after a load, and when a map edit is first made. Each map
  change keeps what it did to the list (the links it changed and the seats that moved), as it
  keeps the values it changed, so going back or forward past it, undoing it or replaying it
  doesn't work the list out again. A timeline copy shares it. If an edit cuts a province in two,
  the way to its far piece goes over the neighbours' land at half speed (a piece left on another
  island has none). Only a one-province island far out at sea has no neighbour. Trade (in the
  markets economy) goes along these ways. Working it out takes 0.11–0.23 s on a large world (on all
  cores, depending on how busy the machine is), once per new map edit, never per day; there a
  province has six neighbours at the median, and the median trip is about 7 days by land and 9 by
  sea. After the sea rises 250 m, 310 provinces are in pieces and it takes 0.35 s
  (`examples/route_stats.rs`). Worlds are the same as before: same fingerprints, same tester
  table, same save format.
- **Painting the land** (`edit.rs`): a brush stroke raises, lowers, smooths, roughens or flattens
  the land along a path (full strength in the brush's middle half, fading to nothing at its edge),
  and the sea can rise or fall everywhere. Flattening below 0 m makes sea, so an inland sea or a
  lake at sea level is painted this way. Afterwards climate, biomes, rivers and farmland are worked
  out again by the world's own module versions; provinces are not regrown. Land that sinks leaves
  its province; new land joins the province next to it, spreading from the old coast; a new island
  that touches no province becomes one. A province whose land all sinks sends its people and goods
  to the nearest province with land. Only the downstream links of the rivers are stored now, not the
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
- **Story 1 on markets** (`tests/stories.rs`): the same world and drought with Economics "markets".
  The famine still comes (a quarter of the region starves in the worst year) and ends with the rain.
  The region buys food from provinces outside it every drought year: 274 million rations over the
  three years, against 36 million without the drought. Its local food price is about six times
  normal (1.09 coins a ration against 0.17). Trade helps only a little: food crosses one border a
  month, and carrying it across this Earth-sized world's big provinces is dear, so it covers a few
  percent of what the region eats, and 5% fewer people starve than with no trade.
- **Old scenario files** (`tests/old_scenarios.rs`): files written by older builds, kept with what
  each build printed (every province's people, starved, stores, prices and workers, per timeline).
  Opened today they make the same worlds, value for value: one from the end of step 5 (Economics
  "food", an edit of every kind, two droughts, two timelines), one from markets before trade, and
  one from trade before prices followed the money.

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

The combination tester (about 25–30 s on all cores for its 48 mixes; run it at below-normal priority). `--write` saves
the table, `--check` runs again what the table lists and fails if anything came out differently,
`--only economics=off,population=off` runs one mix, and `--out DIR` writes, for each run that
wasn't green, a scenario that replays it up to just after its first problem. Replays are not
strict, so a crash from a module writing a value it doesn't own does not happen again in them (the
other problems do):

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

**Markets** (Oct 15, below-normal priority, machine partly busy): on the large world (5,092
provinces, seeds 1 and 7, 10 years) an average day takes 0.34–0.42 ms against 0.24 ms for Economics
"food" (one busy-machine run measured 2.2 ms). Making the world takes about 0.5 s more, for
settling work and prices. Trade (Oct 15, timed a month at a time against the same world without
it, so both saw the same load on a busy machine) adds about 0.15 ms a day: 0.48–0.59 ms against
0.34–0.44.

**Re-checked** (Oct 15, end of step 6). Other projects' tests kept all 16 cores busy, so clock time
was meaningless (markets 3–6 ms a day, v0 0.25–0.32 ms). Counted instead as work on one core
(`RAYON_NUM_THREADS=1`, the process's processor time for 10 years less that for none, large world,
seeds 1 and 7): a day costs 0.2–0.3 ms for Economics "food", about 1.6 ms for markets without trade
and 2.3 ms with it. Markets keeps to the 1.5 ms budget only by working provinces on several cores;
on one free core it would not. Clock time on a quiet machine still to be re-measured.

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

With Economics "markets" (Oct 15, counted, not measured) a province keeps 27 running per-good
values instead of 1 (stores, prices and workers for 9 goods), each with its 8-byte stamp: about
2.7 MB a yearly checkpoint on the large world instead of 0.58 MB, so roughly 0.27 GB a century and,
on a huge world, about 11 GB a millennium. A known cost, to trim with the stamps when long runs
need it.

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

Re-measured Oct 15 after province neighbours (step 6, piece 5), with the machine busier than on
Oct 14, so the run before the change was repeated alongside it:

| | Before | After |
| --- | --- | --- |
| Brush stroke / sea rises 50 m / split | 374 / 376 / 53 ms | 400 / 346 / 162 ms |
| Painting while paused, stroke 1 → 200: taking it back | 47 → 108 ms | 33 → 64 ms |
| Same, explaining the day | 70 → 139 ms | 49 → 93 ms |
| Go back 250 days past all three | 31 ms | 22 ms |
| Map changes kept (200 strokes, the sea, the split) | 44.8 MB | 45.2 MB |

A new map edit now also works out the neighbours (about 0.1 s); going back and replaying reuse
what each change kept. A first version worked them out again at every replayed map edit: taking
back the 50th stroke of a paused day took 17 s. Keeping a whole copy with each change instead cost
0.56 MB per stroke.

Step 1 measured 2.5 s a century. The rest goes to the request and event passes and the checks
after every stage. There's still very little simulation, so these figures are a floor, not a
forecast.
