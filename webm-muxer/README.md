# webm-muxer

A WebM writer (muxer) in pure, safe Rust, with no dependencies. It puts
VP8, VP9 or AV1 video and Opus or Vorbis audio into a `.webm` file or
stream as frames arrive, and never holds a frame after writing it.

Its output is checked with mkvalidator 0.6.0, ffprobe, and playback in
Chromium and Firefox, and its API is fuzzed.

## Which mode

- **Live** (`Muxer::new`, any `Write`): for streaming to a socket, a pipe
  or an upload. The Segment and Clusters have unknown sizes and nothing is
  written twice, so bytes can go out as soon as they are made. The output
  is never seekable: there are no Cues and no Duration. A stream cut off
  at any point plays up to its last whole cluster.
- **Seekable** (`Muxer::new_seekable`, `Write + Seek`): for a file. It
  writes the same way, then `finish()` goes back to write the Cues, the
  Duration, the SeekHead and every size, so players can seek. A file whose
  writer stops before `finish()` still plays up to its last whole cluster.
  `Options::with_cues_space` reserves room so the Cues go before the
  clusters, as the WebM guidelines recommend; by default they go after.

## Example

```rust
use std::io::Cursor;
use webm_muxer::{AudioTrack, Muxer, Options, TrackId, VideoCodec, VideoTrack};

fn main() -> Result<(), webm_muxer::Error> {
    // Any `Write + Seek` works here, such as a `std::fs::File`.
    let mut mux = Muxer::new_seekable(Cursor::new(Vec::new()), Options::new())?;
    let video = mux.add_video_track(VideoTrack::new(VideoCodec::Vp8, 640, 480))?;
    // 2 channels; 312 is the pre-skip: the samples (at 48 kHz) the Opus
    // encoder's lookahead adds at the start. Take it from your encoder's
    // OpusHead.
    let audio = mux.add_audio_track(AudioTrack::opus(2, 312))?;

    // Frames from your encoders: (bytes, timestamp in nanoseconds,
    // keyframe). These stand-in bytes aren't real VP8 or Opus.
    let vp8_frames: Vec<(Vec<u8>, u64, bool)> =
        (0..30u64).map(|i| (vec![0x10; 100], i * 33_333_333, i % 15 == 0)).collect();
    let opus_packets: Vec<(Vec<u8>, u64)> = (0..50u64).map(|i| (vec![0xFC; 40], i * 20_000_000)).collect();

    // Write in time order across tracks: merge the two streams by
    // timestamp. At equal timestamps, write the video first.
    let mut frames: Vec<(TrackId, &[u8], u64, bool)> = Vec::new();
    frames.extend(vp8_frames.iter().map(|(d, ts, key)| (video, &d[..], *ts, *key)));
    frames.extend(opus_packets.iter().map(|(d, ts)| (audio, &d[..], *ts, true)));
    frames.sort_by_key(|&(track, _, ts, _)| (ts, track != video));
    for (track, data, ts, keyframe) in frames {
        mux.write_frame(track, data, ts, keyframe)?;
    }

    let file = mux.finish()?.into_inner();
    assert_eq!(&file[..4], &[0x1A, 0x45, 0xDF, 0xA3]); // the EBML magic
    Ok(())
}
```

For live output, use `Muxer::new` with any writer, wrapped in a
`BufWriter` when it is slow; `flush()` pushes buffered bytes out.

## How it writes

- **Clusters** start on each keyframe of the first video track, with a
  duration cap (5 s) and a size cap (5 MiB) you can change in `Options`.
  An audio-only file starts clusters at the caps only, and its Cues point
  at every cluster that starts with a frame of its first track.
- **Cues** (seekable mode) list one point per keyframe cluster.
- **Timestamps** are nanoseconds in, stored at the `TimestampScale` you
  choose (1 ms by default). They must increase on each track and must not
  go back across tracks. Frames out of order are refused with a typed
  error, never reordered.
- **Audio is always a keyframe**: the `keyframe` flag is ignored for audio
  tracks.
- **Duration and DiscardPadding**: `write_frame_with` with `FrameOptions`
  gives a frame a duration or `DiscardPadding` (for example, an Opus
  stream's end trim). Such frames are written as a `BlockGroup`, the rest
  as `SimpleBlock`s.
- **Tracks**: up to 126, all added before the first frame.

Terms, from [RFC 9559](https://www.rfc-editor.org/rfc/rfc9559): **Cues**
are the seek index ([section 5.1.5](https://www.rfc-editor.org/rfc/rfc9559#section-5.1.5)),
the **SeekHead** says where the top-level elements are
([section 5.1.1](https://www.rfc-editor.org/rfc/rfc9559#section-5.1.1)),
a **BlockGroup** is a block with extra fields
([section 5.1.3.5](https://www.rfc-editor.org/rfc/rfc9559#section-5.1.3.5)),
and the **TimestampScale** is the nanoseconds per timestamp tick
([section 5.1.2.9](https://www.rfc-editor.org/rfc/rfc9559#section-5.1.2.9)).

## Codecs

- **VP8** takes no `CodecPrivate`.
- **VP9**'s `CodecPrivate` is optional; `vp9_codec_private` builds it.
- **AV1** needs its AV1CodecConfigurationRecord (`av1C`) as
  `CodecPrivate`: the 4-byte header (marker and version, profile and
  level, tier, bit depth and chroma flags) followed by the Sequence Header
  OBU. Many AV1 encoders output it with their stream headers. Pass each
  temporal unit without its temporal delimiter OBU.
- **Opus**: `AudioTrack::opus(channels, pre_skip)` covers mono and stereo;
  `AudioTrack::opus_with_head` takes a full `OpusHead` (more channels).
  The track gets `CodecDelay` and an 80 ms `SeekPreRoll`.
- **Vorbis**: `AudioTrack::vorbis(identification, comment, setup)` takes
  the three header packets and stores them with Xiph lacing (`xiph_lace`).

## Errors, not panics

Every refusal is an `Error` variant: a track added after the first frame,
an unknown track, a missing `CodecPrivate`, a timestamp that doesn't
increase, an I/O failure (after which the muxer refuses further calls,
since the output is incomplete).

**Not included:** reading or demuxing, encryption, and Matroska features
outside WebM (chapters, attachments, subtitles).

## Features

None. The crate has no optional features and no dependencies.

## MSRV, platforms and stability

Rust 1.80 or later. Pure safe Rust (`#![forbid(unsafe_code)]`), so it
builds on any platform with `std`.

0.x: the API may change in minor versions; output is checked against the
tools above.

## Licence

Licensed under either of
[Apache License, Version 2.0](https://github.com/AlakazamLabs/webm/blob/main/webm-muxer/LICENSE-APACHE)
or [MIT licence](https://github.com/AlakazamLabs/webm/blob/main/webm-muxer/LICENSE-MIT)
at your option (`MIT OR Apache-2.0`). Written from the published
specifications; see
[PROVENANCE.md](https://github.com/AlakazamLabs/webm/blob/main/webm-muxer/PROVENANCE.md).

## Issues

Please [open an issue](https://github.com/AlakazamLabs/webm/issues/new?labels=webm-muxer&template=webm-muxer.yml).

## Security

Please report vulnerabilities privately, as described in
[SECURITY.md](https://github.com/AlakazamLabs/webm/blob/main/SECURITY.md).
