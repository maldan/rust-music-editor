use std::sync::Arc;

/// CPU snapshot for one visualizer frame.
///
/// Time is musical beats, not wall clock — the same struct can drive the
/// window and, later, an offline MP4 renderer.
#[derive(Clone, Debug)]
pub struct Frame {
    pub now_beats: f64,
    /// How far ahead of `now` is visible (beats). Future sits to the right.
    pub window_beats: f64,
    pub notes: Arc<Vec<Note>>,
    /// Downsampled L/R pairs for the goniometer, typically −1..1.
    pub gonio: Vec<[f32; 2]>,
    /// Per-instrument oscilloscope strips (bottom of the frame).
    pub waves: Vec<Wave>,
    /// Drop phosphor trails (seek / first frame).
    pub reset: bool,
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub credit: String,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            now_beats: 0.0,
            window_beats: 8.0,
            notes: Arc::new(Vec::new()),
            gonio: Vec::new(),
            waves: Vec::new(),
            reset: false,
            width: 1,
            height: 1,
            title: String::new(),
            credit: String::new(),
        }
    }
}

/// One sequenced note after graph routing (transpose / chord / arp).
///
/// `start` is loop-relative beats. Occurrences are unwrapped by the layout
/// pass using `loop_len`. `when` is song-time windows; empty means always.
#[derive(Clone, Debug)]
pub struct Note {
    pub pitch: u8,
    pub start: f64,
    pub dur: f64,
    pub loop_len: f64,
    pub color: [f32; 4],
    pub when: Vec<(f64, f64)>,
    /// Independent vertical band (sequence + note group).
    pub lane: u32,
    /// Editor id of the voice this note hits (waveform strip).
    pub wave: String,
}

impl Note {
    pub fn sounding_at(&self, song: f64) -> bool {
        if self.dur <= 1e-9 || self.loop_len <= 1e-9 {
            return false;
        }
        if !in_when(song, &self.when) {
            return false;
        }
        let start = self.start.rem_euclid(self.loop_len);
        let t = (song - start).rem_euclid(self.loop_len);
        t < self.dur
    }
}

/// One instrument waveform for the bottom strip row.
#[derive(Clone, Debug)]
pub struct Wave {
    pub color: [f32; 4],
    pub samples: Vec<f32>,
}

pub fn in_when(song: f64, when: &[(f64, f64)]) -> bool {
    if when.is_empty() {
        return true;
    }
    when.iter().any(|(a, b)| song >= *a && song < *b)
}

pub const WINDOW_BEATS: f64 = 8.0 / 1.3;
/// Keep notes until they have fully left the left edge.
pub const BEHIND_BEATS: f64 = 8.0 / 1.3;
/// Hit line from the left of the frame (0.5 = center).
pub const HIT_X: f32 = 0.35;
pub const HIT_W: f32 = 0.002;
pub const PRESS_IN: f64 = 0.08;
pub const PRESS_OUT: f64 = 0.16;
/// Beats after note-off to fade the played bar out.
pub const FADE_PAST: f64 = 2.4;
/// Fraction of the frame from the right edge used to fade incoming notes in.
pub const FADE_IN_X: f32 = 0.14;
/// Note piano-roll height; remainder is waveform strips.
pub const NOTES_H: f32 = 0.86;
/// Gap above the highest notes so they are not flush with the frame edge.
pub const NOTE_TOP: f32 = 0.048;
pub const MAX_WAVES: usize = 8;
pub const WAVE_BINS: usize = 256;
