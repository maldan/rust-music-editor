use glam::Vec2;

pub mod port {
    pub const AUDIO: u16 = 32;
    pub const CLOCK: u16 = 33;
    pub const NOTES: u16 = 34;
}

pub const SEQ_STEPS: u32 = 16;
pub const SEQ_PITCHES: u32 = 12;
pub const SEQ_BASE_PITCH: u8 = 60;
pub const SEQ_OCTAVE_MIN: i32 = 0;
pub const SEQ_OCTAVE_MAX: i32 = 8;
/// One sequencer cell = one 16th note (0.25 beat at 4/4).
pub const BEATS_PER_STEP: f32 = 0.25;
/// 16 sixteenths = 4 beats = 1 bar in 4/4.
pub const BEATS_PER_BAR: f32 = SEQ_STEPS as f32 * BEATS_PER_STEP;
pub const NOTE_JOIN_INS: [&str; 8] = ["1", "2", "3", "4", "5", "6", "7", "8"];
pub const MIX_INS: [&str; 8] = NOTE_JOIN_INS;
pub const MIX_VOL_INS: [&str; 8] = ["v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8"];
pub const MIX_PAN_INS: [&str; 8] = ["p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SeqNote {
    pub step: u32,
    pub pitch: u8,
    pub len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Osc,
    Lfo,
    Filter,
    Gain,
    Pan,
    Mix,
    Mixer,
    NoteJoin,
    Transpose,
    Chord,
    Arp,
    Delay,
    Distortion,
    Chorus,
    Flanger,
    Reverb,
    Compressor,
    Eq,
    TranceGate,
    Mul,
    Clamp,
    Remap,
    Value,
    Scope,
    Spectrum,
    Spectrogram,
    NoteScope,
    Clock,
    Sequencer,
    Instrument,
    Voice,
    Guitar,
    Input,
    AudioIn,
    Output,
}

impl NodeKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Osc => "Oscillator",
            Self::Lfo => "LFO",
            Self::Filter => "Filter",
            Self::Gain => "Gain",
            Self::Pan => "Pan",
            Self::Mix => "Join Audio",
            Self::Mixer => "Mixer",
            Self::NoteJoin => "Join Notes",
            Self::Transpose => "Transpose",
            Self::Chord => "Chord",
            Self::Arp => "Arp",
            Self::Delay => "Delay / Echo",
            Self::Distortion => "Distortion",
            Self::Chorus => "Chorus",
            Self::Flanger => "Flanger",
            Self::Reverb => "Reverb",
            Self::Compressor => "Comp / Limit",
            Self::Eq => "EQ Curve",
            Self::TranceGate => "Trance Gate",
            Self::Mul => "Multiply",
            Self::Clamp => "Clamp",
            Self::Remap => "Remap",
            Self::Value => "Value",
            Self::Scope => "Waveform",
            Self::Spectrum => "Spectrum",
            Self::Spectrogram => "Spectrogram",
            Self::NoteScope => "Notes",
            Self::Clock => "Clock",
            Self::Sequencer => "Sequencer",
            Self::Instrument => "Instrument",
            Self::Voice => "Basic Synth",
            Self::Guitar => "Guitar",
            Self::Input => "In",
            Self::AudioIn => "Audio In",
            Self::Output => "Output",
        }
    }

    pub fn can_delete(self) -> bool {
        !matches!(self, Self::Output | Self::Input)
    }

    /// Inspector bypass: skip processing / pass signal through.
    pub fn can_bypass(self) -> bool {
        !matches!(self, Self::Output | Self::Input | Self::NoteJoin)
    }
}

