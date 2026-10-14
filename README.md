# Fantasy World Simulator

A native-desktop, god/observer fantasy planet simulator. A colourful, layered 2D map of a whole planet
develops its geology, climate, nature, peoples, economies, wars, magic, religions and cultures while you
watch, poke, edit and fast-forward it.

**Status:** build steps 1 to 4 done.

- [`crates/sim`](crates/sim/): the simulation core. It generates a planet, runs pops that grow and
  starve, takes pokes such as a drought, goes back to any earlier day, explains why a province
  changed on a given day, saves and loads worlds, and writes PNG maps without a window. Every part
  of the simulation is a module with a declared contract, and some come in several versions.
- [`crates/app`](crates/app/): the map window. It shows the planet as an oval world map or a repeating
  rectangle, with a parchment look, layer sliders, and smoothed borders and rivers. The world plays
  live at speeds from a day to a year a second. Click a province to see every value, where it comes
  from and when it last changed, ask why, read its event log, or start a drought there.

- [Design & Architecture](Fantasy%20Planet%20Simulator%20%E2%80%94%20Design%20%26%20Architecture.md): the full design document.
- [`spikes/`](spikes/): throwaway prototypes used to make design decisions. Right now that's a renderer
  trial comparing Bevy against egui + wgpu, drawing the same cube-sphere map in both.

Renderer: **egui + wgpu**, decided Oct 14, 2026 from that trial.

Built in Rust.
