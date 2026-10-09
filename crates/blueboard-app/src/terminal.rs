//! A magic window's live shell: spawns a real process behind a PTY (`portable-pty`) and
//! interprets its output with `vte` — just the ANSI/VT escape-sequence *parser*, not a
//! full terminal emulator like `alacritty_terminal`. This crate owns a much simpler grid
//! model than a real terminal (`Grid`, below): plain lines of text, cursor row/col, basic
//! cursor-movement and clear-screen/line handling, no color/bold/underline, no scrollback
//! beyond what's on screen. Enough for ordinary shell use (`ls`, `cd`, `git status`, a
//! REPL) — not a real terminal emulator, a full-screen TUI (`vim`, `htop`) will look
//! wrong. Deliberate scope for a first magic-window step — see the ledger.

use std::io::{Read, Write};
use std::sync::mpsc;

/// Fixed character grid size every magic window starts at — no PTY resize support yet
/// (the rect can still be moved/resized on the board like any element, it just doesn't
/// reflow the shell inside it — see this module's doc comment on known limits).
pub const COLS: usize = 80;
pub const ROWS: usize = 24;

/// The simple text grid `Perform` mutates as `vte::Parser::advance` feeds it bytes.
struct Grid {
    lines: Vec<Vec<char>>,
    cursor_row: usize,
    cursor_col: usize,
}

impl Grid {
    fn new() -> Self {
        Self {
            lines: vec![vec![' '; COLS]; ROWS],
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    fn put(&mut self, c: char) {
        if self.cursor_col >= COLS {
            self.newline();
        }
        self.lines[self.cursor_row][self.cursor_col] = c;
        self.cursor_col += 1;
    }

    fn newline(&mut self) {
        self.cursor_col = 0;
        if self.cursor_row + 1 < ROWS {
            self.cursor_row += 1;
        } else {
            self.lines.remove(0);
            self.lines.push(vec![' '; COLS]);
        }
    }

    fn lines(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_string())
            .collect()
    }
}

impl vte::Perform for Grid {
    fn print(&mut self, c: char) {
        self.put(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.cursor_col = 0,
            0x08 => self.cursor_col = self.cursor_col.saturating_sub(1), // backspace
            b'\t' => self.cursor_col = ((self.cursor_col / 8) + 1) * 8,
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let n = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(1) as usize;
        match action {
            'A' => self.cursor_row = self.cursor_row.saturating_sub(n.max(1)),
            'B' => self.cursor_row = (self.cursor_row + n.max(1)).min(ROWS - 1),
            'C' => self.cursor_col = (self.cursor_col + n.max(1)).min(COLS - 1),
            'D' => self.cursor_col = self.cursor_col.saturating_sub(n.max(1)),
            'H' | 'f' => {
                // Cursor position: row;col, 1-indexed, defaulting to 1;1.
                let mut iter = params.iter();
                let row = iter.next().and_then(|p| p.first().copied()).unwrap_or(1);
                let col = iter.next().and_then(|p| p.first().copied()).unwrap_or(1);
                self.cursor_row = (row.max(1) as usize - 1).min(ROWS - 1);
                self.cursor_col = (col.max(1) as usize - 1).min(COLS - 1);
            }
            'J' => {
                // Erase in display — only "clear everything" (2/3) and default (0) are
                // handled, both as a full clear; "clear above cursor" (1) isn't
                // distinguished, a known simplification.
                self.lines = vec![vec![' '; COLS]; ROWS];
                self.cursor_row = 0;
                self.cursor_col = 0;
            }
            'K' => {
                // Erase in line, from cursor to end — the common case (bash's own prompt
                // redraw uses this); "erase to start"/"erase whole line" aren't
                // distinguished.
                for col in self.cursor_col..COLS {
                    self.lines[self.cursor_row][col] = ' ';
                }
            }
            _ => {}
        }
    }
}

pub enum SpawnError {
    Pty(String),
    Command(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::Pty(err) => write!(f, "failed to open a PTY: {err}"),
            SpawnError::Command(err) => write!(f, "failed to spawn shell: {err}"),
        }
    }
}

/// A magic window's live state — the process/PTY/parsed grid this session owns, keyed by
/// element id (see `ElementKind::MagicWindow`'s doc comment on why none of this is saved).
pub struct LiveTerminal {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    parser: vte::Parser,
    grid: Grid,
}

impl LiveTerminal {
    pub fn spawn(shell: &str) -> Result<Self, SpawnError> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows: ROWS as u16,
                cols: COLS as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| SpawnError::Pty(err.to_string()))?;
        let cmd = portable_pty::CommandBuilder::new(shell);
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|err| SpawnError::Command(err.to_string()))?;
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .map_err(|err| SpawnError::Pty(err.to_string()))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| SpawnError::Pty(err.to_string()))?;

        // PTY reads block, so they happen on their own thread and arrive here through a
        // channel `pump` drains non-blockingly each frame — same shape `main.rs`'s
        // `latex::render_math` uses a (synchronous, in that case) call for, just async
        // here since a shell's output is open-ended, not a one-shot result.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            child,
            writer,
            output_rx: rx,
            parser: vte::Parser::new(),
            grid: Grid::new(),
        })
    }

    /// Drains whatever PTY output has arrived since the last call, feeding it through the
    /// ANSI parser into the grid — call once per redraw.
    pub fn pump(&mut self) {
        while let Ok(bytes) = self.output_rx.try_recv() {
            self.parser.advance(&mut self.grid, &bytes);
        }
    }

    pub fn write_input(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
    }

    pub fn lines(&self) -> Vec<String> {
        self.grid.lines()
    }
}

impl Drop for LiveTerminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
