# Changelog

## 0.1.0

First release.

- Live mode (`Muxer::new`, any `Write`): unknown-size Segment and
  Clusters, written once as frames arrive.
- Seekable mode (`Muxer::new_seekable`, `Write + Seek`): Cues, Duration,
  SeekHead and every size filled in at `finish()`; optional space for the
  Cues before the first cluster.
- Tracks: VP8, VP9 and AV1 video; Opus and Vorbis audio; up to 126.
- Clusters start on the first video track's keyframes, with duration and
  size caps.
- Timestamps in nanoseconds, stored at a configurable `TimestampScale`;
  out-of-order frames are refused with a typed `Error`.
- `BlockGroup` for frames with a duration or `DiscardPadding`.
- Helpers: `opus_head`, `xiph_lace`, `vp9_codec_private`.
