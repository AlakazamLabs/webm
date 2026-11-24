//! webm-conformance: muxes a matrix of test media with webm-muxer and
//! checks every output with outside tools run as separate processes:
//! mkvalidator, ffprobe and ffmpeg. Test-only; not published.
//!
//! Usage: webm-conformance <work dir>
//! Tools come from the environment: FFMPEG, FFPROBE, MKVALIDATOR
//! (default: on PATH).

#[path = "../../webm-muxer/tests/support/demux.rs"]
mod demux;
mod inputs;

use std::fmt::Write as _;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use inputs::Packet;
use vp8_encoder::{Encoder, EncoderConfig, Picture};
use webm_muxer::{vp9_codec_private, AudioTrack, FrameOptions, Muxer, Options, VideoCodec, VideoTrack};

pub struct Tools {
    pub ffmpeg: String,
    pub ffprobe: String,
    pub mkvalidator: String,
}

pub fn run_ok(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what}: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

pub fn capture(cmd: &mut Command, what: &str) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what}: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Output of a command and its exit code (-1 if none).
fn run_code(cmd: &mut Command) -> (bool, String, String, i32) {
    match cmd.output() {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
            o.status.code().unwrap_or(-1),
        ),
        Err(e) => (false, String::new(), e.to_string(), -1),
    }
}

/// Output of a command: (success, stdout, stderr).
fn run(cmd: &mut Command) -> (bool, String, String) {
    match cmd.output() {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => (false, String::new(), e.to_string()),
    }
}

enum Spec {
    Video(VideoTrack),
    Audio(AudioTrack),
}

/// One stream to mux: its track, its packets, and per-packet options.
struct Stream {
    delay_ns: u64,
    codec: &'static str,
    spec: Spec,
    packets: Vec<Packet>,
    extra: Vec<(usize, FrameOptions)>,
}

struct Case {
    name: String,
    live: bool,
    scale: u64,
    options: Options,
    streams: Vec<Stream>,
}

struct Written {
    name: String,
    live: bool,
    path: PathBuf,
    bytes: Vec<u8>,
    scale: u64,
    /// (track number, ticks, keyframe), in file order.
    frames: Vec<(u64, u64, bool)>,
    codecs: Vec<&'static str>,
    /// Per track: the CodecDelay ffprobe subtracts from its timestamps.
    delays_ns: Vec<u64>,
    has_video: bool,
}

fn mux(case: Case, out_dir: &Path) -> Result<Written, String> {
    let scale = case.scale;
    let mut order: Vec<(u64, u8, usize, usize)> = Vec::new(); // (ticks, video first, stream, packet)
    for (si, s) in case.streams.iter().enumerate() {
        let video = matches!(s.spec, Spec::Video(_));
        for (pi, p) in s.packets.iter().enumerate() {
            order.push((p.ns / scale, if video { 0 } else { 1 }, si, pi));
        }
    }
    order.sort_by_key(|&(t, v, s, p)| (t, v, s, p));
    let order: Vec<(usize, usize)> = order.into_iter().map(|(_, _, s, p)| (s, p)).collect();

    fn go<W: std::io::Write>(
        m: &mut Muxer<W>,
        case: &Case,
        order: &[(usize, usize)],
    ) -> Result<Vec<(u64, u64, bool)>, String> {
        let mut ids = Vec::new();
        for s in &case.streams {
            let id = match &s.spec {
                Spec::Video(v) => m.add_video_track(v.clone()),
                Spec::Audio(a) => m.add_audio_track(a.clone()),
            }
            .map_err(|e| e.to_string())?;
            ids.push(id);
        }
        let mut frames = Vec::new();
        for &(si, pi) in order {
            let s = &case.streams[si];
            let p = &s.packets[pi];
            let video = matches!(s.spec, Spec::Video(_));
            let fo = s
                .extra
                .iter()
                .find(|(i, _)| *i == pi)
                .map(|(_, o)| *o)
                .unwrap_or(FrameOptions::new(p.keyframe));
            m.write_frame_with(ids[si], &p.data, p.ns, fo)
                .map_err(|e| format!("{}: {e}", case.name))?;
            frames.push((ids[si].number(), 0, fo.keyframe || !video));
        }
        Ok(frames)
    }

    let opts = case.options.clone();
    let (bytes, mut frames) = if case.live {
        let mut m = Muxer::new(Vec::new(), opts).map_err(|e| e.to_string())?;
        let f = go(&mut m, &case, &order)?;
        (m.finish().map_err(|e| e.to_string())?, f)
    } else {
        let mut m = Muxer::new_seekable(Cursor::new(Vec::new()), opts).map_err(|e| e.to_string())?;
        let f = go(&mut m, &case, &order)?;
        (m.finish().map_err(|e| e.to_string())?.into_inner(), f)
    };
    for (f, &(si, pi)) in frames.iter_mut().zip(&order) {
        f.1 = case.streams[si].packets[pi].ns / scale;
    }
    let codecs = case.streams.iter().map(|s| s.codec).collect();
    let has_video = case.streams.iter().any(|s| matches!(s.spec, Spec::Video(_)));
    let delays_ns = case.streams.iter().map(|s| s.delay_ns).collect();
    let path = out_dir.join(format!("{}.webm", case.name));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(Written {
        name: case.name,
        live: case.live,
        path,
        bytes,
        scale,
        frames,
        codecs,
        has_video,
        delays_ns,
    })
}

