//! Lock-free-ish UI meters: playhead bits and a sample ring for Scope nodes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
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

#[derive(Default)]
pub struct Monitor {
    playheads: Mutex<HashMap<String, Arc<AtomicU32>>>,
    scopes: Mutex<HashMap<String, Arc<ScopeBuf>>>,
}

impl Monitor {
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
}
