//! Loads/saves `default_config.toml` and converts [`blueboard_core::config::Config`]'s
//! plain-data shape to/from the actual runtime types (`GridSettings`, `BackgroundMode`,
//! `ThemeColors`) it has no dependency on. A missing file, a missing key, or the whole
//! file failing to parse all fall back to documented defaults rather than refusing to
//! start.
//!
//! `default_config.toml` is what a brand-new board (`:new`) starts from; each `.bbd` then
//! carries its own embedded `Config` snapshot that can drift from it, the same way a
//! board's elements do (see `blueboard_core::config`'s own doc comment). Previously split
//! across `config.toml`/`macros.toml`/`camera_macros.toml` to protect a hand-edited
//! file's formatting from a `:m`/`:fm` write-back clobbering it — now that editing
//! happens through the in-app config window (or by hand, accepting plain TOML
//! round-tripping like any other app-owned file), that concern no longer applies.

use std::path::PathBuf;

use blueboard_core::config::{
    BackgroundConfig, Config, GridConfig, GridStyleConfig, ImageFitConfig, Theme,
};
use blueboard_render::{BackgroundMode, GridSettings, GridStyle, ImageFit, Renderer, ThemeColors};

// Plain conversion functions, not `From`/`Into` impls — both sides of every one of these
// are foreign types (one from `blueboard-core`, one from `blueboard-render`), which the
// orphan rule forbids implementing a foreign trait between regardless of which crate
// does it.

pub fn theme_to_colors(theme: Theme) -> ThemeColors {
    ThemeColors {
        board: theme.board_color,
        grid: theme.grid_color,
        selection: theme.selection_color,
    }
}

fn grid_style_to_runtime(style: GridStyleConfig) -> GridStyle {
    match style {
        GridStyleConfig::Lines => GridStyle::Lines,
        GridStyleConfig::HorizontalOnly => GridStyle::HorizontalOnly,
        GridStyleConfig::Hex => GridStyle::Hex,
        GridStyleConfig::Dots => GridStyle::Dots,
    }
}

fn grid_style_to_config(style: GridStyle) -> GridStyleConfig {
    match style {
        GridStyle::Lines => GridStyleConfig::Lines,
        GridStyle::HorizontalOnly => GridStyleConfig::HorizontalOnly,
        GridStyle::Hex => GridStyleConfig::Hex,
        GridStyle::Dots => GridStyleConfig::Dots,
    }
}

pub fn grid_to_runtime(config: GridConfig) -> GridSettings {
    GridSettings {
        visible: config.visible,
        spacing: config.spacing,
        style: grid_style_to_runtime(config.style),
    }
}

pub fn grid_to_config(settings: GridSettings) -> GridConfig {
    GridConfig {
        visible: settings.visible,
        spacing: settings.spacing,
        style: grid_style_to_config(settings.style),
    }
}

fn image_fit_to_runtime(fit: ImageFitConfig) -> ImageFit {
    match fit {
        ImageFitConfig::ZoomWithCamera => ImageFit::ZoomWithCamera,
        ImageFitConfig::Tile => ImageFit::Tile,
    }
}

fn image_fit_to_config(fit: ImageFit) -> ImageFitConfig {
    match fit {
        ImageFit::ZoomWithCamera => ImageFitConfig::ZoomWithCamera,
        ImageFit::Tile => ImageFitConfig::Tile,
    }
}

/// The runtime `(BackgroundMode, image_path)` pair a `BackgroundConfig` becomes —
/// `image_path` is `None` unless the mode is `Image` (threaded through separately since
/// `BackgroundMode::Image` itself doesn't carry a path — see `BackgroundConfig::Image`'s
/// doc comment). `renderer` is needed only to decode-and-upload an `Image` background;
/// a decode failure (missing/corrupt file) falls back to a solid fill rather than
/// failing the whole config load, logged rather than silent.
pub fn apply_background(
    config: BackgroundConfig,
    renderer: &mut Renderer,
) -> (BackgroundMode, Option<String>) {
    match config {
        BackgroundConfig::Solid(color) => (BackgroundMode::Solid(color), None),
        BackgroundConfig::Gradient {
            from,
            to,
            angle_deg,
        } => (
            BackgroundMode::Gradient {
                from,
                to,
                angle_deg,
            },
            None,
        ),
        BackgroundConfig::Image { path, fit } => {
            let full_path = background_images_dir().join(&path);
            match image::open(&full_path) {
                Ok(decoded) => {
                    let rgba = decoded.to_rgba8();
                    let (width, height) = rgba.dimensions();
                    renderer.set_background_image(rgba.as_raw(), width, height);
                    (BackgroundMode::Image(image_fit_to_runtime(fit)), Some(path))
                }
                Err(err) => {
                    log::warn!(
                        "config: failed to load background image {full_path:?}: {err}, falling back to solid"
                    );
                    (BackgroundMode::Solid(ThemeColors::default().board), None)
                }
            }
        }
    }
}

