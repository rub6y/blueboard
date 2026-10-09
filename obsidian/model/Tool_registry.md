# Tool registry

The tool registry is a central table of drawing tools, each implementing one `Tool`
trait.

The tool registry holds the [[Vector_pen]] (Phase 2), rect/ellipse/line shape tools, a
select/move tool (see [[Scene_graph]]'s `hit_test` and `set_element_path`), two eraser
tools (`eraser` partial / `eraser-object` whole-element), two multi-select tools:
`select-rect` (marquee, reuses `Scene::elements_in_viewport`) and `select-lasso`
(freehand closed loop, see `blueboard_core::lasso`), and the [[Text_tool]].

The `text` tool is a structural exception: its `Tool` trait methods are unreachable
no-ops. `main.rs` special-cases `MouseInput`/keyboard dispatch while it's the active tool
(composing state — the in-progress string, where it's anchored — lives on `App`, which
`ToolContext` deliberately never exposes to a `Tool`), the same bypass precedent the color
picker already established for its own App-owned UI state.

`select-rect` and `select-lasso` share their group-move/delete logic via a
`GroupSelection` helper in `tools.rs`, differing only in how a fresh selection region
gets defined. `Tool` itself gained `delete_selection` (default no-op, overridden by all
three selection-based tools) so a single `:delete` command (key `d`) works across all of
them without each needing its own delete command.

The tool registry follows the same addon-friendly pattern as the [[Command_registry]], but
a `Tool` is given a `ToolContext` (camera + scene only), not `&mut App` — tools never need
the whole app, so tool dispatch never runs into the whole-self borrow conflict `Command`
dispatch has to work around with `std::mem::take`.

`ToolContext` also carries `color` (the current drawing color, set by `:color`) — the pen
and shape tools read it at `on_pointer_down` and hold it in their own state for the rest
of the gesture, since `Tool::preview` has no `ctx` to re-read it from.

The tool registry lives in `blueboard-app/src/tools.rs`.

The tool registry switches tools via `:tool <name>` (a [[Command_registry]] command) or a
[[Keybindings]] entry — both ultimately call the same `set_active`.

The tool registry also answers `completions(prefix)` — every tool name starting with
`prefix` — mirroring [[Command_registry]]'s own `completions`; it's what lets
[[Command_line]]'s Tab-completion complete `:tool <name>`'s argument, not just the
command name itself.
