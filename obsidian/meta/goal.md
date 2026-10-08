# Goal

[[Blueboard]] is a whiteboard application inspired by Miro, built as a learning project.

The project exists for two reasons at once: to produce a working whiteboard app, and to
teach ruba systems programming (GPU rendering, vector geometry, binary file formats,
performance work) by building it slowly and explaining each piece as it's written.

The build follows [[planning_PLAN]] (`planning/PLAN.md` in the repo), which breaks the work
into phases. [[planning_RESEARCH]] (`planning/RESEARCH.md`) holds the researched rationale
behind each technical choice. This vault tracks the resulting objects and decisions as a
graph, not the phase breakdown itself — the phase plan is kept in the repo files, not
duplicated here.

Core features (first draft, in scope now):
- [[Semi_infinite_canvas]] with [[Zoom_range]] 1%-200%
- [[Vector_pen]] drawing via [[Bezier_curve]]s, not pixels
- [[File_format]] (`.bbd`) that saves/loads boards, compressed
- [[Config_file]] for keybindings and other customization
- [[Command_line]] (vim-like, `:`-prefixed) for actions like new/save/clear

Explicitly deferred (designed for, not built yet):
- [[Magic_window]] — embedding another live app (browser, shell, VSCode) inside a
  drawn rectangle on the board, e.g. to read a PDF and annotate it in place.

Explicit non-goal for the vault itself: this is a code project with its own
`planning/` folder for the brief/research/detailed plan — the vault complements that
with the object-level model (what each piece is and how it relates to the others),
not a duplicate of the phase plan.
