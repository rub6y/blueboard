# Ledger

Running log of decisions and progress. Newest first.

## 2026-10-08 — Phase 1 complete: window, camera, grid
- A [[Camera]] (pan/zoom state, board-space <-> screen-space math) was added to
  [[Core_crate]] — pure `glam`-based math, no GPU dependency, covered by unit tests for
  zoom clamping (1%-200%) and zoom-to-cursor anchoring.
- `winit` 0.30's `ApplicationHandler` trait is the event-loop API actually resolved by
  cargo (not the older closure-based `run`), since window/surface creation now has to
  happen in the `resumed` callback rather than before `run_app`.
- `wgpu` 30.0.1 resolved with a substantially changed API versus what RESEARCH.md assumed
  (`RequestAdapterOptions`/`SurfaceConfiguration`/pipeline descriptors gained new required
  fields; `Surface::get_current_texture` now returns a `CurrentSurfaceTexture` enum instead
  of `Result<_, SurfaceError>`; presenting moved to `Queue::present`). [[Render_crate]]
  wraps this in its own `RenderOutcome` enum so [[App_crate]] isn't coupled to wgpu's
  exact shape.
- [[Render_crate]] renders a static reference grid (not real scene content — that's
  Phase 2) so camera pan/zoom has something visible to confirm against.
- Verified by actually running the binary (forced X11/XWayland backend, driven with
  `xdotool`, screenshotted with `import`): window opens, grid clears/draws, drag pans the
  grid, scroll zooms anchored under the cursor. One real bug caught this way that the
  compiler didn't: the WGSL fragment shader's return value was missing its `@location(0)`
  binding, which `cargo build`/`clippy` don't check (shader compilation only happens at
  runtime) but immediately panicked the running app.
- Milestone tag: `v0.1-window-and-camera` (see [[Git_workflow]]).

## 2026-10-08 — Phase 0 scaffold complete
- [[Rust]] chosen as the implementation language over C and Python; see
  [[planning_RESEARCH]] §1 for the comparison.
- Workspace split into [[Core_crate]], [[Render_crate]], [[App_crate]] from the start,
  to keep the scene graph/file-format logic independent of rendering and UI.
- [[Dev_environment]] set up as a Nix flake + direnv shell (reproducible toolchain,
  no imperative installs, per this machine's conventions).
- [[Git_workflow]] conventions adopted: trunk-based `main`, conventional commits,
  milestone tags, `lefthook` pre-commit running `cargo fmt --check` + `cargo clippy`.
- `planning/` folder holds the original brief (`plan.md`), the research note
  (`RESEARCH.md`), the detailed phase plan (`PLAN.md`), and `math.tex` (the math
  behind [[Bezier_curve]] fitting/flattening, [[LOD_simplification]], and the
  [[RTree_index]]).
- [[Magic_window]] explicitly deferred past the first draft; three implementation
  strategies researched and ranked (webview/PTY embedding first, portal capture
  second, true window reparenting ruled out as non-viable on Wayland).
