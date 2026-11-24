//! A small WebM reader for the tests, written from RFC 8794 and RFC 9559.
//! It checks the structure the muxer promises and returns every frame.
//! With `allow_truncated`, a stream cut off anywhere yields the frames
//! whose blocks are wholly present.
#![allow(dead_code)]

pub const EBML: u32 = 0x1A45_DFA3;
pub const SEGMENT: u32 = 0x1853_8067;
pub const SEEK_HEAD: u32 = 0x114D_9B74;
pub const INFO: u32 = 0x1549_A966;
pub const TRACKS: u32 = 0x1654_AE6B;
pub const CLUSTER: u32 = 0x1F43_B675;
pub const CUES: u32 = 0x1C53_BB6B;
pub const VOID: u32 = 0xEC;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub number: u64,
    pub uid: u64,
    pub kind: u64,
    pub codec_id: String,
    pub codec_private: Option<Vec<u8>>,
    pub codec_delay: Option<u64>,
    pub seek_pre_roll: Option<u64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub rate: Option<f64>,
    pub channels: Option<u64>,
    pub lacing: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub track: u64,
    /// Absolute timestamp, in ticks.
    pub ticks: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
    pub duration: Option<u64>,
    pub discard_padding: Option<i64>,
    pub reference: Option<i64>,
    pub group: bool,
    /// Index into `Demuxed::clusters`.
    pub cluster: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cluster {
    /// Offset of the Cluster element, relative to the Segment's data.
    pub offset: u64,
    pub timestamp: u64,
    pub known_size: bool,
    /// Its end was seen (a known size fully present, or the next element).
    pub complete: bool,
    /// Offset of the first block, relative to the cluster's data.
    pub first_block: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    pub time: u64,
    pub track: u64,
    pub cluster: u64,
    pub relative: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Demuxed {
    pub doc_type: String,
    pub doc_type_version: u64,
    pub segment_known_size: bool,
    pub segment_data_at: u64,
    pub timestamp_scale: u64,
    pub duration: Option<f64>,
    pub title: Option<String>,
    pub muxing_app: Option<String>,
    pub writing_app: Option<String>,
    pub seek_head: Vec<(u32, u64)>,
    /// Top-level elements in order: (id, offset relative to Segment data).
    pub top_level: Vec<(u32, u64)>,
    pub tracks: Vec<Track>,
    pub clusters: Vec<Cluster>,
    pub frames: Vec<Frame>,
    pub cues: Vec<Cue>,
    pub truncated: bool,
}

struct R<'a> {
    b: &'a [u8],
    trunc_ok: bool,
}

type Res<T> = Result<T, String>;

fn vint(b: &[u8], at: usize) -> Res<Option<(u64, usize, bool)>> {
    let Some(&first) = b.get(at) else { return Ok(None) };
    if first == 0 {
        return Err(format!("invalid VINT at {at}"));
    }
    let len = first.leading_zeros() as usize + 1;
    if at + len > b.len() {
        return Ok(None);
    }
    let mut v = u64::from(first) & (0xFF >> len);
    for i in 1..len {
        v = (v << 8) | u64::from(b[at + i]);
    }
    let unknown = v == (1u64 << (7 * len)) - 1;
    Ok(Some((v, len, unknown)))
}

fn id(b: &[u8], at: usize) -> Res<Option<(u32, usize)>> {
    let Some(&first) = b.get(at) else { return Ok(None) };
    if first == 0 {
        return Err(format!("invalid ID at {at}"));
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return Err(format!("ID longer than 4 at {at}"));
    }
    if at + len > b.len() {
        return Ok(None);
    }
    let mut v = 0u32;
    for i in 0..len {
        v = (v << 8) | u32::from(b[at + i]);
    }
    Ok(Some((v, len)))
}

/// One element header: (id, data start, data end or None for unknown).
struct Head {
    id: u32,
    start: usize,
    data: usize,
    end: Option<usize>,
}

