# Blueboard — detailed project plan

Builds on `plan.md` (the original brief, kept as-is as the guiding doc) and `RESEARCH.md`
(the background for the choices below). This file is the actionable breakdown.

Working name: **Blueboard**. File extension: `.bbd`.

## Stack (from RESEARCH.md §1)
Rust, `winit` + `wgpu` for windowing/GPU, `lyon`+`kurbo` for vector geometry/tessellation,
`egui` for UI chrome, `rstar` for spatial indexing, `serde`+`bincode`+`zstd` for the file
format, `rayon` for off-thread work. Dev env: `flake.nix` (rust-overlay) + `direnv`, per this
machine's NixOS conventions.

## Architecture, up front (so addons are easy from day one)

- **Core crate (`blueboard-core`)**: scene graph (elements, transforms, spatial index),
  file format read/write, undo/redo history. No rendering, no UI — pure data + logic, so it's
  testable headlessly and reusable if a second frontend ever appears.
- **Render crate (`blueboard-render`)**: wgpu pipeline, tessellation + caching, viewport
  culling/LOD. Depends on `core`, knows nothing about UI widgets.
- **App crate (`blueboard-app`)**: `winit` event loop, `egui` chrome, the `:`-command line,
  config loading, wires `core` + `render` together.
- **Command registry**: every `:`-command (`:save`, `:new`, `:clear`, ...) is registered
  through one trait (`Command { name, run(&mut AppState, args) }`) in a central registry
  rather than hardcoded in a match statement. New commands (including future addon-provided
  ones) register the same way — this is the concrete mechanism behind "easy to make addons."
- **Tool registry**: same pattern for drawing tools (pen, shape, text, ...) — a `Tool` trait,
  registered in a table keyed by name/shortcut, so adding a tool never means touching
  existing tool code.
- **Addon boundary (forward-looking, not built in v1)**: both registries above are designed
  so that a later addon system just needs a way to call `register_command`/`register_tool`
  from outside the core binary — first via Rust trait objects loaded at compile time
  (workspace crates), later (if ever needed) via `wasmtime` for sandboxed, truly external
  addons. Not implemented now; the trait-based registries are the only thing v1 needs to
  commit to.
- **Config file**: a single TOML file (`XDG_CONFIG_HOME/blueboard/config.toml`) — keybindings
  (map of key-chord → command name, reusing the command registry's names, so no separate
  keybinding vocabulary), default tool, theme colors, autosave interval. Parsed at startup
  with `serde` + `toml`; unknown/missing keys fall back to documented defaults rather than
  erroring, since this is the kind of file users hand-edit and typo.

## Phased build order

### Phase 0 — scaffolding (pending sign-off, see end of this file)
- `git init`, `flake.nix` + `direnv`, workspace `Cargo.toml` with the three crates above,
  CI-less for now (add later if/when pushed to a remote), pre-commit hook for `fmt`+`clippy`.
- `obsidian/` vault for project notes/decisions (separate from this plan — the vault is where
  day-to-day decisions get logged as the project evolves; this PLAN.md stays the stable
  reference).

### Phase 1 — draw something
- `winit` window + `wgpu` surface, clear screen, no UI yet.
- Pan/zoom camera (board-space ↔ screen-space transform), mouse/trackpad input, zoom clamped
  1%–200% per the brief.
- Milestone tag: `v0.1-window-and-camera`.

### Phase 2 — the pen
- Freehand input → `kurbo::BezPath` fitting (sample points, fit cubic Beziers rather than
  storing raw polylines) so strokes are true vector curves, not pixel traces, per the brief.
- `lyon` tessellation of strokes into GPU meshes, basic flat-color stroke rendering.
- Element storage in `core`'s scene graph + `rstar` index; viewport culling wired in even
  though boards are small at this stage — establishing this now avoids a rewrite later.
- Milestone tag: `v0.2-vector-pen`.

### Phase 3 — file format
- `.bbd` save/load: `serde` scene graph → `bincode` → `zstd`. Single-blob first (no tiling
  yet, per RESEARCH.md §3 — add tiling only once a real board exists to profile against).
- Round-trip tests (save → load → scene equality) from the start, since file-format bugs are
  the kind that destroy user data.
- Milestone tag: `v0.3-file-format`.

### Phase 4 — command line + config
- Vim-like modal `:`-command input (`egui` text widget intercepting `:`), command registry
  wired to `:new`, `:save`, `:save <path>`, `:clear`, `:q`.
- Config file loading (keybindings, defaults) per the Architecture section above.
- Milestone tag: `v0.4-commands-and-config`.

### Phase 5 — more tools & polish
- Shapes (rect/ellipse/line), text, eraser, selection+move, undo/redo (the `core` history
  stack). Basic LOD for zoomed-out rendering (RESEARCH.md §4) once there's enough on a board
  to make it visible.
- Milestone tag: `v0.5-tools`.

### Phase 6 — performance pass
- Profile against a deliberately large synthetic board; add tessellation caching, dirty-rect
  redraw, and file-format tiling/lazy-load if the profile shows it's needed (RESEARCH.md §3–4).
  Don't build these preemptively before Phase 3/5 give something real to measure.
- Milestone tag: `v0.6-perf`.

### Phase 7+ — magic window (explicitly post-v1, per the brief)
Per RESEARCH.md §5: start with strategy 1 (embedded `wry` webview for browser/PDF, embedded
PTY+terminal for shell) since it covers the brief's own motivating example (read a PDF,
annotate on the board) with no platform-fragile capture code. Treat portal/PipeWire capture
of arbitrary external apps (VSCode, etc.) as a separate, later, isolated effort layered on
top, not a blocker for the first magic-window release.

## How this plan will be used day to day
Per the brief ("slow, almost line by line"): each phase's work happens in small commits,
each change explained as it's written (what the code does and *why* this structure/crate/
approach over alternatives), reusing the git workflow from RESEARCH.md §2 (feature branches,
conventional commits, tags at each milestone above).

## Open before Phase 0 can start
Project-setup convention on this machine requires confirming the dev-env scaffold and vault
before creating either — see the sign-off ask accompanying this plan.
