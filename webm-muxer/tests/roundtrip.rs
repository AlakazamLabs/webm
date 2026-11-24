//! Muxes real VP8 frames and Opus packets, reads the output back with the
//! test demuxer and checks frames, timestamps, layout, SeekHead and Cues.

mod support;

use std::io::{Cursor, Seek, SeekFrom, Write};

use support::demux::{self, Demuxed, CLUSTER, CUES, INFO, TRACKS};
use support::{av_schedule, vp8_frames, In, OPUS_SILENCE};
use webm_muxer::{
    opus_head, vp9_codec_private, AudioTrack, FrameOptions, Muxer, Options, VideoCodec, VideoTrack,
};

fn mux_av(input: &[In], seekable: bool, options: Options) -> Vec<u8> {
    let tracks = |m: &mut Muxer<_>| {
        let v = m.add_video_track(VideoTrack::new(VideoCodec::Vp8, 64, 48)).unwrap();
        let a = m.add_audio_track(AudioTrack::opus(1, 312)).unwrap();
        assert_eq!((v.number(), a.number()), (1, 2));
        [v, a]
    };
    if seekable {
        let mut m = Muxer::new_seekable(Cursor::new(Vec::new()), options).unwrap();
        let ids = tracks(&mut m);
        for f in input {
            m.write_frame(ids[f.track as usize - 1], &f.data, f.ns, f.keyframe).unwrap();
        }
        m.finish().unwrap().into_inner()
    } else {
        let mut m = Muxer::new(Vec::new(), options).unwrap();
        let ids = tracks(&mut m);
        for f in input {
            m.write_frame(ids[f.track as usize - 1], &f.data, f.ns, f.keyframe).unwrap();
        }
        m.finish().unwrap()
    }
}

fn assert_same_frames(d: &Demuxed, input: &[In], scale: u64) {
    assert_eq!(d.frames.len(), input.len(), "frame count");
    for (i, (got, want)) in d.frames.iter().zip(input).enumerate() {
        assert_eq!(got.track, want.track, "frame {i} track");
        assert_eq!(got.ticks, want.ns / scale, "frame {i} timestamp");
        assert_eq!(got.keyframe, want.keyframe, "frame {i} keyframe flag");
        assert_eq!(got.data, want.data, "frame {i} bytes");
    }
}

/// Every video keyframe is the first block of its cluster.
fn assert_keyframes_start_clusters(d: &Demuxed) {
    for (i, f) in d.frames.iter().enumerate() {
        if f.track == 1 && f.keyframe {
            let first_of_cluster = i == 0 || d.frames[i - 1].cluster != f.cluster;
            assert!(first_of_cluster, "video keyframe {i} is not at a cluster start");
        }
    }
}

/// Cues: one per video keyframe, each pointing at a real Cluster whose
/// timestamp is the cue time and whose first block (at the cue's relative
/// position) is that keyframe.
fn assert_cues_point_at_clusters(d: &Demuxed) {
    let keyframes: Vec<_> = d.frames.iter().filter(|f| f.track == 1 && f.keyframe).collect();
    assert_eq!(d.cues.len(), keyframes.len(), "one cue per video keyframe");
    for (cue, kf) in d.cues.iter().zip(&keyframes) {
        assert_eq!(cue.track, 1);
        assert_eq!(cue.time, kf.ticks);
        let cluster = d
            .clusters
            .iter()
            .find(|c| c.offset == cue.cluster)
            .unwrap_or_else(|| panic!("cue at {} points at no cluster", cue.cluster));
        assert!(d.top_level.contains(&(CLUSTER, cue.cluster)));
        assert_eq!(cluster.timestamp, cue.time);
        assert_eq!(cue.relative, cluster.first_block);
        assert_eq!(&d.clusters[kf.cluster], cluster);
    }
}

fn seek_entry(d: &Demuxed, id: u32) -> u64 {
    d.seek_head.iter().find(|(i, _)| *i == id).unwrap_or_else(|| panic!("no SeekHead entry for {id:#x}")).1
}