fn vp8_stream(count: usize) -> Vec<Packet> {
    let (w, h) = (320usize, 240usize);
    let cfg = EncoderConfig::new(320, 240, 30, 600_000).with_keyframe_interval(Some(30));
    let mut enc = Encoder::new(cfg).expect("encoder");
    let mut out = Vec::new();
    let mut i = 0usize;
    while out.len() < count {
        let mut buf = vec![0u8; w * h * 3 / 2];
        for y in 0..h {
            for x in 0..w {
                let bx = (x + i * 3) / 40;
                let by = y / 40;
                buf[y * w + x] = if (bx + by) % 2 == 0 { 200 } else { 40 };
            }
        }
        for c in 0..w * h / 2 {
            buf[w * h + c] = (100 + (i % 60) as u8) as u8;
        }
        let pic = Picture::from_i420(&buf, 320, 240).expect("picture");
        let n = out.len();
        if let Some(f) = enc.encode(&pic, n % 30 == 0).expect("encode") {
            out.push(Packet {
                data: f.data,
                ns: n as u64 * 1_000_000_000 / 30,
                keyframe: f.keyframe,
            });
        }
        i += 1;
    }
    out
}

fn video(codec: VideoCodec, packets: Vec<Packet>, cp: Option<Vec<u8>>) -> Stream {
    let mut t = VideoTrack::new(codec, 320, 240);
    if let Some(cp) = cp {
        t = t.with_codec_private(cp);
    }
    let name = match codec {
        VideoCodec::Vp8 => "vp8",
        VideoCodec::Vp9 => "vp9",
        _ => "av1",
    };
    Stream {
        delay_ns: 0,
        codec: name,
        spec: Spec::Video(t),
        packets,
        extra: Vec::new(),
    }
}

