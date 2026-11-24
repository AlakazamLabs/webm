//! Track settings and the codec helpers that build `CodecPrivate`.

use crate::ebml::Buf;
use crate::ids;
use crate::Error;

/// A video codec WebM carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VideoCodec {
    /// VP8 (`V_VP8`). No `CodecPrivate`.
    Vp8,
    /// VP9 (`V_VP9`). `CodecPrivate` is optional; see [`vp9_codec_private`].
    Vp9,
    /// AV1 (`V_AV1`). `CodecPrivate` is required: the
    /// AV1CodecConfigurationRecord.
    Av1,
}

impl VideoCodec {
    /// The Matroska codec ID.
    pub fn codec_id(self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "V_VP8",
            VideoCodec::Vp9 => "V_VP9",
            VideoCodec::Av1 => "V_AV1",
        }
    }
}

/// An audio codec WebM carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AudioCodec {
    /// Opus (`A_OPUS`).
    Opus,
    /// Vorbis (`A_VORBIS`).
    Vorbis,
}

impl AudioCodec {
    /// The Matroska codec ID.
    pub fn codec_id(self) -> &'static str {
        match self {
            AudioCodec::Opus => "A_OPUS",
            AudioCodec::Vorbis => "A_VORBIS",
        }
    }
}

/// A video track's settings, for [`Muxer::add_video_track`](crate::Muxer::add_video_track).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VideoTrack {
    pub(crate) codec: VideoCodec,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) display: Option<(u32, u32)>,
    pub(crate) codec_private: Option<Vec<u8>>,
    pub(crate) common: Common,
}

/// An audio track's settings, for [`Muxer::add_audio_track`](crate::Muxer::add_audio_track).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AudioTrack {
    pub(crate) codec: AudioCodec,
    pub(crate) source: AudioSource,
    pub(crate) bit_depth: Option<u8>,
    pub(crate) seek_pre_roll_ns: Option<u64>,
    pub(crate) common: Common,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AudioSource {
    OpusHead(Vec<u8>),
    Vorbis(Vec<u8>, Vec<u8>, Vec<u8>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Common {
    pub(crate) name: Option<String>,
    pub(crate) language: Option<String>,
    pub(crate) uid: Option<u64>,
}

macro_rules! common_setters {
    () => {
        /// A human-readable track name (`Name`). None by default.
        pub fn with_name(mut self, name: impl Into<String>) -> Self {
            self.common.name = Some(name.into());
            self
        }

        /// The track's language (`Language`), for example `"eng"`. Not
        /// written by default, which readers take as English.
        pub fn with_language(mut self, language: impl Into<String>) -> Self {
            self.common.language = Some(language.into());
            self
        }

        /// The track's `TrackUID` (non-zero). By default it is derived
        /// from the track number, so output is reproducible; pass a
        /// random value for tracks that must stay unique across files.
        pub fn with_uid(mut self, uid: u64) -> Self {
            self.common.uid = Some(uid);
            self
        }
    };
}

impl VideoTrack {
    /// A video track of this codec and coded size in pixels.
    pub fn new(codec: VideoCodec, width: u32, height: u32) -> Self {
        VideoTrack {
            codec,
            width,
            height,
            display: None,
            codec_private: None,
            common: Common::default(),
        }
    }

    /// The codec's `CodecPrivate` bytes: required for AV1, optional for
    /// VP9 ([`vp9_codec_private`]), not used for VP8.
    pub fn with_codec_private(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.codec_private = Some(data.into());
        self
    }

    /// The display size, when it differs from the coded size (for
    /// non-square pixels). Not written by default.
    pub fn with_display_size(mut self, width: u32, height: u32) -> Self {
        self.display = Some((width, height));
        self
    }

    common_setters!();

    pub(crate) fn entry(&self, number: u64) -> Result<Vec<u8>, Error> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::InvalidConfig("video width and height must be non-zero"));
        }
        if let Some((w, h)) = self.display {
            if w == 0 || h == 0 {
                return Err(Error::InvalidConfig("display width and height must be non-zero"));
            }
        }
        match (self.codec, &self.codec_private) {
            (VideoCodec::Av1, None) => return Err(Error::MissingCodecPrivate),
            (VideoCodec::Vp8, Some(_)) => {
                return Err(Error::InvalidConfig("VP8 has no CodecPrivate"));
            }
            _ => {}
        }
        let mut b = Buf::new();
        b.master(ids::TRACK_ENTRY, |b| {
            self.common.write(b, number, 1);
            b.string(ids::CODEC_ID, self.codec.codec_id());
            if let Some(cp) = &self.codec_private {
                b.binary(ids::CODEC_PRIVATE, cp);
            }
            b.master(ids::VIDEO, |b| {
                b.uint(ids::PIXEL_WIDTH, self.width.into());
                b.uint(ids::PIXEL_HEIGHT, self.height.into());
                if let Some((w, h)) = self.display {
                    b.uint(ids::DISPLAY_WIDTH, w.into());
                    b.uint(ids::DISPLAY_HEIGHT, h.into());
                }
            });
        });
        self.common.check()?;
        Ok(b.0)
    }
}

