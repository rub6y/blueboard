# Ledger

Running log of decisions and progress. Newest first.

## 2026-10-09 — Command-line Tab-completion + persistent tool-name status
- [[Command_registry]] gained `completions(prefix)`, returning every registered command
  whose name starts with `prefix` as `(name, usage)` pairs — same data `:help` prints,
  filtered. Kept in `blueboard-app`'s `commands.rs` rather than `blueboard-render`, so the
  registry doesn't need a dependency on render-crate presentation types just to answer
  "what matches so far."
- [[Command_line]] now shows live, qutebrowser-style completions: as soon as a command
  name prefix (text before the first space) matches one or more commands, they're listed
  above the input, re-filtered every frame — not only after pressing Tab. Tab cycles a
  highlighted selection through that list (wrapping around) and fills the input with it;
  Enter still just runs whatever's in the input, unchanged. Typing or Backspace resets the
  highlight (a stale index would otherwise point at an unrelated candidate once the match
  set changes underneath it). Once a space is typed (onto arguments), completion stops —
  only the command name itself completes.
- The command bar is now always drawn, not only while composing/showing a status — its
  left side permanently shows the active tool's name, functioning as a persistent status
  line (same idea as vim's own bottom status line) rather than something that only
  appears on demand. [[Render_crate]]'s `CommandBarContent::Editing` gained `completions`
  and `selected` fields; `Renderer::render` gained an `active_tool_name` parameter.
- Verified by actually running the app under the established Xvfb convention: screenshots
  confirm the tool name updates immediately on switching tools even with the bar otherwise
  idle, typing `:s` shows exactly the one matching command (`save [name]`) live before any
  Tab press, pressing Tab fills the input to `:save`, and `:u` matches only `undo` (not
  itself, since `u` isn't a registered command name). `cargo test`/`clippy`/`fmt --check`
  all clean.
- **Harness note, not a project finding**: `pkill` (not `kill <pid>`) reliably hangs/kills
  this session's sandboxed shell outright when used to clean up the Xvfb/app processes
  between test runs — no error text, the whole command just never completes. Plain `kill
  <pid>` on a specific PID works fine. Recorded here since it cost real time working out
  during this session and will otherwise resurface in any future Xvfb-based verification.

## 2026-10-09 — Phase 5 (partial): two eraser tools
- [[Tool_registry]] gained two eraser tools: `eraser` (key `x`, partial — subtracts a
  circular region from whatever it passes over, splitting strokes/shapes rather than
  deleting them whole) and `eraser-object` (key `X`, whole-element — same `hit_test`
  outline-click [[Selection_tool]] uses, but deletes instead of selecting).
- New `blueboard-core` module `erase.rs`: real segment/circle clipping geometry, not just
  nearest-vertex removal — flattens an element's `BezPath` to a polyline (`kurbo::flatten`),
  walks it computing exact segment-circle intersection points, and splits into the runs that
  survive outside the circle. A closed shape (rect/ellipse) that's partially erased
  correctly becomes an open arc chain (rotates the vertex list to an outside point first, so
  the loop can be cut open without losing geometry, then re-joins the first/last resulting
  runs if the cut's far side is actually one continuous piece); fully circled, it disappears
  entirely. [[Scene_graph]] gained `remove_element` (map + R-tree removal) for both the
  object eraser and `erase.rs`'s remove-then-reinsert-the-pieces step.
- Both tools mutate the scene directly on every `on_pointer_down`/`on_pointer_move` while
  dragging, unlike the selection tool's preview-then-commit-on-release — there's no
  ambiguous in-progress state to roll back for an eraser, so there's nothing to gain by
  waiting for release. `main.rs` still only snapshots once around the whole pointer-down..up
  span (confirmed by re-reading it, not assumed), so a multi-element erase drag still
  collapses to one undo step for free, same mechanism the selection tool already relies on.
- [[Render_crate]] gained `ScenePreview::EraseCursor { center, radius }` — a circle outline
  (32-segment `LineList`, same preview pipeline as `Selection`) showing both what's about to
  be erased and, for the partial eraser, exactly how big a bite the next erase will take.
- Five new `erase.rs` unit tests (line split in two, full removal, untouched-when-nowhere-
  near, closed-shape partial erase leaves one open arc, closed-shape full removal) — all
  passing, 23 total in `blueboard-core`. `cargo clippy --all-targets` clean.
- **Verified by actually running the app** under the established Xvfb convention (not
  `DISPLAY=:0`). One new wrinkle this session, worth recording since it cost real time: a
  plain `nix-shell -p xorg.xvfb xdotool imagemagick --run "... nix develop -c ..."` (nested
  nix invocations, everything backgrounded with `&` inside the `--run` string) reliably
  hung or produced a blank capture — `./target/debug/blueboard` run bare (outside `nix
  develop`) also crashes immediately for a different reason, missing `libxkbcommon-x11.so`
  at dlopen time, because the X11 runtime libs only exist on `nix develop`'s
  `LD_LIBRARY_PATH`, not on a bare Xvfb/xdotool `nix-shell -p` shell. What actually worked:
  resolve `Xvfb`/`xdotool`/`import`'s store paths once via `nix-shell -p ... --run 'which
  ...'`, then call those paths directly (no further nix-shell nesting) alongside `nix
  develop -c ./target/debug/blueboard`, with `timeout` wrapping both the X server and the
  app instead of relying on background PIDs + manual `kill`. Confirmed visually, not just
  by log lines: screenshots at each step show the rectangle's corner cleanly notched out by
  the partial eraser (the two edges no longer meet), two separate lines with only the
  clicked one removed by the object eraser, and an empty board after two undos (proving the
  partial erase's scene mutation really did collapse to one real undo step, not zero or two).

## 2026-10-08 — `:help` command
- [[Command_registry]] gained `:help`, listing every registered command and its usage
  hint on one line (fits the command bar's fixed single-line height — no render-crate
  changes needed).
- Non-obvious wrinkle: `HelpCommand::run` can't actually build the listing — by the time
  any command's `run` executes, `app.registry` has been swapped out for the call (the
  `std::mem::take` dance `run_command` already does to dodge a borrow conflict), so it's
  an empty placeholder from inside `run`. Fixed by intercepting `"help"` one level up, in
  `CommandRegistry::run` itself, where `self` is still the one live copy. `HelpCommand`
  stays registered (so `:help` is discoverable and lists itself) but its `run` body is
  effectively unreachable — returns an `Err` rather than panicking if that ever changes,
  matching this project's established crash-resilience stance rather than `unreachable!()`.
- Verified by running the app (Xvfb, per the convention established for the selection
  tool below) and typing `:help`: bar showed
  `:save [name]  :open [name]  :clear  :new  :tool <name>  :undo  :redo  :help  :q`.

## 2026-10-08 — Phase 5 (partial): selection + move tool
- [[Tool_registry]] gained a `select` tool (keybinding `v`): click-to-select,
  drag-to-move. [[Scene_graph]] gained the two primitives it needed:
  `hit_test(point, tolerance)` (nearest element within tolerance, by distance to the
  path's rendered *outline* — every element renders as a stroke, so this matches what's
  visibly clickable rather than treating rect/ellipse as hit-testable by interior fill)
  and `set_element_path` (replace a path in place, reindexing the [[RTree_index]] entry
  since a move changes the bounding box).
- A drag only *previews* the move (translating the originally-grabbed path via
  `kurbo::Affine::translate`, re-done from the fixed original each move rather than
  compounded, so float drift can't accumulate) and commits in one step on release — this
  was a deliberate fit to the existing undo architecture: `main.rs` only
  snapshots-and-records around pointer down/up, not every move, so a tool that mutated
  the scene mid-drag would need its own History plumbing. One commit on release means the
  whole drag is a single undo step for free.
- [[Render_crate]] gained `ScenePreview::Selection(Rect)` — a plain 4-segment box outline
  via the existing `LineList` preview pipeline, no shader/theme changes. Deliberately
  reuses the same line color as other previews; a distinct highlight color is left as
  future polish, not required to tell selection apart from content (shape, not color,
  does that here).
- Four new `Scene` unit tests (`hit_test` hit/miss/nearest-of-two-overlapping,
  `set_element_path` reindexing) plus the existing suite, all passing (18 total in
  `blueboard-core`).
- **Verified by actually running the app**, on an isolated `Xvfb` display rather than the
  shared live desktop this time — the Phase 4 ledger entry flagged `DISPLAY=:0` testing as
  a contamination risk (real desktop input landing on the test window), and driving a
  window on ruba's actual desktop via synthetic input wasn't something to do without
  asking first anyway. `xdotool windowfocus` (not `windowactivate` — no window manager is
  running on a bare `Xvfb`, so activation fails, but focus alone is enough for winit to
  deliver keyboard events) was the one wrinkle; otherwise drew a shape, selected it,
  watched the live drag preview follow the cursor, confirmed the commit on release,
  confirmed clicking empty space deselects, and confirmed `:undo`/`u` reverts the move —
  all via screenshots at each step, not just trusting the log.
- Undo/redo (`[[Command_registry]]`'s `undo`/`redo`, [[Core_crate]]'s `history.rs`) was
  already complete and tested on this branch from a prior session but uncommitted;
  committed first, as its own commit, before starting this work.
- Still open in Phase 5: text tool, eraser, basic LOD for zoomed-out rendering.

## 2026-10-08 — Phase 4 complete: command line + config
- Vim-like `:`-command line implemented: [[App_crate]] owns input handling (raw `winit`
  key events, not an egui widget) and state; [[Render_crate]] renders the bar via
  `egui`/`egui-wgpu` as a second render pass composited on top of the board.
- [[Command_registry]] (`blueboard-app/src/commands.rs`) added, with built-in `save`,
  `open`, `clear`, `new`, and `q` commands — replacing Phase 3's temporary `S`/`O` hotkeys.
  Lives in the app crate (not core) since `Command::run` needs `&mut App`.
- [[Config_file]] (`$XDG_CONFIG_HOME/blueboard/config.toml`) added: [[Keybindings]] (single
  character -> command name) and [[Theme]] colors. Deliberately left out `default_tool` and
  `autosave_interval` from the original Phase 0 design sketch — no tool registry or
  autosave implementation exists yet for those fields to configure.
- [[Theme]] colors reach the GPU via a new small uniform buffer (`ThemeUniform`, bind group
  1) in `shader.wgsl` — `fs_grid`/`fs_stroke` read from it instead of hardcoded constants.
- **Bug caught immediately by the new logging infrastructure** (first real payoff of the
  crash-resilience work): first run panicked on `TexturesDelta` drop
  ("Dropped TexturesDelta with 1 unapplied deltas") — iterating `.set`/`.free` by reference
  doesn't drain them; fixed by calling `.clear()` explicitly after use.
- **A second, more concerning finding from testing**: a background test process exited
  cleanly (no panic, no crash) right as testing began. Root-caused to a design risk, not
  necessarily what literally happened: Phase 4's first draft bound the bare, unmodified `q`
  key directly to quit with zero confirmation — exactly the footgun vim itself avoids by
  *not* binding plain `q` to quit. Removed the default `q` keybinding entirely; quit stays
  reachable only via the deliberate `:q` command. Also added a log line on quit
  ("quit command received, exiting") so a clean exit is now distinguishable from a crash
  just by reading the log, which is what made this ambiguous in the first place.
- Verified by running the app: typed `:save test.bbd` live in the command bar (monospace,
  visible as typed), pressed Enter, saw the status line confirm it and the file actually
  appear on disk; drew a stroke and quit cleanly via `:q`, confirmed in the log.
- Milestone tag: `v0.4-commands-and-config` (see [[Git_workflow]]).

## 2026-10-08 — Phase 3 complete: file format
- `.bbd` save/load implemented in `blueboard-core/src/file_format.rs`, per
  [[planning_RESEARCH]] §3: [[Scene_graph]] -> `serde` -> `bincode` -> [[Compression]]
  (`zstd`), single compressed blob (no spatial tiling yet — Phase 6, once a real board
  exists to profile against).
- Caught a real dependency-ecosystem trap while adding it: `cargo add bincode` resolved to
  `bincode 3.0.0`, which turned out to be a placeholder/joke package
  (`compile_error!("https://xkcd.com/2347/")`), not the real crate — the actively
  maintained bincode is 2.x. Pinned explicitly to `bincode@2`. Worth remembering:
  `cargo add` picking the nominally-latest version isn't automatically trustworthy,
  especially for a crate whose maintained line is a major version behind what resolves by
  default.
- On-disk schema (`SceneDocument`/`StrokeDocument`) deliberately kept separate from the
  runtime `Scene`/`Stroke` types — the runtime scene also carries the [[RTree_index]],
  which is derived data that would be dead weight on disk and gets rebuilt on load anyway.
- [[Scene_graph]] gained `insert_loaded_stroke` (preserves ids from a loaded file,
  advancing `next_id` past the highest loaded id) and `all_strokes`/`len`/`is_empty`
  alongside the existing viewport-filtered query.
- Four round-trip tests from the start (empty scene, multi-stroke round-trip, id
  continuation after load, rejecting an unsupported format version) — file-format bugs are
  the kind that destroy user data, per the project's own safety conventions.
- [[App_crate]] got temporary `S`/`O` hotkeys (save/load to a fixed `board.bbd` path) so
  the format could be tested by actually running the app, not just unit tests — standing in
  until [[Command_line]]'s `:save`/`:open` in Phase 4.
- Verified end-to-end on a real file: drew two strokes, saved, killed the process entirely,
  launched a fresh one, loaded, and confirmed the same strokes rendered — not just an
  in-memory round trip.
- Milestone tag: `v0.3-file-format` (see [[Git_workflow]]).

## 2026-10-08 — Streamline smoothing, crash resilience, logging
- Follow-up to the pen smoothing fix below: a *second* comparison against Miro, zoomed in
  past where a stroke was drawn, still showed small regular kinks — decimation thins point
  density but doesn't touch the position of points that survive, so jitter was still baked
  into each recorded sample, just invisible until zoomed in far enough. Researched how
  open-source tools solve this; found [[Perfect_freehand]] (used by tldraw, Excalidraw) and
  adopted its "streamline" idea — an exponential moving average pulling each new point
  toward a running average before decimation/fitting. Confirmed fixed: same jittery test
  line, zoomed to the level that showed kinks before, now smooth.
- Developed on a branch (`experiment/pen-streamline-smoothing`) specifically so alternative
  smoothing strategies could be tried if this one hadn't worked.
- **Crash resilience**, prompted by ruba reporting the app "sometimes crashes for no
  reason": found that wgpu's default behavior for an uncaptured device error is to treat
  it as fatal and panic the whole process — confirmed by Phase 1's shader bug, whose log
  had literally read "Handling wgpu errors as fatal by default" right before crashing.
  Installed `on_uncaptured_error` and `set_device_lost_callback` handlers that log instead
  of panicking, and replaced stroke tessellation's `.expect(...)` with a graceful
  skip-and-log (one malformed stroke now drops just that stroke, not the whole app).
- **Logging**: added a small dual-sink logger (`blueboard-app/src/logging.rs`) writing to
  both stderr and `$XDG_STATE_HOME/blueboard/blueboard.log`, plus a panic hook appending to
  the same file — a GUI app's stderr is easy to lose (closed terminal, launched from a
  desktop icon), exactly the situation "why did it crash" needs answered.
- **Verification-methodology finding**: a background test process logged a committed
  stroke before any synthetic input had been sent to it. Root cause: test processes were
  launched on `DISPLAY=:0`, ruba's actual live desktop session, not an isolated display —
  real mouse activity on the shared display can land on a test window. This most likely
  also explains the earlier "stale content in screenshots" mystery from Phase 2's first
  bug hunt, previously (incorrectly) suspected to be compositor caching. Logged here so
  future "verified by running" claims in this project are read with that caveat.
- Added `planning/phase2_explained.tex` (companion to Phase 1's, same format) covering the
  fitting/scene-graph/tessellation code and all three Phase 2 bug hunts.

## 2026-10-08 — Pen smoothing fix + zoom raised to 400%
- **Bug found by ruba** (side-by-side comparison with Miro, this time a near-horizontal
  line specifically): blueboard's strokes showed visible staircase-like kinks that Miro's
  didn't, even with the MSAA fix from earlier the same day. Root cause was different from
  the earlier aliasing bug: real mouse/trackpad jitter (a pixel or two) was being recorded
  on every `CursorMoved` event and handed to [[Fitting]] with a tight (1.5px) tolerance,
  so the curve fitter faithfully reproduced the jitter as real direction changes in the
  fitted path — worst-case visible on a horizontal line, where the jitter is the line's
  entire vertical variation. Not a rendering/tessellation-resolution problem at all (ruled
  out by the clean diagonal edges already confirmed in the earlier AA fix).
- Fix, two parts, both in [[App_crate]]: (1) decimate raw pointer samples by screen-space
  distance (2px) before they ever reach [[Fitting]], so near-stationary jitter is dropped
  rather than recorded; (2) loosened the fit tolerance 1.5px -> 2.5px so the fitter smooths
  over what jitter does get through, trading imperceptible positional fidelity for
  smoothness.
- [[Zoom_range]] raised from the brief's original 200% to 400% (`MAX_ZOOM` in
  `blueboard-core/src/camera.rs`), after ruba compared directly against Miro's deeper zoom.
  `MIN_ZOOM` (1%) left as-is.
- Verified by drawing a synthetic jittery near-horizontal line via `xdotool` (±1px random
  vertical noise) and confirming it renders smooth; verified 400% zoom by checking grid
  spacing scaled exactly 4x after scrolling to the new clamp.

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
