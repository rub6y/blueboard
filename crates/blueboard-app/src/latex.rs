//! Renders a `$$...$$` math span (`blueboard_core::text_markup::MathToken::Math`'s raw
//! source) to a transparent RGBA8 image via a real LaTeX install, rather than the hand-
//! rolled math subset an earlier version of the text tool used — see `text_markup.rs`'s
//! doc comment for why that was dropped. Pipeline: wrap the source in a minimal
//! `standalone`+`preview`-class document (tightly crops to content, no page margins to
//! trim), compile with `pdflatex` (shell-escape left off, this binary's own default, so
//! arbitrary user-typed LaTeX can't run shell commands via `\write18`), rasterize the
//! resulting PDF with `pdftocairo -transp` (transparent background), decode via the
//! `image` crate. Cached by exact source string: in memory for the session (`App` owns
//! the map) and on disk (`$XDG_CACHE_HOME/blueboard/latex_cache/`), so reopening a board
//! doesn't recompile formulas unchanged since last time.

use std::path::PathBuf;
use std::process::Command;

/// DPI the PDF is rasterized at — high enough to stay crisp once the resulting texture is
/// scaled up to fill a large text element or a deep zoom; `blueboard-render` scales the
/// decoded image to the element's actual font size at paint time (see
/// `text::paint_runs`), so this only needs to be "high enough," not calibrated to any
/// particular on-screen size.
const RENDER_DPI: u32 = 300;

#[derive(Debug)]
pub enum LatexError {
    /// `pdflatex`/`pdftocairo` isn't on `PATH` — most likely LaTeX isn't installed.
    ToolNotFound(&'static str),
    /// The snippet failed to compile — `log_tail` is the last few lines of `pdflatex`'s
    /// own output, shown in the status bar so a syntax error is at least somewhat
    /// diagnosable without digging through a log file.
    CompileFailed {
        log_tail: String,
    },
    Rasterize(String),
    Decode(String),
    Io(std::io::Error),
}

impl std::fmt::Display for LatexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LatexError::ToolNotFound(tool) => {
                write!(f, "{tool} not found (is LaTeX installed?)")
            }
            LatexError::CompileFailed { log_tail } => {
                write!(f, "LaTeX compile failed: {log_tail}")
            }
            LatexError::Rasterize(msg) => write!(f, "failed to rasterize PDF: {msg}"),
            LatexError::Decode(msg) => write!(f, "failed to decode rendered image: {msg}"),
            LatexError::Io(err) => write!(f, "I/O error: {err}"),
        }
    }
}

pub struct RenderedMath {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// `$XDG_CACHE_HOME/blueboard/latex_cache/`, falling back to `~/.cache/blueboard/...`,
/// then the system temp dir — same fallback chain `config::config_dir` uses, just rooted
/// at the cache dir instead of the config dir since this is regeneratable derived data,
/// not user configuration.
fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("blueboard").join("latex_cache")
}

/// Bumped whenever the *rendered appearance* of a given source string would change for a
/// reason this module controls (not a reason the source text itself captures) — e.g. v1
/// rendered in black ink, v2 switched to white-for-tinting (see `compile_and_rasterize`'s
/// doc comment). Folded into `cache_key` so a stale on-disk entry from an older version
/// never gets served as if it were the new rendering.
const CACHE_VERSION: &str = "v2";

