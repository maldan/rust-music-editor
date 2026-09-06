//! Real radix-2 FFT + log-frequency fold for Spectrum / Spectrogram taps.

pub const FFT_N: usize = 1024;
pub const TAP_FFT_N: usize = 2048;
pub const TAP_HOP: usize = 512;
pub const SPEC_BINS: usize = 256;
pub const SPEC_COLS: usize = 72;
pub const FMIN: f32 = 20.0;
const DB_FLOOR: f32 = -72.0;

/// Log axis marks (Hz). `200` is on purpose — that's the rumble band people look for.
pub const FREQ_MARKS: &[(f32, &str)] = &[
    (20.0, "20Hz"),
    (100.0, "100"),
    (200.0, "200"),
    (500.0, "500"),
    (1_000.0, "1k"),
    (5_000.0, "5k"),
    (10_000.0, "10k"),
];

pub fn log_freq_t(freq: f32, sample_rate: f32) -> f32 {
    let nyq = (sample_rate * 0.5).max(FMIN * 2.0);
    ((freq.max(FMIN) / FMIN).ln() / (nyq / FMIN).ln()).clamp(0.0, 1.0)
}

pub fn freq_ticks(sample_rate: f32) -> Vec<(f32, &'static str)> {
    view_freq_ticks(sample_rate, 0.0, 1.0)
}

pub fn t_to_freq(t: f32, sample_rate: f32) -> f32 {
    let nyq = (sample_rate * 0.5).max(FMIN * 2.0);
    FMIN * (nyq / FMIN).powf(t.clamp(0.0, 1.0))
}

pub fn view_freq_ticks(sample_rate: f32, t0: f32, span: f32) -> Vec<(f32, &'static str)> {
    let nyq = sample_rate * 0.5;
    let span = span.max(1e-4);
    FREQ_MARKS
        .iter()
        .copied()
        .filter(|(f, _)| *f <= nyq)
        .filter_map(|(f, s)| {
            let t = (log_freq_t(f, sample_rate) - t0) / span;
            (t >= -0.02 && t <= 1.02).then_some((t.clamp(0.0, 1.0), s))
        })
        .collect()
}

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

fn midi_freq(m: i32) -> f32 {
    440.0 * 2f32.powf((m - 69) as f32 / 12.0)
}

fn midi_name(m: i32) -> String {
    format!("{}{}", NOTE_NAMES[((m % 12) + 12) as usize % 12], m.div_euclid(12) - 1)
}

/// Note labels in a log-frequency window (`t0` + `span` in 0..=1).
pub fn view_note_ticks(sample_rate: f32, t0: f32, span: f32) -> Vec<(f32, String)> {
    let span = span.max(1e-4);
    let f0 = t_to_freq(t0, sample_rate);
    let f1 = t_to_freq(t0 + span, sample_rate);
    let m0 = (12.0 * (f0.max(FMIN) / 440.0).log2() + 69.0).floor() as i32;
    let m1 = (12.0 * (f1.max(FMIN) / 440.0).log2() + 69.0).ceil() as i32;
    let range = (m1 - m0).max(1);
    let mut out = Vec::new();
    for m in m0.max(0)..=m1.min(127) {
        let pc = m.rem_euclid(12);
        // Density follows how many octaves fit in the window, not a fixed C/G grid.
        let show = if range > 60 {
            pc == 0
        } else if range > 36 {
            pc == 0 || pc == 7
        } else if range > 10 {
            matches!(pc, 0 | 2 | 4 | 5 | 7 | 9 | 11)
        } else {
            true
        };
        if !show {
            continue;
        }
        let t = (log_freq_t(midi_freq(m), sample_rate) - t0) / span;
        if t < 0.0 || t > 1.0 {
            continue;
        }
        out.push((t, midi_name(m)));
    }
    out
}

/// Stretch log-bins in `[t0, t0+span]` to `rows`, and zero values below `floor`.
pub fn spec_window(cells: &[f32], cols: usize, rows: usize, t0: f32, span: f32, floor: f32) -> Vec<f32> {
    let mut out = vec![0.0; cols * rows];
    if cells.len() < cols * rows || rows == 0 {
        return out;
    }
    let full = t0 <= 0.001 && span >= 0.999;
    for c in 0..cols {
        let src = &cells[c * rows..(c + 1) * rows];
        let dst = &mut out[c * rows..(c + 1) * rows];
        if full {
            for (d, &v) in dst.iter_mut().zip(src) {
                *d = if v < floor { 0.0 } else { v };
            }
            continue;
        }
        let span = span.max(1e-4);
        let n = (rows - 1).max(1) as f32;
        for (i, d) in dst.iter_mut().enumerate() {
            let t = t0 + span * (i as f32 / n);
            let x = t.clamp(0.0, 1.0) * (rows - 1) as f32;
            let i0 = x.floor() as usize;
            let f = x - i0 as f32;
            let a = src[i0.min(rows - 1)];
            let b = src[(i0 + 1).min(rows - 1)];
            let v = a + (b - a) * f;
            *d = if v < floor { 0.0 } else { v };
        }
    }
    out
}

