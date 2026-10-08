# Research & best practices — Blueboard (working name)

Gathered before writing the detailed plan. Scope: language/runtime choice, git workflow,
file format & compression, rendering/perf strategy, and the magic-window feature design.

## 1. Language & environment

Candidates considered: C, Python, Rust.

| | C | Python | Rust |
|---|---|---|---|
| Perf ceiling | highest | low (GIL, interpreted) | at/near C |
| Memory safety | none (manual) | GC, safe | safe by construction (borrow checker) |
| Vector graphics / GPU ecosystem | exists but low-level, manual bindings | bindings exist (PyOpenGL, skia-python) but glue-heavy | native: `wgpu`, `lyon`, `kurbo` — all actively maintained in 2026 |
| Serialization/compression | manual | easy, but slow for big scenes | `serde` + `bincode`/`zstd`, zero-copy friendly |
| Addon/plugin story | dlopen, unsafe | easy but slow, GIL blocks parallel addons | trait objects now, WASM (`wasmtime`) later for sandboxed addons |
| Cross-platform GUI | manual per-OS | tkinter/Qt bindings, packaging pain | `wry`/`tao` (used by Tauri), `egui`, native windowing via `winit` |
| Learning value (user wants to learn) | teaches manual memory mgmt, less transferable | easy but hides the interesting problems (GPU, perf, formats) | directly teaches GPU rendering, perf-sensitive data structures, safe systems code |

**Recommendation: Rust.** It gets the performance ceiling needed for a GPU-rendered infinite
canvas, has first-class crates for every piece of this project (vector tessellation, GPU,
serialization, compression), and the borrow checker surfaces data-ownership questions (who
owns a stroke, who owns the scene graph) that are exactly the kind of thing worth learning
from. C would work but the manual memory management is overhead unrelated to the actual
problem; Python's GIL and interpreter overhead make an infinite pannable/zoomable canvas with
many live vector objects a real bottleneck, not a hypothetical one.

