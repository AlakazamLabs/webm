//! Test inputs: elementary streams in IVF and Ogg files, made by running
//! ffmpeg as a separate process, and the readers for those two formats.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{run_ok, Tools};

/// One compressed frame or packet.
#[derive(Clone, Debug)]
pub struct Packet {
    pub data: Vec<u8>,
    pub ns: u64,
    pub keyframe: bool,
}

pub struct Inputs {
    pub dir: PathBuf,
}

/// Makes every input file in `dir` with ffmpeg.
pub fn make(tools: &Tools, dir: &Path) -> Result<Inputs, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let src = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240:rate=30",
        "-t",
        "6",
        "-pix_fmt",
        "yuv420p",
    ];
    let tone = |f: &str| format!("sine=frequency={f}:sample_rate=48000:duration=6");
    let jobs: Vec<(String, Vec<String>)> = vec![
        (
            "vp9.ivf".into(),
            strs(&src)
                .into_iter()
                .chain(strs(&[
                    "-c:v",
                    "libvpx-vp9",
                    "-deadline",
                    "realtime",
                    "-cpu-used",
                    "8",
                    "-g",
                    "30",
                    "-keyint_min",
                    "30",
                    "-lag-in-frames",
                    "0",
                    "-b:v",
                    "500k",
                    "-f",
                    "ivf",
                ]))
                .collect(),
        ),
        (
            "av1.ivf".into(),
            strs(&src)
                .into_iter()
                .chain(strs(&[
                    "-c:v",
                    "libaom-av1",
                    "-cpu-used",
                    "8",
                    "-usage",
                    "realtime",
                    "-row-mt",
                    "1",
                    "-g",
                    "30",
                    "-keyint_min",
                    "30",
                    "-b:v",
                    "500k",
                    "-f",
                    "ivf",
                ]))
                .collect(),
        ),
        (
            "opus.ogg".into(),
            strs(&[
                "-f",
                "lavfi",
                "-i",
                &tone("440"),
                "-ac",
                "2",
                "-c:a",
                "libopus",
                "-b:a",
                "64k",
                "-frame_duration",
                "20",
                "-f",
                "ogg",
            ]),
        ),
        (
            "vorbis.ogg".into(),
            strs(&[
                "-f",
                "lavfi",
                "-i",
                &tone("660"),
                "-ac",
                "2",
                "-ar",
                "44100",
                "-c:a",
                "libvorbis",
                "-q:a",
                "3",
                "-f",
                "ogg",
            ]),
        ),
    ];
    for (name, mut args) in jobs {
        let out = dir.join(&name);
        args.insert(0, "-y".into());
        args.insert(1, "-v".into());
        args.insert(2, "error".into());
        args.push(out.display().to_string());
        let mut cmd = Command::new(&tools.ffmpeg);
        cmd.args(&args);
        run_ok(&mut cmd, &format!("ffmpeg making {name}"))?;
    }
    Ok(Inputs { dir: dir.to_path_buf() })
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

impl Inputs {
    /// The video frames of an IVF file, with keyframe flags as ffprobe
    /// reports them.
    pub fn ivf(&self, tools: &Tools, name: &str) -> Result<(Vec<Packet>, (u32, u32)), String> {
        let path = self.dir.join(name);
        let b = std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?;
        if b.len() < 32 || &b[..4] != b"DKIF" {
            return Err(format!("{name}: not IVF"));
        }
        let hdr = u16::from_le_bytes([b[6], b[7]]) as usize;
        let w = u16::from_le_bytes([b[12], b[13]]) as u32;
        let h = u16::from_le_bytes([b[14], b[15]]) as u32;
        let rate = u32::from_le_bytes(b[16..20].try_into().unwrap()) as u64;
        let scale = u32::from_le_bytes(b[20..24].try_into().unwrap()) as u64;
        let mut at = hdr;
        let mut frames = Vec::new();
        while at + 12 <= b.len() {
            let size = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
            let pts = u64::from_le_bytes(b[at + 4..at + 12].try_into().unwrap());
            at += 12;
            if at + size > b.len() {
                return Err(format!("{name}: frame cut"));
            }
            frames.push(Packet {
                data: b[at..at + size].to_vec(),
                ns: pts * scale * 1_000_000_000 / rate,
                keyframe: false,
            });
            at += size;
        }
        // Keyframe flags from ffprobe, in packet order.
        let out = crate::capture(
            Command::new(&tools.ffprobe)
                .args(["-v", "error", "-show_entries", "packet=flags", "-of", "csv=p=0"])
                .arg(&path),
            "ffprobe on an IVF input",
        )?;
        let flags: Vec<bool> = out.lines().filter(|l| !l.is_empty()).map(|l| l.contains('K')).collect();
        if flags.len() != frames.len() {
            return Err(format!(
                "{name}: ffprobe saw {} packets, the reader {}",
                flags.len(),
                frames.len()
            ));
        }
        for (f, k) in frames.iter_mut().zip(flags) {
            f.keyframe = k;
        }
        Ok((frames, (w, h)))
    }
}

