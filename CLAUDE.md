# fantasymap: rules for this project

## Every module's numbers are settings the user can change, each explained

Make the largest reasonable share of every module's numbers changeable by the user, each with a
plain-words explanation. This holds for every new module, every new version of a module and every
new parameter, and for constants in code a person could picture ("how high the highest mountains
are", "how strongly borders follow rivers").

**Why:** the user wants to shape worlds themselves (decided 2026-10-17): new worlds (how the land,
climate, provinces and realms come out) and running worlds (prices, wars, unrest). Numbers buried in
code can't be reached; numbers without an explanation can't be understood.

**How to apply:**

- Put the number in the module's parameter struct (in `ModuleConfig`), not in a `const`. Give it
  `#[serde(default)]` (or a default function) equal to the value it had, so old scenario files make
  the same world. `crates/sim/tests/old_scenarios.rs` must still pass value for value.
- Describe it in `crates/sim/src/settings.rs` (`GROUPS`): its key, a few-word label, an explanation
  in plain words (what it does, and what raising or lowering it does), its range and unit, and
  whether it may change while the world runs (`live`) or only when a world is made (`made`: it
  shapes the map or the start). Keep the range inside what the module's own `check` accepts and
  wide enough to be useful; it must hold the default.
- A part a module can switch on or off (an `Option` sub-table) goes in `SECTIONS`. Every module and
  part must be on in `settings::everything()`, which names every field of `ModuleConfig` so a new
  module doesn't build until it is added; a test fails for a part left off there.
- Anything that is not a number to set (a name in a tree, a content table, a version picked from
  its own list) goes in `LEFT_OUT`, with the reason.
- `settings::tests::every_value_is_a_setting_or_left_out_with_a_reason` fails for any value of the
  configuration that is in neither list, so a new parameter can't be forgotten. Its other tests
  check every description names a value and holds its default.
- Live settings must be safe to change on any day: the module is made again with the new settings
  and given its old state (`save_state` / `load_state`). Anything a module works out from its
  settings must be rebuilt from them, not kept in its state. `crates/sim/tests/settings.rs` checks
  this for every module.
- A setting's range must work at both ends: `tests/settings.rs` makes a tiny world with each setting
  alone at its minimum and its maximum and runs it; it must run cleanly or be refused with a reason.
  Keep generated values inside their fields' bounds (`fields.rs`).
- Nothing more is needed for the window or the command line: the New world panel, the running
  world's Settings panel, `simrun --set KEY=VALUE`, `simrun --settings` and the window's
  `--set` / `--live-set` all work from `settings.rs`.
- A changed configuration layout bumps `save::FORMAT_VERSION`.

Leave out, and say so in `LEFT_OUT` where it is in the configuration: noise-shaping internals (octave
counts, warp offsets), content tables (the goods and troop trees, a good's land formula), names of
nodes in a tree, and versions.
