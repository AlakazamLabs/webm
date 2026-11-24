use std::fmt;
use std::io;

/// Why the muxer refused a call or could not write.
///
/// The variants are part of the API: new ones may be added (the enum is
/// `non_exhaustive`), but existing ones keep their meaning.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The writer failed. The muxer can't know how much of the last write
    /// reached the output, so every later call returns
    /// [`Error::Poisoned`].
    Io(io::Error),
    /// An earlier I/O error left the output in an unknown state.
    Poisoned,
    /// A track was added after the first frame was written. Tracks are
    /// written at the start of the file, so they must all be added first.
    TracksLocked,
    /// More than 126 tracks.
    TooManyTracks,
    /// The [`TrackId`](crate::TrackId) belongs to no track of this muxer.
    UnknownTrack,
    /// A frame or `finish()` with no tracks added.
    NoTracks,
    /// The track's codec needs a `CodecPrivate` and none was given (AV1's
    /// configuration record, or Vorbis's three headers).
    MissingCodecPrivate,
    /// A track or option setting is invalid; the text names it.
    InvalidConfig(&'static str),
    /// A frame's timestamp is not later than the previous frame on the
    /// same track, once both are rounded down to the timestamp scale.
    /// Frames are never reordered.
    TimestampNotIncreasing {
        /// The track number.
        track: u64,
        /// The previous frame's timestamp, in timestamp-scale ticks.
        previous: u64,
        /// This frame's timestamp, in timestamp-scale ticks.
        got: u64,
    },
    /// A frame's timestamp is earlier than a frame already written on
    /// another track. WebM needs block timestamps to increase across the
    /// whole file, so write frames in time order.
    OutOfOrder {
        /// The latest timestamp written so far, in ticks.
        latest: u64,
        /// This frame's timestamp, in ticks.
        got: u64,
    },
    /// The timestamp doesn't fit: over `u64` nanoseconds after scaling, or
    /// a block duration or offset beyond what a block can store.
    TimestampOutOfRange,
    /// A frame is empty, or too large for an EBML element.
    InvalidFrame,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "write failed: {e}"),
            Error::Poisoned => f.write_str("an earlier write failed; the output is incomplete"),
            Error::TracksLocked => f.write_str("tracks must be added before the first frame"),
            Error::TooManyTracks => f.write_str("too many tracks (at most 126)"),
            Error::UnknownTrack => f.write_str("no such track"),
            Error::NoTracks => f.write_str("no tracks were added"),
            Error::MissingCodecPrivate => f.write_str("this codec needs CodecPrivate data"),
            Error::InvalidConfig(why) => write!(f, "invalid setting: {why}"),
            Error::TimestampNotIncreasing { track, previous, got } => write!(
                f,
                "track {track}: timestamp {got} is not after the previous frame's {previous} (in timestamp-scale ticks)"
            ),
            Error::OutOfOrder { latest, got } => write!(
                f,
                "timestamp {got} is earlier than {latest}, already written (in timestamp-scale ticks)"
            ),
            Error::TimestampOutOfRange => f.write_str("timestamp out of range"),
            Error::InvalidFrame => f.write_str("frame is empty or too large"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}
