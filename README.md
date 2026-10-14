# Fantasy World Simulator

A native-desktop, god/observer fantasy planet simulator. A colourful, layered 2D map of a whole planet
develops its geology, climate, nature, peoples, economies, wars, magic, religions and cultures while you
watch, poke, edit and fast-forward it.

**Status:** build steps 1 and 2 done.

- [`crates/sim`](crates/sim/): the simulation core. It generates a planet, runs pops that grow and
  starve, and writes PNG maps without a window.
- [`crates/app`](crates/app/): the map window. It shows the planet as an oval world map or a repeating
  rectangle, with a parchment look, layer sliders, smoothed borders and rivers, and a hover inspector.

- [Design & Architecture](Fantasy%20Planet%20Simulator%20%E2%80%94%20Design%20%26%20Architecture.md): the full design document.
- [`spikes/`](spikes/): throwaway prototypes used to make design decisions. Right now that's a renderer
  trial comparing Bevy against egui + wgpu, drawing the same cube-sphere map in both.

Renderer: **egui + wgpu**, decided Oct 14, 2026 from that trial.

Built in Rust.