pub fn fft_radix2(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && n == im.len());
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2usize;
    while len <= n {
        let half = len / 2;
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let mut i = 0usize;
        while i < n {
            for k in 0..half {
                let a = ang * k as f32;
                let wr = a.cos();
                let wi = a.sin();
                let vr = re[i + k + half] * wr - im[i + k + half] * wi;
                let vi = re[i + k + half] * wi + im[i + k + half] * wr;
                let ur = re[i + k];
                let ui = im[i + k];
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + half] = ur - vr;
                im[i + k + half] = ui - vi;
            }
            i += len;
        }
        len *= 2;
    }
}

pub fn ifft_radix2(re: &mut [f32], im: &mut [f32]) {
    for v in im.iter_mut() {
        *v = -*v;
    }
    fft_radix2(re, im);
    let n = re.len() as f32;
    for i in 0..re.len() {
        re[i] /= n;
        im[i] = -im[i] / n;
    }
}

pub const EQ_HOP: usize = FFT_N / 2;

/// Linear sample of EQ gain curve (`t` in 0..=1, `v` pass amount).
pub fn eq_gain_at(pts: &[(f32, f32)], t: f32) -> f32 {
    if pts.is_empty() {
        return 1.0;
    }
    if pts.len() == 1 {
        return pts[0].1.clamp(0.0, 1.0);
    }
    let t = t.clamp(0.0, 1.0);
    if t <= pts[0].0 {
        return pts[0].1.clamp(0.0, 1.0);
    }
    let last = pts[pts.len() - 1];
    if t >= last.0 {
        return last.1.clamp(0.0, 1.0);
    }
    for w in pts.windows(2) {
        if t >= w[0].0 && t <= w[1].0 {
            let dt = (w[1].0 - w[0].0).max(1e-5);
            let u = (t - w[0].0) / dt;
            return (w[0].1 + (w[1].1 - w[0].1) * u).clamp(0.0, 1.0);
        }
    }
    last.1.clamp(0.0, 1.0)
}

pub fn eq_bin_gains(pts: &[(f32, f32)], sample_rate: f32, out: &mut [f32]) {
    let n = out.len();
    if n < 2 {
        return;
    }
    let half = n / 2;
    for k in 0..=half {
        let freq = k as f32 * sample_rate / n as f32;
        let t = log_freq_t(freq.max(FMIN), sample_rate);
        let g = eq_gain_at(pts, t);
        out[k] = g;
        if k != 0 && k != half && n - k < n {
            out[n - k] = g;
        }
    }
}

pub fn hann(i: usize, n: usize) -> f32 {
    0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n as f32 - 1.0)).cos()
}

pub fn mag_to_unit(mag: f32) -> f32 {
    let db = 20.0 * mag.max(1e-12).log10();
    ((db - DB_FLOOR) / -DB_FLOOR).clamp(0.0, 1.0)
}

/// Fold linear FFT bins into log-spaced 20 Hz … Nyquist magnitudes in `0..=1`.
pub fn fold_log_bins(re: &[f32], im: &[f32], sample_rate: f32, out: &mut [f32]) {
    let n = re.len() as f32;
    let nyq = sample_rate * 0.5;
    let nbin = out.len();
    out.fill(0.0);
    if nbin == 0 || nyq <= FMIN {
        return;
    }
    let span = (nyq / FMIN).ln();
    let half = re.len() / 2;
    for k in 1..half {
        let freq = k as f32 * sample_rate / n;
        if freq < FMIN || freq > nyq {
            continue;
        }
        let mag = re[k].hypot(im[k]) / n;
        let t = (freq / FMIN).ln() / span;
        let x = t * (nbin as f32 - 1.0);
        let i = x.floor() as usize;
        let f = x - i as f32;
        let p = mag * mag;
        if i < nbin {
            out[i] += p * (1.0 - f);
        }
        if i + 1 < nbin {
            out[i + 1] += p * f;
        }
    }
    for v in out.iter_mut() {
        *v = mag_to_unit(v.sqrt());
    }
}

pub fn analyze_window(samples: &[f32], sample_rate: f32, out: &mut [f32]) {
    debug_assert_eq!(samples.len(), TAP_FFT_N);
    let mut re: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, s)| s * hann(i, TAP_FFT_N))
        .collect();
    let mut im = vec![0.0f32; TAP_FFT_N];
    fft_radix2(&mut re, &mut im);
    fold_log_bins(&re, &im, sample_rate, out);
}