#[test]
fn seekable_vp8_opus_round_trips() {
    let video = vp8_frames(64, 48, 150, 30);
    let input = av_schedule(&video, 250);
    let file = mux_av(&input, true, Options::new());
    let d = demux::parse(&file).unwrap();

    assert_eq!(d.doc_type, "webm");
    assert_eq!(d.doc_type_version, 4);
    assert!(d.segment_known_size);
    assert_eq!(d.timestamp_scale, 1_000_000);
    assert!(d.muxing_app.as_deref().unwrap().starts_with("webm-muxer "));
    assert!(d.writing_app.as_deref().unwrap().starts_with("webm-muxer "));
    assert_eq!(d.title, None);
    assert_same_frames(&d, &input, 1_000_000);
    assert!(d.clusters.iter().all(|c| c.known_size && c.complete));
    assert_keyframes_start_clusters(&d);
    assert_cues_point_at_clusters(&d);
    assert_eq!(d.cues.len(), 5);

    // The SeekHead points at the real elements.
    for id in [INFO, TRACKS, CUES] {
        let pos = seek_entry(&d, id);
        assert!(d.top_level.contains(&(id, pos)), "SeekHead entry {id:#x} at {pos}");
    }
    // Cues come after the last cluster by default.
    let cues_at = seek_entry(&d, CUES);
    assert!(d.clusters.iter().all(|c| c.offset < cues_at));

    // Duration: the last video frame (149 / 30 s) plus one frame gap, or
    // the last audio packet plus 20 ms, whichever is later.
    let audio_end = 249 * 20 + 20;
    let video_end = 149 * 1000 / 30 + (149 * 1000 / 30 - 148 * 1000 / 30);
    assert_eq!(d.duration, Some(audio_end.max(video_end) as f64));

    // Tracks.
    let v = &d.tracks[0];
    assert_eq!((v.number, v.kind, v.codec_id.as_str()), (1, 1, "V_VP8"));
    assert_eq!((v.width, v.height, v.codec_private.as_ref()), (Some(64), Some(48), None));
    assert_eq!(v.lacing, Some(0));
    let a = &d.tracks[1];
    assert_eq!((a.number, a.kind, a.codec_id.as_str()), (2, 2, "A_OPUS"));
    assert_eq!(a.codec_private.as_deref(), Some(&opus_head(1, 312, 48_000, 0)[..]));
    assert_eq!(a.codec_delay, Some(312 * 1_000_000_000 / 48_000));
    assert_eq!(a.seek_pre_roll, Some(80_000_000));
    assert_eq!((a.rate, a.channels), (Some(48_000.0), Some(1)));
    assert_ne!(v.uid, a.uid);
}

#[test]
fn live_vp8_opus_round_trips() {
    let video = vp8_frames(64, 48, 90, 30);
    let input = av_schedule(&video, 150);
    let stream = mux_av(&input, false, Options::new());
    let d = demux::parse(&stream).unwrap();
    assert!(!d.segment_known_size);
    assert!(d.clusters.iter().all(|c| !c.known_size));
    assert!(d.cues.is_empty() && d.seek_head.is_empty());
    assert_eq!(d.duration, None);
    assert_same_frames(&d, &input, 1_000_000);
    assert_keyframes_start_clusters(&d);
    assert_eq!(d.clusters.len(), 3);
}

#[test]
fn live_and_seekable_carry_the_same_clusters() {
    let video = vp8_frames(64, 48, 60, 15);
    let input = av_schedule(&video, 100);
    let live = demux::parse(&mux_av(&input, false, Options::new())).unwrap();
    let file = demux::parse(&mux_av(&input, true, Options::new())).unwrap();
    assert_eq!(live.frames, file.frames);
    let ts = |d: &Demuxed| d.clusters.iter().map(|c| c.timestamp).collect::<Vec<_>>();
    assert_eq!(ts(&live), ts(&file));
}

