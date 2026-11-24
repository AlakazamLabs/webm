//! Random use of the whole API, checked against a model: random track
//! mixes, frame sizes, timestamps (some going back), keyframes, options
//! and both modes. Every call either succeeds or returns the error the
//! model predicts; the output always reads back with exactly the accepted
//! frames. The cargo-fuzz target in `fuzz/` drives the same API from
//! fuzzer input.

mod support;

use std::io::Cursor;

use support::demux;
use support::Rng;
use webm_muxer::{AudioTrack, Error, FrameOptions, Muxer, Options, TrackId, VideoCodec, VideoTrack};

struct Accepted {
    track: u64,
    ticks: u64,
    keyframe: bool,
    len: usize,
    first: u8,
}

fn run(seed: u64) {
    let mut rng = Rng(seed);
    let scale = [1_000_000u64, 1000, 100_000, 1_000_000_000][rng.below(4) as usize];
    let mut opts = Options::new().with_timestamp_scale(scale);
    if rng.chance(50) {
        opts = opts.with_max_cluster_bytes(1 + rng.below(200_000));
    }
    if rng.chance(50) {
        opts = opts.with_max_cluster_duration(scale * (1 + rng.below(40_000)));
    }
    let seekable = rng.chance(50);
    if seekable && rng.chance(30) {
        opts = opts.with_cues_space(2 + rng.below(2000));
    }

    let mut cur = Some(Cursor::new(Vec::new()));
    let mut live = Some(Vec::new());
    let mut m_seek = None;
    let mut m_live = None;
    if seekable {
        m_seek = Some(Muxer::new_seekable(cur.take().unwrap(), opts).unwrap());
    } else {
        m_live = Some(Muxer::new(live.take().unwrap(), opts).unwrap());
    }

    macro_rules! with_m {
        ($m:ident => $body:expr) => {
            if let Some($m) = m_seek.as_mut() {
                $body
            } else {
                let $m = m_live.as_mut().unwrap();
                $body
            }
        };
    }

    let ntracks = 1 + rng.below(5) as usize;
    let mut tracks: Vec<(TrackId, bool)> = Vec::new();
    for _ in 0..ntracks {
        let video = rng.chance(50);
        let id = if video {
            let codec = [VideoCodec::Vp8, VideoCodec::Vp9, VideoCodec::Av1][rng.below(3) as usize];
            let mut t = VideoTrack::new(codec, 1 + rng.below(4000) as u32, 1 + rng.below(4000) as u32);
            if codec == VideoCodec::Av1 {
                t = t.with_codec_private(vec![0x81, 0, 0, 0]);
            }
            with_m!(m => m.add_video_track(t)).unwrap()
        } else {
            let t = AudioTrack::opus(1 + rng.below(2) as u8, rng.below(4000) as u16);
            with_m!(m => m.add_audio_track(t)).unwrap()
        };
        tracks.push((id, video));
    }

    let mut accepted: Vec<Accepted> = Vec::new();
    let mut last: Vec<Option<u64>> = vec![None; ntracks];
    let mut latest = 0u64;
    let mut now_ns = 0u64;
    for _ in 0..rng.below(400) {
        let ti = rng.below(ntracks as u64) as usize;
        let (id, video) = tracks[ti];
        // Mostly forward, sometimes the same time, sometimes back.
        let ns = match rng.below(10) {
            0 => now_ns.saturating_sub(rng.below(50_000_000)),
            1 => now_ns,
            _ => now_ns + rng.below(80_000_000),
        };
        let len = match rng.below(20) {
            0 => 0,
            1 => 20_000 + rng.below(100_000) as usize,
            _ => 1 + rng.below(3000) as usize,
        };
        let data = rng.bytes(len);
        let mut fo = FrameOptions::new(rng.chance(30));
        if rng.chance(10) {
            fo = fo.with_duration(rng.below(100_000_000));
        }
        if rng.chance(10) {
            fo = fo.with_discard_padding(rng.below(10_000_000) as i64 - 1_000_000);
        }
        let ticks = ns / scale;
        let expect = if len == 0 {
            Some("InvalidFrame")
        } else if last[ti].is_some_and(|p| ticks <= p) {
            Some("TimestampNotIncreasing")
        } else if ticks < latest {
            Some("OutOfOrder")
        } else {
            None
        };
        let got = with_m!(m => m.write_frame_with(id, &data, ns, fo));
        match (expect, got) {
            (None, Ok(())) => {
                last[ti] = Some(ticks);
                latest = ticks;
                now_ns = now_ns.max(ns);
                accepted.push(Accepted {
                    track: id.number(),
                    ticks,
                    keyframe: fo.keyframe || !video,
                    len,
                    first: data[0],
                });
            }
            (Some("InvalidFrame"), Err(Error::InvalidFrame)) => {}
            (Some("TimestampNotIncreasing"), Err(Error::TimestampNotIncreasing { .. })) => {}
            (Some("OutOfOrder"), Err(Error::OutOfOrder { .. })) => {}
            (want, got) => panic!("seed {seed:#x}: expected {want:?}, got {got:?}"),
        }
    }

    let bytes = if let Some(m) = m_seek.take() {
        m.finish().unwrap().into_inner()
    } else {
        m_live.take().unwrap().finish().unwrap()
    };
    let d = demux::parse(&bytes).unwrap_or_else(|e| panic!("seed {seed:#x}: {e}"));
    assert_eq!(d.tracks.len(), ntracks);
    assert_eq!(d.frames.len(), accepted.len(), "seed {seed:#x}");
    for (f, a) in d.frames.iter().zip(&accepted) {
        assert_eq!(
            (f.track, f.ticks, f.keyframe, f.data.len(), f.data[0]),
            (a.track, a.ticks, a.keyframe, a.len, a.first)
        );
    }
    if seekable {
        assert!(d.segment_known_size);
        for cue in &d.cues {
            assert!(d
                .clusters
                .iter()
                .any(|c| c.offset == cue.cluster && c.timestamp == cue.time));
        }
    }
}

#[test]
fn random_api_use_matches_the_model() {
    for i in 0..400u64 {
        run(0x9E37_79B9_7F4A_7C15 ^ (i * 0x1000_0001));
    }
}
