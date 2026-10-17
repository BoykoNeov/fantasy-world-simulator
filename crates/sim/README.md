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
- **Settings** (`settings.rs`, Oct 17): every number of every module's parameters, and of the world
  (map detail, provinces, land share, planet radius), described in plain words with its range,
  unit, and whether it may change while the world runs or only when a world is made. Values are
  read and set by their key in the configuration (`modules.markets.trade.change`), through its
  serialised form, so a new parameter needs only its line there; what isn't a setting is listed
  with the reason, and a test fails for any value in neither list (the rule is in the project's
  `CLAUDE.md`). The map-making slots have settings of their own (terrain, climate, biome limits,
  evaporation, food per km² of each biome, how provinces are divided), defaults equal to the
  numbers they had, so old scenario files make the same worlds. Realms gained the largest
  city-state and a number of realms to start with (about that many, plus one for each island none
  reaches; 0 keeps "provinces per realm"). A live change is an edit (`Edit::Settings`): it goes into
  the command log, and the engine makes the modules again with the new settings, each keeping its
  state; going back, taking it back, branching, saves (format 22) and scenario files all keep the
  settings of each day (`Sim::settings`; `Sim::config` stays the configuration the world was made
  with). Tests in `tests/settings.rs`.
- **The engine** (`sim.rs`): the daily clock. After every stage it checks that people and each
  class of goods changed only by what was reported (summing those held in fields the stage's
  modules own; the others, which they can't change, must have no flows reported), and clamps values
  that leave their bounds; both kinds of problem are counted and sent out as events. In strict
  mode (tests) a module that writes a field it doesn't own stops the run. A module may tell the
  engine it is idle on a day its cadence is due (`Module::idle`); its stage is then skipped.
- **Saves** (`save.rs`): configuration, module versions and state schemas, the command log, and a
  snapshot with every field by name (a per-good field also names the goods it holds) and
  everything in flight. Format 6 (step 6) added the goods tree and the per-good store; format 7
  the good a change request asks for; format 8 the edit that sets any good; format 9 the land facts;
  format 10 the markets economy (its parameters, prices, workers and four new causes: made, made
  into other goods, used up, worn out); format 11 trade (its parameters, and the causes trade in and
  trade out); format 12 trade's prices following the money; format 13 the step-7 groundwork (who
owns each province, realms and their stores, armies, four new slots, requests that move what they
take, and the causes and event that come with them); format 14 the realms module's parameters in
the configuration; format 15 rulers (the characters module's parameters, the character fields,
each realm's ruler and heir, and the succession event); format 16 wars (the diplomacy module's
parameters, the war fields, occupation, and the events of war and peace); format 17 taxes; format 18
unrest (its parameters, unrest, uprisings and the realm each broke away from, the realms module's
private state with each province's hunger this month, the events of uprisings and rebellions, and
a peace saying whether it ended a rebellion); format 19 troops (the warfare module's parameters
with the troop tree, each army's kind and home, and the causes deserted, equipped and bounty);
format 20 armies moving and fighting (each company's role, group, destination, next province, days
left on its way and siege days, the warfare module's new private state, the battle event, the cause
killed and the year's killed in the ledger).
Loads refuse other format versions,
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
  | Realms | `basic`: makes the starting realms and claims unowned settled land, taxes, unrest and rebellion (see below); or off (the default until step 7's checks) | monthly |
  | Characters | `rulers`: every realm's ruler and heir, who are born, age and die (see below); needs realms; or off (the default until step 7's checks) | monthly |
  | Diplomacy | `wars`: wars, peace and truces between realms, by pokes; a rebellion's war ends when the old realm takes the rebels' capital (see below); needs realms; `treaties`: the same, and realms choose war and make peace themselves, with opinions of each other (see below); needs realms and armies; or off (the default until step 7's checks) | monthly, and on pokes |
  | Warfare | `troops`: provinces raise soldiers, who eat where they stand and desert when hungry, march under their realm's general, fight battles and take provinces by siege (see below); needs realms; or off (the default until step 7's checks) | daily (raising and eating monthly) |

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
  far are a drought (a centre, a radius, how much rain goes missing, and for how long), and, with
  wars on, declaring war and forcing a peace (see Wars below).
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
- **Realms and armies, the groundwork** (step 7; `realms.rs`, `warfare.rs`, tests in
  `realm_tests.rs`). No module makes realms or armies yet; this is what they will stand on.
  - Each province has an owner (`province.owner`, the realm's number, or none). Realms and armies
    are entities like pops, kept as parallel arrays: a realm has a capital (`realm.capital`) and
    stores of every good the world keeps (`realm.stock`, laid out like a province's; its coins are
    its treasury); an army has a realm, the province it stands in and its soldiers (`army.realm`,
    `army.province`, `army.size`). Their numbers are never reused: a dissolved realm keeps its entry
    with no capital, a disbanded army keeps its entry with no soldiers, as a merged province does.
  - Soldiers count in the people total and treasuries in the goods totals (money among them), so
    the daily checks cover them. A realm's stores always have a row per realm: the engine adds an
    empty row when a realm appears, so whoever makes a realm needn't own its stores.
  - **Moving coins** stays with one owner. A request can ask its owner to move what it grants into
    another value it keeps instead of using it up (`Request::to`, `Ctx::ask_move`): a province's
    coins into its realm's treasury, or a treasury's coins to a province. No flow is booked, since
    nothing was made or used. A module lists each pair of fields it moves between (`moves` in its
    manifest), and a configuration where the two are kept by different modules, or hold different
    kinds of count, is refused.
  - **Moving people** goes between two owners, a day apart. A request for people names a province,
    never one pop, and the pops' owner takes them from all its pops by their sizes
    (`serve_people`). The grant books them out ("raised"); the army's owner books them in when the
    grant arrives the next day. Going home, the army's owner books them out ("returned") and sends
    an event, `Arrived`; the next day Population books them in, in the province's biggest pop (the
    nearest province with land, if that one has none). People still on the way when a year ends
    are counted in that year's summary (`Ledger::people_on_the_way`, negative), so every year
    balances: people at its end = at its start + births − deaths + clamped + edited + on the way.
  - Worlds without these modules are unchanged: no province has an owner and there are no realms
    or armies. Old scenario files make the same worlds value for value; fingerprints differ only
    because the new fields are hashed.
- **Realms at world creation** (step 7; Realms "basic", `realms.rs`, tests in
  `realm_start_tests.rs`). Off by default, and not among the tester's choices yet, so its table is
  unchanged; a configuration (`ModuleConfig::realms`), a scenario file or `simrun --realms` turns it
  on. Realms are made once the people are placed, over settled land (provinces with people), and
  need Population (refused without it).
  - **Three ways to start, mixable** (`RealmParams`, a weight for each): realms over settled land
    (the default; about one realm per 25 provinces, every settled province owned), city-states (one
    to three provinces each, all settled; every settled province owned), and a frontier (realms the same
    size but holding only the 40% of their settled land nearest their capital, the rest unclaimed).
    A mix goes by region: a large smooth pattern over the planet (the terrain's noise) splits the
    settled land into areas of each way in the shares asked for, so a coast of city-states can lie
    beside a kingdom's plains.
  - **How the land is shared out.** Starting provinces are spread out (the most populous first,
    then each time the one farthest from those chosen, favouring the more populous, shuffled a
    little), and realms grow from them by travel time along the province ways, by land and short
    sea crossings, within their own way's area; mountains and open sea become borders. Settled land
    no realm reached (a far island) starts a realm of its own. A realm holds empty land only as far
    out as its farthest settled province, so deserts and ice beyond the settled land are nobody's;
    a city-state holds none.
    Each realm starts in one piece. Its capital is its most populous province.
  - **Claims.** Each month a settled province nobody owns is claimed, with a chance of 0.5%, by the
    neighbouring realm nearest it in days (by the owners at the month's start). On the large world a
    frontier is about two-thirds claimed after 10 years and over 90% after 30.
  - **Names** come from the world's seed and the realm's number alone (`realms::names`): two or three
    syllables, sometimes with an ending ("Draetith", "Keitraemark"); no two alike. Nothing is stored,
    so realms made later (a rebellion) get names the same way.
  - **Edits.** After any edit that changes the map, `realms::repair` puts realms right: a province
    with no land left (merged away, all its land moved out, or sunk) belongs to nobody; a capital the
    realm no longer holds moves to its most populous province; a realm left with no land is
    dissolved (its number kept, with no capital). A rising sea can cut a realm in two; it stays one
    realm. Worlds without realms are untouched.
  - Speed: making the realms adds about 0.3 s to making a large world (5,069 provinces); a day costs
    the same as without realms. `simrun` prints the realms (count, sizes, unowned settled land) and,
    with `--out`, draws a realms map (realm colours, realm borders, a dark mark on each capital).
- **Rulers** (step 7; Characters "rulers", `characters.rs`, tests in `ruler_tests.rs`). Off by
  default and not among the tester's choices yet, like realms; a configuration
  (`ModuleConfig::characters`), a scenario file or `simrun --rulers` turns it on. It needs realms
  (refused without them).
  - **Characters** are entities like realms, kept as parallel arrays (`character.*` fields): the
    realm they rule, are heir to or last belonged to; their parent; a name number; the day they were
    born (below 0 for those born before the world's first day), died and were crowned; how they came
    to the throne; and three traits from 0 to 100, most often near the middle: cautious to warlike
    (`martial`), `greed` and `ability`. Each realm has a ruler and an heir (`realm.ruler`,
    `realm.heir`; none while there is none). Numbers are never reused: the dead keep their entries.
    Rulers are not extra people, so the people total doesn't count them.
  - **At the start** every realm gets a founding ruler, 18 to 65 years old (most often about 40),
    and an heir if the ruler is old enough to have a child (18 to 35 years younger). The world's
    starting rulers are not announced.
  - **Each month** everyone alive may die (`characters::death_per_year`: a child's risk early on,
    then about 2% a year at 40, 6% at 60, 14% at 70 and a third at 80; `RulerParams::mortality`
    scales it). A ruler without an heir, aged 16 to 50, has a child with a 30% chance a year, who
    becomes heir; any ruler without an heir names a grown kinsman (16 to 40) with a 10% chance a
    year. A child takes half of each trait from its parent, and is often named after its parent or
    grandparent.
  - **Succession.** When a ruler dies, the heir is crowned the same day: a smooth succession if the
    heir is grown (16 or more), a weak one if a child. With no heir a claimant from outside the line
    takes the throne: a contested succession. How each ruler came to the throne and when are kept
    on the ruler (`character.succession`, `character.crowned`) and sent as a `Succession` event
    (shown in the capital's event log), for unrest to read when it comes (a later piece of step 7);
    nothing reacts to them yet. A realm made after the start gets a founding ruler, announced, at
    the next month's start; the ruler and heir of a dissolved realm lose their places then and live
    out their lives.
  - **Names** come from the world's seed and the name number alone (`characters::name`: "Aldric",
    "Merwen"); a ruler named like an earlier ruler of the same realm gets a number
    (`characters::title`: "Aldric II").
  - On the large world (202 realms, seed 7) a century brings about 720 successions: 73% to a grown
    heir, 16% to a child, 11% contested. Reigns that end last about 23 years on average, and rulers
    die at about 58. Rulers cost nothing measurable per day. `simrun` prints these figures and the
    largest realm's ruler and heir.
  - The engine gives each new realm a ruler and heir of nobody (`World::fit_realm_rows`, as it does
    its stores), so a world with realms but no rulers saves and loads.
- **Wars** (step 7; Diplomacy "wars", `diplomacy.rs`, tests in `war_tests.rs`). Off by default and
  not among the tester's choices yet, like realms; a configuration (`ModuleConfig::diplomacy`), a
  scenario file or `simrun --wars` turns it on. It needs realms (refused without them).
  - **Wars** are entities like realms, kept as parallel arrays (`war.*` fields): the realm that
    declared it and the one it was declared on, why (`war.cause`: the player's poke, or rebels who
    broke away),
    the day it began and the day of the peace, who won (nobody, or one of the two), and the first
    day the two may fight again (`war.truce_until`). Numbers are never reused: an ended war keeps
    its entry. Two realms are at war while a war between them goes on, under a truce until its truce
    ends, and at peace otherwise (`diplomacy::standing`, `at_war`; `enemies` lists each realm's
    enemies at once, for readers that ask about many pairs).
  - **Pokes.** `DeclareWar { attacker, defender }` starts a war; it also breaks a truce between them
    (the truce then ends that day: the player may always start a war). `ForcePeace { a, b, winner }`
    ends the war between the two, won by one of them or by nobody. A poke that doesn't fit (a realm
    and itself, a realm that doesn't exist or was dissolved, two already at war, a peace with no war,
    a winner who isn't one of the two) is turned down with a `WarRefused` event saying why, in the
    event log. Pokes about the same two realms on the same day keep the player's order.
  - **The peace.** The winner keeps every province of the other's that it occupies, and every other
    occupation between the two ends: what it doesn't hold goes back, and with nobody winning
    everything goes back (the user's choice: a war is not fought over named provinces). A truce of
    5 years follows (`WarParams::truce_years`). Diplomacy sends `Peace`, and the realms module hands
    the land over when it arrives: capitals lost move, and a realm left with no land is dissolved.
    A war with a dissolved realm ends at the next month's start, won by the other side; until then
    `at_war` already counts it as over.
  - **Occupation** (`province.occupier`, the enemy realm holding a province) is kept by the realms
    module. It comes by `Occupied { province, by }` events, which warfare will send for enemies at
    war (and which tests poke in directly); `by` being the owner frees the province. Occupied land
    changes hands only at the peace. A split province keeps its occupier; one with no land, or whose
    occupier is gone, has none.
  - **Who reads whom.** The engine refuses modules that read each other's fields in a circle. Realms
    keep the land and occupation; the wars read realms; armies read both and send occupation
    back by event. So the realms module can't read the wars, and peace changes land by an event
    (a test builds this order). Diplomacy "treaties" reads stores and people too, and Warfare reads
    the wars, so Diplomacy can't read the armies (it counts battles from their events). The next
    piece, trade between realms, needs a way to read the day before's values without an order
    between the modules: markets will read owners and wars, while realms (taxes) and Diplomacy
    (wants) read markets' stores.
  - With no pokes and no unrest, worlds with wars on hold the same values as without (a test);
    with unrest, rebels who break away start wars of their own. Wars cost nothing
    measurable per day. `simrun` prints how many wars were declared, go on, were won and ended with
    nobody winning.
- **Treasury and taxes** (step 7; Realms "basic" with markets, `realms.rs`, tests in
  `treasury_tests.rs`). Under markets, realms tax coins; under Economics v0 there are none, and
  treasuries stay empty.
  - **Who keeps the coins.** Markets keeps the realms' stores (`realm.stock`; their coins are the
    treasuries) as well as the provinces', so coins keep one keeper. Realms ask it for moves:
    a province's coins into its realm's treasury (cause "taxes"), and a treasury's coins into a
    province (cause "spent"). Nothing is made or lost, and no flow is booked.
  - **Two manifests, one version.** The configuration builds the realms module to ask for coins
    only when the world keeps them (`Founding::new`, by whether markets is chosen); the other asks for
    nothing, since a manifest asking for coins in a world without them is refused. Both have the
    same version name and private state, so saves and scenario files don't depend on it.
  - **Each month** (`TaxParams`), every province a realm holds pays 1% of its coins (rounded by
    chance, so small provinces pay their share on average). The realm then spends 10% of its
    treasury as it stood that morning. The spending goes to its provinces by their people, with the
    capital's counted 3 times. Taxes are moved first, so the spending is always there. A treasury
    settles at about a tenth of its land's coins (1% ÷ 10%).
  - **Why the capital is counted by its people, not given a fixed share of the spending.** Taxes go
    by coins and spending by people, so the capital settles near 3 times its realm's coins a person
    before trade evens it out. A fixed half of the spending would settle the capital at half its
    realm's coins, however small it is; tried on the large world, one capital reached 28 times the
    world's coins a person in 50 years and was still climbing. Now, after 50 years, the middle
    capital has 1.2 times the world's coins a person and the richest 4.6 times, and treasuries hold
    9–10% of their land's coins.
  - A province an enemy occupies pays nobody and gets nothing, a capital too. A dissolved realm's
    treasury goes, at the next month's start, into the province that was last its capital (whoever
    holds it then taxes it), or the nearest with land if it sank. The realms module keeps each
    realm's last capital as private state (schema `capitals-1`, now `capitals-hunger-1`: see unrest).
  - Paying the provinces that raise troops waits for the troops piece. A province in uprising, too,
    pays nobody and gets nothing (see unrest, below).
  - Treasuries hold 7.6% of their land's coins after a year, 9.5% after two and 10.0% from the third
    (large world, seed 7). Against the same world without realms over 50 years
    (`examples/coin_spread.rs`), the middle and the top tenth of provinces are as rich either way. The
    richest 1% settle near 10 times the world's coins a person, where without realms they keep
    climbing (33 times by year 50, a drift markets already had). Rerun with unrest on (Oct 16): treasuries
    and the richest 1% are as before, but the single richest province reaches 120 times by year 50
    (28 before), probably one that paid no taxes for a while: in uprising, or newly broken away.
  - Speed: the large world's day costs about 0.3 ms more than markets alone, as CPU time on one
    worker thread (about 15%), for the roughly 10,000 coin moves a month and their replies. Not
    re-measured on all cores, since the machine was fully busy; the budget check is step 7's checks
    piece. `simrun --economics markets --realms
    settled=1` prints the treasuries' share of all coins, the largest realm's treasury against its
    land's coins, and how rich capitals are against the world (the richest named).
- **Unrest and rebellion** (step 7; Realms "basic", `realms.rs`, tests in `unrest_tests.rs`). On
  with realms (`RealmParams::unrest`); a scenario file written before it has none, so it makes the
  same world it made then.
  - **Unrest** (`province.unrest`) is kept in thousandths of the way to an uprising (the line,
    1000). Each month it fades (it halves in 2 quiet years; rounded by chance, so a little fades as
    surely as a lot) and rises with the month before's hunger: 180 for a month in which 2% or more
    of the province's people starved, in proportion for fewer. The realms module counts the starved
    from `Famine` events through the month (private state, schema `capitals-hunger-1`) and reads
    the people once at the month's start. Each month an enemy occupies it adds 25. A child heir adds
    150 in every province of the realm, a contested succession 300 (from `Succession` events; a
    grown heir or a founding adds nothing). Taxes add nothing yet: every realm taxes alike. War
    losses wait for the armies.
  - **Uprisings** (`province.uprising`, the province where it began) go by unrest as it stood at
    the month's end, before the new month's is added. A province at the line rises up; a province
    of 500 or more next to an uprising of its own realm joins it (one step a month); an uprising
    dies down below 250. A realm's capital and land an enemy occupies never rise up (occupation puts
    an uprising down; a freed province keeps its unrest). A province in uprising pays no taxes and
    gets no spending. Events: `Uprising` (begun or joined) and `Calmed`.
  - **Breaking away.** Provinces in uprising of one realm that hold together by the province ways,
    3 or more, become a new realm (`Rebellion`): its capital the most populous of them, its unrest
    spent, and `realm.broke_from` naming the realm it left. Its name comes from its number, as every
    realm's; the rulers module gives it a ruler the same day. The wars module sets it at war with
    the realm it left the day after (cause "rebellion"). On a month's first day, a rebellion's war
    in which the realm the rebels left occupies the rebels' capital ends, won by that realm (armies
    take it by siege); one still going after 5 years (`WarParams::rebel_war_years`) ends with the
    rebels winning. At a rebellion's peace (`Peace` says so), the realm they left, winning, takes back all
    their land; with the rebels or nobody winning, they stay free. A later, ordinary war between the
    two hands over only what is occupied. Without wars kept, rebels are simply free.
  - **How often** (story 1's world, `examples/unrest_check.rs`, the user's choice of speed). The
    world's own famines are as deep and as long as the story-1 drought's: about 1% of
    province-months starve more than 1% of the people, and every decade a few provinces have 7 or
    more famine months within two years (populations grow until a poor harvest empties the stores).
    So a famine's depth or length can't tell a drought from an ordinary bad patch; the setting is
    the one at which the drought region rises up within 5 years on 2 of 3 seeds under both
    economies (seeds 21 and 3, after 1.4 to 2.9 years; seed 7's drought starves too little), and
    the same worlds without it don't. Left alone for 150 years (seed 21), the world's own famines
    start about 5 uprisings a decade on Economics v0 and about 13 under markets, and the 18 realms
    become 36 and 49: nothing joins realms together again without armies (with them, see Armies
    moving and fighting below).
    The work is a pass over the provinces once a month; it was not timed on its own.
  - `simrun` prints how many provinces are in uprising, how many are at least half way there, and
    how many realms have broken away.
- **Troops** (step 7; Warfare "troops", `warfare.rs`, `data/troops.toml`, tests in
  `troops_tests.rs`). Off by default and not among the tester's choices yet, like realms; a
  configuration (`ModuleConfig::warfare`), a scenario file or `simrun --armies` turns it on. It needs
  realms (refused without them). How they move and fight is the next section.
  - **Troop kinds are data**, a tree like the goods tree saved with the world: levies, heavy
    infantry and cavalry on land, ships at sea. Each kind has a weight, the land that suits it (land
    with ore in reach for heavy infantry, food better from herds for cavalry, woodland for ships,
    which also need a shore) and, where the economy keeps goods, what each soldier uses up when
    raised (a set of tools for heavy infantry, a cartload of timber for each ship's crewman).
  - **An army entry is a company**: soldiers of one kind (`army.kind`, a node of the troop tree)
    from one province (`army.home`), serving the realm that held it when they were raised. A company
    is a guard (`army.role` 0), the peace-time force, which stands at home and fights only for its
    own province, or a field company (1), raised in war, which marches (`army.province` is where it
    stands, or the province it left while on its way). Each province keeps one guard company of
    each kind; recruits for field companies join one standing at home in no group, or an emptied
    one, or a new one, so entries number at most the most ever out at once.
  - **Each month** (`TroopParams`): a province a realm holds keeps 0.5% of its people (its soldiers
    included) as guards, and while its realm is at war with anyone, field companies up to 5% in
    all. A tenth or more below either, it asks Population for recruits up to that number, at most a
    third of the war-time number a month (so a war reaches 5% in 3 months), shared between the
    kinds by their weights times how well the land suits each. More than a quarter over, the extra
    go home ("returned"): guards from the guards, field soldiers only from companies standing at home
    in no group. A province an enemy occupies or in uprising raises nobody. Companies whose realm no
    longer holds their home (it changed hands, or the realm was dissolved) go home, all of them,
    and so do the guards of a province an enemy occupies. A company waiting for recruits or their
    goods isn't sent anywhere until they come.
  - **Eating**: the companies standing in a province ask it together for a month of food, at the
    priority people ask with, so a shortage is shared alike (tested), and each company gets the same
    share; a company on its way eats from the province it left. The module keeps who asked for how
    many until the grant comes (in its private state, saved). When the grant comes, half of the unfed
    desert home ("deserted", its own cause, a day on the way like soldiers going home), the unfed who
    stay die as hungry civilians do, and soldiers die of age and illness at the civilians' rate.
    The food was asked for the soldiers of that morning; recruits booked in the same day ate with
    the civilians (found by a test: counting them as unfed made too many desert). Soldiers' hunger
    is not a famine, so it adds nothing to unrest.
  - **Goods and the bounty** (markets only): the day the recruits come, Warfare asks for their
    goods, for as many as came ("equipped"); those the province can't equip join its levies. And
    the realm pays 10 coins a recruit from its treasury into the province ("bounty", a move, so no
    flow): asked at low priority on the month's second day, so it never cuts into the realm's own
    spending; a treasury short of coins pays what it has. Under Economics v0 the module asks for food
    and people only (a second manifest, as realms' taxes), and the land alone decides the kinds.
  - **Who reads whom**: Warfare reads owners, occupation, uprisings, wars, people, land facts and
    the ways between provinces; nothing else reads the army fields, so realms, markets and diplomacy
    don't depend on them. They learn of occupations and battles by events.
  - `simrun --armies` prints the soldiers, their share of the people, the companies and kinds, and
    how many went home and deserted in the last year; then the last year's battles, the soldiers
    killed in the last full year and the provinces occupied now.
  - **Measured** (Oct 16). Story 1 (`examples/unrest_check.rs ECONOMICS SEED drought armies`) comes
    out exactly as without troops on seeds 21, 3 and 7 under both economies. Over 150 quiet years
    (seed 21, `... quiet armies`) troops change little: Economics v0 88 uprisings against 89, 36
    realms at the end either way, 8.6 starved per 1,000 a year against 8.8; markets 179 against
    191, 48 realms against 49, 8.2 against 8.4. At the decade ends 0.5–1.8% of the people are under
    arms: any war puts a whole realm on a war footing, but few wars go on at once. Treasuries still
    hold 9.9% of their land's coins (`examples/coin_spread.rs 7 50 armies`): the bounty is small
    beside taxes. Speed, large world under markets with wars, processor time on one worker thread:
    0.23–0.39 ms a day more than 1.89–2.07 ms. Two changes got it there from about 0.6 ms: raising
    only a tenth or more below the number wanted, and one food request a province, not a company
    (each request and grant costs the engine about as much as a company's month).

- **Armies moving and fighting** (step 7; the same Warfare "troops", tests in `army_tests.rs`,
  `examples/war_check.rs`). Warfare runs daily now; raising and eating stay on the month's first
  day, and a day with nobody moving, fighting or besieging costs a few checks.
  - **Marching**: a company on its way (`army.next`, `army.days_left`, `army.dest`) counts down the
    way's days divided by its pace (cavalry 1.5 times foot; a group set off together goes at its
    slowest), along a route worked out when it was sent: through land its realm holds or occupies,
    its enemies' land and land nobody owns, never a neutral's (tested). Land troops go by land
    ways and ships by sea ways; land troops cross the sea only in a crossing. A company coming into a
    province where enemies stand stops there. A route whose way is gone (the map changed) stops the
    company and its general plans again.
  - **The general** of each realm at war plans on the month's first day and whenever one of its
    companies arrives, a battle ends or a province is taken. Targets in turn: its own land where
    enemies stand or that an enemy occupies, then enemy provinces next to land it holds or occupies,
    nearest its capital first, then enemy coasts facing its own across the sea. For each it sends
    free field companies (in no group, standing, rested, awaiting no recruits, not in a battle),
    nearest first by travel days, to gather in its provinces next to the target where no enemy
    stands, each taking its realm's field soldiers up to a tenth of its people (`gather_share`), until
    they bring 1.2 times the strength of the enemies standing in the target, counting their quarter
    more on their own land (`outmatch`, `home_edge`). A target it can't send enough to waits. Once all
    of a group (`army.group`, numbers never reused) has gathered and still outmatches the enemy
    there, it sets off together. Companies with nothing to do go home to their province.
  - **Battles**: where realms at war that day both have land troops standing in a province, each
    day each side loses strength 3% of the other's (`battle_rate`), each side's blows up to a quarter
    stronger or weaker at random (`luck`), and the side on its own land deals a quarter more. A
    company's share of its side's loss is its share of the side's strength, and it loses that
    strength over its kind's strength a soldier (levies 1, cavalry 2, heavy infantry 3): soldiers
    killed ("killed", counted in the year's deaths). A side that has lost 30% of the soldiers it
    fought with breaks (`break_share`): its field companies fall back to the nearest province of
    their own where no enemy stands and aren't sent to attack for 10 days; its guards go back to the
    people. When it ends, `Battle` names the province, who held the field, who broke and the killed,
    and the realms module adds to the unrest there what a month of occupation adds, once a battle.
  - **Sieges**: a realm's field companies standing in an enemy province where no enemy stands
    besiege it (`army.siege_days`); after 30 days × (1 + its people / the world's middle province's
    people), doubled for a capital and at most 180 days, Warfare sends `Occupied` and the realms
    module marks it. They free their own province in half the time. Besiegers fewer than 1% of its
    people make no progress; leaving loses it. Measured against the middle province because
    provinces hold ten times more people on a small world than on a large one.
  - **Across the sea**: a group gathers in its coastal provinces facing the target, with ships at
    each enough to carry the soldiers there (5 a crewman, `carry`) and in all 1.2 times the enemy's
    ships at the target, and crosses one stretch of sea. Enemy ships standing where it lands fight
    the fleet first, worked out on arrival day by day; if the fleet breaks, everyone turns back and
    the soldiers the surviving ships can't carry are lost at sea. After a landing the ships go back.
    Fleets don't seek each other out otherwise. Tested: soldiers at sea never outnumber what the ships
    with them carry.
  - **War and peace on the day**: battles and sieges go on only between realms at war that day. At
    a peace a realm with no war left sends all its field companies home; one still at war brings them
    back from the former enemy's land and plans again (tested).
  - **What is kept** (private state `armies-1`, saved): the food asked for, the groups, each moving
    company's route, a group's pace, companies resting after breaking, the battles going on, the
    provinces where a battle or siege may go on, the companies on the move, and those awaiting
    recruits or their goods. Where companies stand is worked out again from the army fields after a
    load. Tested: saving, loading and going back mid-march, mid-battle and mid-siege.
  - **Measured** (Oct 16, `examples/war_check.rs`). Story 1 with armies (`examples/unrest_check.rs
    ECONOMICS SEED drought armies`) comes out as before: the drought region rises up on seeds 21 and
    3 (after 1.4 and 2.9 years on Economics v0, 1.4 and 2.0 under markets), not on 7, and the worlds
    without the drought don't. Over 150 years of story 1's world (seed 21, `... long 150`), with
    armies and without: on Economics v0 the 18 realms become 20 rather than 36, 22 rebellions are put
    down and 3 win (without armies 18 win); under markets 29 rather than 49, 41 put down and 11 win
    (without, 30 win). Starvation is no worse (8.6 per 1,000 a year against 8.9 on Economics v0, 7.7
    against 8.4 under markets); soldiers get 99.2-99.9% of the food they ask for, and few famines
    fall where armies stand away from home (at most 39 of a decade's 479 province-months). About 4
    million soldiers are killed in the 150 years on Economics v0 and 10 million under markets, beside
    about 130 million starved. Soldiers cross the sea: up to about 1,300 company-days afloat in a
    decade under markets. Speed, large world under markets without unrest, on one worker thread: a
    day that isn't a month's first costs nothing in peace (Warfare tells the engine it is idle and is
    skipped); every day averaged, armies add 0.20 ms a day in peace (3.06 against 2.86 ms) and 2.1
    ms a day through a year of war between the two biggest realms that border each other (4.88
    against 2.80 ms). Getting there, the engine now sums only the quantities a module's own fields
    hold around its stage, and checks the rest by their flows, which must come to nothing (before,
    every stage summed every quantity: a daily module cost 15 ms a day).
- **Realms choosing war and peace** (step 7; Diplomacy "treaties", `diplomacy.rs`, tests in
  `treaty_tests.rs`). Off by default, like the other step 7 slots; `WarParams::treaties` (a
  scenario file's `[modules.diplomacy.treaties]`), `simrun --treaties` or the window's `--treaties`
  turns it on. It needs armies (refused without them: with nothing occupied and no battle fought,
  every war would end with nobody winning). "wars" stays as it was, pokes only; worlds running it
  come out the same as before this piece (`war_check` one war and 40 years with rebellions, both
  economies, compared output for output with the build before).
  - **Opinions** (`relation.*` fields, a new tier of entries, numbers never reused): for each pair
    of realms whose armies can reach each other, and any two that have fought, what each thinks of
    the other, from −100 to 100. A new pair starts at 0, rebels and the realm they left at −50. Each
    month at peace (no truce) each opinion rises 0.5, up to 50; rivals (neighbours within 1.5 times
    each other's strength) lose 0.3. A war declared: the realm attacked thinks 60 less of the
    attacker, the attacker 20 less of it. Each battle between them: 5 less, both ways. Each province
    occupied: its owner 10 less of the occupier. Land lost at a peace: the loser 30 less of the
    winner. Opinion of trade waits for the trade piece (trade is not a field yet).
  - **Strength is people**: Diplomacy can't read the armies, since Warfare reads the wars (the
    engine refuses the circle). A realm's strength is the people of its land.
  - **Wants**, each month: a realm whose people starved past 1% over the last twelve months
    (counted from `Famine` events by the land's owner; private state `treaties-1`) wants food, the
    best-stocked province it can reach of a neighbour's. Every realm likes the richest province it
    can reach (coins under markets; people and food stores, a year's rations as one person, under
    Economics v0), by how much richer it is than its own provinces on average, up to 3 times,
    counted for more under a greedy ruler. A province is in reach when it has a land way to, or
    faces across one stretch of sea, a province the realm holds or occupies.
  - **Declaring war**, each month, for each realm: on the realm it wants most of those it can reach,
    at peace with it (realms keep truces; the player's poke still breaks one), not much stronger
    (at least 0.8 times as strong), and, if it already fights a war, only when at least twice as
    strong as all its enemies together, the new one included. The score multiplies what it wants
    (up to 3), how much stronger it is (up to 3 times), the ruler's warlike trait (half at 0, twice
    at 100) and its opinion (lower is likelier: ×2.7 at −50, ×0.37 at 50); the monthly chance is the
    score (at most 10) over 180. A realm with no ruler yet counts as neither warlike nor greedy. The
    war keeps its cause (`WarCause::Food`, `Riches`) and the province it wants (`war.target`), which
    the general goes for first after its own land, while it can reach it (tested).
  - **Peace by treaty**, each month, for each war, rebellions' too: each side's weariness is 100 ×
    the share of its people living in land the enemy occupies, + 10 a year at war, + 5 a battle lost,
    + 50 × its soldiers killed over 5% of its people, + 30 × its land's average unrest over the
    uprising line. At 100 it is worn out: the war ends, won by the less worn side (by nobody when
    both are worn out within 10 of each other); or when the attacker holds the war's target and the
    defender is at 50. A rebellion ended by treaty both ways is the user's choice: the realm the
    rebels left, winning, takes all their land back as when it takes their capital. Each war counts
    each side's soldiers killed and battles lost (`war.attacker_dead` and the like, from `Battle`
    events, which now name each side and its dead). "wars" leaves the new war fields empty.
  - **Measured** (Oct 16, `examples/unrest_check.rs ECONOMICS SEED drought|quiet ... treaties`).
    Story 1 with realms choosing war (only a war for food counts as the story's war): on Economics
    v0 the drought realm rises up or declares a war for food on all three seeds (21: uprising after
    1.4 years and a war for food after 2.9; 3: uprising after 2.9; 7: war for food after 1.0), and
    the worlds without the drought do neither on seeds 21 and 3; on seed 7 the same realm declares
    its war for food after 1.0 years without the drought too (its own famine, not the drought's):
    2 of 3. Under markets the drought realm rises up on 21 and 3 and declares a war for food on 7;
    without the drought nothing on all three. Over 150 years (seed 21), against armies alone:
    on Economics v0, 34 wars for food and 115 for riches (about one a realm every 20 years), at most
    4 wars at once, 4 second wars, 21 realms at the end (20 without), 0.5-1.7% under arms at the
    decade ends (0.5-1.2%), starved 7.9 per 1,000 a year (8.5); under markets 47 for food and 113
    for riches, at most 5 at once, 2 second wars, 24 realms at the end (29), starved 7.5 (7.8).
    Nearly every peace has a winner. Rebellions' wars: 19 put down and 4 won on Economics v0 (22
    and 3), 50 put down and 5 won under markets (41 and 11). Wars for riches aim at a capital half the time on Economics v0
    and three times in four under markets (capitals hold the most coins), and few of the richest 5%
    of provinces change hands at a peace (none on Economics v0, 6 under markets in 150 years):
    story 2's rich city is rarely taken. Speed (large world, one worker thread, `war_check ...
    time-treaties`): about 7 ms more on a month's first day under markets (0.24 ms a day averaged),
    below the noise on Economics v0.

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

Tests (a few seconds; the story tests run story 1's world for four years, twice on Economics "food"
and three times on markets):

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
`--terrain null|noise`, `--climate null|latitude`, `--economics food|markets|off`,
`--realms settled=1,city=1,frontier=1` (realms on,
with the ways to start weighted as given), `--rulers` (rulers on, and realms the default way if
`--realms` isn't given), `--wars`, `--armies` (troops on, realms likewise), `--save FILE`, `--load FILE` (continue a saved
world for `--years` more years), `--scenario FILE` (open a scenario's played timeline and continue it)
and `--write-scenario FILE`. `--set KEY=VALUE` (repeatable) changes any setting of the world made,
e.g. `--set modules.province_params.climb_step=30 --set modules.realms.realm_count=12`;
`--settings` lists every setting of the world the other options pick, with its key, value, range
and explanation, and stops.

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
on one free core it would not. Once the load fell to about 7 of 16 cores busy, clock time was
0.48–0.54 ms a day for markets with trade and 0.15–0.16 ms for "food" (two rounds, seeds 1 and 7),
as when trade was built. With every core busy, markets on all cores was slower than on one (4.7–6.5
ms against about 3.4 ms): each month's work waits for its slowest share. Capping its cores, or
handing out work in bigger shares, could help.

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