#[test]
fn cues_go_before_the_clusters_when_space_is_reserved() {
    let video = vp8_frames(64, 48, 120, 30);
    let input = av_schedule(&video, 200);
    let file = mux_av(&input, true, Options::new().with_cues_space(400));
    let d = demux::parse(&file).unwrap();
    let cues_at = seek_entry(&d, CUES);
    assert!(d.top_level.contains(&(CUES, cues_at)));
    assert!(d.clusters.iter().all(|c| c.offset > cues_at));
    assert_cues_point_at_clusters(&d);
    assert_same_frames(&d, &input, 1_000_000);

    // Too little space: the Cues go at the end and the space stays Void.
    let file = mux_av(&input, true, Options::new().with_cues_space(20));
    let d = demux::parse(&file).unwrap();
    let cues_at = seek_entry(&d, CUES);
    assert!(d.clusters.iter().all(|c| c.offset < cues_at));
    assert_cues_point_at_clusters(&d);
}

#[test]
fn a_finer_timestamp_scale_keeps_finer_timestamps() {
    let video = vp8_frames(64, 48, 30, 30);
    let mut input = av_schedule(&video, 50);
    // Microsecond ticks: clusters can only span 32.767 ms.
    for f in &mut input {
        f.ns += 333; // sub-microsecond noise is dropped
    }
    input.sort_by_key(|f| (f.ns / 1000, f.track));
    let file = mux_av(&input, true, Options::new().with_timestamp_scale(1000));
    let d = demux::parse(&file).unwrap();
    assert_eq!(d.timestamp_scale, 1000);
    assert_same_frames(&d, &input, 1000);
    for (i, c) in d.clusters.iter().enumerate() {
        let span = d.frames.iter().filter(|f| f.cluster == i).map(|f| f.ticks - c.timestamp).max().unwrap();
        assert!(span <= 32_767);
    }
    assert!(d.clusters.len() > 20, "{} clusters", d.clusters.len());
}

#[test]
fn cluster_caps_split_long_and_large_clusters() {
    // Audio only: no keyframe boundaries, so the duration cap decides.
    let mut m = Muxer::new(Vec::new(), Options::new().with_max_cluster_duration(1_000_000_000)).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    for i in 0..500u64 {
        m.write_frame(a, &OPUS_SILENCE, i * 20_000_000, true).unwrap();
    }
    let d = demux::parse(&m.finish().unwrap()).unwrap();
    assert_eq!(d.clusters.len(), 10);
    assert!(d.clusters.iter().enumerate().all(|(i, c)| c.timestamp == i as u64 * 1000));

    // The size cap: 10 KiB clusters of 1 KiB frames.
    let mut m = Muxer::new(Vec::new(), Options::new().with_max_cluster_bytes(10 * 1024)).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    for i in 0..100u64 {
        m.write_frame(a, &[7u8; 1024], i * 20_000_000, true).unwrap();
    }
    let d = demux::parse(&m.finish().unwrap()).unwrap();
    assert!(d.clusters.len() >= 10 && d.clusters.len() <= 12, "{} clusters", d.clusters.len());
    // A frame larger than the cap gets a cluster of its own.
    let mut m = Muxer::new(Vec::new(), Options::new().with_max_cluster_bytes(100)).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 0)).unwrap();
    for i in 0..5u64 {
        m.write_frame(a, &[1u8; 500], i * 20_000_000, true).unwrap();
    }
    let d = demux::parse(&m.finish().unwrap()).unwrap();
    assert_eq!(d.clusters.len(), 5);
    assert_eq!(d.frames.len(), 5);
}

