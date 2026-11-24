//! The muxer: header, clusters, blocks, Cues and the final patch-up.

use std::io::{self, Seek, SeekFrom, Write};

use crate::ebml::{self, Buf, UNKNOWN_SIZE};
use crate::ids;
use crate::track::{AudioTrack, VideoTrack};
use crate::Error;

/// The space kept at the start of the Segment for the SeekHead: room for
/// three entries (Info, Tracks, Cues) with 8-octet positions.
const SEEK_HEAD_SPACE: u64 = 68;
/// A Duration element with an 8-octet float: 2 (ID) + 1 (size) + 8.
const DURATION_LEN: u64 = 11;
/// The most tracks a 1-octet track number VINT holds.
const MAX_TRACKS: usize = 126;

/// Muxer settings. Build with [`Options::new`] and the `with_` methods.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    timestamp_scale_ns: u64,
    max_cluster_duration_ns: u64,
    max_cluster_bytes: u64,
    title: Option<String>,
    muxing_app: String,
    writing_app: String,
    cues_space: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options::new()
    }
}

impl Options {
    /// The defaults: a 1 ms timestamp scale, clusters of at most 5 s and
    /// 5 MiB, no title, `MuxingApp` and `WritingApp` both
    /// `webm-muxer <version>`, and Cues written after the last cluster.
    pub fn new() -> Self {
        let app = concat!("webm-muxer ", env!("CARGO_PKG_VERSION"));
        Options {
            timestamp_scale_ns: 1_000_000,
            max_cluster_duration_ns: 5_000_000_000,
            max_cluster_bytes: 5 * 1024 * 1024,
            title: None,
            muxing_app: app.to_string(),
            writing_app: app.to_string(),
            cues_space: 0,
        }
    }

    /// `TimestampScale`: nanoseconds per timestamp tick (default
    /// 1,000,000, so ticks are milliseconds). Frame timestamps are rounded
    /// down to it. A cluster spans at most 32,767 ticks.
    pub fn with_timestamp_scale(mut self, ns_per_tick: u64) -> Self {
        self.timestamp_scale_ns = ns_per_tick;
        self
    }

    /// The longest a cluster may run before a new one starts, in
    /// nanoseconds (default 5 s; never more than 32,767 ticks).
    pub fn with_max_cluster_duration(mut self, ns: u64) -> Self {
        self.max_cluster_duration_ns = ns;
        self
    }

    /// The largest a cluster may grow before a new one starts, in bytes
    /// (default 5 MiB). A single frame larger than this gets a cluster of
    /// its own.
    pub fn with_max_cluster_bytes(mut self, bytes: u64) -> Self {
        self.max_cluster_bytes = bytes;
        self
    }

    /// The `Title`. None by default.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The `MuxingApp` string.
    pub fn with_muxing_app(mut self, app: impl Into<String>) -> Self {
        self.muxing_app = app.into();
        self
    }

    /// The `WritingApp` string.
    pub fn with_writing_app(mut self, app: impl Into<String>) -> Self {
        self.writing_app = app.into();
        self
    }

    /// Seekable mode only: reserve this many bytes before the first
    /// cluster for the Cues, as the WebM guidelines recommend, so a player
    /// finds them without reading the end of the file. If the Cues don't
    /// fit at [`finish`](Muxer::finish), they go after the last cluster
    /// and the space stays as a Void element. Each cue point takes about
    /// 20 to 30 bytes. Default 0: Cues go after the last cluster.
    pub fn with_cues_space(mut self, bytes: u64) -> Self {
        self.cues_space = bytes;
        self
    }

    fn check(&self) -> Result<(), Error> {
        if self.timestamp_scale_ns == 0 {
            return Err(Error::InvalidConfig("timestamp scale must be non-zero"));
        }
        if self.max_cluster_bytes == 0 {
            return Err(Error::InvalidConfig("max cluster bytes must be non-zero"));
        }
        if self.max_cluster_duration_ns < self.timestamp_scale_ns {
            return Err(Error::InvalidConfig("max cluster duration must be at least one tick"));
        }
        if self.cues_space == 1 {
            return Err(Error::InvalidConfig("cues space must be 0 or at least 2 bytes"));
        }
        if self.cues_space > ebml::MAX_SIZE {
            return Err(Error::InvalidConfig("cues space is too large"));
        }
        Ok(())
    }
}