fn cases(tools: &Tools, inp: &inputs::Inputs) -> Result<Vec<Case>, String> {
    let vp8 = vp8_stream(180);
    let (vp9, _) = inp.ivf(tools, "vp9.ivf")?;
    let (mut av1, _) = inp.ivf(tools, "av1.ivf")?;
    let seq = inputs::av1_prepare(&mut av1)?;
    let level: u8 = capture(
        Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=level",
                "-of",
                "csv=p=0",
            ])
            .arg(inp.dir.join("av1.ivf")),
        "ffprobe AV1 level",
    )?
    .trim()
    .parse()
    .unwrap_or(8);
    let av1c = inputs::av1c(level, &seq);
    let (opus_head, opus, opus_trim) = inputs::opus(&inp.dir.join("opus.ogg"))?;
    let (vorbis_headers, vorbis, _rate) = inputs::vorbis(&inp.dir.join("vorbis.ogg"))?;
    let vp9_cp = vp9_codec_private(Some(0), None, Some(8), Some(0));

    let opus_pre_skip = u64::from(u16::from_le_bytes([opus_head[10], opus_head[11]]));
    let opus_s = || Stream {
        delay_ns: opus_pre_skip * 1_000_000_000 / 48_000,
        codec: "opus",
        spec: Spec::Audio(AudioTrack::opus_with_head(opus_head.clone())),
        packets: opus.clone(),
        extra: Vec::new(),
    };
    let vorbis_s = || Stream {
        delay_ns: 0,
        codec: "vorbis",
        spec: Spec::Audio(AudioTrack::vorbis(
            vorbis_headers[0].clone(),
            vorbis_headers[1].clone(),
            vorbis_headers[2].clone(),
        )),
        packets: vorbis.clone(),
        extra: Vec::new(),
    };
    let mut out = Vec::new();
    for live in [false, true] {
        let m = if live { "live" } else { "file" };
        let c = |name: &str, streams: Vec<Stream>| Case {
            name: format!("{name}.{m}"),
            live,
            scale: 1_000_000,
            options: Options::new(),
            streams,
        };
        out.push(c("vp8_opus", vec![video(VideoCodec::Vp8, vp8.clone(), None), opus_s()]));
        out.push(c(
            "vp9_opus",
            vec![video(VideoCodec::Vp9, vp9.clone(), Some(vp9_cp.clone())), opus_s()],
        ));
        out.push(c(
            "av1_opus",
            vec![video(VideoCodec::Av1, av1.clone(), Some(av1c.clone())), opus_s()],
        ));
        out.push(c(
            "vp8_vorbis",
            vec![video(VideoCodec::Vp8, vp8.clone(), None), vorbis_s()],
        ));
        out.push(c("opus_only", vec![opus_s()]));
        out.push(c("vorbis_only", vec![vorbis_s()]));
    }
    // Several tracks of every codec in one file.
    out.push(Case {
        name: "multi.file".into(),
        live: false,
        scale: 1_000_000,
        options: Options::new().with_title("Test pattern"),
        streams: vec![
            video(VideoCodec::Vp8, vp8.clone(), None),
            video(VideoCodec::Vp9, vp9.clone(), Some(vp9_cp.clone())),
            video(VideoCodec::Av1, av1.clone(), Some(av1c.clone())),
            opus_s(),
            vorbis_s(),
        ],
    });
    // Cues before the clusters.
    out.push(Case {
        name: "vp8_opus_cuesfront.file".into(),
        live: false,
        scale: 1_000_000,
        options: Options::new().with_cues_space(1024),
        streams: vec![video(VideoCodec::Vp8, vp8.clone(), None), opus_s()],
    });
    // A 100 us timestamp scale (clusters of at most 3.2 s).
    for live in [false, true] {
        out.push(Case {
            name: format!("vp9_opus_100us.{}", if live { "live" } else { "file" }),
            live,
            scale: 100_000,
            options: Options::new().with_timestamp_scale(100_000),
            streams: vec![video(VideoCodec::Vp9, vp9.clone(), Some(vp9_cp.clone())), opus_s()],
        });
    }
    // Opus end trim as DiscardPadding, and video durations, in BlockGroups.
    let mut opus_trimmed = opus_s();
    let last = opus_trimmed.packets.len() - 1;
    let pad_ns = (opus_trim.max(1) * 1_000_000_000 / 48_000) as i64;
    opus_trimmed
        .extra
        .push((last, FrameOptions::new(true).with_discard_padding(pad_ns)));
    let mut vp8_dur = video(VideoCodec::Vp8, vp8.clone(), None);
    for (i, p) in vp8.iter().enumerate().skip(170) {
        vp8_dur
            .extra
            .push((i, FrameOptions::new(p.keyframe).with_duration(33_333_333)));
    }
    out.push(Case {
        name: "vp8_opus_groups.file".into(),
        live: false,
        scale: 1_000_000,
        options: Options::new(),
        streams: vec![vp8_dur, opus_trimmed],
    });
    // Small clusters: a 1 s duration cap and a 32 KiB size cap.
    out.push(Case {
        name: "vp8_opus_smallclusters.live".into(),
        live: true,
        scale: 1_000_000,
        options: Options::new()
            .with_max_cluster_duration(1_000_000_000)
            .with_max_cluster_bytes(32 * 1024),
        streams: vec![video(VideoCodec::Vp8, vp8.clone(), None), opus_s()],
    });
    Ok(out)
}

