# Keybindings

Keybindings map a key-chord to a [[Command_registry]] command name.

Keybindings are defined in the [[Config_file]].

Keybindings map a single character, not a full key-chord with modifiers — Phase 4's
implementation is a pragmatic subset of PLAN.md's original "key-chord" design, not full
modifier/special-key support yet.

Keybindings default to `s` -> save and `o` -> open only. There is deliberately no default
binding for quit: a destructive, unconfirmed action one bare keystroke away is a real risk
(found the hard way — a stray keypress during testing closed the app with no crash log,
just a clean, silent exit) — `:q` through the [[Command_line]] stays available, requiring
the deliberate `:` prefix.