/// A track, as returned by `add_video_track` and `add_audio_track`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackId(u64);

impl TrackId {
    /// The track's `TrackNumber` in the file (1 for the first track).
    pub fn number(self) -> u64 {
        self.0
    }
}

/// Per-frame settings beyond the timestamp and keyframe flag, for
/// [`Muxer::write_frame_with`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameOptions {
    /// The frame can be decoded on its own. Audio frames are always
    /// written as keyframes.
    pub keyframe: bool,
    /// The frame's duration in nanoseconds (`BlockDuration`). Without one,
    /// a player takes the gap to the next frame.
    pub duration_ns: Option<u64>,
    /// Nanoseconds of padding at the end of the frame for the player to
    /// drop (`DiscardPadding`), for example the end trim of an Opus
    /// stream. Negative values drop from the start.
    pub discard_padding_ns: Option<i64>,
}

impl FrameOptions {
    /// Options for a frame: keyframe or not, nothing else set.
    pub fn new(keyframe: bool) -> Self {
        FrameOptions {
            keyframe,
            duration_ns: None,
            discard_padding_ns: None,
        }
    }

    /// Sets the frame's duration.
    pub fn with_duration(mut self, ns: u64) -> Self {
        self.duration_ns = Some(ns);
        self
    }

    /// Sets the discard padding.
    pub fn with_discard_padding(mut self, ns: i64) -> Self {
        self.discard_padding_ns = Some(ns);
        self
    }
}

type SeekFn<W> = fn(&mut W, SeekFrom) -> io::Result<u64>;

struct TrackState {
    entry: Vec<u8>,
    video: bool,
    last: Option<u64>,
    last_delta: u64,
    end: u64,
}

struct Cluster {
    /// Offset of the size field, from the writer's start.
    size_at: u64,
    /// Offset of the cluster's data, from the writer's start.
    data_at: u64,
    /// The cluster's timestamp, in ticks.
    timestamp: u64,
    bytes: u64,
    blocks: u64,
}

struct Cue {
    time: u64,
    track: u64,
    cluster: u64,
    relative: u64,
}

struct Layout {
    segment_size_at: u64,
    segment_data_at: u64,
    info_rel: u64,
    tracks_rel: u64,
    duration_at: u64,
    cues_space_rel: u64,
}

/// Writes one WebM file or stream.
///
/// Add every track first, then write frames in time order, then call
/// [`finish`](Self::finish). See the [crate docs](crate) for the two modes.
pub struct Muxer<W: Write> {
    w: W,
    seek: Option<SeekFn<W>>,
    opts: Options,
    max_cluster_ticks: u64,
    tracks: Vec<TrackState>,
    primary_video: Option<usize>,
    /// Bytes written from the writer's start.
    pos: u64,
    /// The writer's position when the muxer took it (seekable mode).
    base: u64,
    layout: Option<Layout>,
    cluster: Option<Cluster>,
    cues: Vec<Cue>,
    latest: u64,
    poisoned: bool,
}

impl<W: Write> Muxer<W> {
    /// A muxer in live mode, for any writer: a socket, a pipe, a file
    /// being uploaded as it grows. The Segment and Clusters have unknown
    /// sizes and nothing is written twice, so output can be sent as it is
    /// made. There are no Cues and no Duration. A stream cut off at any
    /// point plays up to its last whole cluster.
    ///
    /// Nothing is written until the first frame (or `finish`). Wrap slow
    /// writers in a [`std::io::BufWriter`] and call [`flush`](Self::flush)
    /// when a cluster's data must go out now.
    pub fn new(writer: W, options: Options) -> Result<Self, Error> {
        Self::build(writer, None, 0, options)
    }