#[test]
fn audio_only_files_cue_each_cluster() {
    let mut m = Muxer::new_seekable(Cursor::new(Vec::new()), Options::new().with_max_cluster_duration(2_000_000_000)).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 312)).unwrap();
    for i in 0..300u64 {
        m.write_frame(a, &OPUS_SILENCE, i * 20_000_000, true).unwrap();
    }
    let d = demux::parse(&m.finish().unwrap().into_inner()).unwrap();
    assert_eq!(d.clusters.len(), 3);
    assert_eq!(d.cues.len(), 3);
    for (cue, c) in d.cues.iter().zip(&d.clusters) {
        assert_eq!((cue.track, cue.time, cue.cluster), (1, c.timestamp, c.offset));
    }
    assert_eq!(d.duration, Some(6000.0));
}

#[test]
fn several_tracks_of_every_codec() {
    let vorbis_id = {
        let mut h = vec![1u8];
        h.extend_from_slice(b"vorbis");
        h.extend_from_slice(&0u32.to_le_bytes());
        h.push(2);
        h.extend_from_slice(&44_100u32.to_le_bytes());
        h.extend_from_slice(&[0; 12]);
        h.extend_from_slice(&[0xB8, 1]);
        h
    };
    let av1c = [0x81, 0x00, 0x0C, 0x00, 0x0A, 0x0B, 0x00, 0x00, 0x00];
    let mut m = Muxer::new_seekable(Cursor::new(Vec::new()), Options::new().with_title("Test")).unwrap();
    let vp8 = m.add_video_track(VideoTrack::new(VideoCodec::Vp8, 64, 48).with_name("main")).unwrap();
    let vp9 = m
        .add_video_track(VideoTrack::new(VideoCodec::Vp9, 32, 24).with_codec_private(vp9_codec_private(Some(0), Some(10), Some(8), Some(1))))
        .unwrap();
    let av1 = m.add_video_track(VideoTrack::new(VideoCodec::Av1, 16, 16).with_codec_private(av1c.to_vec()).with_display_size(32, 16)).unwrap();
    let opus = m.add_audio_track(AudioTrack::opus(2, 0).with_language("fra")).unwrap();
    let vorbis = m.add_audio_track(AudioTrack::vorbis(vorbis_id.clone(), vec![3u8; 7], vec![5u8; 300]).with_bit_depth(16)).unwrap();
    let video = vp8_frames(64, 48, 10, 5);
    for (i, (f, k)) in video.iter().enumerate() {
        let ns = i as u64 * 100_000_000;
        m.write_frame(vp8, f, ns, *k).unwrap();
        m.write_frame(vp9, &[0x82, 0x49, 0x83, i as u8], ns, i % 5 == 0).unwrap();
        m.write_frame(av1, &[0x12, 0x00, i as u8], ns, i % 5 == 0).unwrap();
        m.write_frame(opus, &OPUS_SILENCE, ns, true).unwrap();
        m.write_frame(vorbis, &[i as u8 + 1; 9], ns, true).unwrap();
    }
    let d = demux::parse(&m.finish().unwrap().into_inner()).unwrap();
    assert_eq!(d.title.as_deref(), Some("Test"));
    let ids: Vec<_> = d.tracks.iter().map(|t| t.codec_id.as_str()).collect();
    assert_eq!(ids, ["V_VP8", "V_VP9", "V_AV1", "A_OPUS", "A_VORBIS"]);
    assert_eq!(d.tracks[1].codec_private.as_deref(), Some(&[1, 1, 0, 2, 1, 10, 3, 1, 8, 4, 1, 1][..]));
    assert_eq!(d.tracks[2].codec_private.as_deref(), Some(&av1c[..]));
    let vorbis_cp = d.tracks[4].codec_private.as_ref().unwrap();
    assert_eq!(&vorbis_cp[..3], &[2, vorbis_id.len() as u8, 7]);
    assert_eq!(&vorbis_cp[3..3 + vorbis_id.len()], &vorbis_id[..]);
    assert_eq!(vorbis_cp.len(), 3 + vorbis_id.len() + 7 + 300);
    assert_eq!((d.tracks[4].rate, d.tracks[4].channels, d.tracks[4].codec_delay), (Some(44_100.0), Some(2), None));
    assert_eq!(d.frames.len(), 50);
    // Clusters follow the first video track's keyframes only.
    assert_eq!(d.clusters.len(), 2);
    assert_eq!(d.cues.len(), 2);
    assert!(d.cues.iter().all(|c| c.track == 1));
}

