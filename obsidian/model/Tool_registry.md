# Tool registry

The tool registry is a central table of drawing tools, each implementing one `Tool`
trait.

The tool registry holds the [[Vector_pen]] (Phase 2), rect/ellipse/line shape tools, a
select/move tool (see [[Scene_graph]]'s `hit_test` and `set_element_path`), and, as of
Phase 5's eraser work, two eraser tools: `eraser` (partial — subtracts a circular region
from whatever it passes over, via [[Scene_graph]]'s `erase_circle`) and `eraser-object`
(whole-element — `hit_test` plus `remove_element`) — text is still a future tool.

The tool registry follows the same addon-friendly pattern as the [[Command_registry]], but
a `Tool` is given a `ToolContext` (camera + scene only), not `&mut App` — tools never need
the whole app, so tool dispatch never runs into the whole-self borrow conflict `Command`
dispatch has to work around with `std::mem::take`.

The tool registry lives in `blueboard-app/src/tools.rs`.

The tool registry switches tools via `:tool <name>` (a [[Command_registry]] command) or a
[[Keybindings]] entry — both ultimately call the same `set_active`.

The tool registry also answers `completions(prefix)` — every tool name starting with
`prefix` — mirroring [[Command_registry]]'s own `completions`; it's what lets
[[Command_line]]'s Tab-completion complete `:tool <name>`'s argument, not just the
command name itself.