    fn build(w: W, seek: Option<SeekFn<W>>, base: u64, opts: Options) -> Result<Self, Error> {
        opts.check()?;
        let max_cluster_ticks = (opts.max_cluster_duration_ns / opts.timestamp_scale_ns).min(i16::MAX as u64);
        Ok(Muxer {
            w,
            seek,
            opts,
            max_cluster_ticks,
            tracks: Vec::new(),
            primary_video: None,
            pos: 0,
            base,
            layout: None,
            cluster: None,
            cues: Vec::new(),
            latest: 0,
            poisoned: false,
        })
    }

    /// Adds a video track. The first video track added sets the cluster
    /// boundaries: each of its keyframes starts a cluster.
    pub fn add_video_track(&mut self, track: VideoTrack) -> Result<TrackId, Error> {
        self.ready_for_track()?;
        let number = self.tracks.len() as u64 + 1;
        let entry = track.entry(number)?;
        if self.primary_video.is_none() {
            self.primary_video = Some(self.tracks.len());
        }
        self.push_track(entry, true)
    }

    /// Adds an audio track.
    pub fn add_audio_track(&mut self, track: AudioTrack) -> Result<TrackId, Error> {
        self.ready_for_track()?;
        let number = self.tracks.len() as u64 + 1;
        let entry = track.entry(number)?;
        self.push_track(entry, false)
    }

