use glam::Vec2;

pub mod port {
    pub const AUDIO: u16 = 32;
    pub const CLOCK: u16 = 33;
    pub const NOTES: u16 = 34;
}

pub const SEQ_STEPS: u32 = 16;
pub const SEQ_PITCHES: u32 = 12;
pub const SEQ_BASE_PITCH: u8 = 60;
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
    Delay,
    Scope,
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
            Self::Delay => "Delay / Echo",
            Self::Scope => "Waveform",
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
        (NodeKind::Sequencer, "notes") | (NodeKind::NoteJoin, "out") => port::NOTES,
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
    #[serde(default)]
    pub seq_start: f32,
    #[serde(default)]
    pub seq_bars: f32,
    #[serde(default)]
    pub notes: Vec<SeqNote>,
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
            seq_start: 0.0,
            seq_bars: 0.0,
            notes: Vec::new(),
        }
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
