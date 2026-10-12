# Blueboard

An infinite, vector-based whiteboard — pan, zoom, draw, and annotate on a canvas with no
fixed edges. Built from scratch in Rust (`winit` + `wgpu` + `egui`) as both a real app and
a systems-programming teaching project: GPU rendering, vector geometry, binary file
formats, and Wayland portal integration, each built and explained a piece at a time.

## Features

- **Semi-infinite canvas** — pan and zoom (1%–400%) over a board with no fixed bounds.
- **Vector drawing** — pen (freehand, curve-fit from raw samples), rectangle, ellipse,
  line, each with a configurable color and stroke width.
- **Text** — markdown-ish bold (`**bold**`) and a basic inline LaTeX math subset
  (`$$x^2 + \frac{1}{2}$$`, Greek letters, super/subscript).
- **Images & GIFs** — paste from the OS clipboard or load a file; GIFs animate in place.
- **Magic windows** — a drawn rectangle that streams a live screen/window capture via the
  `org.freedesktop.portal.ScreenCast` portal + PipeWire (the same mechanism Discord/Zoom
  use for screen share) — read a PDF or watch another app without leaving the board.
- **Selection & editing** — click, rectangle, or lasso select; drag to move; resize
  handles scale a shape's geometry *and* its stroke width/font size proportionally;
  whole-object and partial (circular) erasers.
- **Customizable background & grid** — solid color, gradient, or image background; grid
  styles (lines, hex, dots, horizontal-only).
- **Undo/redo**, a vim-style `:`-command line with live Tab/arrow-key completion, and
  tool/camera macros bound to number/function keys.
- **`.bbd` file format** — `serde` + `bincode` + `zstd`, versioned so an incompatible file
  is rejected with a clear error instead of being misread.

## Quick start

Blueboard uses [Nix](https://nixos.org/) + [direnv](https://direnv.net/) for a fully
reproducible dev environment — no manual toolchain setup.

```sh
git clone https://github.com/rub6y/blueboard.git
cd blueboard
nix develop -c cargo run -p blueboard-app
```

A log is written to both stderr and `$XDG_STATE_HOME/blueboard/blueboard.log` (falling
back to `~/.local/state/...`).

Without Nix, you'll need a stable Rust toolchain plus the system libraries `flake.nix`
lists (Vulkan loader, X11/Wayland libs, `pipewire`, LaTeX for the text tool's math
rendering) — using the flake is the supported path.

## Usage

### The command line

Press `:` to open it, type a command, Enter to run it, Escape to cancel. The bar's left
side always shows the active tool's name; its right side shows either the command line,
the result of the last command, or an error.

Completions appear live as you type (not just after Tab) — cycle through them with
**Tab**, or **←/→**, and Enter runs whichever is highlighted.

| Command | Usage | What it does |
|---|---|---|
| `:save` | `[name]` | Saves the board to `name.bbd` (defaults to `board.bbd`). |
| `:open` | `[name]` | Loads a board, replacing the current scene. |
| `:clear` / `:new` | | Deletes every element on the board. |
| `:tool` | `<name>` | Switches the active tool (see Tools below). |
| `:delete` | | Deletes the current selection. |
| `:yank` / `:paste` | | Copies the current selection / pastes it back (or an external clipboard image, if nothing's yanked). |
| `:color` | `<name\|#hex\|picker>` | Sets the next stroke's color, or toggles the interactive picker. |
| `:width` | `<number>` | Sets the next stroke's width (board-space units). |
| `:grid` | `<show\|hide\|bigger\|smaller\|<number>\|style <name>>` | Grid visibility, spacing, or style (`lines`/`horizontal`/`hex`/`dots`). |
| `:background` | `<color <hex>\|gradient <hex> <hex> <deg>\|image <path> [zoom\|tile]>` | Sets the board's background fill. |
| `:gif` | `<path>` | Places an animated GIF as an image element. |
| `:m` / `:fm` | `<0-9>` / `<1-12>` | Binds the active tool+color / camera pan+zoom to a number/function key. |
| `:undo` / `:redo` | | Steps through edit history. |
| `:help` | | Lists every command and its usage hint. |
| `:q` | | Quits. Deliberately *not* bound to a bare keystroke. |

### Tools

Switch via `:tool <name>` or the keybinding below.

| Tool | `:tool` name | Key | What it does |
|---|---|---|---|
| Pen | `pen` | `p` | Freehand drawing, smoothed and curve-fit on release. |
| Rectangle | `rect` | `r` | Drag corner to corner. |
| Ellipse | `ellipse` | `e` | Drag a bounding box. |
| Line | `line` | `l` | Drag endpoint to endpoint. |
| Text | `text` | `t` | Click to place, type, Enter to commit. Ctrl+V pastes clipboard text. |
| Magic window | `magic-window` | `m` | Drag a rect, pick a capture source from the system picker. |
| Select | `select` | `v` | Click to select, drag to move; drag a handle to resize. |
| Eraser (partial) | `eraser` | `x` | Drag a circular eraser; subtracts exactly what it passes over. |
| Eraser (whole object) | `eraser-object` | `X` | Click/drag to delete whole elements. |
| Select (rectangle) | `select-rect` | `V` | Drag to select every element the box touches. |
| Select (lasso) | `select-lasso` | `L` | Freehand loop select. |

### Camera

| Input | Effect |
|---|---|
| Middle- or right-click drag | Pan. |
| Scroll wheel | Zoom, anchored on the cursor. |

### Status bar

The bottom-left shows the active tool's name and the current color's hex code — both are
clickable: the tool name opens a grid to pick any tool, the hex code opens the interactive
color picker.

### Config

`$XDG_CONFIG_HOME/blueboard/config.toml` (optional — missing file/key/parse error all fall
back to defaults): a `[keybindings]` table (remaps *replace* the whole default map, not
merge into it) and a `[theme]` table (`board_color`/`grid_color`/`stroke_color`, each an
`[r, g, b]` 0.0–1.0 array). Tool/color macros and camera macros persist to their own
`macros.toml`/`camera_macros.toml` next to it.

## Project structure

```
crates/
  blueboard-core/    scene graph, file format, undo/redo — no rendering, no UI
  blueboard-render/  wgpu pipeline, tessellation, egui chrome — no windowing
  blueboard-app/     winit event loop, command/tool registries, wires everything together
planning/            the original brief, researched rationale, and phase-by-phase plan
obsidian/            a linked-note vault tracking decisions and how pieces relate
```

Every `:`-command and drawing tool is one trait implementation registered in a table
(`Command`/`Tool`), not a hardcoded match arm — the explicit design goal is that adding
either never means touching existing command/tool code.

## Status

Actively developed, phase by phase — see `planning/PLAN.md` for the roadmap and
`obsidian/meta/ledger.md` for a running decision log. Not yet at a 1.0; the file format
and config layout may still change between versions (each is version-checked and will
tell you clearly if a file/config predates a breaking change, rather than silently
misreading it).

## License

No license file yet — all rights reserved by default until one is added.
