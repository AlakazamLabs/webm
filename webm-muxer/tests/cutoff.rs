//! A recording cut off at any byte still reads up to its last whole
//! cluster: live streams, and seekable files whose writer stopped before
//! `finish`.

mod support;

use std::io::Cursor;

use support::demux::{self, Demuxed};
use support::{av_schedule, vp8_frames, In, Rng};
use webm_muxer::{AudioTrack, Muxer, Options, VideoCodec, VideoTrack};

fn write_all<W: std::io::Write>(m: &mut Muxer<W>, input: &[In]) {
    let v = webm_muxer_track(m, 1);
    let a = webm_muxer_track(m, 2);
    for f in input {
        m.write_frame(if f.track == 1 { v } else { a }, &f.data, f.ns, f.keyframe).unwrap();
    }
}

fn webm_muxer_track<W: std::io::Write>(m: &mut Muxer<W>, n: u64) -> webm_muxer::TrackId {
    if n == 1 {
        m.add_video_track(VideoTrack::new(VideoCodec::Vp8, 64, 48)).unwrap()
    } else {
        m.add_audio_track(AudioTrack::opus(1, 312)).unwrap()
    }
}

/// The end offset (in the whole buffer) of each cluster in a full parse.
fn cluster_ends(full: &Demuxed, total: usize) -> Vec<usize> {
    let starts: Vec<usize> = full.clusters.iter().map(|c| (full.segment_data_at + c.offset) as usize).collect();
    let mut ends: Vec<usize> = starts.iter().skip(1).copied().collect();
    // The last cluster ends at the next top-level element, or the end.
    let last = *starts.last().unwrap();
    let after = full
        .top_level
        .iter()
        .map(|(_, o)| (full.segment_data_at + o) as usize)
        .find(|&o| o > last)
        .unwrap_or(total);
    ends.push(after);
    ends
}

fn check_cuts(bytes: &[u8], input: &[In], cuts: usize, seed: u64) {
    let full = demux::parse(bytes).unwrap();
    assert_eq!(full.frames.len(), input.len());
    let ends = cluster_ends(&full, bytes.len());
    let mut rng = Rng(seed);
    let mut offsets: Vec<usize> = (0..cuts).map(|_| rng.below(bytes.len() as u64) as usize).collect();
    // Also every cluster boundary, one byte either side of it.
    for &e in &ends {
        offsets.extend([e.saturating_sub(1), e, (e + 1).min(bytes.len())]);
    }
    for cut in offsets {
        let part = &bytes[..cut];
        let d = demux::parse_truncated(part).unwrap_or_else(|e| panic!("cut at {cut}: {e}"));
        // Every frame read is the frame that was written, in order.
        assert!(d.frames.len() <= full.frames.len());
        for (i, f) in d.frames.iter().enumerate() {
            assert_eq!(f.data, full.frames[i].data, "cut at {cut}: frame {i}");
            assert_eq!((f.track, f.ticks, f.keyframe), (full.frames[i].track, full.frames[i].ticks, full.frames[i].keyframe));
        }
        // Every frame of every cluster wholly inside the cut is there.
        let whole = ends.iter().filter(|&&e| e <= cut).count();
        let expected = full.frames.iter().filter(|f| f.cluster < whole).count();
        assert!(d.frames.len() >= expected, "cut at {cut}: {} frames, {expected} expected", d.frames.len());
        if !d.frames.is_empty() {
            assert!(!d.tracks.is_empty());
        }
    }
}

#[test]
fn live_streams_cut_anywhere_keep_whole_clusters() {
    let video = vp8_frames(64, 48, 120, 15);
    let input = av_schedule(&video, 200);
    let mut m = Muxer::new(Vec::new(), Options::new()).unwrap();
    write_all(&mut m, &input);
    let bytes = m.finish().unwrap();
    check_cuts(&bytes, &input, 400, 0x5EED_0001);
}

#[test]
fn seekable_files_that_stop_before_finish_keep_whole_clusters() {
    let video = vp8_frames(64, 48, 120, 15);
    let input = av_schedule(&video, 200);
    let mut m = Muxer::new_seekable(Cursor::new(Vec::new()), Options::new()).unwrap();
    write_all(&mut m, &input);
    // No finish: the process died. The last cluster's size is still the
    // unknown-size placeholder, and so is the Segment's.
    let bytes = m.get_ref().get_ref().clone();
    drop(m);
    let d = demux::parse(&bytes).unwrap();
    assert!(!d.segment_known_size);
    assert!(d.clusters[..d.clusters.len() - 1].iter().all(|c| c.known_size));
    assert!(!d.clusters.last().unwrap().known_size);
    check_cuts(&bytes, &input, 300, 0x5EED_0002);
}

#[test]
fn finished_files_cut_anywhere_keep_whole_clusters() {
    let video = vp8_frames(64, 48, 60, 15);
    let input = av_schedule(&video, 100);
    let mut m = Muxer::new_seekable(Cursor::new(Vec::new()), Options::new()).unwrap();
    write_all(&mut m, &input);
    let bytes = m.finish().unwrap().into_inner();
    check_cuts(&bytes, &input, 200, 0x5EED_0003);
}
