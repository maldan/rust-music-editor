use glam::Vec2;

pub mod port {
    pub const AUDIO: u16 = 32;
    pub const CLOCK: u16 = 33;
    pub const NOTES: u16 = 34;
}

pub const SEQ_STEPS: u32 = 16;
pub const SEQ_PITCHES: u32 = 12;
pub const SEQ_BASE_PITCH: u8 = 60;
pub const SEQ_MAX_BARS: u32 = 8;
pub const SEQ_OCTAVE_MIN: i32 = 0;
pub const SEQ_OCTAVE_MAX: i32 = 8;
/// One sequencer cell = one 16th note (0.25 beat at 4/4).
pub const BEATS_PER_STEP: f32 = 0.25;
/// 16 sixteenths = 4 beats = 1 bar in 4/4.
pub const BEATS_PER_BAR: f32 = SEQ_STEPS as f32 * BEATS_PER_STEP;
pub const NOTE_JOIN_INS: [&str; 8] = ["1", "2", "3", "4", "5", "6", "7", "8"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SeqNote {
    pub step: u8,
    pub pitch: u8,
    pub len: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Osc,
    Lfo,
    Filter,
    Gain,
    Mix,
    NoteJoin,
    Transpose,
    Delay,
    Scope,
    NoteScope,
    Clock,
    Sequencer,
    Voice,
    Output,
}

impl NodeKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Osc => "Oscillator",
            Self::Lfo => "LFO",
            Self::Filter => "Filter",
            Self::Gain => "Gain",
            Self::Mix => "Join Audio",
            Self::NoteJoin => "Join Notes",
            Self::Transpose => "Transpose",
            Self::Delay => "Delay / Echo",
            Self::Scope => "Waveform",
            Self::NoteScope => "Notes",
            Self::Clock => "Clock",
            Self::Sequencer => "Sequencer",
            Self::Voice => "Voice",
            Self::Output => "Output",
        }
    }

    pub fn can_delete(self) -> bool {
        !matches!(self, Self::Output)
    }
}

