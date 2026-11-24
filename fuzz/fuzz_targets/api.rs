//! Drives the whole muxer API from fuzzer input: random options, track
//! mixes (some invalid), frame sizes, timestamps (some going back),
//! keyframe flags, durations and discard padding, in live, seekable and
//! large-frame live modes. It must never panic; every finished output
//! must read back with exactly the frames that were accepted; and in the
//! large-frame mode the muxer must not buffer frames (run with
//! -malloc_limit_mb below the total frame bytes).
#![no_main]

use std::io::{self, Cursor, Write};

use libfuzzer_sys::fuzz_target;
use webm_muxer::{AudioTrack, FrameOptions, Muxer, Options, TrackId, VideoCodec, VideoTrack};

#[path = "../../webm-muxer/tests/support/demux.rs"]
mod demux;

struct Input<'a> {
    d: &'a [u8],
    i: usize,
}

impl Input<'_> {
    fn u8(&mut self) -> u8 {
        let v = self.d.get(self.i).copied().unwrap_or(0);
        self.i += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }
    fn more(&self) -> bool {
        self.i < self.d.len()
    }
}

fn vorbis_id() -> Vec<u8> {
    let mut h = vec![1u8];
    h.extend_from_slice(b"vorbis");
    h.extend_from_slice(&0u32.to_le_bytes());
    h.push(2);
    h.extend_from_slice(&48_000u32.to_le_bytes());
    h.extend_from_slice(&[0; 12]);
    h.extend_from_slice(&[0xB8, 1]);
    h
}

fn add_track<W: Write>(m: &mut Muxer<W>, r: &mut Input) -> Option<TrackId> {
    let kind = r.u8() % 9;
    let w = u32::from(r.u16());
    let h = u32::from(r.u16());
    let res = match kind {
        0 => m.add_video_track(VideoTrack::new(VideoCodec::Vp8, w, h)),
        1 => m.add_video_track(VideoTrack::new(VideoCodec::Vp9, w, h).with_codec_private(vec![1, 1, 0])),
        2 => m.add_video_track(VideoTrack::new(VideoCodec::Av1, w, h).with_codec_private(vec![0x81, 0, 0, 0])),
        3 => m.add_video_track(VideoTrack::new(VideoCodec::Av1, w, h)),
        4 | 5 => m.add_audio_track(AudioTrack::opus(r.u8() % 4, r.u16())),
        6 => m.add_audio_track(AudioTrack::vorbis(vorbis_id(), vec![3; 5], vec![5; (w % 600) as usize])),
        7 => m.add_audio_track(AudioTrack::vorbis(vec![1, 2], vec![], vec![])),
        _ => m.add_video_track(VideoTrack::new(VideoCodec::Vp8, w, h).with_uid(u64::from(r.u8()))),
    };
    res.ok()
}

/// Runs the ops; returns the writer and the accepted frames (track,
/// ticks, length).
fn drive<W: Write>(mut m: Muxer<W>, r: &mut Input, big: bool, scale: u64) -> Option<(Muxer<W>, Vec<(u64, u64, usize)>)> {
    let mut tracks = Vec::new();
    for _ in 0..1 + r.u8() % 6 {
        if let Some(t) = add_track(&mut m, r) {
            tracks.push(t);
        }
    }
    let mut accepted = Vec::new();
    let mut now: u64 = 0;
    let mut ops = 0;
    while r.more() && ops < 2000 {
        ops += 1;
        let op = r.u8();
        match op % 10 {
            0..=6 => {
                let ti = r.u8() as usize;
                let track = match tracks.get(ti % (tracks.len() + 1)) {
                    Some(t) => *t,
                    None => dummy_track(),
                };
                let step = u64::from(r.u16());
                let ts = match op % 7 {
                    0 => now.saturating_sub(step * 1000),
                    1 => now,
                    _ => now + step * 10_000,
                };
                let len = if big { usize::from(r.u16()) * 64 } else { usize::from(r.u16() % 4096) };
                let data = vec![op; len];
                let mut fo = FrameOptions::new(op & 0x80 != 0);
                if op & 0x40 != 0 {
                    fo = fo.with_duration(u64::from(r.u16()) * 1000);
                }
                if op & 0x20 != 0 {
                    fo = fo.with_discard_padding(i64::from(r.u16() as i16) * 1000);
                }
                if m.write_frame_with(track, &data, ts, fo).is_ok() {
                    accepted.push((track.number(), ts / scale, len));
                    now = now.max(ts);
                }
            }
            7 => {
                let _ = m.flush();
            }
            8 => {
                let _ = add_track(&mut m, r);
            }
            _ => {
                let _ = m.bytes_written();
            }
        }
    }
    Some((m, accepted))
}

fn dummy_track() -> TrackId {
    // A track id that belongs to another muxer (number 1 or more).
    let mut other = Muxer::new(io::sink(), Options::new()).unwrap();
    let mut id = other.add_audio_track(AudioTrack::opus(1, 0)).unwrap();
    for _ in 0..3 {
        id = other.add_audio_track(AudioTrack::opus(1, 0)).unwrap();
    }
    id
}

fn check(bytes: &[u8], accepted: &[(u64, u64, usize)]) {
    let d = demux::parse(bytes).unwrap_or_else(|e| panic!("output does not parse: {e}"));
    assert_eq!(d.frames.len(), accepted.len());
    for (f, a) in d.frames.iter().zip(accepted) {
        assert_eq!((f.track, f.ticks, f.data.len()), *a);
    }
    for cue in &d.cues {
        assert!(d.clusters.iter().any(|c| c.offset == cue.cluster && c.timestamp == cue.time));
    }
}

fuzz_target!(|data: &[u8]| {
    let mut r = Input { d: data, i: 0 };
    let mode = r.u8() % 3;
    let scale = [1_000_000u64, 1000, 1, 1_000_000_000, 33_333][usize::from(r.u8() % 5)];
    let mut opts = Options::new().with_timestamp_scale(scale);
    let flags = r.u8();
    if flags & 1 != 0 {
        opts = opts.with_max_cluster_bytes(u64::from(r.u16()));
    }
    if flags & 2 != 0 {
        opts = opts.with_max_cluster_duration(u64::from(r.u16()) * 1_000_000);
    }
    if flags & 4 != 0 {
        opts = opts.with_cues_space(u64::from(r.u16()));
    }
    match mode {
        0 => {
            let Ok(m) = Muxer::new(Vec::new(), opts) else { return };
            if let Some((m, acc)) = drive(m, &mut r, false, scale) {
                if let Ok(out) = m.finish() {
                    check(&out, &acc);
                }
            }
        }
        1 => {
            let Ok(m) = Muxer::new_seekable(Cursor::new(Vec::new()), opts) else { return };
            if let Some((m, acc)) = drive(m, &mut r, false, scale) {
                if let Ok(out) = m.finish() {
                    check(&out.into_inner(), &acc);
                }
            }
        }
        _ => {
            // Frames up to 4 MiB into a sink: nothing may be kept.
            let Ok(m) = Muxer::new(io::sink(), opts) else { return };
            if let Some((m, _)) = drive(m, &mut r, true, scale) {
                let _ = m.finish();
            }
        }
    }
});