pub fn peak_bin(bins: &[f32]) -> usize {
    bins.iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_lands_in_bin_zero() {
        let mut re = vec![1.0f32; 16];
        let mut im = vec![0.0f32; 16];
        fft_radix2(&mut re, &mut im);
        assert!((re[0] - 16.0).abs() < 1e-4);
        for k in 1..16 {
            assert!(re[k].abs() < 1e-4 && im[k].abs() < 1e-4);
        }
    }

    #[test]
    fn cosine_peaks_at_bin() {
        let n = 64usize;
        let k0 = 5usize;
        let mut re: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * k0 as f32 * i as f32 / n as f32).cos())
            .collect();
        let mut im = vec![0.0f32; n];
        fft_radix2(&mut re, &mut im);
        let mags: Vec<f32> = (0..n / 2).map(|k| re[k].hypot(im[k])).collect();
        let peak = peak_bin(&mags);
        assert_eq!(peak, k0);
    }

    #[test]
    fn ifft_roundtrip() {
        let mut re: Vec<f32> = (0..16).map(|i| (i as f32 * 0.3).sin()).collect();
        let orig = re.clone();
        let mut im = vec![0.0f32; 16];
        fft_radix2(&mut re, &mut im);
        ifft_radix2(&mut re, &mut im);
        for i in 0..16 {
            assert!((re[i] - orig[i]).abs() < 1e-5, "i={i} {} vs {}", re[i], orig[i]);
            assert!(im[i].abs() < 1e-5);
        }
    }

    #[test]
    fn eq_gain_cuts_mid() {
        let pts = [(0.0, 1.0), (0.5, 0.0), (1.0, 1.0)];
        assert!((eq_gain_at(&pts, 0.0) - 1.0).abs() < 1e-5);
        assert!(eq_gain_at(&pts, 0.5) < 0.05);
        assert!((eq_gain_at(&pts, 1.0) - 1.0).abs() < 1e-5);
        let mut g = vec![0.0; 16];
        eq_bin_gains(&pts, 48_000.0, &mut g);
        assert!(g[0] > 0.8);
    }

    #[test]
    fn a440_is_below_nyquist_mid() {
        let sr = 48_000.0;
        let mut samples = vec![0.0f32; TAP_FFT_N];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr).sin();
        }
        let mut bins = vec![0.0f32; SPEC_BINS];
        analyze_window(&samples, sr, &mut bins);
        let peak = peak_bin(&bins);
        assert!(
            peak < SPEC_BINS / 2,
            "440 Hz should sit in the lower half of a log 20 Hz–Nyquist axis, got {peak}"
        );
        assert!(bins[peak] > 0.25);
    }

    #[test]
    fn log_ticks_put_200hz_left_of_1k() {
        let sr = 48_000.0;
        let t20 = log_freq_t(20.0, sr);
        let t200 = log_freq_t(200.0, sr);
        let t1k = log_freq_t(1_000.0, sr);
        let t10k = log_freq_t(10_000.0, sr);
        assert!((t20 - 0.0).abs() < 1e-5);
        assert!(t200 > t20);
        assert!(t200 < t1k);
        assert!(t1k < t10k);
        let ticks = freq_ticks(sr);
        assert!(ticks.iter().any(|(_, s)| *s == "200"));
        assert!(ticks.iter().any(|(_, s)| *s == "1k"));
    }

    #[test]
    fn t_to_freq_inverts_log_axis() {
        let sr = 48_000.0;
        for f in [20.0, 440.0, 1000.0, 10_000.0] {
            let back = t_to_freq(log_freq_t(f, sr), sr);
            assert!((back - f).abs() / f < 1e-4, "{back} vs {f}");
        }
    }

    #[test]
    fn zoomed_window_lists_naturals() {
        let sr = 48_000.0;
        let t0 = log_freq_t(190.0, sr);
        let span = log_freq_t(621.0, sr) - t0;
        let names: Vec<String> = view_note_ticks(sr, t0, span)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        for n in ["C4", "D4", "E4", "F4", "G4", "A4", "B4", "C5"] {
            assert!(names.iter().any(|s| s == n), "missing {n} in {names:?}");
        }
    }

    #[test]
    fn note_ticks_mark_a4() {
        let sr = 48_000.0;
        let ticks = view_note_ticks(sr, 0.0, 1.0);
        let a4 = ticks.iter().find(|(_, n)| n == "A4");
        assert!(a4.is_none(), "full-range view only labels C (and maybe G)");
        let t0 = log_freq_t(400.0, sr);
        let span = log_freq_t(500.0, sr) - t0;
        let zoom = view_note_ticks(sr, t0, span);
        assert!(zoom.iter().any(|(_, n)| n == "A4"));
        let t = zoom.iter().find(|(_, n)| n == "A4").unwrap().0;
        assert!((t - (log_freq_t(440.0, sr) - t0) / span).abs() < 1e-4);
    }

    #[test]
    fn spec_window_zooms_and_gates() {
        let cols = 2;
        let rows = 8;
        let mut cells = vec![0.0f32; cols * rows];
        cells[3] = 1.0;
        cells[rows + 3] = 0.4;
        let zoom = spec_window(&cells, cols, rows, 3.0 / 7.0, 1.0 / 7.0, 0.0);
        assert!(zoom[0] > 0.9);
        assert!((zoom[rows] - 0.4).abs() < 0.05);
        let gated = spec_window(&cells, cols, rows, 0.0, 1.0, 0.5);
        assert!(gated[3] > 0.9);
        assert!(gated[rows + 3] < 0.01);
    }
}
