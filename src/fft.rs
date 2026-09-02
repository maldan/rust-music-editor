//! Real radix-2 FFT + log-frequency fold for Spectrum / Spectrogram taps.

pub const FFT_N: usize = 1024;
pub const FFT_HOP: usize = 256;
pub const SPEC_BINS: usize = 40;
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
    let nyq = sample_rate * 0.5;
    FREQ_MARKS
        .iter()
        .copied()
        .filter(|(f, _)| *f <= nyq)
        .map(|(f, s)| (log_freq_t(f, sample_rate), s))
        .collect()
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
    debug_assert_eq!(samples.len(), FFT_N);
    let mut re: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, s)| s * hann(i, FFT_N))
        .collect();
    let mut im = vec![0.0f32; FFT_N];
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
        let mut samples = vec![0.0f32; FFT_N];
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
}
