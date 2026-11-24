//! Writes a 5-second test pattern to a WebM file: VP8 video from the
//! `vp8-encoder` crate and silent Opus audio.
//!
//! cargo run --example test_pattern -- out.webm [--live]

use std::fs::File;
use std::io::BufWriter;

use vp8_encoder::{Encoder, EncoderConfig, Picture};
use webm_muxer::{AudioTrack, Muxer, Options, TrackId, VideoCodec, VideoTrack};

/// One 20 ms Opus frame of silence (CELT, fullband, mono).
const OPUS_SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "test_pattern.webm".into());
    let live = args.any(|a| a == "--live");
    let file = File::create(&path)?;
    if live {
        let mut mux = Muxer::new(BufWriter::new(file), Options::new())?;
        write(&mut mux)?;
        mux.finish()?;
    } else {
        let mut mux = Muxer::new_seekable(BufWriter::new(file), Options::new())?;
        write(&mut mux)?;
        mux.finish()?;
    }
    println!("wrote {path}");
    Ok(())
}

fn write<W: std::io::Write>(mux: &mut Muxer<W>) -> Result<(), Box<dyn std::error::Error>> {
    let (w, h) = (320u32, 240u32);
    let video: TrackId = mux.add_video_track(VideoTrack::new(VideoCodec::Vp8, w, h))?;
    let audio: TrackId = mux.add_audio_track(AudioTrack::opus(1, 0))?;
    let mut enc = Encoder::new(EncoderConfig::new(w, h, 30, 500_000).with_keyframe_interval(Some(30)))?;
    let mut picture = vec![0u8; (w * h * 3 / 2) as usize];
    let mut next_audio_ns = 0u64;
    for i in 0..150u64 {
        let ns = i * 1_000_000_000 / 30;
        // A bar moving across a grey background.
        for y in 0..h as usize {
            for x in 0..w as usize {
                let bar = (x as u64 + 320 - (i * 4) % 320) % 320 < 40;
                picture[y * w as usize + x] = if bar { 235 } else { 90 };
            }
        }
        for c in &mut picture[(w * h) as usize..] {
            *c = 128;
        }
        let pic = Picture::from_i420(&picture, w, h).ok_or("picture too small")?;
        if let Some(frame) = enc.encode(&pic, false)? {
            mux.write_frame(video, &frame.data, ns, frame.keyframe)?;
        }
        // Audio up to the next video frame, in time order.
        let next_video_ns = (i + 1) * 1_000_000_000 / 30;
        while next_audio_ns < next_video_ns {
            mux.write_frame(audio, &OPUS_SILENCE, next_audio_ns, true)?;
            next_audio_ns += 20_000_000;
        }
    }
    Ok(())
}
