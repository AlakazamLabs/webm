//! Every refusal is a typed error, and an I/O failure poisons the muxer.

mod support;

use std::io::{self, Cursor, Write};

use support::OPUS_SILENCE;
use webm_muxer::{opus_head, AudioTrack, Error, Muxer, Options, VideoCodec, VideoTrack};

fn vp8() -> VideoTrack {
    VideoTrack::new(VideoCodec::Vp8, 64, 48)
}

#[test]
fn timestamps_must_increase_per_track() {
    let mut m = Muxer::new(Vec::new(), Options::new()).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    m.write_frame(a, &OPUS_SILENCE, 20_000_000, true).unwrap();
    let e = m.write_frame(a, &OPUS_SILENCE, 20_000_000, true).unwrap_err();
    assert!(
        matches!(
            e,
            Error::TimestampNotIncreasing {
                track: 1,
                previous: 20,
                got: 20
            }
        ),
        "{e:?}"
    );
    let e = m.write_frame(a, &OPUS_SILENCE, 10_000_000, true).unwrap_err();
    assert!(
        matches!(
            e,
            Error::TimestampNotIncreasing {
                track: 1,
                previous: 20,
                got: 10
            }
        ),
        "{e:?}"
    );
    // Below one tick apart rounds to the same tick.
    let e = m.write_frame(a, &OPUS_SILENCE, 20_999_999, true).unwrap_err();
    assert!(matches!(e, Error::TimestampNotIncreasing { .. }), "{e:?}");
    // A refused frame changes nothing: the next good one is fine.
    m.write_frame(a, &OPUS_SILENCE, 40_000_000, true).unwrap();
}

#[test]
fn frames_must_not_go_back_across_tracks() {
    let mut m = Muxer::new(Vec::new(), Options::new()).unwrap();
    let v = m.add_video_track(vp8()).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    m.write_frame(v, &[0x10, 0, 0], 100_000_000, true).unwrap();
    let e = m.write_frame(a, &OPUS_SILENCE, 80_000_000, true).unwrap_err();
    assert!(matches!(e, Error::OutOfOrder { latest: 100, got: 80 }), "{e:?}");
    m.write_frame(a, &OPUS_SILENCE, 100_000_000, true).unwrap();
}

#[test]
fn tracks_lock_at_the_first_frame() {
    let mut m = Muxer::new(Vec::new(), Options::new()).unwrap();
    let v = m.add_video_track(vp8()).unwrap();
    m.write_frame(v, &[1], 0, true).unwrap();
    assert!(matches!(
        m.add_audio_track(AudioTrack::opus(2, 0)),
        Err(Error::TracksLocked)
    ));
}

#[test]
fn bad_tracks_and_frames_are_refused() {
    let mut m = Muxer::new(Vec::new(), Options::new()).unwrap();
    assert!(matches!(
        m.add_video_track(VideoTrack::new(VideoCodec::Av1, 64, 48)),
        Err(Error::MissingCodecPrivate)
    ));
    assert!(matches!(
        m.add_video_track(VideoTrack::new(VideoCodec::Vp8, 0, 48)),
        Err(Error::InvalidConfig(_))
    ));
    assert!(matches!(
        m.add_video_track(vp8().with_codec_private(vec![1])),
        Err(Error::InvalidConfig(_))
    ));
    assert!(matches!(
        m.add_video_track(vp8().with_uid(0)),
        Err(Error::InvalidConfig(_))
    ));
    assert!(matches!(
        m.add_video_track(vp8().with_language("")),
        Err(Error::InvalidConfig(_))
    ));
    assert!(matches!(
        m.add_audio_track(AudioTrack::opus(3, 0)),
        Err(Error::InvalidConfig(_))
    ));
    assert!(matches!(
        m.add_audio_track(AudioTrack::opus(0, 0)),
        Err(Error::InvalidConfig(_))
    ));
    assert!(matches!(
        m.add_audio_track(AudioTrack::opus_with_head(b"OpusHead".to_vec())),
        Err(Error::InvalidConfig(_))
    ));
    assert!(matches!(
        m.add_audio_track(AudioTrack::vorbis(vec![1, 2, 3], vec![1], vec![1])),
        Err(Error::InvalidConfig(_))
    ));
    // Family 1 with its mapping table is accepted.
    let mut head = opus_head(6, 312, 48_000, 0);
    head[18] = 1;
    head.extend_from_slice(&[4, 2, 0, 4, 1, 2, 3, 5]);
    let a = m.add_audio_track(AudioTrack::opus_with_head(head)).unwrap();
    assert_eq!(a.number(), 1);

    assert!(matches!(m.write_frame(a, &[], 0, true), Err(Error::InvalidFrame)));
    let mut other = Muxer::new(Vec::new(), Options::new()).unwrap();
    let foreign = other.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    other.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    let second = other.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    assert_eq!(foreign.number(), 1);
    assert!(matches!(
        m.write_frame(second, &OPUS_SILENCE, 0, true),
        Err(Error::UnknownTrack)
    ));
}

#[test]
fn no_tracks_is_refused() {
    let m = Muxer::new(Vec::new(), Options::new()).unwrap();
    assert!(matches!(m.finish(), Err(Error::NoTracks)));
}

#[test]
fn too_many_tracks_is_refused() {
    let mut m = Muxer::new(Vec::new(), Options::new()).unwrap();
    for _ in 0..126 {
        m.add_audio_track(AudioTrack::opus(1, 0)).unwrap();
    }
    assert!(matches!(
        m.add_audio_track(AudioTrack::opus(1, 0)),
        Err(Error::TooManyTracks)
    ));
}

#[test]
fn bad_options_are_refused() {
    for opts in [
        Options::new().with_timestamp_scale(0),
        Options::new().with_max_cluster_bytes(0),
        Options::new().with_max_cluster_duration(10),
        Options::new().with_cues_space(1),
    ] {
        assert!(matches!(Muxer::new(Vec::new(), opts), Err(Error::InvalidConfig(_))));
    }
}

/// A writer that fails once `limit` bytes have been written.
struct Failing {
    written: usize,
    limit: usize,
}

impl Write for Failing {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written + buf.len() > self.limit {
            return Err(io::Error::other("disk full"));
        }
        self.written += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn an_io_error_poisons_the_muxer() {
    let mut m = Muxer::new(Failing { written: 0, limit: 400 }, Options::new()).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    let mut failed = None;
    for i in 0..100u64 {
        if let Err(e) = m.write_frame(a, &[9u8; 50], i * 20_000_000, true) {
            failed = Some(e);
            break;
        }
    }
    let e = failed.expect("the writer should fail");
    assert!(matches!(e, Error::Io(_)), "{e:?}");
    assert!(std::error::Error::source(&e).is_some());
    assert!(matches!(
        m.write_frame(a, &[9u8; 50], 10_000_000_000, true),
        Err(Error::Poisoned)
    ));
    assert!(matches!(m.flush(), Err(Error::Poisoned)));
    assert!(matches!(
        m.add_audio_track(AudioTrack::opus(2, 0)),
        Err(Error::Poisoned)
    ));
    assert!(matches!(m.finish(), Err(Error::Poisoned)));
}

#[test]
fn errors_display_plainly() {
    let e = Error::TimestampNotIncreasing {
        track: 2,
        previous: 5,
        got: 5,
    };
    assert_eq!(
        e.to_string(),
        "track 2: timestamp 5 is not after the previous frame's 5 (in timestamp-scale ticks)"
    );
    let _ = Muxer::new_seekable(Cursor::new(Vec::new()), Options::default()).unwrap();
}
