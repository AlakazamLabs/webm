# Provenance

`webm-muxer` was written from the published specifications only:

- RFC 8794, Extensible Binary Meta Language (EBML);
- RFC 9559, Matroska Media Container Format Specification;
- the WebM container guidelines on webmproject.org (including the VP9
  codec feature metadata);
- the Matroska codec mappings for VP8, VP9, AV1, Opus and Vorbis, and the
  documents they point to for `CodecPrivate` contents (the Opus
  identification header of RFC 7845 section 5.1, the Vorbis
  identification header, and the AV1 codec configuration record).

## Clean-room rule

The rule the work followed:

- Design only from RFC 8794 (EBML), RFC 9559 (Matroska), webmproject.org's
  WebM container guidelines and the Matroska codec mappings for VP8, VP9,
  AV1, Opus and Vorbis.
- Never open, read or paste from libwebm, FFmpeg's matroskaenc,
  GStreamer's matroskamux, webm-iterable, the `webm` crate, or any other
  muxer's or demuxer's source, and keep none of it in the workspace.
- Tools such as mkvalidator, ffprobe, ffmpeg and browsers run only as
  separate processes, as oracles: their outputs are compared, their code
  is never read.

The authors followed this rule. No source code of any other muxer or
demuxer was opened, read or copied while writing this crate, its tests or
its test tools. The test demuxer in `tests/support/` was written from the
same specifications.

## Test oracles

Used only as separate processes, to check the output:

- `mkvalidator` 0.6.0 (the Matroska project's validator), built from its
  published release tag (`mkvalidator-0.6.0`). A build of the source
  repository's main branch crashed on every file, ffmpeg's own output
  included, so the release is used;
- `ffprobe` and `ffmpeg` (FFmpeg command-line tools, a static Linux
  build of the development branch): structure, packet
  timestamps, keyframe flags, seeking to each keyframe, and decoding of
  whole and cut-off files. `ffmpeg` also encoded test inputs (VP9, AV1,
  Opus and Vorbis) into IVF and Ogg files, which the test tool reads;
- Chromium 131 and Firefox 132 (headless, through Playwright 1.49.1):
  playback to the `ended` event and seeking.

The VP8 test frames come from Alakazam Labs' own `vp8-encoder` crate.