impl<'a> R<'a> {
    /// Reads a header at `at`; `Ok(None)` when the bytes run out.
    fn head(&self, at: usize, limit: usize) -> Res<Option<Head>> {
        let Some((idv, il)) = id(&self.b[..limit], at)? else {
            return Ok(None);
        };
        let Some((size, sl, unknown)) = vint(&self.b[..limit], at + il)? else {
            return Ok(None);
        };
        let data = at + il + sl;
        let end = if unknown { None } else { Some(data + size as usize) };
        Ok(Some(Head {
            id: idv,
            start: at,
            data,
            end,
        }))
    }

    /// Children of a known-size master: (id, data).
    fn children(&self, start: usize, end: usize) -> Res<Vec<(u32, &'a [u8])>> {
        let mut out = Vec::new();
        let mut at = start;
        while at < end {
            let h = self.head(at, end)?.ok_or_else(|| format!("child header cut at {at}"))?;
            let e = h.end.ok_or_else(|| format!("unknown size inside a master at {at}"))?;
            if e > end {
                return Err(format!("child at {at} ({:#x}) runs past its parent", h.id));
            }
            out.push((h.id, &self.b[h.data..e]));
            at = e;
        }
        Ok(out)
    }
}

pub fn uint(d: &[u8]) -> u64 {
    assert!(d.len() <= 8, "uint longer than 8");
    d.iter().fold(0u64, |a, &x| (a << 8) | u64::from(x))
}

pub fn int(d: &[u8]) -> i64 {
    assert!(!d.is_empty() && d.len() <= 8, "int length {}", d.len());
    let mut v: i64 = if d[0] & 0x80 != 0 { -1 } else { 0 };
    for &x in d {
        v = (v << 8) | i64::from(x);
    }
    v
}

pub fn float(d: &[u8]) -> f64 {
    match d.len() {
        4 => f64::from(f32::from_be_bytes([d[0], d[1], d[2], d[3]])),
        8 => f64::from_be_bytes(d.try_into().unwrap()),
        n => panic!("float of length {n}"),
    }
}

fn string(d: &[u8]) -> String {
    String::from_utf8(d.to_vec()).expect("UTF-8 string")
}

pub fn parse(b: &[u8]) -> Res<Demuxed> {
    parse_with(b, false)
}

pub fn parse_truncated(b: &[u8]) -> Res<Demuxed> {
    parse_with(b, true)
}

