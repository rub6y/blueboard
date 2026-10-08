# Core crate

The core crate (`blueboard-core`) holds the [[Scene_graph]], the [[File_format]]
read/write logic, and undo/redo history.

The core crate has no rendering or UI code, so it is testable headlessly.

The core crate is depended on by [[Render_crate]] and [[App_crate]].