pub fn output_port_type(kind: NodeKind, port: &str) -> u16 {
    match (kind, port) {
        (NodeKind::Clock, "clock") | (NodeKind::Sequencer, "clock") => port::CLOCK,
        (NodeKind::Sequencer, "notes")
        | (NodeKind::Input, "notes")
        | (NodeKind::NoteJoin, "out")
        | (NodeKind::Transpose, "out")
        | (NodeKind::Chord, "out")
        | (NodeKind::Arp, "out")
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
    pub bypass: bool,
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
    /// Index into [`FILTER_NAMES`] (low / high / band / notch).
    #[serde(default)]
    pub filter_kind: usize,
    #[serde(default = "default_gain")]
    pub gain: f32,
    /// Pan: `-1` left, `0` center, `+1` right.
    #[serde(default)]
    pub pan: f32,
    #[serde(default = "default_drive")]
    pub drive: f32,
    #[serde(default = "default_pulse_width")]
    pub pulse_width: f32,
    /// Unison oscillator count (1 = one osc, no stack).
    #[serde(default = "default_unison")]
    pub unison: f32,
    /// Unison spread, cents (lowest to highest osc).
    #[serde(default)]
    pub detune: f32,
    /// Unison stereo width, 0 = center, 1 = hard L/R spread.
    #[serde(default)]
    pub unison_pan: f32,
    #[serde(default)]
    pub value: f32,
    #[serde(default = "default_gate_pattern")]
    pub gate_pattern: u16,
    #[serde(default = "default_gate_smooth")]
    pub gate_smooth: f32,
    /// 0 = dry, 1 = fully gated.
    #[serde(default = "default_gate_mix")]
    pub gate_mix: f32,
    /// Index into [`GATE_DIV_NAMES`] (1/1 … 1/32).
    #[serde(default = "default_gate_div")]
    pub gate_div: usize,
    #[serde(default = "default_clamp_min")]
    pub clamp_min: f32,
    #[serde(default = "default_clamp_max")]
    pub clamp_max: f32,
    #[serde(default = "default_map_in_min")]
    pub map_in_min: f32,
    #[serde(default = "default_map_in_max")]
    pub map_in_max: f32,
    #[serde(default = "default_map_out_min")]
    pub map_out_min: f32,
    #[serde(default = "default_map_out_max")]
    pub map_out_max: f32,
    #[serde(default = "default_delay_time")]
    pub delay_time: f32,
    #[serde(default = "default_delay_feedback")]
    pub delay_feedback: f32,
    #[serde(default = "default_delay_mix")]
    pub delay_mix: f32,
    #[serde(default = "default_chorus_rate")]
    pub chorus_rate: f32,
    #[serde(default = "default_chorus_depth")]
    pub chorus_depth: f32,
    #[serde(default = "default_chorus_mix")]
    pub chorus_mix: f32,
    #[serde(default = "default_flange_rate")]
    pub flange_rate: f32,
    #[serde(default = "default_flange_depth")]
    pub flange_depth: f32,
    #[serde(default = "default_flange_feedback")]
    pub flange_feedback: f32,
    #[serde(default = "default_flange_mix")]
    pub flange_mix: f32,
    #[serde(default = "default_rev_mix")]
    pub rev_mix: f32,
    #[serde(default = "default_rev_room")]
    pub rev_room: f32,
    #[serde(default = "default_rev_damp")]
    pub rev_damp: f32,
    #[serde(default = "default_comp_thresh")]
    pub comp_thresh: f32,
    #[serde(default = "default_comp_ratio")]
    pub comp_ratio: f32,
    #[serde(default = "default_comp_attack")]
    pub comp_attack: f32,
    #[serde(default = "default_comp_release")]
    pub comp_release: f32,
    #[serde(default = "default_comp_makeup")]
    pub comp_makeup: f32,
    #[serde(default = "default_mix")]
    pub mix_a: f32,
    #[serde(default = "default_mix")]
    pub mix_b: f32,
    /// 8 mixer strips (vol + pan). Empty on Join Audio / old files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mix_strips: Vec<MixStrip>,
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
    /// Sequence entity id for [`NodeKind::Sequencer`].
    #[serde(default)]
    pub seq_id: String,
    /// Instrument entity id for [`NodeKind::Instrument`].
    #[serde(default)]
    pub inst_id: String,
    #[serde(default)]
    pub notes: Vec<SeqNote>,
    #[serde(default)]
    pub transpose_notes: i32,
    #[serde(default)]
    pub transpose_octaves: i32,
    /// Time shift in sequencer cells (1 = one 16th). Fractions allowed.
    #[serde(default)]
    pub transpose_steps: f32,
    /// Index into [`CHORD_NAMES`].
    #[serde(default)]
    pub chord_kind: usize,
    #[serde(default)]
    pub arp_mode: usize,
    #[serde(default = "default_arp_rate")]
    pub arp_rate: f32,
    #[serde(default = "default_adsr_attack")]
    pub adsr_attack: f32,
    #[serde(default = "default_adsr_decay")]
    pub adsr_decay: f32,
    #[serde(default = "default_adsr_sustain")]
    pub adsr_sustain: f32,
    #[serde(default = "default_adsr_release")]
    pub adsr_release: f32,
    #[serde(default = "default_eq_pts")]
    pub eq_pts: Vec<EqPt>,
    /// Output / Audio In device name. Empty = system default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub audio_device: String,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EqPt {
    pub t: f32,
    pub v: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MixStrip {
    #[serde(default = "default_mix")]
    pub vol: f32,
    #[serde(default)]
    pub pan: f32,
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
fn default_drive() -> f32 {
    4.0
}
fn default_pulse_width() -> f32 {
    0.5
}
fn default_unison() -> f32 {
    1.0
}
fn default_gate_pattern() -> u16 {
    0x5555
}
fn default_gate_smooth() -> f32 {
    0.004
}
fn default_gate_mix() -> f32 {
    1.0
}
fn default_gate_div() -> usize {
    4
}
fn default_clamp_min() -> f32 {
    -1.0
}
fn default_clamp_max() -> f32 {
    1.0
}
fn default_map_in_min() -> f32 {
    -1.0
}
fn default_map_in_max() -> f32 {
    1.0
}
fn default_map_out_min() -> f32 {
    -1.0
}
fn default_map_out_max() -> f32 {
    1.0
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
fn default_chorus_rate() -> f32 {
    0.8
}
fn default_chorus_depth() -> f32 {
    0.35
}
fn default_chorus_mix() -> f32 {
    0.45
}
fn default_flange_rate() -> f32 {
    0.25
}
fn default_flange_depth() -> f32 {
    0.7
}
fn default_flange_feedback() -> f32 {
    0.55
}
fn default_flange_mix() -> f32 {
    0.5
}
fn default_rev_mix() -> f32 {
    0.35
}
fn default_rev_room() -> f32 {
    0.7
}
fn default_rev_damp() -> f32 {
    0.4
}
fn default_comp_thresh() -> f32 {
    0.35
}
fn default_comp_ratio() -> f32 {
    4.0
}
fn default_comp_attack() -> f32 {
    0.012
}
fn default_comp_release() -> f32 {
    0.12
}
fn default_comp_makeup() -> f32 {
    1.4
}
fn default_arp_rate() -> f32 {
    1.0
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
fn default_adsr_attack() -> f32 {
    0.01
}
fn default_adsr_decay() -> f32 {
    0.1
}
fn default_adsr_sustain() -> f32 {
    0.7
}
fn default_adsr_release() -> f32 {
    0.2
}
fn default_eq_pts() -> Vec<EqPt> {
    vec![EqPt { t: 0.0, v: 1.0 }, EqPt { t: 1.0, v: 1.0 }]
}

impl GraphNode {
    pub fn new(id: String, kind: NodeKind, pos: Vec2) -> Self {
        Self {
            id,
            kind,
            pos,
            bypass: false,
            waveform: match kind {
                NodeKind::Osc | NodeKind::Voice => 1,
                _ => 0,
            },
            freq: 220.0,
            lfo_rate: 5.0,
            lfo_depth: 12.0,
            cutoff: 1200.0,
            q: 0.8,
            filter_kind: 0,
            gain: 0.7,
            pan: 0.0,
            drive: 4.0,
            pulse_width: 0.5,
            unison: 1.0,
            detune: 0.0,
            unison_pan: 0.0,
            value: 0.0,
            gate_pattern: 0x5555,
            gate_smooth: 0.004,
            gate_mix: 1.0,
            gate_div: 4,
            clamp_min: -1.0,
            clamp_max: 1.0,
            map_in_min: -1.0,
            map_in_max: 1.0,
            map_out_min: -1.0,
            map_out_max: 1.0,
            delay_time: 0.28,
            delay_feedback: 0.42,
            delay_mix: 0.38,
            chorus_rate: 0.8,
            chorus_depth: 0.35,
            chorus_mix: 0.45,
            flange_rate: 0.25,
            flange_depth: 0.7,
            flange_feedback: 0.55,
            flange_mix: 0.5,
            rev_mix: 0.35,
            rev_room: 0.7,
            rev_damp: 0.4,
            comp_thresh: 0.35,
            comp_ratio: 4.0,
            comp_attack: 0.012,
            comp_release: 0.12,
            comp_makeup: 1.4,
            mix_a: 1.0,
            mix_b: 1.0,
            mix_strips: match kind {
                NodeKind::Mixer => vec![MixStrip { vol: 1.0, pan: 0.0 }; MIX_INS.len()],
                _ => Vec::new(),
            },
            bpm: 120.0,
            seq_when: String::new(),
            seq_start: 0.0,
            seq_bars: 0.0,
            seq_loop_bars: 1,
            seq_octave: match kind {
                NodeKind::NoteScope => 3,
                _ => 4,
            },
            seq_id: String::new(),
            inst_id: String::new(),
            notes: Vec::new(),
            transpose_notes: 0,
            transpose_octaves: 0,
            transpose_steps: 0.0,
            chord_kind: 0,
            arp_mode: 0,
            arp_rate: 1.0,
            adsr_attack: 0.01,
            adsr_decay: 0.1,
            adsr_sustain: 0.7,
            adsr_release: 0.2,
            eq_pts: default_eq_pts(),
            audio_device: String::new(),
        }
    }

    pub fn adsr_params(&self) -> (f32, f32, f32, f32) {
        (
            self.adsr_attack.clamp(0.001, 4.0),
            self.adsr_decay.clamp(0.001, 4.0),
            self.adsr_sustain.clamp(0.0, 1.0),
            self.adsr_release.clamp(0.001, 6.0),
        )
    }

    pub fn pitch_shift(&self) -> i32 {
        self.transpose_notes + self.transpose_octaves * 12
    }

    pub fn time_shift_beats(&self) -> f64 {
        self.transpose_steps as f64 * BEATS_PER_STEP as f64
    }

    pub fn chord_intervals(&self) -> &'static [i32] {
        chord_intervals(self.chord_kind)
    }

    pub fn arp_step_beats(&self) -> f64 {
        self.arp_rate.clamp(0.25, 8.0) as f64 * BEATS_PER_STEP as f64
    }

    pub fn gate_step_beats(&self) -> f64 {
        GATE_DIV_BEATS
            .get(self.gate_div.min(GATE_DIV_BEATS.len() - 1))
            .copied()
            .unwrap_or(0.25)
    }

    /// Root `pitch` (semitone offset) → staggered chord tones.
    pub fn arp_events(&self, pitch: i32, delay: f64) -> Vec<(i32, f64)> {
        let mut tones: Vec<i32> = self.chord_intervals().iter().map(|iv| pitch + iv).collect();
        match self.arp_mode {
            1 => tones.reverse(),
            2 => {
                let mut down = tones.clone();
                down.reverse();
                if down.len() > 1 {
                    down.remove(0);
                }
                tones.extend(down);
            }
            _ => {}
        }
        let step = self.arp_step_beats();
        tones
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p, delay + i as f64 * step))
            .collect()
    }

    /// Files saved while Join Audio was the 8-strip mixer keep `kind: mix` plus strips.
    pub fn migrate_mixer_kind(&mut self) {
        if self.kind == NodeKind::Mix && !self.mix_strips.is_empty() {
            self.kind = NodeKind::Mixer;
        }
        if self.kind == NodeKind::Mixer {
            self.ensure_mix_strips();
        }
    }

    pub fn mix_strip(&self, i: usize) -> MixStrip {
        if let Some(s) = self.mix_strips.get(i) {
            return *s;
        }
        MixStrip {
            vol: match i {
                0 => self.mix_a,
                1 => self.mix_b,
                _ => 1.0,
            },
            pan: 0.0,
        }
    }

    pub fn ensure_mix_strips(&mut self) {
        if self.mix_strips.len() == MIX_INS.len() {
            return;
        }
        let mut strips = vec![MixStrip { vol: 1.0, pan: 0.0 }; MIX_INS.len()];
        if self.mix_strips.is_empty() {
            strips[0].vol = self.mix_a;
            strips[1].vol = self.mix_b;
        } else {
            for (i, s) in self.mix_strips.iter().take(MIX_INS.len()).enumerate() {
                strips[i] = *s;
            }
        }
        self.mix_strips = strips;
    }

    pub fn eq_pairs(&self) -> Vec<(f32, f32)> {
        let mut pts: Vec<(f32, f32)> = self.eq_pts.iter().map(|p| (p.t, p.v)).collect();
        if pts.len() < 2 {
            pts = vec![(0.0, 1.0), (1.0, 1.0)];
        }
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    }

    pub fn loop_bars(&self) -> u32 {
        self.seq_loop_bars.max(1)
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

    pub fn toggle_note(&mut self, step: u32, pitch: u8) {
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

pub const CHORD_NAMES: [&str; 9] = [
    "Major", "Minor", "Sus2", "Sus4", "Maj7", "Min7", "Dom7", "Power", "Octave",
];

pub const ARP_NAMES: [&str; 3] = ["Up", "Down", "UpDown"];

pub const FILTER_NAMES: [&str; 4] = ["Low pass", "High pass", "Band pass", "Notch"];

/// Musical length of one Trance Gate step, synced to BPM.
pub const GATE_DIV_NAMES: [&str; 6] = ["1/1", "1/2", "1/4", "1/8", "1/16", "1/32"];
pub const GATE_DIV_BEATS: [f64; 6] = [4.0, 2.0, 1.0, 0.5, 0.25, 0.125];

pub fn chord_intervals(kind: usize) -> &'static [i32] {
    match kind {
        1 => &[0, 3, 7],
        2 => &[0, 2, 7],
        3 => &[0, 5, 7],
        4 => &[0, 4, 7, 11],
        5 => &[0, 3, 7, 10],
        6 => &[0, 4, 7, 10],
        7 => &[0, 7],
        8 => &[0, 12],
        _ => &[0, 4, 7],
    }
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
        assert_eq!(n.loop_bars(), 99);
        assert_eq!(n.view_octave(), SEQ_OCTAVE_MIN);
        let p = GraphNode::new("p".into(), NodeKind::NoteScope, Vec2::ZERO);
        assert_eq!(p.view_octaves(), 3);
        assert_eq!(p.view_octave(), 3);
        assert_eq!(p.view_pitch_count(), 36);
    }

    #[test]
    fn voice_adsr_defaults_and_clamp() {
        let mut n = GraphNode::new("v".into(), NodeKind::Voice, Vec2::ZERO);
        assert_eq!(n.adsr_params(), (0.01, 0.1, 0.7, 0.2));
        n.adsr_attack = 0.0;
        n.adsr_sustain = 2.0;
        n.adsr_release = 99.0;
        let (a, _, s, r) = n.adsr_params();
        assert!(a >= 0.001);
        assert_eq!(s, 1.0);
        assert_eq!(r, 6.0);
    }

    #[test]
    fn pulse_width_default() {
        let n = GraphNode::new("o".into(), NodeKind::Osc, Vec2::ZERO);
        assert!((n.pulse_width - 0.5).abs() < 1e-6);
        let v = GraphNode::new("v".into(), NodeKind::Voice, Vec2::ZERO);
        assert_eq!(v.detune, 0.0);
        assert_eq!(v.unison, 1.0);
        assert_eq!(v.unison_pan, 0.0);
        let num = GraphNode::new("k".into(), NodeKind::Value, Vec2::ZERO);
        assert_eq!(num.value, 0.0);
        let g = GraphNode::new("g".into(), NodeKind::TranceGate, Vec2::ZERO);
        assert_eq!(g.gate_pattern, 0x5555);
        assert!((g.gate_mix - 1.0).abs() < 1e-6);
        assert_eq!(g.gate_div, 4);
        assert!((g.gate_step_beats() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn distortion_drive_default() {
        let n = GraphNode::new("d".into(), NodeKind::Distortion, Vec2::ZERO);
        assert!((n.drive - 4.0).abs() < 1e-6);
        let c = GraphNode::new("c".into(), NodeKind::Chorus, Vec2::ZERO);
        assert!((c.chorus_rate - 0.8).abs() < 1e-6);
    }

    #[test]
    fn bypass_kinds() {
        assert!(NodeKind::Instrument.can_bypass());
        assert!(!NodeKind::Input.can_delete());
        assert!(NodeKind::Sequencer.can_bypass());
        assert!(NodeKind::Voice.can_bypass());
        assert!(NodeKind::Guitar.can_bypass());
        assert!(!NodeKind::Output.can_bypass());
        assert!(!NodeKind::Input.can_bypass());
        assert!(NodeKind::AudioIn.can_bypass());
        assert!(NodeKind::AudioIn.can_delete());
        assert!(!NodeKind::NoteJoin.can_bypass());
        assert!(NodeKind::Chord.can_bypass());
        assert!(NodeKind::Eq.can_bypass());
        assert!(NodeKind::Pan.can_bypass());
        assert!(NodeKind::Reverb.can_bypass());
        assert!(NodeKind::Compressor.can_bypass());
    }

    #[test]
    fn eq_defaults_flat_pass() {
        let n = GraphNode::new("eq".into(), NodeKind::Eq, Vec2::ZERO);
        let pts = n.eq_pairs();
        assert_eq!(pts.len(), 2);
        assert!((pts[0].1 - 1.0).abs() < 1e-6);
        assert!((pts[1].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn filter_defaults_lowpass() {
        let n = GraphNode::new("f".into(), NodeKind::Filter, Vec2::ZERO);
        assert_eq!(n.filter_kind, 0);
        assert_eq!(FILTER_NAMES[1], "High pass");
    }

    #[test]
    fn chord_intervals_major_minor() {
        let mut n = GraphNode::new("c".into(), NodeKind::Chord, Vec2::ZERO);
        assert_eq!(n.chord_intervals(), &[0, 4, 7]);
        n.chord_kind = 1;
        assert_eq!(n.chord_intervals(), &[0, 3, 7]);
        n.chord_kind = 6;
        assert_eq!(n.chord_intervals(), &[0, 4, 7, 10]);
    }

    #[test]
    fn arp_staggers_major_up() {
        let n = GraphNode::new("a".into(), NodeKind::Arp, Vec2::ZERO);
        let ev = n.arp_events(0, 0.0);
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[0], (0, 0.0));
        assert_eq!(ev[1].0, 4);
        assert!((ev[1].1 - BEATS_PER_STEP as f64).abs() < 1e-9);
        assert_eq!(ev[2].0, 7);
    }

    #[test]
    fn mix_strips_fill_from_legacy_ab() {
        let mut n = GraphNode::new("m".into(), NodeKind::Mixer, Vec2::ZERO);
        n.mix_strips.clear();
        n.mix_a = 0.25;
        n.mix_b = 0.75;
        n.ensure_mix_strips();
        assert_eq!(n.mix_strips.len(), 8);
        assert!((n.mix_strips[0].vol - 0.25).abs() < 1e-6);
        assert!((n.mix_strips[1].vol - 0.75).abs() < 1e-6);
        assert!((n.mix_strips[0].pan).abs() < 1e-6);
    }

    #[test]
    fn mix_without_strips_stays_join() {
        let mut n = GraphNode::new("j".into(), NodeKind::Mix, Vec2::ZERO);
        assert!(n.mix_strips.is_empty());
        n.migrate_mixer_kind();
        assert_eq!(n.kind, NodeKind::Mix);
        n.mix_strips = vec![MixStrip { vol: 1.0, pan: -0.5 }; 8];
        n.migrate_mixer_kind();
        assert_eq!(n.kind, NodeKind::Mixer);
    }
}
