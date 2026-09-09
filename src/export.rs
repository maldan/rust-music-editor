use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::compile::{Live, Patch};
use crate::graph::BEATS_PER_BAR;
use crate::monitor::Monitor;

pub const SAMPLE_RATE: f32 = 44_100.0;
pub const MAX_BARS: i32 = 1024;
const RENDER_BLOCK: usize = 512;

pub fn sample_count(bars: u32, bpm: f32, sample_rate: f32) -> usize {
    let beats = bars.max(1) as f64 * f64::from(BEATS_PER_BAR);
    let secs = beats * 60.0 / f64::from(bpm.max(1.0));
    (secs * f64::from(sample_rate.max(1.0))).round() as usize
}

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn with_mp3_ext(mut path: PathBuf) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("mp3") => path,
        _ => {
            path.set_extension("mp3");
            path
        }
    }
}

pub fn write_mp3(
    patch: &Patch,
    bars: u32,
    path: &Path,
    progress: &AtomicU32,
) -> Result<(), String> {
    let path = with_mp3_ext(path.to_path_buf());
    let n = sample_count(bars, patch.bpm, SAMPLE_RATE);
    if n == 0 {
        return Err("export length is empty".into());
    }
    let bytes = render_mp3(patch, n, progress)?;
    progress.store(1000, Ordering::Relaxed);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

fn render_mp3(patch: &Patch, frames: usize, progress: &AtomicU32) -> Result<Vec<u8>, String> {
    let monitor = Arc::new(Monitor::default());
    let (mut live, mut graph) = Live::new_at(patch, SAMPLE_RATE, monitor, RENDER_BLOCK);
    let mut encoder = lame_encoder()?;
    let mut mp3 = Vec::new();
    let mut left = Vec::with_capacity(1152);
    let mut right = Vec::with_capacity(1152);
    let mut i = 0;
    while i < frames {
        let take = (frames - i).min(graph.block_size());
        live.tick_block(&mut graph, take);
        let _ = graph.process_frames(take);
        let (l, r) = graph.master_stereo();
        let n = take.min(l.len()).min(r.len());
        for s in 0..n {
            left.push(to_i16(l[s]));
            right.push(to_i16(r[s]));
        }
        while left.len() >= 1152 {
            encode_pcm(&mut encoder, &left[..1152], &right[..1152], &mut mp3)?;
            left.drain(..1152);
            right.drain(..1152);
        }
        i += take;
        let t = (i as f32 / frames as f32 * 950.0) as u32;
        progress.store(t.min(950), Ordering::Relaxed);
    }
    if !left.is_empty() {
        encode_pcm(&mut encoder, &left, &right, &mut mp3)?;
    }
    flush_mp3(&mut encoder, &mut mp3)?;
    Ok(mp3)
}

fn lame_encoder() -> Result<mp3lame_encoder::Encoder, String> {
    let builder = mp3lame_encoder::Builder::new().ok_or("failed to start MP3 encoder")?;
    builder
        .with_num_channels(2)
        .map_err(|e| format!("{e:?}"))?
        .with_sample_rate(SAMPLE_RATE as u32)
        .map_err(|e| format!("{e:?}"))?
        .with_brate(mp3lame_encoder::Bitrate::Kbps192)
        .map_err(|e| format!("{e:?}"))?
        .with_quality(mp3lame_encoder::Quality::Best)
        .map_err(|e| format!("{e:?}"))?
        .build()
        .map_err(|e| format!("{e:?}"))
}

fn encode_pcm(
    encoder: &mut mp3lame_encoder::Encoder,
    left: &[i16],
    right: &[i16],
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let input = mp3lame_encoder::DualPcm { left, right };
    out.reserve(mp3lame_encoder::max_required_buffer_size(left.len()));
    let n = encoder
        .encode(input, out.spare_capacity_mut())
        .map_err(|e| format!("{e:?}"))?;
    unsafe {
        out.set_len(out.len() + n);
    }
    Ok(())
}

fn flush_mp3(encoder: &mut mp3lame_encoder::Encoder, out: &mut Vec<u8>) -> Result<(), String> {
    out.reserve(mp3lame_encoder::max_required_buffer_size(1152));
    let n = encoder
        .flush::<mp3lame_encoder::FlushNoGap>(out.spare_capacity_mut())
        .map_err(|e| format!("{e:?}"))?;
    unsafe {
        out.set_len(out.len() + n);
    }
    Ok(())
}

/// Drain leftover work so tests can encode a short burst without a full song.
pub fn encode_pcm_mp3(left: &[i16], right: &[i16]) -> Result<Vec<u8>, String> {
    let mut encoder = lame_encoder()?;
    let mut out = Vec::new();
    encode_pcm(&mut encoder, left, right, &mut out)?;
    flush_mp3(&mut encoder, &mut out)?;
    Ok(out)
}

pub fn render_peak(patch: &Patch, frames: usize) -> f32 {
    let monitor = Arc::new(Monitor::default());
    let (mut live, mut graph) = Live::new_at(patch, SAMPLE_RATE, monitor, RENDER_BLOCK);
    let mut peak = 0.0f32;
    let mut i = 0;
    while i < frames {
        let take = (frames - i).min(graph.block_size());
        live.tick_block(&mut graph, take);
        let _ = graph.process_frames(take);
        let (l, r) = graph.master_stereo();
        let n = take.min(l.len()).min(r.len());
        for s in 0..n {
            peak = peak.max(l[s].abs()).max(r[s].abs());
        }
        i += take;
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::Patch;
    use crate::graph::Project;

    #[test]
    fn sample_count_two_bars_at_120() {
        // 2 bars * 4 beats * 0.5s = 4s at 120bpm
        assert_eq!(sample_count(2, 120.0, 100.0), 400);
    }

    #[test]
    fn default_graph_makes_sound() {
        let p = Project::new_default();
        let patch = Patch::from_main(&p, true);
        let peak = render_peak(&patch, sample_count(1, patch.bpm, SAMPLE_RATE).min(12_000));
        assert!(peak > 1e-3, "export should hear the default seq, peak {peak}");
    }

    #[test]
    fn mp3_header_is_mpeg() {
        let n = 2304;
        let left: Vec<i16> = (0..n)
            .map(|i| ((i as f32 * 0.1).sin() * 16000.0) as i16)
            .collect();
        let bytes = encode_pcm_mp3(&left, &left).unwrap();
        assert!(bytes.len() > 16, "mp3 too small: {}", bytes.len());
        let mpeg = bytes.windows(2).any(|w| w[0] == 0xFF && w[1] & 0xE0 == 0xE0);
        assert!(mpeg, "no MPEG frame sync in {} bytes", bytes.len());
    }
}
