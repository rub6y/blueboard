# Tool registry

The tool registry is a central table of drawing tools, each implementing one `Tool`
trait.

The tool registry holds the [[Vector_pen]] (Phase 2) and, as of Phase 5, rect/ellipse/line
shape tools — text, eraser, and selection are still future tools.

The tool registry follows the same addon-friendly pattern as the [[Command_registry]], but
a `Tool` is given a `ToolContext` (camera + scene only), not `&mut App` — tools never need
the whole app, so tool dispatch never runs into the whole-self borrow conflict `Command`
dispatch has to work around with `std::mem::take`.

The tool registry lives in `blueboard-app/src/tools.rs`.

The tool registry switches tools via `:tool <name>` (a [[Command_registry]] command) or a
[[Keybindings]] entry — both ultimately call the same `set_active`.
