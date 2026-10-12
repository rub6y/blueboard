//! The plain-data shape of a board's configuration: keybindings, theme, grid,
//! background, and tool/camera macros. Lives in this crate (not `blueboard-app`, where
//! loading/saving and the in-app editor live) purely so [`crate::file_format`] can embed
//! it in a `.bbd` — `blueboard-core` doesn't and can't depend on `blueboard-app` or
//! `blueboard-render`, so the actual runtime types this gets converted to/from
//! (`GridSettings`, `BackgroundMode`, `ThemeColors`) stay out of this module entirely;
//! `blueboard-app`'s own `config.rs` owns those conversions, since it depends on both.
//!
//! A board's `Config` can drift from `default_config.toml` (what a brand-new board
//! starts from) the same way its elements do — opening a board replaces the whole active
//! config with its embedded one, saving snapshots whatever's currently active.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Single-character key -> command text, parsed the same way as the `:`-command line
    /// (e.g. `"tool rect"`). A `[keybindings]` section present in a file *replaces* the
    /// default map entirely rather than merging into it (a `serde` `#[serde(default)]`
    /// limitation) — remapping one key means re-specifying all of them you want to keep.
    pub keybindings: HashMap<String, String>,
    pub theme: Theme,
    pub grid: GridConfig,
    pub background: BackgroundConfig,
    /// Tool+color macros bound to number keys (`:m`) — see `Macro`'s doc comment.
    pub macros: HashMap<String, Macro>,
    /// Camera (pan+zoom) macros bound to function keys (`:fm`) — see `CameraMacro`'s doc
    /// comment.
    pub camera_macros: HashMap<String, CameraMacro>,
}

/// Default single-character keybindings. Deliberately no bare-letter binding for "q"
/// (quit): a destructive, unconfirmed action one unmodified keystroke away is a real risk
/// (this is why vim itself doesn't bind plain `q` to quit either) — `:q` via the command
/// line stays available, requiring the deliberate `:` prefix.
pub fn default_keybindings() -> HashMap<String, String> {
    [
        ("s", "save"),
        ("o", "open"),
        ("p", "tool pen"),
        ("r", "tool rect"),
        ("e", "tool ellipse"),
        ("l", "tool line"),
        ("t", "tool text"),
        ("m", "tool magic-window"),
        ("v", "tool select"),
        ("x", "tool eraser"),
        ("d", "delete"),
        ("c", "color picker"),
        // Same "distinct literal character via Shift" trick as `u`/`U` below — not a
        // modifier binding, just the fact that winit reports Shift+x's logical key as the
        // literal `"X"`.
        ("X", "tool eraser-object"),
        // `V`/`L` ("rectangle variant of select" / mnemonic "Lasso") — same Shift trick
        // again, not related to lowercase `v`/`l`'s own tools beyond the mnemonic.
        ("V", "tool select-rect"),
        ("L", "tool select-lasso"),
        // `U` (redo) is just a distinct single character, not "shift+u" — with no
        // modifier support in this map, it works only because winit reports Shift+u's
        // logical key as the literal `"U"`, which the lookup in `main.rs` matches as-is.
        ("u", "undo"),
        ("U", "redo"),
        // `y`/`Y` (yank/paste) — same Shift-gives-a-distinct-literal-character trick as
        // `u`/`U` above, not a modifier binding. Paste isn't bound to vim's own `p`: that's
        // already this app's pen-tool key, so it sits on yank's shift-variant instead.
        ("y", "yank"),
        ("Y", "paste"),
    ]
    .into_iter()
    .map(|(key, command)| (key.to_string(), command.to_string()))
    .collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keybindings: default_keybindings(),
            theme: Theme::default(),
            grid: GridConfig::default(),
            background: BackgroundConfig::default(),
            macros: HashMap::new(),
            camera_macros: HashMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(default)]
pub struct Theme {
    pub board_color: [f32; 3],
    pub grid_color: [f32; 3],
    /// Selection-box outline and eraser-cursor color — *not* a drawn stroke's color. See
    /// `blueboard_render::ThemeColors::selection`'s doc comment.
    pub selection_color: [f32; 3],
    /// The default drawing color for a brand-new board (what `current_color` starts as) —
    /// not a GPU theme value at all, just a config default; `blueboard-app` threads it
    /// through to `current_color` on load/apply, same as `:color` would.
    pub stroke_color: [f32; 3],
}

impl Default for Theme {
    fn default() -> Self {
        // `board_color`/`grid_color`/`selection_color` must match
        // `blueboard_render::ThemeColors::default()` — duplicated here rather than read
        // from it, since this crate can't depend on `blueboard-render` (see this module's
        // doc comment). `stroke_color` must match `scene::DEFAULT_ELEMENT_COLOR`.
        Self {
            board_color: [0.05, 0.08, 0.20],
            grid_color: [0.3, 0.38, 0.58],
            selection_color: [1.0, 1.0, 1.0],
            stroke_color: [1.0, 1.0, 1.0],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridStyleConfig {
    #[default]
    Lines,
    HorizontalOnly,
    Hex,
    Dots,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(default)]
pub struct GridConfig {
    pub visible: bool,
    pub spacing: f32,
    pub style: GridStyleConfig,
}

impl Default for GridConfig {
    fn default() -> Self {
        // Must match `blueboard_render::GridSettings::default()`.
        Self {
            visible: true,
            spacing: 100.0,
            style: GridStyleConfig::Lines,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum ImageFitConfig {
    ZoomWithCamera,
    Tile,
}

/// Mirrors `blueboard_render::BackgroundMode`, with one difference: `Image` carries the
/// source file's `path` — the runtime `BackgroundMode::Image` doesn't (its pixels live
/// decoded in the `Renderer` instead), but a config needs the path to re-decode from on
/// load, so `blueboard-app`'s conversion threads it through separately (see
/// `blueboard-app`'s `App::background_image_path`).
#[derive(Serialize, Deserialize, Clone)]
pub enum BackgroundConfig {
    Solid([f32; 3]),
    Gradient {
        from: [f32; 3],
        to: [f32; 3],
        angle_deg: f32,
    },
    Image {
        path: String,
        fit: ImageFitConfig,
    },
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        // Matches `Theme::default()`'s `board_color` above — the usual starting
        // background is a solid fill in the default theme's board color.
        Self::Solid([0.05, 0.08, 0.20])
    }
}

/// A tool+color macro bound to a number key (`:m <0-9>`) — switches both at once when the
/// digit is pressed afterward (outside the command line).
#[derive(Serialize, Deserialize, Clone)]
pub struct Macro {
    /// A `Tool::name()` — looked up with `ToolRegistry::set_active` when triggered, same
    /// as `:tool <name>`'s argument.
    pub tool: String,
    /// Hex (`#rrggbb`), so a hand-edited config file reads the same way a `:color`
    /// argument would — parsed with `blueboard-app`'s `color::parse_color`.
    pub color: String,
}

/// A camera (pan+zoom) macro bound to a function key (`:fm <1-12>`) — jumps the camera
/// straight back when the F-key is pressed afterward, no tool/color change.
#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct CameraMacro {
    pub pan: [f64; 2],
    pub zoom: f64,
}

/// Keyed by the digit as a one-character string (`"0"`-`"9"`), matching how a TOML table
/// key naturally reads in a hand-edited file (`[macros."3"]`) better than a bare integer
/// key would.
pub type Macros = HashMap<String, Macro>;

/// Keyed `"F1"`-`"F12"`, matching how a TOML table key naturally reads in a hand-edited
/// file (`[camera_macros."F3"]`).
pub type CameraMacros = HashMap<String, CameraMacro>;
