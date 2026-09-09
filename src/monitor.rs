//! Lock-free-ish UI meters: playhead bits and sample rings for Scope / Gonio nodes.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::fft::{SPEC_BINS, SPEC_COLS};

pub const SCOPE_LEN: usize = 2048;
pub const GONIO_LEN: usize = 4096;
pub const GONIO_BINS: usize = 72;
pub const SLICE_BINS: usize = 1024;

pub struct ScopeBuf {
    samples: Box<[AtomicU32]>,
    write: AtomicUsize,
}

impl ScopeBuf {
    fn new() -> Self {
        Self {
            samples: (0..SCOPE_LEN)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            write: AtomicUsize::new(0),
        }
    }

    pub fn push(&self, x: f32) {
        let i = self.write.fetch_add(1, Ordering::Relaxed) % SCOPE_LEN;
        self.samples[i].store(x.to_bits(), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Vec<f32> {
        let w = self.write.load(Ordering::Relaxed);
        let mut out = vec![0.0; SCOPE_LEN];
        for i in 0..SCOPE_LEN {
            let idx = (w + i) % SCOPE_LEN;
            out[i] = f32::from_bits(self.samples[idx].load(Ordering::Relaxed));
        }
        trigger_wave(&out)
    }
}

/// Interleaved L/R ring for a mid/side goniometer.
pub struct GonioBuf {
    lr: Box<[AtomicU32]>,
    write: AtomicUsize,
}

impl GonioBuf {
    fn new() -> Self {
        Self {
            lr: (0..GONIO_LEN * 2)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            write: AtomicUsize::new(0),
        }
    }

    pub fn push(&self, l: f32, r: f32) {
        let i = self.write.fetch_add(1, Ordering::Relaxed) % GONIO_LEN;
        self.lr[i * 2].store(l.to_bits(), Ordering::Relaxed);
        self.lr[i * 2 + 1].store(r.to_bits(), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (Vec<f32>, Vec<f32>) {
        let w = self.write.load(Ordering::Relaxed);
        let mut left = vec![0.0; GONIO_LEN];
        let mut right = vec![0.0; GONIO_LEN];
        for i in 0..GONIO_LEN {
            let idx = (w + i) % GONIO_LEN;
            left[i] = f32::from_bits(self.lr[idx * 2].load(Ordering::Relaxed));
            right[i] = f32::from_bits(self.lr[idx * 2 + 1].load(Ordering::Relaxed));
        }
        (left, right)
    }

    pub fn snapshot_recent(&self, n: usize) -> (Vec<f32>, Vec<f32>) {
        let n = n.clamp(1, GONIO_LEN);
        let w = self.write.load(Ordering::Relaxed);
        let mut left = vec![0.0; n];
        let mut right = vec![0.0; n];
        let start = w + GONIO_LEN - n;
        for i in 0..n {
            let idx = (start + i) % GONIO_LEN;
            left[i] = f32::from_bits(self.lr[idx * 2].load(Ordering::Relaxed));
            right[i] = f32::from_bits(self.lr[idx * 2 + 1].load(Ordering::Relaxed));
        }
        (left, right)
    }
}

/// Mid/side density, column-major, row 0 = bottom (negative mid). Mono stacks in the center column.
pub fn gonio_field(left: &[f32], right: &[f32]) -> (Vec<f32>, f32) {
    let n = GONIO_BINS;
    let mut cells = vec![0.0; n * n];
    let n1 = (n.saturating_sub(1)) as f32;
    let len = left.len().min(right.len());
    let mut acc_lr = 0.0f32;
    let mut acc_l2 = 0.0f32;
    let mut acc_r2 = 0.0f32;
    for i in 0..len {
        let l = left[i];
        let r = right[i];
        acc_lr += l * r;
        acc_l2 += l * l;
        acc_r2 += r * r;
        let mid = (l + r) * 0.5;
        let side = (l - r) * 0.5;
        if mid.abs() + side.abs() < 1e-5 {
            continue;
        }
        let col = ((side + 1.0) * 0.5 * n1).round().clamp(0.0, n1) as usize;
        let row = ((mid + 1.0) * 0.5 * n1).round().clamp(0.0, n1) as usize;
        cells[col * n + row] += 1.0;
    }
    let max = cells.iter().copied().fold(0.0f32, f32::max).max(1.0);
    for v in &mut cells {
        *v = (*v / max).sqrt();
    }
    let denom = (acc_l2.sqrt() * acc_r2.sqrt()).max(1e-8);
    let corr = (acc_lr / denom).clamp(-1.0, 1.0);
    (cells, corr)
}

/// Frozen min/max peaks for one capture window (sample-style waveform).
pub struct SliceBuf {
    pages: [Box<[AtomicU32]>; 2],
    page: AtomicUsize,
    fill: AtomicU32,
}

impl SliceBuf {
    fn new() -> Self {
        Self {
            pages: [
                (0..SLICE_BINS * 2)
                    .map(|_| AtomicU32::new(0))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                (0..SLICE_BINS * 2)
                    .map(|_| AtomicU32::new(0))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ],
            page: AtomicUsize::new(0),
            fill: AtomicU32::new(0),
        }
    }

    pub fn publish(&self, peaks: &[f32]) {
        let cur = self.page.load(Ordering::Relaxed);
        let next = 1 - (cur % 2);
        let dst = &self.pages[next];
        let n = peaks.len().min(dst.len());
        for i in 0..n {
            dst[i].store(peaks[i].to_bits(), Ordering::Relaxed);
        }
        for i in n..dst.len() {
            dst[i].store(0, Ordering::Relaxed);
        }
        self.page.store(next, Ordering::Release);
    }

    pub fn set_fill(&self, t: f32) {
        self.fill.store(t.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Vec<f32> {
        let p = self.page.load(Ordering::Acquire) % 2;
        self.pages[p]
            .iter()
            .map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
            .collect()
    }

    pub fn fill(&self) -> f32 {
        f32::from_bits(self.fill.load(Ordering::Relaxed))
    }
}

/// Start the plot on a rising zero so a periodic wave stays put.
pub fn trigger_wave(samples: &[f32]) -> Vec<f32> {
    let n = samples.len();
    if n < 16 {
        return samples.to_vec();
    }
    let win = n / 2;
    let search_end = n - win;
    let mut start = 0;
    for i in 1..=search_end {
        if samples[i - 1] <= 0.0 && samples[i] > 0.0 {
            start = i;
            break;
        }
    }
    samples[start..start + win].to_vec()
}

/// Latest log-FFT column plus a time ring for spectrogram.
pub struct FftBuf {
    latest: Box<[AtomicU32]>,
    cells: Box<[AtomicU32]>,
    write: AtomicUsize,
}

impl FftBuf {
    fn new() -> Self {
        Self {
            latest: (0..SPEC_BINS)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            cells: (0..SPEC_COLS * SPEC_BINS)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            write: AtomicUsize::new(0),
        }
    }

    pub fn push_bins(&self, bins: &[f32]) {
        let n = bins.len().min(SPEC_BINS);
        for i in 0..n {
            self.latest[i].store(bins[i].to_bits(), Ordering::Relaxed);
        }
        let c = self.write.fetch_add(1, Ordering::Relaxed) % SPEC_COLS;
        let base = c * SPEC_BINS;
        for i in 0..n {
            self.cells[base + i].store(bins[i].to_bits(), Ordering::Relaxed);
        }
    }

    pub fn spectrum(&self) -> Vec<f32> {
        self.latest
            .iter()
            .map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
            .collect()
    }

    pub fn spectrogram(&self) -> Vec<f32> {
        let w = self.write.load(Ordering::Relaxed);
        let mut out = vec![0.0; SPEC_COLS * SPEC_BINS];
        let filled = w.min(SPEC_COLS);
        if filled == 0 {
            return out;
        }
        for t in 0..SPEC_COLS {
            let src_col = if w < SPEC_COLS {
                if t < SPEC_COLS - filled {
                    continue;
                }
                t - (SPEC_COLS - filled)
            } else {
                (w + t) % SPEC_COLS
            };
            let src = src_col * SPEC_BINS;
            let dst = t * SPEC_BINS;
            for i in 0..SPEC_BINS {
                out[dst + i] = f32::from_bits(self.cells[src + i].load(Ordering::Relaxed));
            }
        }
        out
    }
}

/// MIDI pitches 0..=127 as two atomics (audio writes, UI reads).
pub struct PitchSet {
    lo: AtomicU64,
    hi: AtomicU64,
}

impl PitchSet {
    fn new() -> Self {
        Self {
            lo: AtomicU64::new(0),
            hi: AtomicU64::new(0),
        }
    }

    pub fn clear(&self) {
        self.lo.store(0, Ordering::Relaxed);
        self.hi.store(0, Ordering::Relaxed);
    }

    pub fn insert(&self, p: u8) {
        if p < 64 {
            self.lo.fetch_or(1 << p, Ordering::Relaxed);
        } else if p < 128 {
            self.hi.fetch_or(1 << (p - 64), Ordering::Relaxed);
        }
    }

    pub fn remove(&self, p: u8) {
        if p < 64 {
            self.lo.fetch_and(!(1 << p), Ordering::Relaxed);
        } else if p < 128 {
            self.hi.fetch_and(!(1 << (p - 64)), Ordering::Relaxed);
        }
    }

    pub fn load(&self) -> HashSet<u8> {
        let lo = self.lo.load(Ordering::Relaxed);
        let hi = self.hi.load(Ordering::Relaxed);
        let mut out = HashSet::new();
        for i in 0..64u8 {
            if lo & (1 << i) != 0 {
                out.insert(i);
            }
            if hi & (1 << i) != 0 {
                out.insert(i + 64);
            }
        }
        out
    }
}

/// Peak L/R per mixer strip (linear amplitude, audio writes / UI reads).
pub struct MixLevels {
    peaks: Box<[AtomicU32]>,
}

impl MixLevels {
    pub fn new(strips: usize) -> Self {
        Self {
            peaks: (0..strips * 2)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        }
    }

    pub fn set(&self, strip: usize, left: f32, right: f32) {
        let i = strip * 2;
        if i + 1 >= self.peaks.len() {
            return;
        }
        self.peaks[i].store(left.to_bits(), Ordering::Relaxed);
        self.peaks[i + 1].store(right.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self, strip: usize) -> (f32, f32) {
        let i = strip * 2;
        if i + 1 >= self.peaks.len() {
            return (0.0, 0.0);
        }
        (
            f32::from_bits(self.peaks[i].load(Ordering::Relaxed)),
            f32::from_bits(self.peaks[i + 1].load(Ordering::Relaxed)),
        )
    }
}

pub struct Monitor {
    playheads: Mutex<HashMap<String, Arc<AtomicU32>>>,
    meters: Mutex<HashMap<String, Arc<AtomicU32>>>,
    mixers: Mutex<HashMap<String, Arc<MixLevels>>>,
    scopes: Mutex<HashMap<String, Arc<ScopeBuf>>>,
    slices: Mutex<HashMap<String, Arc<SliceBuf>>>,
    gonios: Mutex<HashMap<String, Arc<GonioBuf>>>,
    ffts: Mutex<HashMap<String, Arc<FftBuf>>>,
    notes: Mutex<HashMap<String, Arc<PitchSet>>>,
    song_beats: AtomicU64,
    viz_gonio: Arc<GonioBuf>,
    viz_waves: Mutex<HashMap<String, Arc<ScopeBuf>>>,
    horizon: Mutex<Arc<Vec<crate::viz::Note>>>,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            playheads: Mutex::new(HashMap::new()),
            meters: Mutex::new(HashMap::new()),
            mixers: Mutex::new(HashMap::new()),
            scopes: Mutex::new(HashMap::new()),
            slices: Mutex::new(HashMap::new()),
            gonios: Mutex::new(HashMap::new()),
            ffts: Mutex::new(HashMap::new()),
            notes: Mutex::new(HashMap::new()),
            song_beats: AtomicU64::new(0),
            viz_gonio: Arc::new(GonioBuf::new()),
            viz_waves: Mutex::new(HashMap::new()),
            horizon: Mutex::new(Arc::new(Vec::new())),
        }
    }
}

impl Monitor {
    pub fn set_song_beats(&self, beats: f64) {
        self.song_beats.store(beats.to_bits(), Ordering::Relaxed);
    }

    pub fn song_beats(&self) -> f64 {
        f64::from_bits(self.song_beats.load(Ordering::Relaxed))
    }

    pub fn playhead_slot(&self, id: &str) -> Arc<AtomicU32> {
        let mut m = self.playheads.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(AtomicU32::new(0)))
            .clone()
    }

    pub fn playhead(&self, id: &str) -> Option<f32> {
        let m = self.playheads.lock().ok()?;
        m.get(id).map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
    }

    pub fn meter_slot(&self, id: &str) -> Arc<AtomicU32> {
        let mut m = self.meters.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(AtomicU32::new(0)))
            .clone()
    }

    pub fn meter_value(&self, id: &str) -> f32 {
        let m = self.meters.lock().unwrap_or_else(|e| e.into_inner());
        m.get(id)
            .map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
    }

    pub fn mix_levels(&self, id: &str, strips: usize) -> Arc<MixLevels> {
        let mut m = self.mixers.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(MixLevels::new(strips)))
            .clone()
    }

    pub fn mix_strip(&self, id: &str, strip: usize) -> (f32, f32) {
        let m = self.mixers.lock().unwrap_or_else(|e| e.into_inner());
        m.get(id).map(|s| s.get(strip)).unwrap_or((0.0, 0.0))
    }

    pub fn scope_buf(&self, id: &str) -> Arc<ScopeBuf> {
        let mut m = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(ScopeBuf::new()))
            .clone()
    }

    pub fn scope_samples(&self, id: &str) -> Vec<f32> {
        let buf = {
            let m = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
            m.get(id).cloned()
        };
        buf.map(|b| b.snapshot()).unwrap_or_default()
    }

    pub fn slice_buf(&self, id: &str) -> Arc<SliceBuf> {
        let mut m = self.slices.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(SliceBuf::new()))
            .clone()
    }

