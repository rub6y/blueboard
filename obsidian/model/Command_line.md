# Command line

The command line is Blueboard's vim-like modal input, entered with `:`.

The command line dispatches to the [[Command_registry]].

The command line's input handling (intercepting `:`, collecting typed characters, Enter to
run, Escape to cancel) is done directly from raw `winit` key events in [[App_crate]], not
through an egui text widget — egui is used only to *render* the bar, not to own the input.

The command line supports actions like opening a new whiteboard, saving, and clearing,
per the original brief.

As of the Tab-completion work, the command line also shows live completions from the
[[Command_registry]] — filtered by whatever's typed so far, before Tab is even pressed —
and Tab cycles a highlighted selection through them, filling the input. Its bar is now
always visible (not just while composing/showing a status), since it's also where the
active tool's name is shown — see [[Render_crate]].
