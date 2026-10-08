# File format

The file format is Blueboard's save format, extension `.bbd`.

The file format serializes the [[Scene_graph]] with `serde` into `bincode`, then wraps it
with [[Compression]].

The file format starts as a single compressed blob (Phase 3 of [[planning_PLAN]]) and may
later add spatial tiling for lazy loading of large boards (Phase 6).

The file format has round-trip tests (save, load, compare) from its first implementation,
since file-format bugs can destroy user data.

The file format's on-disk schema (`SceneDocument`/`StrokeDocument` in
`blueboard-core/src/file_format.rs`) is a separate type from the runtime [[Scene_graph]],
not a direct serialization of it — the runtime `Scene` also carries the [[RTree_index]],
which is derived data, rebuilt on load, and would be dead weight on disk.

The file format embeds a version number, checked on load; loading a file with an
unrecognized version is a refused, logged error rather than misreading the bytes.

The file format is reachable in [[App_crate]] today via temporary `S`/`O` hotkeys (save/
load to a fixed `board.bbd` path), standing in until [[Command_line]]'s `:save`/`:open`
arrive in Phase 4.
