# Fantasy World Simulator

A native-desktop, god/observer fantasy planet simulator. A colourful, layered 2D map of a whole planet
develops its geology, climate, nature, peoples, economies, wars, magic, religions and cultures while you
watch, poke, edit and fast-forward it.

**Status:** design phase. Nothing playable yet.

- [Design & Architecture](Fantasy%20Planet%20Simulator%20%E2%80%94%20Design%20%26%20Architecture.md): the full design document.
- [`spikes/`](spikes/): throwaway prototypes used to make design decisions. Right now that's a renderer
  trial comparing Bevy against egui + wgpu, drawing the same cube-sphere map in both.

Renderer: **egui + wgpu**, decided Oct 14, 2026 from that trial.

Built in Rust.