Toolchain on this machine (NixOS): pin via `flake.nix` + `rust-overlay` + `direnv`, so
`rustc`/`cargo` and native deps (e.g. for `wgpu`'s Vulkan/EGL backends) are reproducible
instead of relying on FHS paths.

### Core crates (subject to version pinning once the repo exists)
- Windowing: `winit`
- GPU: `wgpu` (Vulkan/Metal/DX12/GL backends, actively maintained, used by multiple 2026
  whiteboard-style projects found below)
- Vector path construction & math: `kurbo` (Bezier curves, same lineage as `lyon`)
- Tessellation (vector path → GPU triangles): `lyon`
- Immediate-mode UI chrome (toolbars, command line, panels): `egui` via `egui-wgpu`
- Spatial indexing for viewport culling: `rstar` (R-tree)
- Parallelism (tessellation, file I/O off the main thread): `rayon`
- Serialization: `serde` + `bincode` (compact binary) for the scene graph
- Compression: `zstd` crate (fast, good ratio, streaming — better fit than `flate2`/gzip for
  this use case)

Confirmed via search that this stack (wgpu + lyon + kurbo, sometimes `gpui`) is the live 2026
pattern for Rust whiteboard/infinite-canvas projects (e.g. `gpui-whiteboard`, `Drafft-ink`,
`Nezumo`, `BSE` — all Rust+WebGPU, actively updated through mid/late 2026).

## 2. Git workflow

Research question: how to use git efficiently for a solo/learning project that may later
take contributions.

Recommended setup:
- **Trunk-based development**: a single long-lived `main`, short-lived feature branches
  (`feat/pen-tool`, `feat/file-format`), merged via PRs even solo — keeps history readable
  and gives a natural changelog.
- **Conventional commits** (`feat:`, `fix:`, `refactor:`, `docs:`, `perf:`): cheap to adopt,
  makes `git log --oneline` self-explanatory and enables auto-generated changelogs later.
- **Small, focused commits**: one logical change per commit — essential for a project meant
  to be read back later for learning.
- **Pre-commit hooks** via `lefthook` or plain `.git/hooks`: run `cargo fmt --check` and
  `cargo clippy -- -D warnings` before commit, so style/lint issues never land in history.
- **Tags for milestones** (`v0.1-draft-canvas`, `v0.2-file-format`), matching the plan's
  phases — gives checkpoints to diff against later.
- **.gitignore**: `target/`, `*.bb` test fixtures if large, `.direnv/`.
- No git-lfs needed initially — the custom file format is already compressed binary, and
  test fixtures should stay small.

## 3. File format & compression

Requirements: compact on disk, fast to encode/decode, good fit for "mostly vector paths +
some metadata", and ideally supports partial/lazy loading as boards grow.

Approach:
- **Scene representation**: a flat list of elements (strokes, shapes, text, groups), each
  with a stable ID, transform, style, and geometry. Strokes stored as cubic Bezier segments
  (`kurbo::BezPath`), *not* rasterized — this is what makes infinite zoom lossless, same as
  Miro's actual approach.
- **Serialization**: `serde`-derived structs → `bincode` for a dense binary encoding (far
  smaller and faster than JSON/MessagePack for this shape of data; no schema evolution
  machinery needed at this stage, unlike flatbuffers/protobuf, which would be overkill for a
  single-app file format with no cross-service compatibility requirement).
- **Compression**: wrap the bincode payload in `zstd` (app-level compression, not filesystem-
  level) — zstd gets gzip-beating ratios at much higher speed, and supports streaming, which
  matters once boards get large.
- **Chunking for lazy load (perf-relevant)**: once boards get big, split the scene into
  spatial tiles (e.g. 4096×4096 board-space units) each independently
  serialized+compressed, with a small uncompressed header/index listing tile bounds and byte
  offsets. This lets the app load only the tiles that intersect the current viewport instead
  of deserializing the whole board on open — directly addresses the "hard to run on a worse
  machine" concern. First draft can skip chunking (single blob) and add it once real boards
  exist to profile against.
- File extension: pick something distinctive, e.g. `.bbd` (blueboard document) — avoid
  colliding with the eventual app name.

## 4. Rendering / performance strategy

Concerns researched: infinite pannable/zoomable canvas, vector pen strokes, running fine on
weaker hardware.

- **Viewport culling**: keep all elements in an R-tree (`rstar`) keyed by bounding box; each
  frame, query only elements intersecting the visible viewport (expanded by a small margin)
  for rendering. This is what makes "infinite" canvas not mean "O(n) over everything, every
  frame."
  Note on terminology: "infinite" is semi-infinite as stated in the user's plan —
  coordinates can be large but not unbounded (practically, `f64` board-space coords are more
  than enough headroom).
- **Zoom range (1%–200%)**: store geometry in board-space float coordinates, apply a single
  view transform (pan/zoom) at render time, never rebake geometry into screen-space. At very
  low zoom, tessellating full-detail curves for thousands of strokes is wasted work — apply a
  **level-of-detail** simplification (e.g. Douglas-Peucker on the Bezier polyline
  approximation) when a stroke's on-screen size drops below a pixel threshold.
- **Tessellation caching**: tessellating a vector path to GPU triangles is the expensive step;
  cache the tessellated mesh per element and only re-tessellate on edit or significant zoom-
  level change (LOD bucket change), not every frame.
- **Dirty-rect / damage tracking**: avoid full-canvas redraws when nothing changed (e.g. only
  the command line blinking, or a small local edit) — track a dirty region and redraw only
  what's affected where the windowing/GPU backend allows it.
- **Multithreaded tessellation**: use `rayon` to tessellate newly-added/edited strokes off the
  render thread, feeding results back via a lock-free or channel-based handoff.
- **Memory**: for very large boards, combine with the file-format chunking above — don't
  even keep off-viewport tiles' full geometry in memory, evict tiles that haven't been
  touched/visible recently (simple LRU over tiles).
- **GPU backend**: `wgpu` targets Vulkan on Linux by default, with sane fallback — keeps a
  single codebase working across this NixOS machine and other platforms later.

## 5. Magic window — feasibility research (explicitly deferred feature)

Goal: embed another live application (browser, shell, VSCode, PDF viewer) inside a
user-drawn rectangle on the board, so e.g. a PDF can be read and annotated in place.

Three implementation strategies, in increasing order of platform fragility:

1. **In-process content via embeddable libraries** (works today, no capture APIs needed):
   - Browser/PDF: embed a webview via `wry` (the webview crate behind Tauri) — can render
     arbitrary web content or a PDF.js-based viewer directly inside a region of the window.
     This alone covers the PDF-reading-and-annotating use case from the plan without touching
     window capture at all.
   - Shell: embed a terminal via `portable-pty` (spawns a real shell/PTY) + a terminal
     renderer (`alacritty_terminal` as a library, or a minimal VTE-based one) drawn into the
     magic window rect.
   - This strategy is the most robust: no compositor permission dialogs, no per-platform
     capture backend, content renders as a texture/sub-view you already own.

2. **External app capture via portals (Wayland)**: for apps that can't be embedded as a
   library (e.g. real VSCode, not just a web view), the only sanctioned way to get another
   window's pixels on Wayland is `org.freedesktop.portal.ScreenCast` (xdg-desktop-portal):
   the app requests a session, the compositor shows the user a picker, and hands back a
   PipeWire video stream of the chosen window/screen, which gets decoded into a GPU texture
   and blitted into the magic window rect. Confirmed this is still the standard (and only)
   route in 2026 — Wayland's security model forbids any other cross-window pixel access.
   Caveats: capture-only (no input forwarding without extra portal support,
   `RemoteDesktop` portal would be needed for that), per-compositor behavior varies, and the
   picker UX means the user re-grants per session unless the portal remembers it
   (`persist_mode`).
   On X11 specifically this is easier (no portal needed, can use XComposite/XShm directly),
   but since the target machine is Wayland-first NixOS, portal+PipeWire should be the
   primary path, with X11 compositing as a secondary/legacy path if ever needed.

3. **True window embedding (reparenting)**: XEmbed-style embedding (the window literally
   becomes a child of the board's window, as some panel/tray implementations do on X11) has
   no real Wayland equivalent — Wayland deliberately does not allow one client to reparent
   another's surface. Not viable as a cross-platform approach; strategy 2 (capture, not
   embedding) is the realistic one for "foreign" apps on Wayland.

**Recommended approach when this feature is built**: start with strategy 1 (webview +
embedded terminal) since it already satisfies the PDF/shell use cases from the plan with no
platform-fragile capture code. Treat strategy 2 (portal capture) as an additive, higher-
effort path for the "any arbitrary app" case, implemented later and clearly isolated behind
an addon-style boundary so it can fail/be unavailable on a given platform without breaking
the rest of the app.

## Sources consulted
- [infinite-canvas · GitHub Topics (Rust)](https://github.com/topics/infinite-canvas?l=rust&o=asc)
- [whiteboard · GitHub Topics (Rust)](https://github.com/topics/whiteboard?l=rust&o=desc&s=stars)
- [Eight million pixels and counting — lyon intro](https://nical.github.io/posts/lyon-intro.html)
- [gpui-whiteboard docs.rs](https://docs.rs/gpui-whiteboard/latest/gpui_whiteboard/)
- [Wayland screen/window capture via PipeWire + xdg-desktop-portal, issue discussion](https://github.com/Just-Antonio/airmedia-share/issues/2)
- [libscreencapture-wayland](https://github.com/DafabHoid/libscreencapture-wayland)
- [portalgrab — xdg-desktop-portal + PipeWire capture](https://github.com/dcastro86/portalgrab)
