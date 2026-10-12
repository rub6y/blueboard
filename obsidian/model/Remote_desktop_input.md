# Remote desktop input

Researched-but-unbuilt path for interacting with (not just viewing) a [[Magic_window]]'s
[[Portal_capture]] stream — e.g. clicking/typing into the captured app from inside
[[Blueboard]].

`org.freedesktop.portal.RemoteDesktop` can be combined with an existing ScreenCast session
to inject synthetic pointer/keyboard events, coordinate-mapped to the capture stream — the
same mechanism remote-control screen-share features (not just viewing) use.

Confirmed feasible during the portal-capture research pass, not built in the first
screencast-based magic-window step (view-only).