/// The inverse of [`apply_background`] — builds the saveable `BackgroundConfig` for
/// whatever's currently active. `image_path` must be `Some` whenever `mode` is
/// `BackgroundMode::Image` (the two are always set together — see `App::
/// background_image_path`'s doc comment); if it's unexpectedly `None`, there's no path
/// to re-decode from on load, so this falls back to a solid fill rather than saving an
/// `Image` config entry that would just fail to load next time.
pub fn background_to_config(mode: BackgroundMode, image_path: Option<&str>) -> BackgroundConfig {
    match mode {
        BackgroundMode::Solid(color) => BackgroundConfig::Solid(color),
        BackgroundMode::Gradient {
            from,
            to,
            angle_deg,
        } => BackgroundConfig::Gradient {
            from,
            to,
            angle_deg,
        },
        BackgroundMode::Image(fit) => match image_path {
            Some(path) => BackgroundConfig::Image {
                path: path.to_string(),
                fit: image_fit_to_config(fit),
            },
            None => {
                log::warn!(
                    "config: background is an image with no tracked path, saving as solid instead"
                );
                BackgroundConfig::Solid(ThemeColors::default().board)
            }
        },
    }
}

/// `$XDG_CONFIG_HOME/blueboard/`, falling back to `~/.config/blueboard/`, then the system
/// temp dir if neither `XDG_CONFIG_HOME` nor `HOME` is set.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("blueboard")
}

pub fn default_config_file_path() -> PathBuf {
    config_dir().join("default_config.toml")
}

/// Where background images set via `:background image`/the config window get copied to —
/// a `Config`'s `BackgroundConfig::Image::path` is always just a filename relative to this
/// directory, never the original source path, so a saved config stays valid even if the
/// source file is later moved, renamed, or deleted.
pub fn background_images_dir() -> PathBuf {
    config_dir().join("backgrounds")
}

/// Copies `source` into [`background_images_dir`], returning the filename it was copied
/// to (relative to that directory) — what actually gets stored in a `Config`. Collisions
/// (another image already imported under the same filename) are resolved by appending
/// `-1`, `-2`, ... before the extension, rather than overwriting an unrelated image that
/// happens to share a name.
pub fn import_background_image(source: &std::path::Path) -> std::io::Result<String> {
    let dir = background_images_dir();
    std::fs::create_dir_all(&dir)?;
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "background".to_string());
    let ext = source.extension().map(|e| e.to_string_lossy().into_owned());
    let mut filename = match &ext {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem.clone(),
    };
    let mut suffix = 1;
    while dir.join(&filename).exists() && dir.join(&filename) != source {
        filename = match &ext {
            Some(ext) => format!("{stem}-{suffix}.{ext}"),
            None => format!("{stem}-{suffix}"),
        };
        suffix += 1;
    }
    let dest = dir.join(&filename);
    if dest != source {
        std::fs::copy(source, &dest)?;
    }
    Ok(filename)
}

/// What a brand-new board (`:new`) starts from, and the very first thing applied at
/// startup (before any board is opened).
pub fn load_default() -> Config {
    let path = default_config_file_path();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log::info!(
                "no default config at {}, using built-in defaults",
                path.display()
            );
            return Config::default();
        }
        Err(err) => {
            log::warn!(
                "failed to read default config {}: {err}, using built-in defaults",
                path.display()
            );
            return Config::default();
        }
    };

    match toml::from_str(&text) {
        Ok(config) => config,
        Err(err) => {
            log::warn!(
                "failed to parse default config {}: {err}, using built-in defaults",
                path.display()
            );
            Config::default()
        }
    }
}

/// Writes `config` to `default_config.toml` — the config window's "save as default"
/// action. Logs and gives up on failure (a full disk, a read-only config dir, ...)
/// rather than panicking, same forgiving stance `load_default` has.
pub fn save_as_default(config: &Config) {
    let path = default_config_file_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(err) = std::fs::create_dir_all(parent) {
        log::warn!("failed to create {}: {err}", parent.display());
        return;
    }
    let text = match toml::to_string_pretty(config) {
        Ok(text) => text,
        Err(err) => {
            log::warn!("failed to encode default config: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(&path, text) {
        log::warn!("failed to write default config {}: {err}", path.display());
    }
}