#[derive(Default)]
struct Row {
    name: String,
    checks: Vec<(String, bool, String)>,
}

impl Row {
    fn check(&mut self, what: &str, ok: bool, note: impl Into<String>) {
        self.checks.push((what.to_string(), ok, note.into()));
    }
    fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.1)
    }
}

fn verify(tools: &Tools, w: &Written, work: &Path, mkv_live_flag: bool) -> Row {
    let mut row = Row {
        name: w.name.clone(),
        ..Default::default()
    };

    // 1. Our test demuxer: identical frames and timestamps.
    let parsed = demux::parse(&w.bytes);
    match &parsed {
        Ok(d) => {
            let got: Vec<(u64, u64, bool)> = d.frames.iter().map(|f| (f.track, f.ticks, f.keyframe)).collect();
            row.check("round trip", got == w.frames, format!("{} frames", got.len()));
            if !w.live {
                let cues_ok = d.cues.iter().all(|c| {
                    d.clusters
                        .iter()
                        .any(|k| k.offset == c.cluster && k.timestamp == c.time && k.first_block == c.relative)
                });
                row.check(
                    "cues point at clusters",
                    cues_ok && !d.cues.is_empty(),
                    format!("{} cues", d.cues.len()),
                );
            }
        }
        Err(e) => row.check("round trip", false, e.clone()),
    }
    let cue_times: Vec<u64> = parsed
        .as_ref()
        .map(|d| d.cues.iter().map(|c| c.time).collect())
        .unwrap_or_default();

    // 2. mkvalidator.
    let mut cmd = Command::new(&tools.mkvalidator);
    if w.live && mkv_live_flag {
        cmd.arg("--live");
    }
    let (ok, out, err, code) = run_code(cmd.arg(&w.path));
    let text = format!("{out}{err}");
    let _ = std::fs::write(work.join(format!("{}.mkvalidator.txt", w.name)), format!("exit {code}\n{text}"));
    let errs: Vec<&str> = text.lines().map(str::trim).filter(|l| l.starts_with("ERR")).collect();
    let wrns: Vec<&str> = text.lines().map(str::trim).filter(|l| l.starts_with("WRN")).collect();
    let mut note = format!("exit {code}, {} ERR, {} WRN", errs.len(), wrns.len());
    for l in errs.iter().chain(&wrns).take(6) {
        let _ = write!(note, " | {l}");
    }
    let valid = text.contains("the file appears to be valid");
    row.check("mkvalidator", ok && valid && errs.is_empty() && wrns.is_empty(), note);

    // 3. ffprobe: streams and every packet's stream, time and key flag.
    let (ok, out, err) = run(Command::new(&tools.ffprobe)
        .args(["-v", "warning", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(&w.path));
    let names: Vec<&str> = out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    row.check(
        "ffprobe streams",
        ok && err.trim().is_empty() && names == w.codecs,
        format!("{names:?} {}", err.trim()),
    );
    let (ok, out, err) = run(Command::new(&tools.ffprobe)
        .args([
            "-v",
            "warning",
            "-show_entries",
            "packet=stream_index,pts_time,flags",
            "-of",
            "csv=p=0",
        ])
        .arg(&w.path));
    let mut mismatch = None;
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    for (i, (l, want)) in lines.iter().zip(&w.frames).enumerate() {
        let mut parts = l.split(',');
        let si: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(u64::MAX);
        let pts: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(-1.0);
        let key = parts.next().is_some_and(|f| f.contains('K'));
        // ffprobe subtracts CodecDelay, rounded to its time base.
        let delay = w.delays_ns.get(si as usize).copied().unwrap_or(0);
        let want_ns = (want.1 * w.scale) as f64;
        let close = (pts * 1e9 + delay as f64 - want_ns).abs() <= w.scale as f64 * 1.001;
        if si + 1 != want.0 || !close || key != want.2 {
            mismatch = Some(format!("packet {i}: got {l}, want {want:?}"));
            break;
        }
    }
    let count_ok = lines.len() == w.frames.len();
    row.check(
        "ffprobe packets",
        ok && err.trim().is_empty() && count_ok && mismatch.is_none(),
        format!(
            "{}/{} {} {}",
            lines.len(),
            w.frames.len(),
            mismatch.unwrap_or_default(),
            err.trim()
        ),
    );

    // 4. ffmpeg decodes every stream without a message.
    let (ok, _, err) = run(Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&w.path)
        .args(["-map", "0", "-f", "null", "-"]));
    row.check(
        "ffmpeg decode",
        ok && err.trim().is_empty(),
        err.trim().lines().take(3).collect::<Vec<_>>().join(" | "),
    );

    // 5. Seekable files: seek to every keyframe, and to mid-GOP points.
    if !w.live {
        let (ok, out, _) = run(Command::new(&tools.ffprobe)
            .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
            .arg(&w.path));
        let dur: f64 = out.trim().parse().unwrap_or(0.0);
        row.check("duration", ok && dur > 5.5 && dur < 6.5, format!("{dur:.3} s"));
        let sel = if w.has_video { "v:0" } else { "a:0" };
        // The cue points: every video keyframe (checked against the
        // frames above), or every cluster start for audio-only files.
        let keys = cue_times.clone();
        if w.has_video {
            let vk: Vec<u64> = w.frames.iter().filter(|f| f.0 == 1 && f.2).map(|f| f.1).collect();
            row.check(
                "one cue per video keyframe",
                vk == keys,
                format!("{} keyframes", vk.len()),
            );
        }
        let mut good = 0;
        let mut bad = Vec::new();
        for &k in &keys {
            let t = k as f64 * w.scale as f64 / 1e9;
            for target in [t, t + 0.2] {
                let (ok, out, _) = run(Command::new(&tools.ffprobe)
                    .args([
                        "-v",
                        "error",
                        "-select_streams",
                        sel,
                        "-read_intervals",
                        &format!("{target:.4}%+#1"),
                        "-show_entries",
                        "packet=pts_time,flags",
                        "-of",
                        "csv=p=0",
                    ])
                    .arg(&w.path));
                let first = out.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                let mut pts: f64 = first.split(',').next().and_then(|s| s.parse().ok()).unwrap_or(-1.0);
                if !w.has_video {
                    pts += w.delays_ns[0] as f64 / 1e9;
                }
                let want_key = !w.has_video || first.contains('K');
                // Video: exactly the keyframe. Audio-only: a packet from
                // the cue point up to the target (ffprobe may skip the packet
                // whose start, less CodecDelay, falls before it).
                let hit = if w.has_video {
                    (pts - t).abs() < 0.0015
                } else {
                    pts > t - 0.0015 && pts <= target + 0.0215
                };
                if ok && hit && want_key {
                    good += 1;
                } else if bad.len() < 3 {
                    bad.push(format!("{target:.3}->{first}"));
                }
            }
        }
        row.check(
            "seek to keyframes",
            good == keys.len() * 2 && !keys.is_empty(),
            format!("{good}/{} {}", keys.len() * 2, bad.join(" ")),
        );
    }

    // 6. Live streams cut at random offsets still decode every frame of
    // every whole cluster.
    if w.live {
        row_cuts(tools, w, work, &mut row);
    }
    row
}

fn row_cuts(tools: &Tools, w: &Written, work: &Path, row: &mut Row) {
    let full = match demux::parse(&w.bytes) {
        Ok(d) => d,
        Err(e) => return row.check("cut-off", false, e),
    };
    let starts: Vec<usize> = full
        .clusters
        .iter()
        .map(|c| (full.segment_data_at + c.offset) as usize)
        .collect();
    let mut ends: Vec<usize> = starts.iter().skip(1).copied().collect();
    ends.push(w.bytes.len());
    let mut seed = 0x1234_5678_9ABC_DEF1u64 ^ w.bytes.len() as u64;
    let mut cuts: Vec<usize> = (0..10)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            starts[0] + (seed % (w.bytes.len() - starts[0]) as u64) as usize
        })
        .collect();
    cuts.extend(ends.iter().take(3).map(|e| e - 1));
    let ntracks = full.tracks.len();
    let mut good = 0;
    let mut notes = Vec::new();
    for (ci, &cut) in cuts.iter().enumerate() {
        let whole = ends.iter().filter(|&&e| e <= cut).count();
        let mut need = vec![0usize; ntracks];
        for f in full.frames.iter().filter(|f| f.cluster < whole) {
            need[f.track as usize - 1] += 1;
        }
        let p = work.join(format!("cut-{}-{ci}.webm", w.name));
        if std::fs::write(&p, &w.bytes[..cut]).is_err() {
            notes.push("write failed".to_string());
            continue;
        }
        let (_, out, _) = run(Command::new(&tools.ffprobe)
            .args([
                "-v",
                "quiet",
                "-count_frames",
                "-show_entries",
                "stream=nb_read_frames",
                "-of",
                "csv=p=0",
            ])
            .arg(&p));
        let got: Vec<usize> = out
            .lines()
            .filter_map(|l| l.trim().trim_end_matches(',').parse().ok())
            .collect();
        let nothing_needed = need.iter().all(|&n| n == 0);
        let ok = nothing_needed || (got.len() == ntracks && got.iter().zip(&need).all(|(g, n)| g >= n));
        if ok {
            good += 1;
        } else if notes.len() < 3 {
            notes.push(format!("cut {cut}: decoded {got:?}, need {need:?}"));
        }
        let _ = std::fs::remove_file(&p);
    }
    row.check(
        "cut-off plays to last whole cluster",
        good == cuts.len(),
        format!("{good}/{} {}", cuts.len(), notes.join(" ")),
    );
}