    fn ready_for_track(&self) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        if self.layout.is_some() {
            return Err(Error::TracksLocked);
        }
        if self.tracks.len() >= MAX_TRACKS {
            return Err(Error::TooManyTracks);
        }
        Ok(())
    }

    fn push_track(&mut self, entry: Vec<u8>, video: bool) -> Result<TrackId, Error> {
        self.tracks.push(TrackState {
            entry,
            video,
            last: None,
            last_delta: 0,
            end: 0,
        });
        Ok(TrackId(self.tracks.len() as u64))
    }

    /// Writes one frame: `timestamp_ns` is its presentation time in
    /// nanoseconds from the start, and `keyframe` says it can be decoded
    /// on its own. The frame's bytes go straight to the writer; nothing is
    /// kept.
    ///
    /// Timestamps must increase on each track and must not go back across
    /// tracks (write frames in time order). At equal timestamps, write a
    /// video keyframe before the audio, so the audio joins its cluster.
    pub fn write_frame(&mut self, track: TrackId, frame: &[u8], timestamp_ns: u64, keyframe: bool) -> Result<(), Error> {
        self.write_frame_with(track, frame, timestamp_ns, FrameOptions::new(keyframe))
    }

    /// [`write_frame`](Self::write_frame) with a duration or discard
    /// padding. A frame with either is written as a `BlockGroup` (with a
    /// `ReferenceBlock` to the track's previous frame when it isn't a
    /// keyframe); every other frame as a `SimpleBlock`.
    pub fn write_frame_with(&mut self, track: TrackId, frame: &[u8], timestamp_ns: u64, options: FrameOptions) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let index = usize::try_from(track.0)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&i| i < self.tracks.len())
            .ok_or(Error::UnknownTrack)?;
        if frame.is_empty() || frame.len() as u64 > ebml::MAX_SIZE - 64 {
            return Err(Error::InvalidFrame);
        }
        let scale = self.opts.timestamp_scale_ns;
        let ticks = timestamp_ns / scale;
        let state = &self.tracks[index];
        if let Some(prev) = state.last {
            if ticks <= prev {
                return Err(Error::TimestampNotIncreasing {
                    track: track.0,
                    previous: prev,
                    got: ticks,
                });
            }
        }
        if ticks < self.latest {
            return Err(Error::OutOfOrder {
                latest: self.latest,
                got: ticks,
            });
        }
        let keyframe = options.keyframe || !state.video;
        let duration_ticks = options.duration_ns.map(|d| d / scale);
        let reference = if keyframe { None } else { Some(state.last.map_or(0, |p| p as i64 - ticks as i64)) };
        let group = options.duration_ns.is_some() || options.discard_padding_ns.is_some();

        // The extras that follow the Block in a BlockGroup.
        let mut extras = Buf::new();
        if group {
            if let Some(d) = duration_ticks {
                extras.uint(ids::BLOCK_DURATION, d);
            }
            if let Some(r) = reference {
                extras.int(ids::REFERENCE_BLOCK, r);
            }
            if let Some(p) = options.discard_padding_ns {
                extras.int(ids::DISCARD_PADDING, p);
            }
        }
        let payload = 4 + frame.len() as u64;
        let block_len = if group {
            let inner = ebml::header_len(ids::BLOCK, payload) + payload + extras.len();
            ebml::header_len(ids::BLOCK_GROUP, inner) + inner
        } else {
            ebml::header_len(ids::SIMPLE_BLOCK, payload) + payload
        };

        self.ensure_header()?;

        let is_primary = self.primary_video == Some(index);
        let new_cluster = match &self.cluster {
            None => true,
            Some(c) => {
                c.blocks > 0
                    && ((is_primary && keyframe)
                        || ticks - c.timestamp >= self.max_cluster_ticks
                        || c.bytes.saturating_add(block_len) > self.opts.max_cluster_bytes)
            }
        };
        if new_cluster {
            self.close_cluster()?;
            self.open_cluster(ticks)?;
            let cue_track = self.primary_video.unwrap_or(0);
            if index == cue_track && keyframe {
                let layout = self.layout.as_ref().ok_or(Error::Poisoned)?;
                let cluster = self.cluster.as_ref().ok_or(Error::Poisoned)?;
                self.cues.push(Cue {
                    time: ticks,
                    track: track.0,
                    cluster: cluster.size_at - 4 - layout.segment_data_at,
                    relative: self.pos - cluster.data_at,
                });
            }
        }
        let cluster_ts = self.cluster.as_ref().map_or(ticks, |c| c.timestamp);
        let relative = i16::try_from(ticks - cluster_ts).map_err(|_| Error::TimestampOutOfRange)?;

        let mut head = Vec::with_capacity(32);
        if group {
            let inner = ebml::header_len(ids::BLOCK, payload) + payload + extras.len();
            ebml::push_header(&mut head, ids::BLOCK_GROUP, inner);
            ebml::push_header(&mut head, ids::BLOCK, payload);
        } else {
            ebml::push_header(&mut head, ids::SIMPLE_BLOCK, payload);
        }
        head.push(0x80 | track.0 as u8);
        head.extend_from_slice(&relative.to_be_bytes());
        head.push(if keyframe && !group { 0x80 } else { 0x00 });
        self.out(&head)?;
        self.out(frame)?;
        if group {
            let extras = std::mem::take(&mut extras.0);
            self.out(&extras)?;
        }

        if let Some(c) = &mut self.cluster {
            c.bytes += block_len;
            c.blocks += 1;
        }
        let state = &mut self.tracks[index];
        if let Some(prev) = state.last {
            state.last_delta = ticks - prev;
        }
        state.last = Some(ticks);
        state.end = ticks.saturating_add(duration_ticks.unwrap_or(state.last_delta));
        self.latest = ticks;
        Ok(())
    }

    /// Flushes the writer.
    pub fn flush(&mut self) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        self.w.flush().map_err(|e| self.poison(e))
    }

    /// The writer.
    pub fn get_ref(&self) -> &W {
        &self.w
    }

    /// Bytes written so far.
    pub fn bytes_written(&self) -> u64 {
        self.pos
    }

    /// Ends the file and returns the writer. In live mode this only
    /// flushes. In seekable mode it writes the Cues, then goes back to
    /// fill in the Duration, the SeekHead and the Segment and Cluster
    /// sizes, and leaves the writer at the end of the file.
    ///
    /// Dropping a muxer without `finish` leaves what was written: it plays
    /// like a live stream cut off at that point.
    pub fn finish(mut self) -> Result<W, Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        self.ensure_header()?;
        self.close_cluster()?;
        if let Some(seek) = self.seek {
            self.finish_seekable(seek)?;
        }
        self.w.flush().map_err(|e| self.poison(e))?;
        Ok(self.w)
    }

    fn finish_seekable(&mut self, seek: SeekFn<W>) -> Result<(), Error> {
        let layout = self.layout.as_ref().ok_or(Error::Poisoned)?;
        let (seg_data, info_rel, tracks_rel, duration_at, cues_space_rel, seg_size_at) = (
            layout.segment_data_at,
            layout.info_rel,
            layout.tracks_rel,
            layout.duration_at,
            layout.cues_space_rel,
            layout.segment_size_at,
        );
        let mut cues_rel = None;
        if !self.cues.is_empty() {
            let mut b = Buf::new();
            b.master(ids::CUES, |b| {
                for cue in &self.cues {
                    b.master(ids::CUE_POINT, |b| {
                        b.uint(ids::CUE_TIME, cue.time);
                        b.master(ids::CUE_TRACK_POSITIONS, |b| {
                            b.uint(ids::CUE_TRACK, cue.track);
                            b.uint(ids::CUE_CLUSTER_POSITION, cue.cluster);
                            b.uint(ids::CUE_RELATIVE_POSITION, cue.relative);
                        });
                    });
                }
            });
            let space = self.opts.cues_space;
            let fits = b.len() == space || (b.len() + 2 <= space);
            if fits {
                if b.len() < space {
                    b.void(space - b.len());
                }
                self.patch(seek, seg_data + cues_space_rel, &b.0)?;
                cues_rel = Some(cues_space_rel);
            } else {
                cues_rel = Some(self.pos - seg_data);
                self.out(&b.0)?;
            }
        }
        let end = self.pos;

        let duration = self.tracks.iter().map(|t| t.end).max().unwrap_or(0);
        let mut d = Buf::new();
        d.float(ids::DURATION, duration as f64);
        self.patch(seek, duration_at, &d.0)?;

        let seek_head = seek_head(info_rel, tracks_rel, cues_rel);
        self.patch(seek, seg_data, &seek_head)?;
        self.patch(seek, seg_size_at, &ebml::size8(end - seg_data))?;
        self.seek_to(seek, end)
    }

    fn ensure_header(&mut self) -> Result<(), Error> {
        if self.layout.is_some() {
            return Ok(());
        }
        if self.tracks.is_empty() {
            return Err(Error::NoTracks);
        }
        let seekable = self.seek.is_some();
        let mut b = Buf::new();
        b.master(ids::EBML, |b| {
            b.uint(ids::EBML_VERSION, 1);
            b.uint(ids::EBML_READ_VERSION, 1);
            b.uint(ids::EBML_MAX_ID_LENGTH, 4);
            b.uint(ids::EBML_MAX_SIZE_LENGTH, 8);
            b.string(ids::DOC_TYPE, "webm");
            b.uint(ids::DOC_TYPE_VERSION, 4);
            b.uint(ids::DOC_TYPE_READ_VERSION, 2);
        });
        ebml::push_id(&mut b.0, ids::SEGMENT);
        let segment_size_at = self.pos + b.len();
        b.0.extend_from_slice(&UNKNOWN_SIZE);
        let segment_data_at = self.pos + b.len();

        let mut info = Buf::new();
        info.master(ids::INFO, |i| {
            i.uint(ids::TIMESTAMP_SCALE, self.opts.timestamp_scale_ns);
            if let Some(t) = &self.opts.title {
                i.string(ids::TITLE, t);
            }
            i.string(ids::MUXING_APP, &self.opts.muxing_app);
            i.string(ids::WRITING_APP, &self.opts.writing_app);
            if seekable {
                // Filled with the Duration at finish.
                i.void(DURATION_LEN);
            }
        });
        let mut tracks = Buf::new();
        tracks.master(ids::TRACKS, |t| {
            for track in &self.tracks {
                t.0.extend_from_slice(&track.entry);
            }
        });

        let info_rel = if seekable { SEEK_HEAD_SPACE } else { 0 };
        let tracks_rel = info_rel + info.len();
        let cues_space_rel = tracks_rel + tracks.len();
        if seekable {
            b.0.extend_from_slice(&seek_head(info_rel, tracks_rel, None));
        }
        let duration_at = segment_data_at + info_rel + info.len() - DURATION_LEN;
        b.0.extend_from_slice(&info.0);
        b.0.extend_from_slice(&tracks.0);
        if seekable && self.opts.cues_space > 0 {
            b.void(self.opts.cues_space);
        }
        self.out(&b.0)?;
        self.layout = Some(Layout {
            segment_size_at,
            segment_data_at,
            info_rel,
            tracks_rel,
            duration_at,
            cues_space_rel,
        });
        Ok(())
    }

    fn open_cluster(&mut self, ticks: u64) -> Result<(), Error> {
        let mut b = Vec::with_capacity(24);
        ebml::push_id(&mut b, ids::CLUSTER);
        let size_at = self.pos + b.len() as u64;
        b.extend_from_slice(&UNKNOWN_SIZE);
        let data_at = self.pos + b.len() as u64;
        let mut t = Buf::new();
        t.uint(ids::TIMESTAMP, ticks);
        b.extend_from_slice(&t.0);
        self.out(&b)?;
        self.cluster = Some(Cluster {
            size_at,
            data_at,
            timestamp: ticks,
            bytes: b.len() as u64,
            blocks: 0,
        });
        Ok(())
    }

    fn close_cluster(&mut self) -> Result<(), Error> {
        let Some(c) = self.cluster.take() else {
            return Ok(());
        };
        if let Some(seek) = self.seek {
            let end = self.pos;
            self.patch(seek, c.size_at, &ebml::size8(end - c.data_at))?;
            self.seek_to(seek, end)?;
        }
        Ok(())
    }

    /// Overwrites bytes at an earlier offset. Leaves the writer there;
    /// callers seek back to the end.
    fn patch(&mut self, seek: SeekFn<W>, at: u64, bytes: &[u8]) -> Result<(), Error> {
        let target = self.base + at;
        seek(&mut self.w, SeekFrom::Start(target)).map_err(|e| self.poison(e))?;
        self.w.write_all(bytes).map_err(|e| self.poison(e))
    }

    fn seek_to(&mut self, seek: SeekFn<W>, at: u64) -> Result<(), Error> {
        let target = self.base + at;
        seek(&mut self.w, SeekFrom::Start(target)).map_err(|e| self.poison(e))?;
        Ok(())
    }

    fn out(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.w.write_all(bytes).map_err(|e| self.poison(e))?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    fn poison(&mut self, e: io::Error) -> Error {
        self.poisoned = true;
        Error::Io(e)
    }
}