pub fn output_port_type(kind: NodeKind, port: &str) -> u16 {
    match (kind, port) {
        (NodeKind::Clock, "clock") | (NodeKind::Sequencer, "clock") => port::CLOCK,
        (NodeKind::Sequencer, "notes")
        | (NodeKind::NoteJoin, "out")
        | (NodeKind::Transpose, "out")
        | (NodeKind::NoteScope, "out") => port::NOTES,
        _ => port::AUDIO,
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub pos: Vec2,
    #[serde(default)]
    pub waveform: usize,
    #[serde(default = "default_freq")]
    pub freq: f32,
    #[serde(default = "default_lfo_rate")]
    pub lfo_rate: f32,
    #[serde(default = "default_lfo_depth")]
    pub lfo_depth: f32,
    #[serde(default = "default_cutoff")]
    pub cutoff: f32,
    #[serde(default = "default_q")]
    pub q: f32,
    #[serde(default = "default_gain")]
    pub gain: f32,
    #[serde(default = "default_delay_time")]
    pub delay_time: f32,
    #[serde(default = "default_delay_feedback")]
    pub delay_feedback: f32,
    #[serde(default = "default_delay_mix")]
    pub delay_mix: f32,
    #[serde(default = "default_mix")]
    pub mix_a: f32,
    #[serde(default = "default_mix")]
    pub mix_b: f32,
    #[serde(default = "default_bpm")]
    pub bpm: f32,
    /// Play windows: `1 3 8` (one bar each) or `1-2 5-1` (start-length). Empty = always.
    /// Bar numbers are 1-based. Length `0` means from that bar onward.
    #[serde(default)]
    pub seq_when: String,
    #[serde(default, skip_serializing)]
    pub seq_start: f32,
    #[serde(default, skip_serializing)]
    pub seq_bars: f32,
    /// Length of the piano-roll loop, in bars (1..=8).
    #[serde(default = "default_seq_loop_bars")]
    pub seq_loop_bars: u32,
    /// Visible piano-roll octave (C0..=C8).
    #[serde(default = "default_seq_octave")]
    pub seq_octave: i32,
    #[serde(default)]
    pub notes: Vec<SeqNote>,
    #[serde(default)]
    pub transpose_notes: i32,
    #[serde(default)]
    pub transpose_octaves: i32,
    /// Time shift in sequencer cells (1 = one 16th). Fractions allowed.
    #[serde(default)]
    pub transpose_steps: f32,
}

fn default_freq() -> f32 {
    220.0
}
fn default_lfo_rate() -> f32 {
    5.0
}
fn default_lfo_depth() -> f32 {
    12.0
}
fn default_cutoff() -> f32 {
    1200.0
}
fn default_q() -> f32 {
    0.8
}
fn default_gain() -> f32 {
    0.7
}
fn default_delay_time() -> f32 {
    0.28
}
fn default_delay_feedback() -> f32 {
    0.42
}
fn default_delay_mix() -> f32 {
    0.38
}
fn default_mix() -> f32 {
    1.0
}
fn default_bpm() -> f32 {
    120.0
}
fn default_seq_loop_bars() -> u32 {
    1
}
fn default_seq_octave() -> i32 {
    4
}

impl GraphNode {
    pub fn new(id: String, kind: NodeKind, pos: Vec2) -> Self {
        Self {
            id,
            kind,
            pos,
            waveform: match kind {
                NodeKind::Osc | NodeKind::Voice => 1,
                _ => 0,
            },
            freq: 220.0,
            lfo_rate: 5.0,
            lfo_depth: 12.0,
            cutoff: 1200.0,
            q: 0.8,
            gain: 0.7,
            delay_time: 0.28,
            delay_feedback: 0.42,
            delay_mix: 0.38,
            mix_a: 1.0,
            mix_b: 1.0,
            bpm: 120.0,
            seq_when: String::new(),
            seq_start: 0.0,
            seq_bars: 0.0,
            seq_loop_bars: 1,
            seq_octave: match kind {
                NodeKind::NoteScope => 3,
                _ => 4,
            },
            notes: Vec::new(),
            transpose_notes: 0,
            transpose_octaves: 0,
            transpose_steps: 0.0,
        }
    }

    pub fn pitch_shift(&self) -> i32 {
        self.transpose_notes + self.transpose_octaves * 12
    }

    pub fn time_shift_beats(&self) -> f64 {
        self.transpose_steps as f64 * BEATS_PER_STEP as f64
    }

    pub fn loop_bars(&self) -> u32 {
        self.seq_loop_bars.clamp(1, SEQ_MAX_BARS)
    }

    pub fn loop_steps(&self) -> u32 {
        SEQ_STEPS * self.loop_bars()
    }

    pub fn loop_beats(&self) -> f64 {
        self.loop_bars() as f64 * BEATS_PER_BAR as f64
    }

    pub fn view_octave(&self) -> i32 {
        self.seq_octave.clamp(SEQ_OCTAVE_MIN, self.view_octave_max())
    }

    pub fn view_octaves(&self) -> u32 {
        match self.kind {
            NodeKind::NoteScope => 3,
            _ => 1,
        }
    }

    pub fn view_octave_max(&self) -> i32 {
        (SEQ_OCTAVE_MAX - self.view_octaves() as i32 + 1).max(SEQ_OCTAVE_MIN)
    }

    pub fn view_pitch_count(&self) -> u32 {
        12 * self.view_octaves()
    }

    /// MIDI pitch of C at the bottom of the visible range (C4 = 60).
    pub fn view_base_pitch(&self) -> u8 {
        ((self.view_octave() + 1) * 12) as u8
    }

    /// Lift old `seq_start` / `seq_bars` into `seq_when` after JSON load.
    pub fn migrate_seq_when(&mut self) {
        if !self.seq_when.trim().is_empty() {
            return;
        }
        let start = self.seq_start.max(0.0).floor() as i32;
        let bars = self.seq_bars.max(0.0).floor() as i32;
        self.seq_when = if start == 0 && bars == 0 {
            String::new()
        } else if bars == 0 {
            format!("{}-0", start + 1)
        } else {
            format!("{}-{}", start + 1, bars)
        };
    }

    pub fn toggle_note(&mut self, step: u8, pitch: u8) {
        if let Some(i) = self
            .notes
            .iter()
            .position(|n| n.step == step && n.pitch == pitch)
        {
            self.notes.remove(i);
        } else {
            self.notes.push(SeqNote {
                step,
                pitch,
                len: 1,
            });
        }
    }
}

pub fn midi_shift(pitch: u8, semitones: i32) -> u8 {
    (pitch as i32 + semitones).clamp(0, 127) as u8
}

/// Beat ranges `[start, end)`. Empty means always (from beat 0).
pub fn parse_seq_when(src: &str) -> Vec<(f64, f64)> {
    let bar = BEATS_PER_BAR as f64;
    let mut out = Vec::new();
    for raw in src.split(|c: char| c.is_whitespace() || c == ',') {
        if raw.is_empty() {
            continue;
        }
        let (start_s, len_s) = match raw.split_once('-') {
            Some((a, b)) => (a, b),
            None => (raw, "1"),
        };
        let Ok(start_bar) = start_s.parse::<i32>() else {
            continue;
        };
        if start_bar < 1 {
            continue;
        }
        let len_bars = if len_s.is_empty() {
            0
        } else {
            match len_s.parse::<i32>() {
                Ok(n) if n >= 0 => n,
                _ => continue,
            }
        };
        let start = (start_bar - 1) as f64 * bar;
        let end = if len_bars == 0 {
            f64::INFINITY
        } else {
            start + len_bars as f64 * bar
        };
        out.push((start, end));
    }
    out
}

pub fn seq_window(windows: &[(f64, f64)], song: f64) -> Option<(f64, f64)> {
    if windows.is_empty() {
        return Some((0.0, f64::INFINITY));
    }
    windows
        .iter()
        .copied()
        .find(|(start, end)| song >= *start && song < *end)
}

pub fn parse_tick(src: &str) -> i32 {
    src.split(|c: char| c.is_whitespace() || c == ',')
        .find_map(|tok| tok.parse::<i32>().ok().filter(|&n| n >= 1))
        .unwrap_or(1)
}

pub fn tick_to_beats(tick: i32) -> f64 {
    (tick.max(1) - 1) as f64 * BEATS_PER_BAR as f64
}

pub fn beats_to_tick(beats: f64) -> i32 {
    (beats.max(0.0) / BEATS_PER_BAR as f64).floor() as i32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beats(bar: i32) -> f64 {
        (bar - 1) as f64 * BEATS_PER_BAR as f64
    }

    #[test]
    fn empty_when_is_always() {
        assert!(parse_seq_when("").is_empty());
        assert!(parse_seq_when("   ").is_empty());
    }

    #[test]
    fn list_is_one_bar_each() {
        assert_eq!(
            parse_seq_when("1 2 5 1"),
            vec![
                (beats(1), beats(2)),
                (beats(2), beats(3)),
                (beats(5), beats(6)),
                (beats(1), beats(2)),
            ]
        );
    }

    #[test]
    fn start_len_pairs() {
        assert_eq!(
            parse_seq_when("1-2 5-1"),
            vec![(beats(1), beats(3)), (beats(5), beats(6))]
        );
    }

    #[test]
    fn zero_len_is_open_ended() {
        assert_eq!(parse_seq_when("3-0"), vec![(beats(3), f64::INFINITY)]);
        assert_eq!(parse_seq_when("3-"), vec![(beats(3), f64::INFINITY)]);
    }

    #[test]
    fn commas_and_junk() {
        assert_eq!(
            parse_seq_when("1-2, foo, 5"),
            vec![(beats(1), beats(3)), (beats(5), beats(6))]
        );
    }

    #[test]
    fn migrate_legacy_fields() {
        let mut n = GraphNode::new("n1".into(), NodeKind::Sequencer, Vec2::ZERO);
        n.seq_start = 2.0;
        n.seq_bars = 4.0;
        n.migrate_seq_when();
        assert_eq!(n.seq_when, "3-4");
        n.migrate_seq_when();
        assert_eq!(n.seq_when, "3-4");
    }

    #[test]
    fn toggle_note_is_one_step() {
        let mut n = GraphNode::new("n1".into(), NodeKind::Sequencer, Vec2::ZERO);
        n.toggle_note(3, 60);
        assert_eq!(n.notes, vec![SeqNote { step: 3, pitch: 60, len: 1 }]);
        n.toggle_note(3, 60);
        assert!(n.notes.is_empty());
    }

    #[test]
    fn tick_field_is_cue_not_clock() {
        assert_eq!(parse_tick(""), 1);
        assert_eq!(parse_tick("2"), 2);
        assert_eq!(parse_tick("  5 extra"), 5);
        assert_eq!(parse_tick("0"), 1);
        assert_eq!(tick_to_beats(1), 0.0);
        assert_eq!(tick_to_beats(2), BEATS_PER_BAR as f64);
        assert_eq!(beats_to_tick(0.0), 1);
        assert_eq!(beats_to_tick(BEATS_PER_BAR as f64), 2);
    }

    #[test]
    fn seq_window_picks_first_match() {
        let bar = BEATS_PER_BAR as f64;
        let wins = vec![(0.0, bar * 2.0), (bar * 4.0, bar * 5.0)];
        assert_eq!(seq_window(&wins, 0.0), Some((0.0, bar * 2.0)));
        assert_eq!(seq_window(&wins, bar * 1.5), Some((0.0, bar * 2.0)));
        assert_eq!(seq_window(&wins, bar * 2.0), None);
        assert_eq!(seq_window(&wins, bar * 4.0), Some((bar * 4.0, bar * 5.0)));
        assert_eq!(seq_window(&[], 99.0), Some((0.0, f64::INFINITY)));
    }

    #[test]
    fn transpose_c4() {
        let mut n = GraphNode::new("t".into(), NodeKind::Transpose, Vec2::ZERO);
        n.transpose_notes = 1;
        assert_eq!(midi_shift(60, n.pitch_shift()), 61);
        n.transpose_notes = 0;
        n.transpose_octaves = 1;
        assert_eq!(midi_shift(60, n.pitch_shift()), 72);
        n.transpose_notes = 1;
        assert_eq!(midi_shift(60, n.pitch_shift()), 73);
        n.transpose_steps = 0.5;
        assert!((n.time_shift_beats() - BEATS_PER_STEP as f64 * 0.5).abs() < 1e-9);
    }

    #[test]
    fn loop_and_octave() {
        let mut n = GraphNode::new("s".into(), NodeKind::Sequencer, Vec2::ZERO);
        assert_eq!(n.loop_steps(), SEQ_STEPS);
        assert_eq!(n.view_base_pitch(), SEQ_BASE_PITCH);
        n.seq_loop_bars = 3;
        n.seq_octave = 5;
        assert_eq!(n.loop_steps(), SEQ_STEPS * 3);
        assert_eq!(n.view_base_pitch(), 72);
        n.seq_loop_bars = 99;
        n.seq_octave = -3;
        assert_eq!(n.loop_bars(), SEQ_MAX_BARS);
        assert_eq!(n.view_octave(), SEQ_OCTAVE_MIN);
        let p = GraphNode::new("p".into(), NodeKind::NoteScope, Vec2::ZERO);
        assert_eq!(p.view_octaves(), 3);
        assert_eq!(p.view_octave(), 3);
        assert_eq!(p.view_pitch_count(), 36);
    }
}
