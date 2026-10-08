# Ledger

Running log of decisions and progress. Newest first.

## 2026-10-08 — Phase 2 complete: vector pen
- [[Fitting]] (`blueboard-core/src/fitting.rs`) fits raw pointer samples to a
  [[Bezier_curve]] path by wrapping them as a `kurbo::ParamCurveFit` source (a
  piecewise-linear polyline parameterized by normalized arc length) and reusing kurbo's
  own `fit_to_bezpath`, rather than hand-implementing Schneider's algorithm — same math as
  `planning/math.tex`, a maintained implementation instead of a bespoke one.
- [[Scene_graph]] implemented as `Scene`/`Stroke` in `blueboard-core/src/scene.rs`, indexed
  by an [[RTree_index]] (`rstar`) keyed on each stroke's bounding box.
- [[Render_crate]] tessellates visible strokes via `lyon` (through a small kurbo-path ->
  lyon-path adapter) and queries the scene's R-tree by the camera's current viewport each
  frame before tessellating — [[Viewport_culling]] is wired in now per PLAN.md, even though
  boards are still small; tessellation *caching* stays future-phase work.
- Artistic direction from ruba: board background is deep blue, pen strokes (and the live
  preview of a stroke still being drawn) are white.
- **Bug found by comparing against Miro's rendering** (ruba's side-by-side screenshot):
  blueboard's lines looked visibly jagged/faceted. Root causes, all fixed:
  1. No MSAA — every triangle edge was a hard aliased boundary. Fixed with a 4x
     multisampled offscreen target resolved into the swapchain each frame.
  2. Stroke tessellation's flattening tolerance was a hardcoded board-space constant,
     not divided by zoom — inconsistent with `planning/math.tex` §3's own documented
     design (tolerance must be screen-space, tightening as zoom increases). The curve
     *fit* tolerance had this right from the start; the tessellation tolerance didn't.
  3. Default miter line joins read as sharper-cornered than Miro's pen; switched to round
     joins/caps.
- [[App_crate]] input scheme: left-drag now draws (was pan in Phase 1), middle-drag pans,
  scroll still zooms.
- Verified by running the binary and drawing with `xdotool`+screenshots. Noted for later
  reference: this environment's `import -window` screenshot sometimes shows stale content
  from a window's *previous* process (confirmed via matching pid/window-id that no second
  process was involved) — looks like compositor-level window-thumbnail caching, not a bug
  in the app; worth knowing before trusting a screenshot taken immediately after launch.
- Milestone tag: `v0.2-vector-pen` (see [[Git_workflow]]).

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