fn main() {
    let work = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "out".into()));
    let tools = Tools {
        ffmpeg: std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".into()),
        ffprobe: std::env::var("FFPROBE").unwrap_or_else(|_| "ffprobe".into()),
        mkvalidator: std::env::var("MKVALIDATOR").unwrap_or_else(|_| "mkvalidator".into()),
    };
    let media = work.join("media");
    std::fs::create_dir_all(&media).expect("work dir");
    let inp = match inputs::make(&tools, &work.join("inputs")) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("inputs: {e}");
            std::process::exit(2);
        }
    };
    let (_, help_out, help_err) = run(Command::new(&tools.mkvalidator).arg("--help"));
    let mkv_live_flag = format!("{help_out}{help_err}").contains("--live");
    println!("mkvalidator --help:\n{help_out}{help_err}");

    let cases = match cases(&tools, &inp) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cases: {e}");
            std::process::exit(2);
        }
    };
    let mut rows = Vec::new();
    for case in cases {
        match mux(case, &media) {
            Ok(w) => rows.push(verify(&tools, &w, &work, mkv_live_flag)),
            Err(e) => rows.push(Row {
                name: "mux".into(),
                checks: vec![("mux".into(), false, e)],
            }),
        }
    }
    let mut report = String::new();
    let mut all_ok = true;
    for r in &rows {
        all_ok &= r.ok();
        let _ = writeln!(report, "{} {}", if r.ok() { "PASS" } else { "FAIL" }, r.name);
        for (what, ok, note) in &r.checks {
            let _ = writeln!(report, "    {} {what}: {note}", if *ok { "ok  " } else { "FAIL" });
        }
    }
    let total: usize = rows.iter().map(|r| r.checks.len()).sum();
    let passed: usize = rows.iter().map(|r| r.checks.iter().filter(|c| c.1).count()).sum();
    let _ = writeln!(report, "\n{} files, {passed}/{total} checks passed", rows.len());
    print!("{report}");
    let _ = std::fs::write(work.join("report.txt"), &report);
    std::process::exit(if all_ok { 0 } else { 1 });
}
