#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod ebml;
mod error;
mod ids;
mod muxer;
mod track;

pub use error::Error;
pub use muxer::{FrameOptions, Muxer, Options, TrackId};
pub use track::{opus_head, vp9_codec_private, xiph_lace, AudioCodec, AudioTrack, VideoCodec, VideoTrack};
