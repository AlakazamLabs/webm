//! Shared test helpers: the test demuxer, real VP8 frames from our own
//! vp8-encoder, Opus silence packets and a small deterministic PRNG.
#![allow(dead_code)]

pub mod demux;

use vp8_encoder::{Encoder, EncoderConfig, Picture};

/// An Opus packet: one 20 ms CELT frame of silence (TOC 0xF8: CELT-only,
/// fullband, 20 ms, mono, one frame).
pub const OPUS_SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

/// `count` real VP8 frames of a moving picture, a keyframe every
/// `keyint` frames: (data, keyframe).
pub fn vp8_frames(width: u32, height: u32, count: usize, keyint: u32) -> Vec<(Vec<u8>, bool)> {
    let cfg = EncoderConfig::new(width, height, 30, 400_000).with_keyframe_interval(Some(keyint));
    let mut enc = Encoder::new(cfg).expect("encoder");
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Vec::new();
    let mut i = 0usize;
    while out.len() < count {
        let mut buf = vec![0u8; w * h + 2 * cw * ch];
        for y in 0..h {
            for x in 0..w {
                buf[y * w + x] = ((x + y + i * 4) % 256) as u8;
            }
        }
        for c in 0..2 * cw * ch {
            buf[w * h + c] = (128 + (c + i) % 32) as u8;
        }
        let pic = Picture::from_i420(&buf, width, height).expect("picture");
        let force = out.len() % keyint as usize == 0;
        if let Some(f) = enc.encode(&pic, force).expect("encode") {
            out.push((f.data, f.keyframe));
        }
        i += 1;
    }
    out
}

/// A deterministic xorshift PRNG.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }
}

/// An input frame as the tests feed it: (track number, ns, keyframe, data).
#[derive(Clone, Debug)]
pub struct In {
    pub track: u64,
    pub ns: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

/// 30 fps VP8 on track 1 and 20 ms Opus on track 2, merged in time order
/// (video first at equal timestamps).
pub fn av_schedule(video: &[(Vec<u8>, bool)], audio_packets: usize) -> Vec<In> {
    let mut v: Vec<In> = video
        .iter()
        .enumerate()
        .map(|(i, (d, k))| In {
            track: 1,
            ns: i as u64 * 1_000_000_000 / 30,
            keyframe: *k,
            data: d.clone(),
        })
        .collect();
    let a: Vec<In> = (0..audio_packets)
        .map(|i| In {
            track: 2,
            ns: i as u64 * 20_000_000,
            keyframe: true,
            data: OPUS_SILENCE.to_vec(),
        })
        .collect();
    v.extend(a);
    // Stable sort: by tick (ms), video before audio at the same tick.
    v.sort_by_key(|f| (f.ns / 1_000_000, f.track));
    v
}
