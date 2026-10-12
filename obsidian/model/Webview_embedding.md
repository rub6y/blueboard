# Webview embedding

Webview embedding would have rendered a browser or PDF.js view directly inside a
[[Magic_window]] rectangle using the `wry` crate — the planned second magic-window
strategy, after the embedded shell.

**Scratched, never built.** `wry`'s Linux child-webview embedding
(`WebViewBuilder::build_as_child`, the only way to position it inside an existing window
rather than as its own top-level) only works under X11, not Wayland — and this machine's
desktop is Wayland-native. Rather than force the whole app onto XWayland (or chase an
uncertain GTK-container-coexistence path) just for this one feature, the magic window's
implementation pivoted to [[Portal_capture]] (screen/window capture via PipeWire) instead,
which works natively under Wayland and covers the same browser/PDF motivating use case
(plus terminal/arbitrary-app, for free) without any X11 dependency.

Terminal embedding (`portable-pty` + a terminal renderer) — originally paired with this —
was also scratched, for the same reason: unifying both under one portal-capture strategy
needed no embedded-library code for either.
