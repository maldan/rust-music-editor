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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SeqNote {
    pub step: u8,
    pub pitch: u8,
    pub len: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
        (NodeKind::Clock, "clock") => port::CLOCK,
        (NodeKind::Sequencer, "notes") | (NodeKind::NoteJoin, "out") => port::NOTES,
        _ => port::AUDIO,
    }
}

#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub pos: Vec2,
    pub waveform: usize,
    pub freq: f32,
    pub lfo_rate: f32,
    pub lfo_depth: f32,
    pub cutoff: f32,
    pub q: f32,
    pub gain: f32,
    pub delay_time: f32,
    pub delay_feedback: f32,
    pub delay_mix: f32,
    pub mix_a: f32,
    pub mix_b: f32,
    pub bpm: f32,
    pub seq_start: f32,
    pub seq_bars: f32,
    pub notes: Vec<SeqNote>,
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
