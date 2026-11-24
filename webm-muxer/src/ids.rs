//! Element IDs, as RFC 8794 (EBML) and RFC 9559 (Matroska) assign them.
//! Each constant holds the ID's bytes as they appear in the file, marker
//! bits included.

// EBML header (RFC 8794, section 11.2).
pub(crate) const EBML: u32 = 0x1A45_DFA3;
pub(crate) const EBML_VERSION: u32 = 0x4286;
pub(crate) const EBML_READ_VERSION: u32 = 0x42F7;
pub(crate) const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
pub(crate) const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
pub(crate) const DOC_TYPE: u32 = 0x4282;
pub(crate) const DOC_TYPE_VERSION: u32 = 0x4287;
pub(crate) const DOC_TYPE_READ_VERSION: u32 = 0x4285;
pub(crate) const VOID: u32 = 0xEC;

// Segment and its top-level children (RFC 9559, section 5.1).
pub(crate) const SEGMENT: u32 = 0x1853_8067;
pub(crate) const SEEK_HEAD: u32 = 0x114D_9B74;
pub(crate) const SEEK: u32 = 0x4DBB;
pub(crate) const SEEK_ID: u32 = 0x53AB;
pub(crate) const SEEK_POSITION: u32 = 0x53AC;

pub(crate) const INFO: u32 = 0x1549_A966;
pub(crate) const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
pub(crate) const DURATION: u32 = 0x4489;
pub(crate) const TITLE: u32 = 0x7BA9;
pub(crate) const MUXING_APP: u32 = 0x4D80;
pub(crate) const WRITING_APP: u32 = 0x5741;

pub(crate) const TRACKS: u32 = 0x1654_AE6B;
pub(crate) const TRACK_ENTRY: u32 = 0xAE;
pub(crate) const TRACK_NUMBER: u32 = 0xD7;
pub(crate) const TRACK_UID: u32 = 0x73C5;
pub(crate) const TRACK_TYPE: u32 = 0x83;
pub(crate) const FLAG_LACING: u32 = 0x9C;
pub(crate) const NAME: u32 = 0x536E;
pub(crate) const LANGUAGE: u32 = 0x22_B59C;
pub(crate) const CODEC_ID: u32 = 0x86;
pub(crate) const CODEC_PRIVATE: u32 = 0x63A2;
pub(crate) const CODEC_DELAY: u32 = 0x56AA;
pub(crate) const SEEK_PRE_ROLL: u32 = 0x56BB;
pub(crate) const VIDEO: u32 = 0xE0;
pub(crate) const PIXEL_WIDTH: u32 = 0xB0;
pub(crate) const PIXEL_HEIGHT: u32 = 0xBA;
pub(crate) const DISPLAY_WIDTH: u32 = 0x54B0;
pub(crate) const DISPLAY_HEIGHT: u32 = 0x54BA;
pub(crate) const AUDIO: u32 = 0xE1;
pub(crate) const SAMPLING_FREQUENCY: u32 = 0xB5;
pub(crate) const CHANNELS: u32 = 0x9F;
pub(crate) const BIT_DEPTH: u32 = 0x6264;

pub(crate) const CLUSTER: u32 = 0x1F43_B675;
pub(crate) const TIMESTAMP: u32 = 0xE7;
pub(crate) const SIMPLE_BLOCK: u32 = 0xA3;
pub(crate) const BLOCK_GROUP: u32 = 0xA0;
pub(crate) const BLOCK: u32 = 0xA1;
pub(crate) const BLOCK_DURATION: u32 = 0x9B;
pub(crate) const REFERENCE_BLOCK: u32 = 0xFB;
pub(crate) const DISCARD_PADDING: u32 = 0x75A2;

pub(crate) const CUES: u32 = 0x1C53_BB6B;
pub(crate) const CUE_POINT: u32 = 0xBB;
pub(crate) const CUE_TIME: u32 = 0xB3;
pub(crate) const CUE_TRACK_POSITIONS: u32 = 0xB7;
pub(crate) const CUE_TRACK: u32 = 0xF7;
pub(crate) const CUE_CLUSTER_POSITION: u32 = 0xF1;
pub(crate) const CUE_RELATIVE_POSITION: u32 = 0xF0;