impl AudioTrack {
    /// An Opus track with one or two channels (channel mapping family 0),
    /// `pre_skip` samples at 48 kHz to drop from the start (the encoder's
    /// lookahead), and an input sample rate of 48 kHz. Its `CodecPrivate`
    /// is the Opus identification header from [`opus_head`].
    pub fn opus(channels: u8, pre_skip: u16) -> Self {
        Self::opus_with_head(opus_head(channels, pre_skip, 48_000, 0))
    }

    /// An Opus track from a complete Opus identification header
    /// (`OpusHead`), for more than two channels or other settings. The
    /// channel count, pre-skip and input sample rate are read from it.
    pub fn opus_with_head(head: impl Into<Vec<u8>>) -> Self {
        AudioTrack {
            codec: AudioCodec::Opus,
            source: AudioSource::OpusHead(head.into()),
            bit_depth: None,
            seek_pre_roll_ns: Some(80_000_000),
            common: Common::default(),
        }
    }

    /// A Vorbis track from its three header packets (identification,
    /// comment and setup). The channel count and sample rate are read
    /// from the identification header.
    pub fn vorbis(
        identification: impl Into<Vec<u8>>,
        comment: impl Into<Vec<u8>>,
        setup: impl Into<Vec<u8>>,
    ) -> Self {
        AudioTrack {
            codec: AudioCodec::Vorbis,
            source: AudioSource::Vorbis(identification.into(), comment.into(), setup.into()),
            bit_depth: None,
            seek_pre_roll_ns: None,
            common: Common::default(),
        }
    }

    /// The `BitDepth` element. Not written by default.
    pub fn with_bit_depth(mut self, bits: u8) -> Self {
        self.bit_depth = Some(bits);
        self
    }

    /// `SeekPreRoll` in nanoseconds: how much audio a player decodes and
    /// drops before a seek point. Opus defaults to 80 ms, as its mapping
    /// asks; `None` leaves it out.
    pub fn with_seek_pre_roll(mut self, ns: Option<u64>) -> Self {
        self.seek_pre_roll_ns = ns;
        self
    }

    /// The codec.
    pub fn codec(&self) -> AudioCodec {
        self.codec
    }

    common_setters!();

    pub(crate) fn entry(&self, number: u64) -> Result<Vec<u8>, Error> {
        let (codec_private, channels, rate, codec_delay_ns) = match &self.source {
            AudioSource::OpusHead(head) => {
                let h = parse_opus_head(head)?;
                let rate = if h.input_rate == 0 { 48_000 } else { h.input_rate };
                let delay = u64::from(h.pre_skip) * 1_000_000_000 / 48_000;
                (head.clone(), h.channels, rate, Some(delay))
            }
            AudioSource::Vorbis(id, comment, setup) => {
                let (channels, rate) = parse_vorbis_id(id)?;
                if comment.is_empty() || setup.is_empty() {
                    return Err(Error::MissingCodecPrivate);
                }
                (xiph_lace(&[id, comment, setup]), channels, rate, None)
            }
        };
        if self.bit_depth == Some(0) {
            return Err(Error::InvalidConfig("bit depth must be non-zero"));
        }
        let mut b = Buf::new();
        b.master(ids::TRACK_ENTRY, |b| {
            self.common.write(b, number, 2);
            b.string(ids::CODEC_ID, self.codec.codec_id());
            b.binary(ids::CODEC_PRIVATE, &codec_private);
            if let Some(d) = codec_delay_ns {
                b.uint(ids::CODEC_DELAY, d);
            }
            if let Some(p) = self.seek_pre_roll_ns {
                b.uint(ids::SEEK_PRE_ROLL, p);
            }
            b.master(ids::AUDIO, |b| {
                b.float(ids::SAMPLING_FREQUENCY, f64::from(rate));
                b.uint(ids::CHANNELS, channels.into());
                if let Some(bits) = self.bit_depth {
                    b.uint(ids::BIT_DEPTH, bits.into());
                }
            });
        });
        self.common.check()?;
        Ok(b.0)
    }
}

impl Common {
    fn write(&self, b: &mut Buf, number: u64, track_type: u64) {
        b.uint(ids::TRACK_NUMBER, number);
        b.uint(ids::TRACK_UID, self.uid.unwrap_or_else(|| default_uid(number)));
        b.uint(ids::TRACK_TYPE, track_type);
        b.uint(ids::FLAG_LACING, 0);
        if let Some(name) = &self.name {
            b.string(ids::NAME, name);
        }
        if let Some(lang) = &self.language {
            b.string(ids::LANGUAGE, lang);
        }
    }

    fn check(&self) -> Result<(), Error> {
        if self.uid == Some(0) {
            return Err(Error::InvalidConfig("TrackUID must be non-zero"));
        }
        if let Some(lang) = &self.language {
            if lang.is_empty() || !lang.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
                return Err(Error::InvalidConfig("language must be a non-empty ASCII code"));
            }
        }
        Ok(())
    }
}

