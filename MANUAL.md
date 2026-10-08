# Blueboard manual

A reference for everything Blueboard can currently do: commands, tools, and keybindings.
For the *why* behind any of this, see `planning/PLAN.md`/`RESEARCH.md` and the
`obsidian/` vault's ledger — this file only documents current behavior.

## Starting the app

```
nix develop -c cargo run -p blueboard-app
```

A log is written to both stderr and `$XDG_STATE_HOME/blueboard/blueboard.log` (falls back
to `~/.local/state/...`), including a line on clean quit — useful for telling a deliberate
exit apart from a crash.

## The command bar

Always visible at the bottom of the window. Its left side permanently shows the **active
tool's name** — it doubles as a status line, not just a place to type commands.

## The command line

Press `:` to open it (vim-style), type a command, Enter to run it, Escape to cancel. The
bar also shows the result (or error) of the last command run until the next one replaces
it.

**Tab-completion**: as soon as what you've typed (before any space) matches one or more
command names, they're listed above the input — live, updating as you type, not only
after pressing Tab. Press Tab to cycle a highlighted selection through the matches
(wrapping back to the first after the last); the input fills with whatever's highlighted,
so Enter then runs it. Only the command *name* completes — once you've typed a space onto
arguments, completion stops, with one exception: `:tool <name>`'s argument also
completes against registered tool names (e.g. `:tool r` + Tab -> `:tool rect`), since
that's the one built-in command whose argument has a fixed, known vocabulary.

| Command | Usage | What it does |
|---|---|---|
| `:save` | `:save [name]` | Saves the board to `name.bbd` (`.bbd` appended if missing). No name saves to the default `board.bbd`. |
| `:open` | `:open [name]` | Loads a board from `name.bbd`, replacing the current scene. No name loads `board.bbd`. |
| `:clear` | `:clear` | Deletes every element on the board. |
| `:new` | `:new` | Same as `:clear` — there's no concept of multiple open boards yet. |
| `:tool` | `:tool <name>` | Switches the active tool by name (see the Tools table below). |
| `:undo` | `:undo` | Steps back one edit. |
| `:redo` | `:redo` | Re-applies an edit just undone. |
| `:help` | `:help` | Lists every command and its usage hint, one line, in the command bar. |
| `:q` | `:q` | Quits. Deliberately *not* also bound to a bare keystroke (not even `q`) — a destructive, unconfirmed action shouldn't be one accidental keypress away. |

Almost every command is recorded as one undo step automatically (a scene snapshot is
taken before and after it runs); `:undo`/`:redo` themselves are the one exception, so
undoing doesn't record "undo the undo."

## Tools

Switch tools either via `:tool <name>` or the keybinding below (both call the same
underlying switch). The active tool is shown in the command bar's status line after
switching.

| Tool | `:tool` name | Default key | What it does |
|---|---|---|---|
| Pen | `pen` | `p` | Freehand drawing. Raw pointer samples are smoothed (exponential moving average), decimated, then curve-fit into a single Bezier path on release. |
| Rectangle | `rect` | `r` | Drag from one corner to the opposite corner. |
| Ellipse | `ellipse` | `e` | Drag a bounding box; the ellipse inscribes it. |
| Line | `line` | `l` | Drag from one endpoint to the other. |
| Select | `select` | `v` | Click an element's outline to select it (shown as a bounding-box outline); drag to move it. Click empty space to deselect. |
| Eraser (partial) | `eraser` | `x` | Drag a circular eraser over the board. Subtracts exactly the circled region from whatever it passes over — a stroke erased in the middle splits into two pieces; a closed shape (rect/ellipse) partially erased becomes an open arc; fully circled, an element disappears entirely. Shows a circle-outline cursor at the eraser's size. |
| Eraser (whole object) | `eraser-object` | `X` (Shift+x) | Click or drag over elements to delete them whole — same outline-click targeting as Select, but deletes instead of selecting. |

Shape tools (rect/ellipse/line) commit nothing on a plain click with no drag (a
zero-length shape is silently skipped). A click with the pen needs at least two recorded
points to produce a stroke, for the same reason.

Both erasers mutate the board live as you drag — there's no separate "commit" step — but
the whole drag still collapses into a single `:undo` step.

## Keybindings

Single-character keys only, no modifiers except the "Shift gives a different character"
trick used for `eraser-object` (`X`) and `redo` (`U`) — these aren't Shift-aware bindings,
they just match the distinct character winit reports for Shift+<key>.

| Key | Command |
|---|---|
| `s` | `:save` |
| `o` | `:open` |
| `p` | `:tool pen` |
| `r` | `:tool rect` |
| `e` | `:tool ellipse` |
| `l` | `:tool line` |
| `v` | `:tool select` |
| `x` | `:tool eraser` |
| `X` | `:tool eraser-object` |
| `u` | `:undo` |
| `U` | `:redo` |

Rebindable via `$XDG_CONFIG_HOME/blueboard/config.toml`'s `[keybindings]` table (key ->
command text, same syntax as the command line, e.g. `"tool rect"`). Note: a
`[keybindings]` section in the file *replaces* the whole default map, not merges into
it — remapping one key currently means re-specifying all of them you want to keep.

## Camera

| Input | Effect |
|---|---|
| Middle-click drag | Pan. |
| Scroll wheel | Zoom, anchored on the cursor (the board point under the mouse stays fixed). Range 1%-400%. |

## Config file

`$XDG_CONFIG_HOME/blueboard/config.toml` (falls back to `~/.config/...`). Optional — a
missing file, missing key, or parse error all fall back to documented defaults rather than
refusing to start. Two sections:

- `[keybindings]` — see above.
- `[theme]` — `board_color`, `grid_color`, `stroke_color`, each an `[r, g, b]` array of
  0.0-1.0 floats. Fixed for the process's lifetime (no live reload).

## File format

`.bbd` files: the scene, serialized with `serde`/`bincode`, then `zstd`-compressed into a
single blob. Versioned — a file written by a future incompatible version is rejected with
a clear error rather than misread.

## What's not here yet

Per `planning/PLAN.md`'s Phase 5: a text tool, and basic level-of-detail rendering for
zoomed-out boards. Multiple open boards, a non-destructive addon-loading system beyond the
in-binary command/tool trait registries, and the "magic window" (embedding another live
app inside a drawn rectangle) are all explicitly designed-for-later, not built.
