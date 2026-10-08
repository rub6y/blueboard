# Compression

Compression keeps the [[File_format]] compact on disk.

Compression is done with the `zstd` crate, chosen over gzip/flate2 for its better
speed/ratio tradeoff and streaming support.

Compression is applied app-level, on top of the `bincode`-serialized [[Scene_graph]], not
at the filesystem level.
