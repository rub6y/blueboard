# Webview embedding

Webview embedding renders a browser or PDF.js view directly inside a [[Magic_window]]
rectangle using the `wry` crate.

Webview embedding is paired with terminal embedding (`portable-pty` + a terminal
renderer) for the shell use case.

Webview embedding requires no compositor permission dialogs and no per-platform capture
backend, unlike [[Portal_capture]].
