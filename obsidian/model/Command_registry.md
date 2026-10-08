# Command registry

The command registry is a central table of `:`-commands, each implementing one `Command`
trait (`name`, `run`).

The command registry is what makes new commands addable without touching existing command
code.

The command registry's names are reused by the [[Config_file]]'s [[Keybindings]], so there
is no separate keybinding vocabulary.

The command registry is the concrete mechanism behind Blueboard's addon-friendliness, per
[[planning_PLAN]]'s architecture section.

The command registry lives in `blueboard-app/src/commands.rs`, since `Command::run` takes
`&mut App` — it operates on application state that only [[App_crate]] owns.

The command registry's built-in commands (Phase 4) are `save`, `open`, `clear`, `new`
(same effect as `clear` — no multi-board concept yet to make them differ), and `q`.

The command registry is reached two ways: typed into the [[Command_line]] (`:save
board.bbd`), or via a [[Keybindings]] entry that names a command directly with no
arguments.

`save`/`open`'s path argument takes a bare name without the `.bbd` extension (`:save
myboard`) and appends it automatically, unless the given name already ends with `.bbd` —
so `:save foo` and `:open foo` refer to the same `foo.bbd` file.

The command registry also answers `completions(prefix)` — every command name starting
with `prefix`, paired with its usage hint — which drives the [[Command_line]]'s
Tab-completion.

`:delete` (key `d`) calls the active [[Tool_registry|tool]]'s `delete_selection` —
whichever of the three selection-based tools is active deletes its own current selection;
every other tool's default no-ops, reported back as "nothing selected."