/// A reproducible, non-zero UID for a track number (a 64-bit mix).
fn default_uid(number: u64) -> u64 {
    let mut z = number.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    if z == 0 {
        1
    } else {
        z
    }
}

/// Builds an Opus identification header (`OpusHead`, RFC 7845 section
/// 5.1) for channel mapping family 0: one or two channels.
///
/// `pre_skip` is in 48 kHz samples, `input_sample_rate` is informational
/// (0 for unknown), and `output_gain` is in Q7.8 dB. Values outside family
/// 0 (more than two channels) are written as given and refused later by
/// [`Muxer::add_audio_track`](crate::Muxer::add_audio_track).
pub fn opus_head(channels: u8, pre_skip: u16, input_sample_rate: u32, output_gain: i16) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1);
    h.push(channels);
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&input_sample_rate.to_le_bytes());
    h.extend_from_slice(&output_gain.to_le_bytes());
    h.push(0);
    h
}

struct OpusHeadInfo {
    channels: u8,
    pre_skip: u16,
    input_rate: u32,
}

fn parse_opus_head(h: &[u8]) -> Result<OpusHeadInfo, Error> {
    const BAD: Error = Error::InvalidConfig("not a valid OpusHead");
    if h.len() < 19 || &h[..8] != b"OpusHead" || h[8] >> 4 != 0 {
        return Err(BAD);
    }
    let channels = h[9];
    let family = h[18];
    if channels == 0 {
        return Err(BAD);
    }
    match family {
        0 if channels > 2 => return Err(BAD),
        0 => {}
        _ => {
            // Family 1 and up carry stream count, coupled count and one
            // mapping byte per channel.
            if h.len() < 21 + usize::from(channels) {
                return Err(BAD);
            }
        }
    }
    Ok(OpusHeadInfo {
        channels,
        pre_skip: u16::from_le_bytes([h[10], h[11]]),
        input_rate: u32::from_le_bytes([h[12], h[13], h[14], h[15]]),
    })
}

fn parse_vorbis_id(h: &[u8]) -> Result<(u8, u32), Error> {
    const BAD: Error = Error::InvalidConfig("not a valid Vorbis identification header");
    if h.len() < 30 || h[0] != 1 || &h[1..7] != b"vorbis" {
        return Err(BAD);
    }
    let channels = h[11];
    let rate = u32::from_le_bytes([h[12], h[13], h[14], h[15]]);
    if channels == 0 || rate == 0 {
        return Err(BAD);
    }
    Ok((channels, rate))
}

/// Packs packets with Xiph lacing, as Vorbis's `CodecPrivate` needs: the
/// packet count minus one, the lengths of all but the last packet (each
/// as a run of 255s and a final byte below 255), then the packets.
pub fn xiph_lace(packets: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(packets.len().saturating_sub(1) as u8);
    if let Some((_, init)) = packets.split_last() {
        for p in init {
            let mut n = p.len();
            while n >= 255 {
                out.push(255);
                n -= 255;
            }
            out.push(n as u8);
        }
    }
    for p in packets {
        out.extend_from_slice(p);
    }
    out
}

/// Builds VP9's optional `CodecPrivate` (the WebM "VP9 Codec Feature
/// Metadata"): each feature given is written as an ID byte, a length byte
/// and the value. IDs: 1 profile (0 to 3), 2 level (10 for 1, 11 for 1.1,
/// and so on), 3 bit depth (8, 10 or 12), 4 chroma subsampling (0: 4:2:0
/// vertical, 1: 4:2:0 colocated, 2: 4:2:2, 3: 4:4:4).
pub fn vp9_codec_private(
    profile: Option<u8>,
    level: Option<u8>,
    bit_depth: Option<u8>,
    chroma_subsampling: Option<u8>,
) -> Vec<u8> {
    let mut out = Vec::new();
    for (id, value) in [(1u8, profile), (2, level), (3, bit_depth), (4, chroma_subsampling)] {
        if let Some(v) = value {
            out.extend_from_slice(&[id, 1, v]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xiph_lacing_writes_runs_of_255() {
        let a = vec![0u8; 300];
        let b = vec![1u8; 2];
        let c = vec![2u8; 4];
        let out = xiph_lace(&[&a, &b, &c]);
        assert_eq!(&out[..4], &[2, 255, 45, 2]);
        assert_eq!(out.len(), 4 + 306);
    }

    #[test]
    fn opus_head_round_trips() {
        let h = opus_head(2, 312, 48_000, 0);
        assert_eq!(h.len(), 19);
        let info = parse_opus_head(&h).unwrap();
        assert_eq!((info.channels, info.pre_skip, info.input_rate), (2, 312, 48_000));
        assert!(parse_opus_head(&opus_head(3, 0, 0, 0)).is_err());
    }

    #[test]
    fn default_uids_are_non_zero_and_distinct() {
        let uids: Vec<u64> = (1..=126).map(default_uid).collect();
        assert!(uids.iter().all(|&u| u != 0));
        let mut sorted = uids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), uids.len());
    }
}
