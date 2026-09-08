//! Lock-free-ish UI meters: playhead bits and a sample ring for Scope nodes.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::fft::{SPEC_BINS, SPEC_COLS};

pub const SCOPE_LEN: usize = 2048;

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

#[derive(Default)]
pub struct Monitor {
    playheads: Mutex<HashMap<String, Arc<AtomicU32>>>,
    meters: Mutex<HashMap<String, Arc<AtomicU32>>>,
    scopes: Mutex<HashMap<String, Arc<ScopeBuf>>>,
    ffts: Mutex<HashMap<String, Arc<FftBuf>>>,
    notes: Mutex<HashMap<String, Arc<PitchSet>>>,
    song_beats: AtomicU64,
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
    fn meter_slot_holds_last() {
        let m = Monitor::default();
        let slot = m.meter_slot("n1");
        slot.store(1.25f32.to_bits(), Ordering::Relaxed);
        assert!((m.meter_value("n1") - 1.25).abs() < 1e-6);
        assert_eq!(m.meter_value("missing"), 0.0);
    }
}
