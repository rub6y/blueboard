# App crate

The app crate (`blueboard-app`) runs the winit event loop.

The app crate renders the egui UI chrome.

The app crate implements the [[Command_line]].

The app crate loads the [[Config_file]] at startup.

The app crate depends on [[Core_crate]] and [[Render_crate]].

The app crate smooths raw pointer samples with an exponential moving average (the
[[Perfect_freehand]]-inspired "streamline" fix), then decimates by screen-space distance,
before handing them to [[Fitting]] — smoothing removes jitter from the recorded positions,
decimation removes redundant near-duplicate points.

The app crate logs to both stderr and a persistent file (`$XDG_STATE_HOME/blueboard/
blueboard.log`), with a panic hook writing to the same file, so a crash is diagnosable even
without a visible terminal.
