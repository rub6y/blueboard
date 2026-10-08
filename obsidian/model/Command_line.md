# Command line

The command line is Blueboard's vim-like modal input, entered with `:`.

The command line dispatches to the [[Command_registry]].

The command line is implemented as an egui text widget intercepting `:` in [[App_crate]].

The command line supports actions like opening a new whiteboard, saving, and clearing,
per the original brief.
