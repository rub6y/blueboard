# Config file

The config file is a TOML file at `XDG_CONFIG_HOME/blueboard/config.toml`.

The config file holds [[Keybindings]] and theme colors (board/grid/stroke) as of Phase 4.
Default tool and autosave interval, mentioned in the original Phase 0 design, were left out
of Phase 4 on purpose — there's no tool registry or autosave implementation yet for those
fields to actually configure, and a config field for a feature that doesn't exist is
exactly the kind of speculative addition the project avoids.

The config file falls back to documented defaults if the file is missing, unreadable, or
fails to parse at all, and per-field (via `#[serde(default)]`) if individual top-level
fields are absent — since this is hand-edited and expected to contain typos. One known
limitation: a `[keybindings]` section present in the file replaces the default keybinding
map entirely rather than merging into it (a consequence of how `serde`'s per-field default
works), so remapping one key means re-specifying all of them.

Tool+color macros (`:m <number>`, see [[Command_registry]]) are deliberately **not** part
of this file — they live in a sibling `macros.toml`, written automatically whenever `:m`
sets one. Folding them into `config.toml` would mean round-tripping the whole file through
`toml::to_string` on every `:m`, silently stripping any comments/formatting in a
hand-edited config — a risk a dedicated, app-only-written file doesn't have. `config.toml`
itself stays read-only to the app, same as always.
