# File format

The file format is Blueboard's save format, extension `.bbd`.

The file format serializes the [[Scene_graph]] with `serde` into `bincode`, then wraps it
with [[Compression]].

The file format starts as a single compressed blob (Phase 3 of [[planning_PLAN]]) and may
later add spatial tiling for lazy loading of large boards (Phase 6).

The file format has round-trip tests (save, load, compare) from its first implementation,
since file-format bugs can destroy user data.
