# Tool registry

The tool registry is a central table of drawing tools, each implementing one `Tool`
trait.

The tool registry holds the [[Vector_pen]] and will hold future tools (shapes, text,
eraser, selection).

The tool registry follows the same addon-friendly pattern as the [[Command_registry]].
