# webm-muxer

A WebM writer (muxer) in pure, safe Rust, with no dependencies. It puts
VP8, VP9 or AV1 video and Opus or Vorbis audio into a `.webm` file or
stream, as frames arrive: it never holds a frame after writing it.

- **Live mode** (`Muxer::new`, any `Write`): unknown-size Segment and
  Clusters, nothing written twice, so the output can go to a socket or a
  pipe as it is made. A recording cut off at any point plays up to its
  last whole cluster.
- **Seekable mode** (`Muxer::new_seekable`, `Write + Seek`): the same, then
  `finish()` adds Cues (one per keyframe cluster) and fills in the
  Duration, the SeekHead and every element size, so players can seek.
- **Clusters** start on the first video track's keyframes, with a duration
  cap (5 s) and a size cap (5 MiB) you can change.
- **Timestamps** are nanoseconds in, stored at the `TimestampScale` you
  choose (1 ms by default). Frames out of order are refused with a typed
  error, never reordered.
- **Opus** gets its identification header, `CodecDelay` and `SeekPreRoll`;
  frames can carry `DiscardPadding` and a duration (written as a
  `BlockGroup`). **Vorbis** takes its three headers.

It writes the container only: frames come from your encoder as they are.

```rust
use std::io::Cursor;
use webm_muxer::{AudioTrack, Muxer, Options, VideoCodec, VideoTrack};

fn main() -> Result<(), webm_muxer::Error> {
    // Any `Write + Seek` works here, such as a `std::fs::File`.
    let mut mux = Muxer::new_seekable(Cursor::new(Vec::new()), Options::new())?;
    let video = mux.add_video_track(VideoTrack::new(VideoCodec::Vp8, 640, 480))?;
    let audio = mux.add_audio_track(AudioTrack::opus(2, 312))?;

    // Frames from your encoders: (bytes, timestamp in nanoseconds, keyframe).
    // These stand-in bytes aren't real VP8 or Opus.
    let vp8_frames: Vec<(Vec<u8>, u64, bool)> = vec![(vec![0x10; 100], 0, true)];
    let opus_packets: Vec<(Vec<u8>, u64)> = vec![(vec![0xFC; 40], 0)];

    // Write in time order; at equal timestamps, video keyframes first.
    for (frame, ts, keyframe) in &vp8_frames {
        mux.write_frame(video, frame, *ts, *keyframe)?;
    }
    for (packet, ts) in &opus_packets {
        mux.write_frame(audio, packet, *ts, true)?;
    }
    let file = mux.finish()?.into_inner();
    assert_eq!(&file[..4], &[0x1A, 0x45, 0xDF, 0xA3]); // the EBML magic
    Ok(())
}
```

For live output, use `Muxer::new` with any writer, wrapped in a
`BufWriter` when it is slow; `flush()` pushes buffered bytes out.

**Codec notes.**
- VP8 takes no `CodecPrivate`.
- VP9's is optional; `vp9_codec_private` builds it.
- AV1 needs the AV1CodecConfigurationRecord as `CodecPrivate`. Pass each
  temporal unit without its temporal delimiter OBU.
- `AudioTrack::opus(channels, pre_skip)` covers mono and stereo; use
  `AudioTrack::opus_with_head` for a full `OpusHead`.

**Errors, not panics.** Every refusal is an `Error` variant: a track added
after the first frame, an unknown track, a missing `CodecPrivate`, a
timestamp that doesn't increase, an I/O failure (after which the muxer
refuses further calls, since the output is incomplete).

**Not included:** reading or demuxing, encryption, and Matroska features
outside WebM (chapters, attachments, subtitles).

## Features

None. The crate has no optional features and no dependencies.

## MSRV and platforms

Rust 1.80 or later. Pure safe Rust (`#![forbid(unsafe_code)]`), so it
builds on any platform with `std`.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT licence](LICENSE-MIT) at your option (`MIT OR Apache-2.0`).
Written from the published specifications; see
[PROVENANCE.md](PROVENANCE.md).

## Issues

Please [open an issue](https://github.com/AlakazamLabs/webm/issues/new?labels=webm-muxer&template=webm-muxer.yml).

## Security

Please report vulnerabilities privately, as described in
[SECURITY.md](https://github.com/AlakazamLabs/webm/blob/main/SECURITY.md).
