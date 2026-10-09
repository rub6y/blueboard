//! Camera macros bound to the function keys (`F1`-`F12`), set with `:fm <number>`
//! (captures the camera's current pan+zoom) and triggered just by pressing that F-key
//! afterward — jumps the camera straight back, no tool/color change (that's
//! `crate::macros`'s job, on the digit keys).
//!
//! Persisted to their own `camera_macros.toml`, next to `macros.toml`/`config.toml` for
//! the same reason `macros.rs` gives for its own file: `:fm` needs to write back
//! immediately, and round-tripping `config.toml` through `toml::to_string` on every `:fm`
//! would strip a hand-edited file's comments/formatting.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct CameraMacro {
    /// Board-space point at the viewport center — `Camera::pan`, as plain fields since
    /// `Camera` itself (`blueboard-core`) deliberately doesn't derive `Serialize` (keeps
    /// persistence decoupled from the runtime type, same relationship `config.rs`'s
    /// `Theme`/`ThemeColors` has).
    pub pan: [f64; 2],
    pub zoom: f64,
}

/// Keyed `"F1"`-`"F12"`, matching how a TOML table key naturally reads in a hand-edited
/// file (`[macros."F3"]`).
pub type CameraMacros = HashMap<String, CameraMacro>;

pub fn camera_macros_file_path() -> PathBuf {
    crate::config::config_dir().join("camera_macros.toml")
}

/// Same forgiving-load stance as `config::load`/`macros::load`: a missing file, or one
/// that fails to parse, falls back to no macros rather than refusing to start.
pub fn load() -> CameraMacros {
    let path = camera_macros_file_path();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log::info!(
                "no camera macros file at {}, starting with none",
                path.display()
            );
            return CameraMacros::default();
        }
        Err(err) => {
            log::warn!(
                "failed to read camera macros file {}: {err}, starting with none",
                path.display()
            );
            return CameraMacros::default();
        }
    };

    match toml::from_str(&text) {
        Ok(macros) => macros,
        Err(err) => {
            log::warn!(
                "failed to parse camera macros file {}: {err}, starting with none",
                path.display()
            );
            CameraMacros::default()
        }
    }
}

/// Writes the whole camera-macro set back to `camera_macros.toml` — called right after
/// `:fm` changes one, so it survives a restart without a separate `:save`-style step.
/// Logs and gives up on failure rather than panicking, same as `macros::save`.
pub fn save(macros: &CameraMacros) {
    let path = camera_macros_file_path();
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
            log::warn!("failed to encode camera macros: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(&path, text) {
        log::warn!(
            "failed to write camera macros file {}: {err}",
            path.display()
        );
    }
}
