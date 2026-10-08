# Magic window

The magic window is a user-drawn rectangle on the board that renders another live
application inside it (browser, shell, VSCode, PDF viewer).

The magic window is explicitly deferred past the first draft of [[Blueboard]].

The magic window's first implementation strategy is [[Webview_embedding]], chosen because
it covers the brief's motivating use case (reading and annotating a PDF) with no
platform-fragile capture code.

The magic window's second, later strategy is [[Portal_capture]], for apps that cannot be
embedded as a library.

True window reparenting (XEmbed-style) was researched and ruled out for the magic window,
since Wayland does not allow one client to reparent another's surface.
