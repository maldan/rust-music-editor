use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::compile::{viz_notes, Live, Patch};
use crate::graph::{Sequence, BEATS_PER_BAR};
use crate::monitor::Monitor;
use crate::viz::{self, Frame};

pub const SAMPLE_RATE: f32 = 44_100.0;
pub const MAX_BARS: i32 = 1024;
const RENDER_BLOCK: usize = 512;
pub const VIDEO_FPS: f32 = 30.0;
pub const VIDEO_W: u32 = 1920;
pub const VIDEO_H: u32 = 1080;

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
    let (mut live, mut graph) = Live::new_export(patch, SAMPLE_RATE, monitor, RENDER_BLOCK);
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
        .with_quality(mp3lame_encoder::Quality::Good)
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

fn with_mp4_ext(mut path: PathBuf) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("mp4") => path,
        _ => {
            path.set_extension("mp4");
            path
        }
    }
}

pub fn video_frame_count(samples: usize, sample_rate: f32, fps: f32) -> usize {
    let secs = samples as f64 / f64::from(sample_rate.max(1.0));
    (secs * f64::from(fps.max(1.0))).round().max(1.0) as usize
}

fn ffmpeg_args(w: u32, h: u32, fps: f32, wav: &Path, out: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-s".into(),
        format!("{w}x{h}"),
        "-r".into(),
        format!("{fps}"),
        "-i".into(),
        "-".into(),
        "-i".into(),
        wav.display().to_string(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "18".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-shortest".into(),
        out.display().to_string(),
    ]
}

fn spawn_ffmpeg(args: &[String]) -> Result<std::process::Child, String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map_err(|e| format!("ffmpeg: {e}"))
}

fn wav_header(data_bytes: u32, sample_rate: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&2u16.to_le_bytes());
    h[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 4;
    h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

fn write_wav_stereo(path: &Path, sample_rate: u32, left: &[i16], right: &[i16]) -> Result<(), String> {
    let n = left.len().min(right.len());
    let data_bytes = (n * 4) as u32;
    let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    f.write_all(&wav_header(data_bytes, sample_rate))
        .map_err(|e| e.to_string())?;
    let mut pair = [0u8; 4];
    for i in 0..n {
        pair[0..2].copy_from_slice(&left[i].to_le_bytes());
        pair[2..4].copy_from_slice(&right[i].to_le_bytes());
        f.write_all(&pair).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn headless_gpu() -> Result<(wgpu::Device, wgpu::Queue), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .map_err(|e| format!("no GPU adapter: {e}"))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("viz export"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::default(),
    }))
    .map_err(|e| format!("gpu: {e}"))
}

pub fn write_mp4(
    patch: &Patch,
    sequences: &[Sequence],
    bars: u32,
    path: &Path,
    progress: &AtomicU32,
) -> Result<(), String> {
    let out = with_mp4_ext(path.to_path_buf());
    let n = sample_count(bars, patch.bpm, SAMPLE_RATE);
    if n == 0 {
        return Err("export length is empty".into());
    }
    let n_frames = video_frame_count(n, SAMPLE_RATE, VIDEO_FPS);
    let notes = Arc::new(viz_notes(patch, sequences));
    let monitor = Arc::new(Monitor::default());
    monitor.set_horizon((*notes).clone());
    let (mut live, mut graph) =
        Live::new_export_viz(patch, SAMPLE_RATE, monitor.clone(), RENDER_BLOCK);

    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    let mut waves = Vec::with_capacity(n_frames);
    let mut i = 0usize;
    let mut next_f = 0usize;
    let spf = SAMPLE_RATE / VIDEO_FPS;
    while i < n {
        let take = (n - i).min(graph.block_size());
        live.tick_block(&mut graph, take);
        let _ = graph.process_frames(take);
        let (l, r) = graph.master_stereo();
        let k = take.min(l.len()).min(r.len());
        for s in 0..k {
            left.push(to_i16(l[s]));
            right.push(to_i16(r[s]));
        }
        i += k;
        while next_f < n_frames && (next_f as f32 * spf) as usize <= i {
            waves.push(monitor.viz_waves(&notes));
            next_f += 1;
        }
        let t = (i as f32 / n as f32 * 350.0) as u32;
        progress.store(t.min(350), Ordering::Relaxed);
    }
    while waves.len() < n_frames {
        waves.push(monitor.viz_waves(&notes));
    }

    let wav_path = out.with_extension("export-wav.tmp");
    write_wav_stereo(&wav_path, SAMPLE_RATE as u32, &left, &right)?;
    progress.store(400, Ordering::Relaxed);

    let (device, queue) = match headless_gpu() {
        Ok(g) => g,
        Err(e) => {
            let _ = std::fs::remove_file(&wav_path);
            return Err(e);
        }
    };
    let mut viz = viz::Renderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
    let args = ffmpeg_args(VIDEO_W, VIDEO_H, VIDEO_FPS, &wav_path, &out);
    let mut child = match spawn_ffmpeg(&args) {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&wav_path);
            return Err(e);
        }
    };
    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = std::fs::remove_file(&wav_path);
            return Err("ffmpeg stdin missing".into());
        }
    };

    let bpm = patch.bpm.max(1.0) as f64;
    let pipe = (|| -> Result<(), String> {
        for (fi, wvs) in waves.into_iter().enumerate() {
            let now_beats = fi as f64 / f64::from(VIDEO_FPS) * bpm / 60.0;
            let frame = Frame {
                now_beats,
                window_beats: viz::WINDOW_BEATS,
                notes: notes.clone(),
                gonio: Vec::new(),
                waves: wvs,
                reset: fi == 0,
                width: VIDEO_W,
                height: VIDEO_H,
            };
            let rgba = viz.render_rgba(&device, &queue, VIDEO_W, VIDEO_H, &frame)?;
            stdin.write_all(&rgba).map_err(|e| format!("ffmpeg pipe: {e}"))?;
            let t = 400 + (fi as f32 / n_frames.max(1) as f32 * 580.0) as u32;
            progress.store(t.min(980), Ordering::Relaxed);
        }
        Ok(())
    })();
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&wav_path);
    pipe?;
    let output = output?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = err.lines().rev().take(8).collect();
        return Err(format!(
            "ffmpeg failed: {}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        ));
    }
    progress.store(1000, Ordering::Relaxed);
    Ok(())
}

pub fn render_peak(patch: &Patch, frames: usize) -> f32 {
    let monitor = Arc::new(Monitor::default());
    let (mut live, mut graph) = Live::new_export(patch, SAMPLE_RATE, monitor, RENDER_BLOCK);
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
    fn video_frame_count_four_seconds() {
        assert_eq!(video_frame_count(400, 100.0, 25.0), 100);
    }

    #[test]
    fn ffmpeg_args_pipe_rgba() {
        let wav = PathBuf::from("a.wav");
        let out = PathBuf::from("b.mp4");
        let args = ffmpeg_args(1920, 1080, 30.0, &wav, &out);
        assert!(args.windows(2).any(|w| w[0] == "-pix_fmt" && w[1] == "rgba"));
        assert!(args.iter().any(|a| a == "libx264"));
        assert_eq!(args.last().map(String::as_str), Some("b.mp4"));
    }

    #[test]
    fn wav_header_riff() {
        let h = wav_header(8, 44100);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..12], b"WAVE");
    }

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