/// The packets of an Ogg file's single logical stream, each with the
/// granule position of the page it completes on, if it is the last packet
/// completed there.
pub fn ogg_packets(path: &Path) -> Result<Vec<(Vec<u8>, Option<i64>)>, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut at = 0;
    let mut out: Vec<(Vec<u8>, Option<i64>)> = Vec::new();
    let mut partial: Vec<u8> = Vec::new();
    while at < b.len() {
        if b.len() < at + 27 || &b[at..at + 4] != b"OggS" {
            return Err(format!("bad Ogg page at {at}"));
        }
        let granule = i64::from_le_bytes(b[at + 6..at + 14].try_into().unwrap());
        let nseg = b[at + 26] as usize;
        let table = &b[at + 27..at + 27 + nseg];
        let mut data = at + 27 + nseg;
        let mut completed_here = Vec::new();
        for &len in table {
            partial.extend_from_slice(&b[data..data + len as usize]);
            data += len as usize;
            if len < 255 {
                completed_here.push(std::mem::take(&mut partial));
            }
        }
        let n = completed_here.len();
        for (i, p) in completed_here.into_iter().enumerate() {
            out.push((p, if i + 1 == n { Some(granule) } else { None }));
        }
        at = data;
    }
    Ok(out)
}

/// Opus: the OpusHead, the 20 ms packets, and the end trim in samples
/// (how much of the last packet is padding).
pub fn opus(path: &Path) -> Result<(Vec<u8>, Vec<Packet>, u64), String> {
    let pk = ogg_packets(path)?;
    if pk.len() < 3 || !pk[0].0.starts_with(b"OpusHead") {
        return Err("no OpusHead".into());
    }
    let head = pk[0].0.clone();
    let audio = &pk[2..];
    let last_granule = audio.iter().rev().find_map(|(_, g)| *g).ok_or("no granule")?;
    let total = audio.len() as u64 * 960;
    let trim = total.saturating_sub(last_granule as u64);
    let packets = audio
        .iter()
        .enumerate()
        .map(|(i, (d, _))| Packet {
            data: d.clone(),
            ns: i as u64 * 20_000_000,
            keyframe: true,
        })
        .collect();
    Ok((head, packets, trim))
}

/// Vorbis: the three headers and the audio packets, timed by spreading
/// each page's packets evenly over its granule span.
pub fn vorbis(path: &Path) -> Result<([Vec<u8>; 3], Vec<Packet>, u32), String> {
    let pk = ogg_packets(path)?;
    if pk.len() < 4 || pk[0].0.get(1..7) != Some(b"vorbis") {
        return Err("no Vorbis headers".into());
    }
    let rate = u32::from_le_bytes(pk[0].0[12..16].try_into().unwrap());
    let headers = [pk[0].0.clone(), pk[1].0.clone(), pk[2].0.clone()];
    let audio = &pk[3..];
    let mut packets = Vec::new();
    let mut prev_granule = 0i64;
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut last_ns: Option<u64> = None;
    for (d, g) in audio {
        pending.push(d.clone());
        if let Some(g) = g {
            let n = pending.len() as i64;
            for (k, p) in pending.drain(..).enumerate() {
                let sample = prev_granule + (g - prev_granule).max(0) * k as i64 / n;
                let mut ns = sample.max(0) as u64 * 1_000_000_000 / u64::from(rate);
                // Keep at least 1 ms apart, so the default scale keeps order.
                if let Some(l) = last_ns {
                    if ns / 1_000_000 <= l / 1_000_000 {
                        ns = (l / 1_000_000 + 1) * 1_000_000;
                    }
                }
                last_ns = Some(ns);
                packets.push(Packet {
                    data: p,
                    ns,
                    keyframe: true,
                });
            }
            prev_granule = *g;
        }
    }
    Ok((headers, packets, rate))
}

/// AV1: strips each temporal unit's leading temporal delimiter OBU, and
/// returns the sequence header OBU from the first unit.
pub fn av1_prepare(frames: &mut [Packet]) -> Result<Vec<u8>, String> {
    for f in frames.iter_mut() {
        if f.data.len() >= 2 && f.data[0] == 0x12 && f.data[1] == 0x00 {
            f.data.drain(..2);
        }
    }
    let first = &frames.first().ok_or("no AV1 frames")?.data;
    let mut at = 0;
    while at < first.len() {
        let h = first[at];
        let obu_type = (h >> 3) & 0x0F;
        let ext = h & 0x04 != 0;
        let has_size = h & 0x02 != 0;
        let mut p = at + 1 + usize::from(ext);
        if !has_size {
            return Err("AV1 OBU without a size field".into());
        }
        let mut size = 0usize;
        let mut shift = 0;
        loop {
            let byte = *first.get(p).ok_or("leb128 cut")?;
            size |= ((byte & 0x7F) as usize) << shift;
            p += 1;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        let end = p + size;
        if obu_type == 1 {
            return Ok(first[at..end].to_vec());
        }
        at = end;
    }
    Err("no AV1 sequence header OBU in the first temporal unit".into())
}

/// The AV1 codec configuration record for 8-bit 4:2:0 main-profile video
/// at this level, followed by the sequence header OBU.
pub fn av1c(level: u8, seq_header_obu: &[u8]) -> Vec<u8> {
    let mut v = vec![0x81, level & 0x1F, 0x0C, 0x00];
    v.extend_from_slice(seq_header_obu);
    v
}
