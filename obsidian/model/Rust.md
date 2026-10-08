# Rust

Rust is the implementation language of [[Blueboard]].

Rust was chosen over C and Python for this project; see [[planning_RESEARCH]] §1.

Rust's borrow checker surfaces ownership questions (e.g. who owns a stroke, who owns the
[[Scene_graph]]) that are part of what ruba wants to learn from this project.

Rust's toolchain for this project is provided by the [[Dev_environment]], not installed
imperatively.