fn parse_with(b: &[u8], trunc_ok: bool) -> Res<Demuxed> {
    let r = R { b, trunc_ok };
    let mut out = Demuxed {
        timestamp_scale: 1_000_000,
        ..Default::default()
    };
    let cut = |what: &str| -> Res<()> {
        if trunc_ok {
            Ok(())
        } else {
            Err(format!("file ends inside {what}"))
        }
    };

    // EBML header.
    let Some(h) = r.head(0, b.len())? else {
        cut("the EBML header")?;
        out.truncated = true;
        return Ok(out);
    };
    if h.id != EBML {
        return Err("no EBML header".into());
    }
    let end = h.end.ok_or("EBML header of unknown size")?;
    if end > b.len() {
        cut("the EBML header")?;
        out.truncated = true;
        return Ok(out);
    }
    for (cid, d) in r.children(h.data, end)? {
        match cid {
            0x4282 => out.doc_type = string(d),
            0x4287 => out.doc_type_version = uint(d),
            _ => {}
        }
    }
    if out.doc_type != "webm" {
        return Err(format!("DocType {:?}", out.doc_type));
    }

    // Segment.
    let Some(seg) = r.head(end, b.len())? else {
        cut("the Segment header")?;
        out.truncated = true;
        return Ok(out);
    };
    if seg.id != SEGMENT {
        return Err(format!("expected Segment, found {:#x}", seg.id));
    }
    out.segment_known_size = seg.end.is_some();
    out.segment_data_at = seg.data as u64;
    let seg_end = match seg.end {
        Some(e) if e > b.len() => {
            cut("the Segment")?;
            out.truncated = true;
            b.len()
        }
        Some(e) => e,
        None => b.len(),
    };
    if let Some(e) = seg.end {
        if e < b.len() {
            return Err(format!("{} bytes after the Segment", b.len() - e));
        }
    }

    let rel = |at: usize| (at - seg.data) as u64;
    let mut at = seg.data;
    while at < seg_end {
        let Some(h) = r.head(at, seg_end)? else {
            cut("a top-level header")?;
            out.truncated = true;
            break;
        };
        out.top_level.push((h.id, rel(at)));
        if h.id == CLUSTER {
            let (next, complete) = parse_cluster(&r, &h, seg_end, rel(at), &mut out)?;
            if !complete {
                cut("a Cluster")?;
                out.truncated = true;
                break;
            }
            at = next;
            continue;
        }
        let Some(e) = h.end else {
            return Err(format!("top-level {:#x} of unknown size", h.id));
        };
        if e > seg_end {
            cut("a top-level element")?;
            out.truncated = true;
            break;
        }
        let kids = || r.children(h.data, e);
        match h.id {
            SEEK_HEAD => {
                for (cid, d) in kids()? {
                    if cid == 0x4DBB {
                        let mut sid = None;
                        let mut pos = None;
                        for (gid, gd) in r.children_of(d)? {
                            match gid {
                                0x53AB => sid = Some(uint(gd) as u32),
                                0x53AC => pos = Some(uint(gd)),
                                _ => {}
                            }
                        }
                        out.seek_head.push((sid.ok_or("SeekID")?, pos.ok_or("SeekPosition")?));
                    }
                }
            }
            INFO => {
                for (cid, d) in kids()? {
                    match cid {
                        0x2AD7B1 => out.timestamp_scale = uint(d),
                        0x4489 => out.duration = Some(float(d)),
                        0x7BA9 => out.title = Some(string(d)),
                        0x4D80 => out.muxing_app = Some(string(d)),
                        0x5741 => out.writing_app = Some(string(d)),
                        _ => {}
                    }
                }
            }
            TRACKS => {
                for (cid, d) in kids()? {
                    if cid == 0xAE {
                        out.tracks.push(parse_track(&r, d)?);
                    }
                }
            }
            CUES => {
                for (cid, d) in kids()? {
                    if cid != 0xBB {
                        continue;
                    }
                    let mut time = None;
                    let mut positions = Vec::new();
                    for (pid, pd) in r.children_of(d)? {
                        match pid {
                            0xB3 => time = Some(uint(pd)),
                            0xB7 => positions.push(pd),
                            _ => {}
                        }
                    }
                    for pd in positions {
                        let mut cue = Cue {
                            time: time.ok_or("CueTime")?,
                            track: 0,
                            cluster: 0,
                            relative: None,
                        };
                        for (qid, qd) in r.children_of(pd)? {
                            match qid {
                                0xF7 => cue.track = uint(qd),
                                0xF1 => cue.cluster = uint(qd),
                                0xF0 => cue.relative = Some(uint(qd)),
                                _ => {}
                            }
                        }
                        out.cues.push(cue);
                    }
                }
            }
            VOID => {}
            other => return Err(format!("unexpected top-level element {other:#x}")),
        }
        at = e;
    }
    Ok(out)
}

impl<'a> R<'a> {
    fn children_of(&self, d: &'a [u8]) -> Res<Vec<(u32, &'a [u8])>> {
        let sub = R { b: d, trunc_ok: false };
        sub.children(0, d.len())
    }
}

