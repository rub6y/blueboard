# Portal capture

Portal capture is the second, later strategy for the [[Magic_window]], for apps that
can't be embedded as a library (e.g. real VSCode).

Portal capture uses `org.freedesktop.portal.ScreenCast` (xdg-desktop-portal) to get a
PipeWire video stream of a chosen window, decoded into a GPU texture.

Portal capture is the only sanctioned route to another window's pixels on Wayland, since
Wayland's security model forbids any other cross-window pixel access.

Portal capture requires the user to re-grant access per session unless the portal's
`persist_mode` is used.
