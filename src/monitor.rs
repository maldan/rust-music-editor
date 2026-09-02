//! Lock-free-ish UI meters: playhead bits and a sample ring for Scope nodes.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const SCOPE_LEN: usize = 512;

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
    scopes: Mutex<HashMap<String, Arc<ScopeBuf>>>,
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