/// A stable, filesystem-safe filename for `source` — just needs to not collide in
/// practice, not be cryptographically strong, so a plain FNV-1a hash is enough.
fn cache_key(source: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in CACHE_VERSION.bytes().chain(source.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Renders `source` (one `$$...$$` span's content) to RGBA8 pixels. Checks the on-disk
/// cache first; a hit skips `pdflatex`/`pdftocairo` entirely. A fresh render is written
/// back to the cache on success (best-effort — a failed cache write doesn't fail the
/// render, the image is still returned).
pub fn render_math(source: &str) -> Result<RenderedMath, LatexError> {
    let cache_path = cache_dir().join(format!("{}.png", cache_key(source)));
    if let Ok(bytes) = std::fs::read(&cache_path) {
        if let Ok(decoded) = decode_png(&bytes) {
            return Ok(decoded);
        }
        // A corrupt/unreadable cache entry falls through to recompiling below.
    }

    let png_bytes = compile_and_rasterize(source)?;
    if let Some(parent) = cache_path.parent() {
        if std::fs::create_dir_all(parent).is_ok() {
            std::fs::write(&cache_path, &png_bytes).ok();
        }
    }
    decode_png(&png_bytes)
}

fn compile_and_rasterize(source: &str) -> Result<Vec<u8>, LatexError> {
    // A fresh scratch directory per call (not reused/cleaned on failure beyond
    // best-effort) — this is temp-dir churn, not state anyone relies on surviving.
    let work_dir = std::env::temp_dir().join(format!(
        "blueboard-latex-{}-{}",
        std::process::id(),
        cache_key(source)
    ));
    std::fs::create_dir_all(&work_dir).map_err(LatexError::Io)?;

    let tex_path = work_dir.join("snippet.tex");
    let pdf_path = work_dir.join("snippet.pdf");
    let png_path = work_dir.join("snippet.png");

    // Rendered in white ink (not whatever color the element actually wants): the cached
    // texture is color-independent, and `blueboard-render` tints it to the element's real
    // color at paint time (multiplying white by any color reproduces it exactly) — so the
    // same compiled span is reusable regardless of which color an element draws it in, and
    // a later `:color` change needs no recompile.
    let document = format!(
        "\\documentclass[preview,border=1pt]{{standalone}}\n\
         \\usepackage{{amsmath,amsfonts,xcolor}}\n\
         \\begin{{document}}\n\
         \\color{{white}}\n\
         $ {source} $\n\
         \\end{{document}}\n"
    );
    std::fs::write(&tex_path, &document).map_err(LatexError::Io)?;

    let pdflatex = run(
        "pdflatex",
        Command::new("pdflatex")
            .arg("-interaction=nonstopmode")
            .arg("-halt-on-error")
            .arg("-output-directory")
            .arg(&work_dir)
            .arg(&tex_path),
    )?;
    if !pdf_path.exists() || !pdflatex.status.success() {
        let log = String::from_utf8_lossy(&pdflatex.stdout);
        let log_tail = log
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" / ");
        std::fs::remove_dir_all(&work_dir).ok();
        return Err(LatexError::CompileFailed { log_tail });
    }

    let pdftocairo = run(
        "pdftocairo",
        Command::new("pdftocairo")
            .arg("-png")
            .arg("-singlefile")
            .arg("-transp")
            .arg("-r")
            .arg(RENDER_DPI.to_string())
            .arg(&pdf_path)
            .arg(work_dir.join("snippet")),
    )?;
    if !png_path.exists() || !pdftocairo.status.success() {
        let message = String::from_utf8_lossy(&pdftocairo.stderr).into_owned();
        std::fs::remove_dir_all(&work_dir).ok();
        return Err(LatexError::Rasterize(message));
    }

    let bytes = std::fs::read(&png_path).map_err(LatexError::Io)?;
    std::fs::remove_dir_all(&work_dir).ok();
    Ok(bytes)
}

fn run(name: &'static str, command: &mut Command) -> Result<std::process::Output, LatexError> {
    match command.output() {
        Ok(output) => Ok(output),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(LatexError::ToolNotFound(name))
        }
        Err(err) => Err(LatexError::Io(err)),
    }
}

fn decode_png(bytes: &[u8]) -> Result<RenderedMath, LatexError> {
    let decoded =
        image::load_from_memory(bytes).map_err(|err| LatexError::Decode(err.to_string()))?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(RenderedMath {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}