impl<W: Write + Seek> Muxer<W> {
    /// A muxer in seekable (file) mode. It writes like live mode, then
    /// [`finish`](Self::finish) adds the Cues (one per cluster that starts
    /// on a keyframe) and fills in the Duration, the SeekHead and every
    /// size. Each closed cluster's size is filled in as soon as it closes,
    /// so a file whose writer stops before `finish` still plays up to its
    /// last whole cluster.
    ///
    /// The file starts at the writer's current position.
    pub fn new_seekable(mut writer: W, options: Options) -> Result<Self, Error> {
        let base = writer.stream_position()?;
        Self::build(writer, Some(<W as Seek>::seek), base, options)
    }
}

/// A SeekHead of exactly `SEEK_HEAD_SPACE` bytes, padded with a Void.
fn seek_head(info_rel: u64, tracks_rel: u64, cues_rel: Option<u64>) -> Vec<u8> {
    let mut entries = vec![(ids::INFO, info_rel), (ids::TRACKS, tracks_rel)];
    if let Some(c) = cues_rel {
        entries.push((ids::CUES, c));
    }
    let mut b = Buf::new();
    b.master(ids::SEEK_HEAD, |b| {
        for (id, rel) in entries {
            b.master(ids::SEEK, |b| {
                b.binary(ids::SEEK_ID, &id.to_be_bytes());
                b.uint8(ids::SEEK_POSITION, rel);
            });
        }
    });
    let rest = SEEK_HEAD_SPACE - b.len();
    if rest > 0 {
        b.void(rest);
    }
    b.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_head_fills_its_space_exactly() {
        assert_eq!(seek_head(68, 200, None).len() as u64, SEEK_HEAD_SPACE);
        assert_eq!(seek_head(68, 200, Some(1 << 40)).len() as u64, SEEK_HEAD_SPACE);
    }
}
