# Ledger

Running log of decisions and progress. Newest first.

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
