# App crate

The app crate (`blueboard-app`) runs the winit event loop.

The app crate renders the egui UI chrome.

The app crate implements the [[Command_line]].

The app crate loads the [[Config_file]] at startup.

The app crate depends on [[Core_crate]] and [[Render_crate]].

The app crate decimates raw pointer samples by screen-space distance before handing them
to [[Fitting]], so pointer jitter doesn't reach the curve fitter as fake curvature.
