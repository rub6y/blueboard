//! The `:`-command registry. Every command is one [`Command`] implementation registered
//! by name; `config.rs`'s keybindings resolve to these same names, so there's one
//! vocabulary for both the command line and keyboard shortcuts (PLAN.md's architecture
//! section — this is the concrete mechanism meant to make adding commands later, e.g.
//! from an addon, not require touching existing command code).

use blueboard_core::file_format;
use blueboard_core::scene::Scene;

use crate::{App, DEFAULT_BOARD_PATH};

pub trait Command {
    fn name(&self) -> &'static str;

    /// Whether running this command should be recorded as an undo point (the app takes a
    /// scene snapshot around every run and hands it to `History::record` when this is
    /// `true`). Commands whose scene changes are the history mechanism itself — `undo`
    /// and `redo` — return `false`: recording them would push an entry that "undoes the
    /// undo", re-applying the very edit the user just stepped back from.
    fn records_undo(&self) -> bool {
        true
    }

    /// Run the command with everything after the command name (and separating
    /// whitespace) as `args`. `Ok` is a status message to show in the command bar; `Err`
    /// is an error message, shown in the command bar's error color.
    fn run(&self, app: &mut App, args: &str) -> Result<String, String>;
}

#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<Box<dyn Command>>,
}

impl CommandRegistry {
    pub fn with_builtin_commands() -> Self {
        let mut registry = Self::default();
        registry.register(SaveCommand);
        registry.register(OpenCommand);
        registry.register(ClearCommand);
        registry.register(NewCommand);
        registry.register(ToolCommand);
        registry.register(UndoCommand);
        registry.register(RedoCommand);
        registry.register(QuitCommand);
        registry
    }

    pub fn register(&mut self, command: impl Command + 'static) {
        self.commands.push(Box::new(command));
    }

    /// Whether running `name` should be recorded as an undo point — see
    /// [`Command::records_undo`]. An unknown name reports `true`, but it also fails to
    /// run at all, so the snapshot taken around it matches itself and records nothing.
    pub fn records_undo(&self, name: &str) -> bool {
        match self.commands.iter().find(|command| command.name() == name) {
            Some(command) => command.records_undo(),
            None => true,
        }
    }

    pub fn run(&self, app: &mut App, name: &str, args: &str) -> Result<String, String> {
        match self.commands.iter().find(|command| command.name() == name) {
            Some(command) => command.run(app, args),
            None => Err(format!("unknown command: {name}")),
        }
    }
}

/// `.bbd` extension `:save`/`:open` append to a bare name — so `:save foo` and
/// `:open foo` both mean `foo.bbd`, without the user having to type the extension
/// (or getting a different file if they forget it on open vs. save).
const BOARD_EXTENSION: &str = ".bbd";

/// Resolve a `:save`/`:open` argument to an actual file path: no argument means
/// [`DEFAULT_BOARD_PATH`]; otherwise the given name, with [`BOARD_EXTENSION`] appended
/// unless it's already there (so `:save board.bbd` doesn't become `board.bbd.bbd`).
fn resolve_board_path(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return DEFAULT_BOARD_PATH.to_string();
    }
    if trimmed.to_ascii_lowercase().ends_with(BOARD_EXTENSION) {
        trimmed.to_string()
    } else {
        format!("{trimmed}{BOARD_EXTENSION}")
    }
}

struct SaveCommand;
impl Command for SaveCommand {
    fn name(&self) -> &'static str {
        "save"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let path = resolve_board_path(args);
        file_format::save(&app.scene, &path)
            .map(|()| format!("saved {} element(s) to {path}", app.scene.len()))
            .map_err(|err| format!("save to {path} failed: {err}"))
    }
}

struct OpenCommand;
impl Command for OpenCommand {
    fn name(&self) -> &'static str {
        "open"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let path = resolve_board_path(args);
        match file_format::load(&path) {
            Ok(scene) => {
                let count = scene.len();
                app.scene = scene;
                Ok(format!("loaded {count} element(s) from {path}"))
            }
            Err(err) => Err(format!("open {path} failed: {err}")),
        }
    }
}

struct ClearCommand;
impl Command for ClearCommand {
    fn name(&self) -> &'static str {
        "clear"
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        let count = app.scene.len();
        app.scene = Scene::new();
        Ok(format!("cleared {count} element(s)"))
    }
}

/// Same effect as `clear` — there's no concept of multiple distinct boards/files yet to
/// make `:new` meaningfully different. Kept as its own command (not just an alias
/// dispatched earlier) so it shows up in the registry under its own name.
struct NewCommand;
impl Command for NewCommand {
    fn name(&self) -> &'static str {
        "new"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        ClearCommand.run(app, args)
    }
}

struct ToolCommand;
impl Command for ToolCommand {
    fn name(&self) -> &'static str {
        "tool"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let name = args.trim();
        if app.tools.set_active(name) {
            Ok(format!("tool: {name}"))
        } else {
            Err(format!("unknown tool: {name}"))
        }
    }
}

struct UndoCommand;
impl Command for UndoCommand {
    fn name(&self) -> &'static str {
        "undo"
    }

    fn records_undo(&self) -> bool {
        false
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        if app.history.undo(&mut app.scene) {
            Ok("undone".to_string())
        } else {
            Err("nothing to undo".to_string())
        }
    }
}

struct RedoCommand;
impl Command for RedoCommand {
    fn name(&self) -> &'static str {
        "redo"
    }

    fn records_undo(&self) -> bool {
        false
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        if app.history.redo(&mut app.scene) {
            Ok("redone".to_string())
        } else {
            Err("nothing to redo".to_string())
        }
    }
}

struct QuitCommand;
impl Command for QuitCommand {
    fn name(&self) -> &'static str {
        "q"
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        app.should_quit = true;
        Ok(String::new())
    }
}
