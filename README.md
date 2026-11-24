# webm

Pure-Rust WebM tools from Alakazam Labs.

| Crate | What it is |
|---|---|
| [`webm-muxer`](webm-muxer/) | A WebM writer (muxer): VP8, VP9, AV1, Opus and Vorbis, live streaming or seekable files with Cues. |

`conformance/` is a test tool (not published): it muxes test media and
checks the output against outside validators and players. `fuzz/` holds
the cargo-fuzz targets. `ci/` holds the scripts that run both on test
machines.

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT licence](LICENSE-MIT) at your option. Security: see
[SECURITY.md](SECURITY.md).
