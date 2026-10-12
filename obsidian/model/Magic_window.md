# Magic window

The magic window is a user-drawn rectangle on the board that renders another live
application inside it (browser, shell, VSCode, PDF viewer).

The magic window is explicitly deferred past the first draft of [[Blueboard]].

The magic window's implementation strategy is [[Portal_capture]] — a live screen/window
capture stream, not an embedded library. Two earlier strategies were tried and scratched:
an embedded shell via `portable-pty` (shipped, then removed) and a planned `wry` embedded
webview for the browser/PDF case (researched, never built) — see [[Webview_embedding]]'s
doc comment and the ledger for why both were abandoned in favor of portal capture.

True window reparenting (XEmbed-style) was researched and ruled out for the magic window,
since Wayland does not allow one client to reparent another's surface — the same
constraint that killed the `wry` child-webview plan (its Linux child-embedding only works
under X11).

Interacting with (not just viewing) the captured app is deferred — see
[[Remote_desktop_input]].