#[test]
fn durations_and_discard_padding_use_block_groups() {
    let video = vp8_frames(64, 48, 4, 4);
    let mut m = Muxer::new_seekable(Cursor::new(Vec::new()), Options::new()).unwrap();
    let v = m.add_video_track(VideoTrack::new(VideoCodec::Vp8, 64, 48)).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 312)).unwrap();
    m.write_frame(v, &video[0].0, 0, true).unwrap();
    m.write_frame_with(v, &video[1].0, 40_000_000, FrameOptions::new(false).with_duration(40_000_000)).unwrap();
    m.write_frame(v, &video[2].0, 80_000_000, false).unwrap();
    m.write_frame_with(a, &OPUS_SILENCE, 80_000_000, FrameOptions::new(true).with_discard_padding(6_500_000)).unwrap();
    m.write_frame_with(v, &video[3].0, 120_000_000, FrameOptions::new(true).with_duration(50_000_000)).unwrap();
    let d = demux::parse(&m.finish().unwrap().into_inner()).unwrap();
    let f = &d.frames;
    assert_eq!(f.len(), 5);
    assert!(!f[0].group && f[0].keyframe);
    assert!(f[1].group && !f[1].keyframe);
    assert_eq!((f[1].duration, f[1].reference), (Some(40), Some(-40)));
    assert!(!f[2].group && !f[2].keyframe);
    assert!(f[3].group && f[3].keyframe);
    assert_eq!((f[3].discard_padding, f[3].reference), (Some(6_500_000), None));
    assert!(f[4].group && f[4].keyframe && f[4].duration == Some(50));
    assert_eq!(d.duration, Some(170.0));
}

#[test]
fn a_seekable_file_may_start_mid_writer() {
    let video = vp8_frames(64, 48, 30, 10);
    let input = av_schedule(&video, 50);
    let mut cur = Cursor::new(Vec::new());
    cur.write_all(&[0xAA; 100]).unwrap();
    let mut m = Muxer::new_seekable(cur, Options::new()).unwrap();
    let v = m.add_video_track(VideoTrack::new(VideoCodec::Vp8, 64, 48)).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(1, 312)).unwrap();
    for f in &input {
        m.write_frame(if f.track == 1 { v } else { a }, &f.data, f.ns, f.keyframe).unwrap();
    }
    let mut cur = m.finish().unwrap();
    assert_eq!(cur.stream_position().unwrap(), cur.get_ref().len() as u64);
    cur.seek(SeekFrom::Start(0)).unwrap();
    let all = cur.into_inner();
    assert_eq!(&all[..100], &[0xAA; 100][..]);
    let d = demux::parse(&all[100..]).unwrap();
    assert_same_frames(&d, &input, 1_000_000);
    assert_cues_point_at_clusters(&d);
}

#[test]
fn tracks_without_frames_make_a_valid_file() {
    let mut m = Muxer::new_seekable(Cursor::new(Vec::new()), Options::new()).unwrap();
    m.add_video_track(VideoTrack::new(VideoCodec::Vp8, 64, 48)).unwrap();
    let d = demux::parse(&m.finish().unwrap().into_inner()).unwrap();
    assert!(d.frames.is_empty() && d.clusters.is_empty() && d.cues.is_empty());
    assert_eq!(d.duration, Some(0.0));
    assert_eq!(d.seek_head.len(), 2);
}

#[test]
fn output_is_reproducible() {
    let video = vp8_frames(64, 48, 30, 10);
    let input = av_schedule(&video, 50);
    assert_eq!(mux_av(&input, true, Options::new()), mux_av(&input, true, Options::new()));
}
