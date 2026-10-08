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
        registry.register(QuitCommand);
        registry
    }

    pub fn register(&mut self, command: impl Command + 'static) {
        self.commands.push(Box::new(command));
    }

    pub fn run(&self, app: &mut App, name: &str, args: &str) -> Result<String, String> {
        match self.commands.iter().find(|command| command.name() == name) {
            Some(command) => command.run(app, args),
            None => Err(format!("unknown command: {name}")),
        }
    }
}

fn path_or_default(args: &str) -> &str {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        DEFAULT_BOARD_PATH
    } else {
        trimmed
    }
}

struct SaveCommand;
impl Command for SaveCommand {
    fn name(&self) -> &'static str {
        "save"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let path = path_or_default(args);
        file_format::save(&app.scene, path)
            .map(|()| format!("saved {} stroke(s) to {path}", app.scene.len()))
            .map_err(|err| format!("save to {path} failed: {err}"))
    }
}

struct OpenCommand;
impl Command for OpenCommand {
    fn name(&self) -> &'static str {
        "open"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let path = path_or_default(args);
        match file_format::load(path) {
            Ok(scene) => {
                let count = scene.len();
                app.scene = scene;
                Ok(format!("loaded {count} stroke(s) from {path}"))
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
        Ok(format!("cleared {count} stroke(s)"))
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
