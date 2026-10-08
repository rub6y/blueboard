# App crate

The app crate (`blueboard-app`) runs the winit event loop.

The app crate owns the [[Command_line]]'s input handling and state (active/not, typed
text, last status message) — [[Render_crate]] only renders what the app crate tells it to.

The app crate loads the [[Config_file]] at startup, before creating the window, since the
loaded [[Theme]] is needed to construct the renderer.

The app crate's [[Command_registry]] runs commands via `std::mem::take(&mut self.registry)`
before the call and restores it after — a `Command::run` needs `&mut App` (the same struct
that owns the registry), so the registry can't stay borrowed from `self` while also being
passed `self` mutably.

The app crate depends on [[Core_crate]] and [[Render_crate]].

The app crate smooths raw pointer samples with an exponential moving average (the
[[Perfect_freehand]]-inspired "streamline" fix), then decimates by screen-space distance,
before handing them to [[Fitting]] — smoothing removes jitter from the recorded positions,
decimation removes redundant near-duplicate points.

The app crate logs to both stderr and a persistent file (`$XDG_STATE_HOME/blueboard/
blueboard.log`), with a panic hook writing to the same file, so a crash is diagnosable even
without a visible terminal.
