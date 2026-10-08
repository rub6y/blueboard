# Command registry

The command registry is a central table of `:`-commands, each implementing one `Command`
trait (`name`, `run`).

The command registry is what makes new commands addable without touching existing command
code.

The command registry's names are reused by the [[Config_file]]'s [[Keybindings]], so there
is no separate keybinding vocabulary.

The command registry is the concrete mechanism behind Blueboard's addon-friendliness, per
[[planning_PLAN]]'s architecture section.
