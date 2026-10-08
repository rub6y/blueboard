//! Loads `$XDG_CONFIG_HOME/blueboard/config.toml`: keybindings and theme colors, per
//! PLAN.md's architecture section. A missing file, a missing key, or the whole file
//! failing to parse all fall back to documented defaults rather than refusing to start —
//! this is a hand-edited file, and a typo in it shouldn't be able to stop the app from
//! opening.

use std::collections::HashMap;
use std::path::PathBuf;

use blueboard_render::ThemeColors;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default)]
pub struct Config {
    /// Single-character key -> command text, parsed the same way as the `:`-command line
    /// (a command name, optionally followed by a space and its arguments — e.g. `"tool
    /// rect"`). Reuses the command registry's own vocabulary rather than a separate one.
    ///
    /// Note: a `[keybindings]` section present in the file *replaces* the default map
    /// entirely rather than merging into it (a limitation of how `serde`'s per-field
    /// `default` works) — remapping one key currently means re-specifying all of them.
    pub keybindings: HashMap<String, String>,
    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keybindings: default_keybindings(),
            theme: Theme::default(),
        }
    }
}

fn default_keybindings() -> HashMap<String, String> {
    // Deliberately no bare-letter binding for "q" (quit): a destructive, unconfirmed
    // action one unmodified keystroke away is a real risk (this is why vim itself
    // doesn't bind plain `q` to quit either) — `:q` via the command line stays
    // available, requiring the deliberate `:` prefix. A user can still rebind this in
    // their own config if they want the bare shortcut.
    [
        ("s", "save"),
        ("o", "open"),
        ("p", "tool pen"),
        ("r", "tool rect"),
        ("e", "tool ellipse"),
        ("l", "tool line"),
        ("v", "tool select"),
        // `U` (redo) is just a distinct single character, not "shift+u" — with no
        // modifier support in this map, it works only because winit reports Shift+u's
        // logical key as the literal `"U"`, which the lookup in `main.rs` matches as-is.
        ("u", "undo"),
        ("U", "redo"),
    ]
    .into_iter()
    .map(|(key, command)| (key.to_string(), command.to_string()))
    .collect()
}

#[derive(Deserialize, Clone, Copy)]
#[serde(default)]
pub struct Theme {
    pub board_color: [f32; 3],
    pub grid_color: [f32; 3],
    pub stroke_color: [f32; 3],
}

impl Default for Theme {
    fn default() -> Self {
        let defaults = ThemeColors::default();
        Self {
            board_color: defaults.board,
            grid_color: defaults.grid,
            stroke_color: defaults.stroke,
        }
    }
}

impl From<Theme> for ThemeColors {
    fn from(theme: Theme) -> Self {
        Self {
            board: theme.board_color,
            grid: theme.grid_color,
            stroke: theme.stroke_color,
        }
    }
}

/// `$XDG_CONFIG_HOME/blueboard/config.toml`, falling back to `~/.config/...`, then the
/// system temp dir if neither `XDG_CONFIG_HOME` nor `HOME` is set.
pub fn config_file_path() -> PathBuf {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    config_dir.join("blueboard").join("config.toml")
}

pub fn load() -> Config {
    let path = config_file_path();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log::info!("no config file at {}, using defaults", path.display());
            return Config::default();
        }
        Err(err) => {
            log::warn!(
                "failed to read config file {}: {err}, using defaults",
                path.display()
            );
            return Config::default();
        }
    };

    match toml::from_str(&text) {
        Ok(config) => config,
        Err(err) => {
            log::warn!(
                "failed to parse config file {}: {err}, using defaults",
                path.display()
            );
            Config::default()
        }
    }
}