    pub fn slice_peaks(&self, id: &str) -> Vec<f32> {
        let buf = {
            let m = self.slices.lock().unwrap_or_else(|e| e.into_inner());
            m.get(id).cloned()
        };
        buf.map(|b| b.snapshot()).unwrap_or_default()
    }

    pub fn slice_fill(&self, id: &str) -> f32 {
        let buf = {
            let m = self.slices.lock().unwrap_or_else(|e| e.into_inner());
            m.get(id).cloned()
        };
        buf.map(|b| b.fill()).unwrap_or(0.0)
    }

    pub fn gonio_buf(&self, id: &str) -> Arc<GonioBuf> {
        let mut m = self.gonios.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(GonioBuf::new()))
            .clone()
    }

    pub fn gonio_view(&self, id: &str) -> (Vec<f32>, f32) {
        let buf = {
            let m = self.gonios.lock().unwrap_or_else(|e| e.into_inner());
            m.get(id).cloned()
        };
        match buf {
            Some(b) => {
                let (l, r) = b.snapshot();
                gonio_field(&l, &r)
            }
            None => (vec![0.0; GONIO_BINS * GONIO_BINS], 0.0),
        }
    }

    pub fn fft_buf(&self, id: &str) -> Arc<FftBuf> {
        let mut m = self.ffts.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(FftBuf::new()))
            .clone()
    }

    pub fn spectrum(&self, id: &str) -> Vec<f32> {
        let buf = {
            let m = self.ffts.lock().unwrap_or_else(|e| e.into_inner());
            m.get(id).cloned()
        };
        buf.map(|b| b.spectrum()).unwrap_or_default()
    }

    pub fn spectrogram(&self, id: &str) -> Vec<f32> {
        let buf = {
            let m = self.ffts.lock().unwrap_or_else(|e| e.into_inner());
            m.get(id).cloned()
        };
        buf.map(|b| b.spectrogram()).unwrap_or_default()
    }

    pub fn note_slot(&self, id: &str) -> Arc<PitchSet> {
        let mut m = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(PitchSet::new()))
            .clone()
    }

    pub fn sounding_notes(&self, id: &str) -> HashSet<u8> {
        let slot = {
            let m = self.notes.lock().unwrap_or_else(|e| e.into_inner());
            m.get(id).cloned()
        };
        slot.map(|s| s.load()).unwrap_or_default()
    }

    pub fn viz_gonio(&self) -> Arc<GonioBuf> {
        self.viz_gonio.clone()
    }

    pub fn viz_gonio_pairs(&self) -> Vec<[f32; 2]> {
        let (l, r) = self.viz_gonio.snapshot_recent(768);
        l.into_iter().zip(r).map(|(a, b)| [a, b]).collect()
    }

    pub fn reset_viz_waves(&self) {
        self.viz_waves
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn viz_wave_buf(&self, id: &str) -> Arc<ScopeBuf> {
        let mut m = self.viz_waves.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(ScopeBuf::new()))
            .clone()
    }

    pub fn viz_waves(&self, notes: &[crate::viz::Note]) -> Vec<crate::viz::Wave> {
        use crate::viz::{MAX_WAVES, WAVE_BINS};
        let map = self.viz_waves.lock().unwrap_or_else(|e| e.into_inner());
        if map.is_empty() {
            return Vec::new();
        }
        let mut color_of: HashMap<String, [f32; 4]> = HashMap::new();
        for n in notes {
            if !n.wave.is_empty() {
                color_of.entry(n.wave.clone()).or_insert(n.color);
            }
        }
        let mut preferred: Vec<String> = notes
            .iter()
            .map(|n| n.wave.clone())
            .filter(|id| !id.is_empty() && map.contains_key(id))
            .collect();
        preferred.sort();
        preferred.dedup();
        let mut extra: Vec<String> = map
            .keys()
            .filter(|k| !preferred.contains(k))
            .cloned()
            .collect();
        extra.sort();
        preferred.extend(extra);
        preferred.truncate(MAX_WAVES);
        preferred
            .into_iter()
            .filter_map(|id| {
                let buf = map.get(&id)?;
                let color = color_of.get(&id).copied().unwrap_or(wave_palette(&id));
                Some(crate::viz::Wave {
                    color,
                    samples: crate::viz::downsample(&buf.snapshot(), WAVE_BINS),
                })
            })
            .collect()
    }

    pub fn set_horizon(&self, notes: Vec<crate::viz::Note>) {
        let mut h = self.horizon.lock().unwrap_or_else(|e| e.into_inner());
        *h = Arc::new(notes);
    }

    pub fn horizon(&self) -> Arc<Vec<crate::viz::Note>> {
        self.horizon
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

fn wave_palette(id: &str) -> [f32; 4] {
    const PAL: [[f32; 4]; 7] = [
        [0.28, 0.62, 0.92, 1.0],
        [0.92, 0.62, 0.22, 1.0],
        [0.72, 0.38, 0.92, 1.0],
        [0.92, 0.82, 0.22, 1.0],
        [0.22, 0.78, 0.72, 1.0],
        [0.92, 0.38, 0.55, 1.0],
        [0.55, 0.72, 0.28, 1.0],
    ];
    let mut h = 0u64;
    for b in id.as_bytes() {
        h = h.wrapping_mul(16777619) ^ *b as u64;
    }
    PAL[(h as usize) % PAL.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fft::SPEC_BINS;

    fn sine(period: usize, phase: usize, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = (i + phase) as f32 / period as f32;
                (t * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    #[test]
    fn spectrogram_pads_left_newest_right() {
        let b = FftBuf::new();
        let mut a = vec![0.0; SPEC_BINS];
        a[0] = 1.0;
        b.push_bins(&a);
        let mut c = vec![0.0; SPEC_BINS];
        c[3] = 1.0;
        b.push_bins(&c);
        let g = b.spectrogram();
        let last = (SPEC_COLS - 1) * SPEC_BINS;
        let prev = (SPEC_COLS - 2) * SPEC_BINS;
        assert_eq!(g[last + 3], 1.0);
        assert_eq!(g[prev], 1.0);
        assert_eq!(g[0], 0.0);
    }

    #[test]
    fn trigger_locks_phase_across_offsets() {
        let a = trigger_wave(&sine(64, 3, 512));
        let b = trigger_wave(&sine(64, 41, 512));
        assert_eq!(a.len(), 256);
        assert!(a[0] > 0.0 && a[0] < 0.12);
        assert!(a[1] > a[0]);
        let mean: f32 = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32;
        assert!(mean < 0.02, "mean abs err {mean}");
    }

    #[test]
    fn trigger_falls_back_without_zero_cross() {
        let v: Vec<f32> = (0..64).map(|i| 0.2 + i as f32 * 0.01).collect();
        let out = trigger_wave(&v);
        assert_eq!(out, v[..32].to_vec());
    }

    #[test]
    fn gonio_mono_is_center_column() {
        let n = 64;
        let left: Vec<f32> = (0..n).map(|i| ((i as f32) / 8.0).sin()).collect();
        let (cells, corr) = gonio_field(&left, &left);
        assert!(corr > 0.99);
        let mid = GONIO_BINS / 2;
        let mut center = 0.0;
        let mut sides = 0.0;
        for c in 0..GONIO_BINS {
            for r in 0..GONIO_BINS {
                let v = cells[c * GONIO_BINS + r];
                if c == mid {
                    center += v;
                } else {
                    sides += v;
                }
            }
        }
        assert!(center > sides * 4.0, "center {center} sides {sides}");
    }

    #[test]
    fn gonio_antiphase_is_center_row() {
        let n = 64;
        let left: Vec<f32> = (0..n).map(|i| ((i as f32) / 8.0).sin()).collect();
        let right: Vec<f32> = left.iter().map(|v| -v).collect();
        let (cells, corr) = gonio_field(&left, &right);
        assert!(corr < -0.99);
        let mid = GONIO_BINS / 2;
        let mut center = 0.0;
        let mut rest = 0.0;
        for c in 0..GONIO_BINS {
            for r in 0..GONIO_BINS {
                let v = cells[c * GONIO_BINS + r];
                if r == mid {
                    center += v;
                } else {
                    rest += v;
                }
            }
        }
        assert!(center > rest * 4.0, "center {center} rest {rest}");
    }

    #[test]
    fn meter_slot_holds_last() {
        let m = Monitor::default();
        let slot = m.meter_slot("n1");
        slot.store(1.25f32.to_bits(), Ordering::Relaxed);
        assert!((m.meter_value("n1") - 1.25).abs() < 1e-6);
        assert_eq!(m.meter_value("missing"), 0.0);
    }

    #[test]
    fn mix_levels_store_lr() {
        let m = Monitor::default();
        let slot = m.mix_levels("mix", 8);
        slot.set(0, 0.5, 0.1);
        slot.set(3, 0.0, 0.8);
        assert_eq!(m.mix_strip("mix", 0), (0.5, 0.1));
        assert_eq!(m.mix_strip("mix", 3), (0.0, 0.8));
        assert_eq!(m.mix_strip("mix", 1), (0.0, 0.0));
        assert_eq!(m.mix_strip("missing", 0), (0.0, 0.0));
    }

    #[test]
    fn slice_publishes_only_when_told() {
        let b = SliceBuf::new();
        assert!(b.snapshot().iter().all(|v| *v == 0.0));
        let mut peaks = vec![0.0; SLICE_BINS * 2];
        peaks[0] = -0.4;
        peaks[1] = 0.7;
        b.publish(&peaks);
        b.set_fill(0.3);
        let snap = b.snapshot();
        assert!((snap[0] + 0.4).abs() < 1e-6);
        assert!((snap[1] - 0.7).abs() < 1e-6);
        assert!((b.fill() - 0.3).abs() < 1e-6);
        let mut next = vec![0.0; SLICE_BINS * 2];
        next[2] = -1.0;
        next[3] = 1.0;
        b.publish(&next);
        let snap = b.snapshot();
        assert_eq!(snap[0], 0.0);
        assert!((snap[2] + 1.0).abs() < 1e-6);
        assert!((b.fill() - 0.3).abs() < 1e-6);
    }
}
