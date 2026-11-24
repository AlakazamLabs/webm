//! Memory stays bounded: frames are streamed to the writer, never copied
//! or kept. Only the Cues (a few bytes per keyframe cluster) grow.
//!
//! This test binary counts live heap bytes with a wrapper around the
//! system allocator. That wrapper is the only `unsafe` in the repository
//! (implementing `GlobalAlloc` requires it); it is test code and is not
//! part of the published crate, which is `#![forbid(unsafe_code)]`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{self, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use webm_muxer::{AudioTrack, Muxer, Options, VideoCodec, VideoTrack};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` with the caller's arguments
// unchanged; the counters are plain atomics.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Discards bytes but tracks the position, so seekable mode works.
#[derive(Default)]
struct NullSeek {
    pos: u64,
    len: u64,
}

impl Write for NullSeek {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pos += buf.len() as u64;
        self.len = self.len.max(self.pos);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for NullSeek {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = match to {
            SeekFrom::Start(p) => p,
            SeekFrom::End(d) => (self.len as i64 + d) as u64,
            SeekFrom::Current(d) => (self.pos as i64 + d) as u64,
        };
        Ok(self.pos)
    }
}

/// Peak heap growth while muxing `frames` frames of `size` bytes, beyond
/// what was live at the start (the frame buffer is allocated first).
fn peak_growth<W: Write>(m: &mut Muxer<W>, frames: u64, size: usize, keyint: u64) -> usize {
    let v = m.add_video_track(VideoTrack::new(VideoCodec::Vp8, 1920, 1080)).unwrap();
    let a = m.add_audio_track(AudioTrack::opus(2, 312)).unwrap();
    let frame = vec![0x5Au8; size];
    let audio = [0xF8u8, 0xFF, 0xFE];
    let start = LIVE.load(Ordering::SeqCst);
    PEAK.store(start, Ordering::SeqCst);
    for i in 0..frames {
        let ns = i * 33_000_000;
        m.write_frame(v, &frame, ns, i % keyint == 0).unwrap();
        m.write_frame(a, &audio, ns, true).unwrap();
    }
    PEAK.load(Ordering::SeqCst) - start
}

#[test]
fn memory_does_not_grow_with_frame_size_or_count() {
    // 300 frames of 1 MiB (300 MiB of video) in live mode.
    let mut live = Muxer::new(io::sink(), Options::new().with_max_cluster_bytes(u64::MAX / 4)).unwrap();
    let growth = peak_growth(&mut live, 300, 1 << 20, 60);
    assert!(growth < 16 * 1024, "live mode grew by {growth} bytes");
    live.finish().unwrap();

    // Seekable mode keeps one small entry per keyframe cluster (5 here).
    let mut file = Muxer::new_seekable(NullSeek::default(), Options::new()).unwrap();
    let growth = peak_growth(&mut file, 300, 1 << 20, 60);
    assert!(growth < 16 * 1024, "seekable mode grew by {growth} bytes");
    let w = file.finish().unwrap();
    assert!(w.len > 300 << 20);

    // The Cues grow with keyframes, by well under 100 bytes each.
    let mut many = Muxer::new_seekable(NullSeek::default(), Options::new()).unwrap();
    let growth = peak_growth(&mut many, 3000, 100, 1);
    assert!(growth < 3000 * 100, "3000 keyframe clusters grew by {growth} bytes");
    many.finish().unwrap();
}
