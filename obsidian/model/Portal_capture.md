# Portal capture

Portal capture is now the [[Magic_window]]'s *only* implementation strategy — the earlier
embedded-shell (`portable-pty`) and planned-but-unbuilt [[Webview_embedding]] (`wry`)
strategies were scratched in favor of it (see the ledger entry on why: `wry`'s child-webview
embedding needs X11, and this machine's desktop is Wayland-native).

Portal capture uses `org.freedesktop.portal.ScreenCast` (xdg-desktop-portal) to get a
PipeWire video stream of a chosen window/screen/output, decoded into a GPU texture — the
same mechanism Discord/Zoom/OBS use for screen share, including the compositor's own native
source picker (the app never draws one itself, and has no visibility into what's offered).

Portal capture is the only sanctioned route to another window's pixels on Wayland, since
Wayland's security model forbids any other cross-window pixel access.

Implemented in `blueboard-app/src/screencast.rs`: `ashpd` (async) drives the portal
handshake (create session -> select sources -> start -> open PipeWire remote), handing off a
PipeWire node id + fd to the `pipewire` crate (sync, callback-driven) on a background thread
per magic window. Decoded frames (RGBA/RGBx/BGRx/RGB only — no DMA-BUF import, no YUV — see
the module's own doc comment) feed `Renderer::set_element_texture`, the same per-element
GPU-texture path [[Blueboard]]'s pasted images and GIF playback already use — so a magic
window renders as an ordinary textured quad, not a bespoke draw path.

Portal capture requires the user to re-grant access per session unless the portal's
`persist_mode` is used — v1 always passes `PersistMode::DoNot` (fresh picker every time a
magic window starts capturing), no restore-token re-pick command yet.

Known v1 limitations: no input forwarding into the captured app yet ([[Remote_desktop_input]]
is the researched-but-unbuilt path for that); no re-pick command if a source is dismissed/
cancelled; a magic window element deleted while its compositor picker dialog is still open
can leave its capture thread running until the user responds to that dialog.
