//! Tool+color macros bound to number keys (`0`-`9`), set with `:m <number>` (captures the
//! currently active tool and the current drawing color) and triggered just by pressing
//! that number key afterward — switches both at once.
//!
//! Persisted to their own `macros.toml`, next to `config.toml` but deliberately *not*
//! part of it: `:m` needs to write a macro back to disk right when it's set, and
//! `config.toml` is a hand-edited file a user might have their own comments/formatting
//! in — round-tripping the whole `Config` struct through `toml::to_string` on every `:m`
//! would silently strip all of that. A dedicated file that only this feature ever writes
//! has no such risk.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Macro {
    /// A `Tool::name()` — looked up with `ToolRegistry::set_active` when triggered, same
    /// as `:tool <name>`'s argument.
    pub tool: String,
    /// Hex (`#rrggbb`), so a hand-edited `macros.toml` reads the same way a `:color`
    /// argument would — parsed with `crate::color::parse_color`.
    pub color: String,
}

/// Keyed by the digit as a one-character string (`"0"`-`"9"`), matching how a TOML table
/// key naturally reads in a hand-edited file (`[macros."3"]`) better than a bare integer
/// key would.
pub type Macros = HashMap<String, Macro>;

pub fn macros_file_path() -> PathBuf {
    crate::config::config_dir().join("macros.toml")
}

/// Same forgiving-load stance as `config::load`: a missing file, or one that fails to
/// parse, falls back to no macros rather than refusing to start.
pub fn load() -> Macros {
    let path = macros_file_path();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log::info!("no macros file at {}, starting with none", path.display());
            return Macros::default();
        }
        Err(err) => {
            log::warn!(
                "failed to read macros file {}: {err}, starting with none",
                path.display()
            );
            return Macros::default();
        }
    };

    match toml::from_str(&text) {
        Ok(macros) => macros,
        Err(err) => {
            log::warn!(
                "failed to parse macros file {}: {err}, starting with none",
                path.display()
            );
            Macros::default()
        }
    }
}

/// Writes the whole macro set back to `macros.toml` — called right after `:m` changes
/// one, so a macro set this session survives a restart without a separate `:save`-style
/// step. Logs and gives up on failure (a full disk, a read-only config dir, ...) rather
/// than panicking — losing the persisted copy of a macro is recoverable (it's still live
/// in memory for the rest of this session), a hard crash mid-edit isn't.
pub fn save(macros: &Macros) {
    let path = macros_file_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(err) = std::fs::create_dir_all(parent) {
        log::warn!("failed to create {}: {err}", parent.display());
        return;
    }
    let text = match toml::to_string_pretty(macros) {
        Ok(text) => text,
        Err(err) => {
            log::warn!("failed to encode macros: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(&path, text) {
        log::warn!("failed to write macros file {}: {err}", path.display());
    }
}