fn parse_track(r: &R<'_>, d: &[u8]) -> Res<Track> {
    let mut t = Track::default();
    for (cid, v) in r.children_of(d)? {
        match cid {
            0xD7 => t.number = uint(v),
            0x73C5 => t.uid = uint(v),
            0x83 => t.kind = uint(v),
            0x86 => t.codec_id = string(v),
            0x63A2 => t.codec_private = Some(v.to_vec()),
            0x56AA => t.codec_delay = Some(uint(v)),
            0x56BB => t.seek_pre_roll = Some(uint(v)),
            0x9C => t.lacing = Some(uint(v)),
            0xE0 => {
                for (vid, vd) in r.children_of(v)? {
                    match vid {
                        0xB0 => t.width = Some(uint(vd)),
                        0xBA => t.height = Some(uint(vd)),
                        _ => {}
                    }
                }
            }
            0xE1 => {
                for (aid, ad) in r.children_of(v)? {
                    match aid {
                        0xB5 => t.rate = Some(float(ad)),
                        0x9F => t.channels = Some(uint(ad)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if t.number == 0 || t.uid == 0 {
        return Err("TrackNumber and TrackUID must be non-zero".into());
    }
    Ok(t)
}

/// Parses one cluster; returns (offset after it, complete).
fn parse_cluster(r: &R<'_>, h: &Head, seg_end: usize, offset: u64, out: &mut Demuxed) -> Res<(usize, bool)> {
    let index = out.clusters.len();
    out.clusters.push(Cluster {
        offset,
        timestamp: 0,
        known_size: h.end.is_some(),
        complete: false,
        first_block: None,
    });
    let limit = match h.end {
        Some(e) if e > seg_end => seg_end,
        Some(e) => e,
        None => seg_end,
    };
    let mut timestamp = None;
    let mut at = h.data;
    let mut frames = Vec::new();
    let mut complete = false;
    loop {
        if Some(at) == h.end {
            complete = true;
            break;
        }
        if at >= limit {
            // An unknown-size cluster ending at the end of the data: whole
            // when the Segment's size says so, or when the caller says the
            // stream wasn't cut.
            complete = h.end.is_none() && (out.segment_known_size || !r.trunc_ok);
            break;
        }
        let Some(c) = r.head(at, limit)? else { break };
        let is_child = matches!(c.id, 0xE7 | 0xA3 | 0xA0 | 0xEC | 0xAB | 0xA7);
        if !is_child {
            if h.end.is_some() {
                return Err(format!("unexpected {:#x} inside a sized cluster", c.id));
            }
            complete = true;
            break;
        }
        let Some(e) = c.end else {
            return Err("unknown-size child in a cluster".into());
        };
        if e > limit {
            break;
        }
        let d = &r.b[c.data..e];
        match c.id {
            0xE7 => timestamp = Some(uint(d)),
            0xA3 | 0xA0 => {
                let ts = timestamp.ok_or("block before the cluster Timestamp")?;
                if out.clusters[index].first_block.is_none() {
                    out.clusters[index].first_block = Some((at - h.data) as u64);
                }
                let f = if c.id == 0xA3 {
                    parse_block(d, true, ts)?
                } else {
                    parse_group(r, d, ts)?
                };
                frames.push(f);
            }
            _ => {}
        }
        at = e;
    }
    out.clusters[index].timestamp = timestamp.unwrap_or(0);
    out.clusters[index].complete = complete;
    for mut f in frames {
        f.cluster = index;
        out.frames.push(f);
    }
    Ok((at, complete))
}

fn parse_block(d: &[u8], simple: bool, cluster_ts: u64) -> Res<Frame> {
    let (track, tl, _) = vint(d, 0)?.ok_or("block track number cut")?;
    if d.len() < tl + 3 {
        return Err("block header cut".into());
    }
    let rel = i16::from_be_bytes([d[tl], d[tl + 1]]);
    let flags = d[tl + 2];
    if flags & 0x06 != 0 {
        return Err("lacing is not expected".into());
    }
    let ticks = cluster_ts as i64 + i64::from(rel);
    if ticks < 0 {
        return Err("negative absolute timestamp".into());
    }
    Ok(Frame {
        track,
        ticks: ticks as u64,
        keyframe: simple && flags & 0x80 != 0,
        data: d[tl + 3..].to_vec(),
        duration: None,
        discard_padding: None,
        reference: None,
        group: !simple,
        cluster: 0,
    })
}

fn parse_group(r: &R<'_>, d: &[u8], cluster_ts: u64) -> Res<Frame> {
    let mut frame = None;
    let mut duration = None;
    let mut discard = None;
    let mut reference = None;
    for (cid, cd) in r.children_of(d)? {
        match cid {
            0xA1 => frame = Some(parse_block(cd, false, cluster_ts)?),
            0x9B => duration = Some(uint(cd)),
            0x75A2 => discard = Some(int(cd)),
            0xFB => reference = Some(int(cd)),
            _ => {}
        }
    }
    let mut f = frame.ok_or("BlockGroup without a Block")?;
    f.keyframe = reference.is_none();
    f.duration = duration;
    f.discard_padding = discard;
    f.reference = reference;
    Ok(f)
}
