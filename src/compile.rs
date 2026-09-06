//! Editor graph → mega-audio graph, plus sequencer tick on the audio thread.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use mega_audio::dsp::{
    AdsrParams, BiquadFilter, Chorus, Clamp, Compressor, Delay, Distortion, FilterKind, Flanger,
    GainCv, Mul, Oscillator, Remap, Reverb, StereoGain, StereoJoin, StereoMixer, StereoPan,
    TranceGate, Waveform, Const,
};
use mega_audio::graph::{Bypass, Graph, Node, NodeId, ProcessContext};
use mega_audio::instrument::{KarplusStrong, PolyphonicInstrument};
use mega_audio::note::NoteEvent;
use mega_audio::{CaptureSource, CaptureTap};

use crate::fft::{
    eq_bin_gains, fft_radix2, fold_log_bins, hann, ifft_radix2, EQ_HOP, FFT_N, SPEC_BINS, TAP_FFT_N,
    TAP_HOP,
};
use crate::graph::{
    midi_shift, output_port_type, parse_seq_when, port, seq_window, EditorView, GraphDoc, Instrument,
    NodeKind, Project, SeqNote, BEATS_PER_BAR, BEATS_PER_STEP, MIX_INS, MIX_PAN_INS, MIX_VOL_INS,
    NOTE_JOIN_INS,
};
use crate::monitor::{FftBuf, Monitor, ScopeBuf};

pub const WAVEFORMS: [(&str, Waveform); 5] = [
    ("Sine", Waveform::Sine),
    ("Saw", Waveform::Saw),
    ("Square", Waveform::Square),
    ("Triangle", Waveform::Triangle),
    ("Noise", Waveform::Noise),
];

pub const MASTER_GAIN: f32 = 0.18;

fn biquad_kind(i: usize) -> FilterKind {
    match i {
        1 => FilterKind::HighPass,
        2 => FilterKind::BandPass,
        3 => FilterKind::Notch,
        _ => FilterKind::LowPass,
    }
}

fn audio_in_wired(patch: &Patch, node: &str, port: &str) -> bool {
    patch
        .links
        .iter()
        .any(|l| l.2 == node && l.3 == port)
}

fn voice_adsr(n: &crate::graph::GraphNode) -> AdsrParams {
    let (attack, decay, sustain, release) = n.adsr_params();
    AdsrParams {
        attack,
        decay,
        sustain,
        release,
    }
}

#[derive(Clone)]
pub struct Patch {
    pub playing: bool,
    pub bpm: f32,
    pub seek_gen: u64,
    pub seek_beats: f64,
    pub output_id: String,
    pub nodes: Vec<crate::graph::GraphNode>,
    pub links: Vec<(String, String, String, String)>,
    pub captures: HashMap<String, Arc<CaptureTap>>,
    /// Sequencer id whose note graph receives editor preview events (instrument test).
    pub preview_seq: Option<String>,
}

impl Patch {
    pub fn from_doc(doc: &GraphDoc, playing: bool) -> Self {
        Self {
            playing,
            bpm: doc.bpm.max(1.0),
            seek_gen: doc.seek_gen,
            seek_beats: doc.seek_beats.max(0.0),
            output_id: doc.output_id.clone(),
            nodes: doc.nodes.clone(),
            links: doc
                .space
                .links
                .iter()
                .map(|l| {
                    (
                        l.from_node.clone(),
                        l.from_port.clone(),
                        l.to_node.clone(),
                        l.to_port.clone(),
                    )
                })
                .collect(),
            captures: HashMap::new(),
            preview_seq: None,
        }
    }

    pub fn from_project(project: &Project, playing: bool) -> Self {
        if let EditorView::Sequence(id) = &project.view {
            if let Some(seq) = project.sequences.iter().find(|s| s.id == *id) {
                return sequence_patch(project, seq, playing);
            }
        }
        if let EditorView::Instrument(id) = &project.view {
            if let Some(inst) = project.instruments.iter().find(|i| i.id == *id) {
                return instrument_patch(project, inst, playing);
            }
        }
        let mut patch = Self::from_main(project, playing);
        patch.seek_beats = project.main.seek_beats.max(0.0);
        patch
    }

    /// Main arrangement, ignoring sequence-edit preview.
    pub fn from_main(project: &Project, playing: bool) -> Self {
        let mut nodes = project.main.nodes.clone();
        let mut links: Vec<(String, String, String, String)> = project
            .main
            .space
            .links
            .iter()
            .map(|l| {
                (
                    l.from_node.clone(),
                    l.from_port.clone(),
                    l.to_node.clone(),
                    l.to_port.clone(),
                )
            })
            .collect();
        expand_instruments(&mut nodes, &mut links, &project.instruments, false);
        Project::apply_seq_notes(&mut nodes, &project.sequences);
        Self {
            playing,
            bpm: project.main.bpm.max(1.0),
            seek_gen: project.main.seek_gen,
            seek_beats: 0.0,
            output_id: project.main.output_id.clone(),
            nodes,
            links,
            captures: HashMap::new(),
            preview_seq: None,
        }
    }

    pub fn topo_hash(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.output_id.hash(&mut h);
        for n in &self.nodes {
            if !dsp_kind(n.kind) {
                continue;
            }
            n.id.hash(&mut h);
            n.kind.hash(&mut h);
            if n.kind == NodeKind::Voice {
                n.waveform.hash(&mut h);
            }
            if n.kind == NodeKind::AudioIn {
                n.audio_device.hash(&mut h);
            }
        }
        for l in &self.links {
            let Some(from) = self.nodes.iter().find(|n| n.id == l.0) else {
                continue;
            };
            if output_port_type(from.kind, &l.1) != port::AUDIO {
                continue;
            }
            l.0.hash(&mut h);
            l.1.hash(&mut h);
            l.2.hash(&mut h);
            l.3.hash(&mut h);
        }
        h.finish()
    }
}

fn sequence_patch(project: &Project, seq: &crate::graph::Sequence, playing: bool) -> Patch {
    let clock = crate::graph::GraphNode::new("clk".into(), NodeKind::Clock, glam::Vec2::ZERO);
    let mut seq_n = crate::graph::GraphNode::new("seq".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
    seq_n.seq_loop_bars = seq.seq_loop_bars;
    seq_n.seq_octave = seq.seq_octave;
    seq_n.notes = seq.notes.clone();

    let inst_ok = !seq.play_inst.is_empty()
        && project.instruments.iter().any(|i| i.id == seq.play_inst);
    let play = if inst_ok {
        let mut n = crate::graph::GraphNode::new("play".into(), NodeKind::Instrument, glam::Vec2::ZERO);
        n.inst_id = seq.play_inst.clone();
        n
    } else {
        let mut n = crate::graph::GraphNode::new("play".into(), NodeKind::Voice, glam::Vec2::ZERO);
        n.waveform = 0;
        n
    };

    let out = crate::graph::GraphNode::new("out".into(), NodeKind::Output, glam::Vec2::ZERO);
    let mut nodes = vec![clock, seq_n, play, out];
    let mut links = vec![
        ("clk".into(), "clock".into(), "seq".into(), "clock".into()),
        ("seq".into(), "notes".into(), "play".into(), "notes".into()),
        ("play".into(), "out".into(), "out".into(), "in".into()),
    ];
    if inst_ok {
        expand_instruments(&mut nodes, &mut links, &project.instruments, false);
    }
    Patch {
        playing,
        bpm: project.main.bpm.max(1.0),
        seek_gen: project.main.seek_gen,
        seek_beats: project.main.seek_beats.max(0.0),
        output_id: "out".into(),
        nodes,
        links,
        captures: HashMap::new(),
        preview_seq: Some("seq".into()),
    }
}

fn patch_audible(patch: &Patch) -> bool {
    patch.playing || patch.preview_seq.is_some()
}

fn instrument_patch(project: &Project, inst: &Instrument, playing: bool) -> Patch {
    let mut keys = crate::graph::GraphNode::new("keys".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
    if playing {
        keys.notes = vec![
            SeqNote { step: 0, pitch: 48, len: 4 },
            SeqNote { step: 4, pitch: 60, len: 4 },
            SeqNote { step: 8, pitch: 72, len: 4 },
        ];
    }
    let mut play = crate::graph::GraphNode::new("play".into(), NodeKind::Instrument, glam::Vec2::ZERO);
    play.inst_id = inst.id.clone();
    let out = crate::graph::GraphNode::new("out".into(), NodeKind::Output, glam::Vec2::ZERO);
    let mut nodes = vec![keys, play, out];
    let mut links = vec![
        ("keys".into(), "notes".into(), "play".into(), "notes".into()),
        ("play".into(), "out".into(), "out".into(), "in".into()),
    ];
    expand_instruments(&mut nodes, &mut links, &project.instruments, true);
    if project.preview_clock || playing {
        let clock_id = if let Some(n) = nodes.iter().find(|n| n.kind == NodeKind::Clock) {
            n.id.clone()
        } else {
            nodes.insert(
                0,
                crate::graph::GraphNode::new("clk".into(), NodeKind::Clock, glam::Vec2::ZERO),
            );
            "clk".into()
        };
        if playing && !links.iter().any(|l| l.2 == "keys" && l.3 == "clock") {
            links.insert(
                0,
                (clock_id.clone(), "clock".into(), "keys".into(), "clock".into()),
            );
        }
        if project.preview_clock {
            let gates: Vec<String> = nodes
                .iter()
                .filter(|n| n.kind == NodeKind::TranceGate)
                .map(|n| n.id.clone())
                .collect();
            for gid in gates {
                if links.iter().any(|l| l.2 == gid && l.3 == "clock") {
                    continue;
                }
                links.push((clock_id.clone(), "clock".into(), gid, "clock".into()));
            }
        }
    }
    Patch {
        // Stay audible for inspector key tests even when the demo loop is stopped.
        playing: true,
        bpm: project.main.bpm.max(1.0),
        seek_gen: project.main.seek_gen,
        seek_beats: 0.0,
        output_id: "out".into(),
        nodes,
        links,
        captures: HashMap::new(),
        preview_seq: Some("keys".into()),
    }
}

fn dsp_bypass(kind: NodeKind) -> Bypass {
    match kind {
        NodeKind::Osc | NodeKind::Voice | NodeKind::Guitar | NodeKind::Lfo | NodeKind::AudioIn | NodeKind::Value => {
            Bypass::Mute
        }
        NodeKind::Filter
        | NodeKind::Gain
        | NodeKind::Pan
        | NodeKind::Mix
        | NodeKind::Mixer
        | NodeKind::Delay
        | NodeKind::Distortion
        | NodeKind::Chorus
        | NodeKind::Flanger
        | NodeKind::Reverb
        | NodeKind::Compressor
        | NodeKind::Eq
        | NodeKind::TranceGate
        | NodeKind::Mul
        | NodeKind::Clamp
        | NodeKind::Remap
        | NodeKind::Scope
        | NodeKind::Spectrum
        | NodeKind::Spectrogram => Bypass::Thru,
        _ => Bypass::Off,
    }
}

fn dsp_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Osc
            | NodeKind::Lfo
            | NodeKind::Filter
            | NodeKind::Gain
            | NodeKind::Pan
            | NodeKind::Mix
            | NodeKind::Mixer
            | NodeKind::Delay
            | NodeKind::Distortion
            | NodeKind::Chorus
            | NodeKind::Flanger
            | NodeKind::Reverb
            | NodeKind::Compressor
            | NodeKind::Eq
            | NodeKind::TranceGate
            | NodeKind::Mul
            | NodeKind::Clamp
            | NodeKind::Remap
            | NodeKind::Value
            | NodeKind::Scope
            | NodeKind::Spectrum
            | NodeKind::Spectrogram
            | NodeKind::Voice
            | NodeKind::Guitar
            | NodeKind::AudioIn
    )
}

fn port_wired(patch: &Patch, node: &str, port: &str) -> bool {
    patch.links.iter().any(|l| l.2 == node && l.3 == port)
}

fn apply_mixer_strips(mix: &mut StereoMixer, n: &crate::graph::GraphNode, patch: &Patch) {
    let strips = mix.gains.len().min(MIX_INS.len());
    for i in 0..strips {
        let s = n.mix_strip(i);
        mix.gains[i] = s.vol.clamp(0.0, 1.5);
        mix.pans[i] = s.pan.clamp(-1.0, 1.0);
        mix.vol_from_cv[i] = port_wired(patch, &n.id, MIX_VOL_INS[i]);
        mix.pan_from_cv[i] = port_wired(patch, &n.id, MIX_PAN_INS[i]);
    }
}

#[derive(Clone, Copy)]
struct Wire {
    id: NodeId,
    l: usize,
    r: usize,
}

impl Wire {
    fn stereo(id: NodeId, l: usize, r: usize) -> Self {
        Self { id, l, r }
    }

    fn mono(id: NodeId, p: usize) -> Self {
        Self { id, l: p, r: p }
    }
}

fn connect_wire(graph: &mut Graph, src: Wire, dst: Wire) {
    graph.connect(src.id, src.l, dst.id, dst.l);
    if dst.l != dst.r {
        graph.connect(src.id, src.r, dst.id, dst.r);
    }
}

pub struct Build {
    pub graph: Graph,
    pub voices: HashMap<String, NodeId>,
    pub dsp: HashMap<String, NodeId>,
    pub lfo_mix: HashMap<String, NodeId>,
    pub master: NodeId,
    pub gates: Vec<NodeId>,
}

pub fn build_graph(patch: &Patch, sample_rate: f32, monitor: &Monitor) -> Build {
    build_graph_at(patch, sample_rate, monitor, 1)
}

fn build_graph_at(patch: &Patch, sample_rate: f32, monitor: &Monitor, block_size: usize) -> Build {
    let block_size = block_size.max(1);
    let mut graph = Graph::new(sample_rate, block_size);
    let mut out_port: HashMap<(String, String), Wire> = HashMap::new();
    let mut in_port: HashMap<(String, String), Wire> = HashMap::new();
    let mut voices = HashMap::new();
    let mut dsp = HashMap::new();
    let mut lfo_mix_ids = HashMap::new();
    let mut gates = Vec::new();

    for n in &patch.nodes {
        match n.kind {
            NodeKind::Output
            | NodeKind::Input
            | NodeKind::Instrument
            | NodeKind::Clock
            | NodeKind::Sequencer
            | NodeKind::NoteJoin
            | NodeKind::Transpose
            | NodeKind::Chord
            | NodeKind::Arp
            | NodeKind::NoteScope => {}
            NodeKind::Voice => {
                let wf = WAVEFORMS.get(n.waveform).map(|w| w.1).unwrap_or(Waveform::Saw);
                let id = graph.add_node(Box::new({
                    let mut inst = PolyphonicInstrument::new(
                        8,
                        sample_rate,
                        block_size,
                        wf,
                        voice_adsr(n),
                        n.pulse_width,
                    );
                    inst.set_unison(n.unison.round().clamp(1.0, 16.0) as usize);
                    inst.set_detune(n.detune);
                    inst.set_unison_pan(n.unison_pan);
                    inst
                }));
                voices.insert(n.id.clone(), id);
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "freq".into()), Wire::mono(id, 0));
                in_port.insert((n.id.clone(), "pitch".into()), Wire::mono(id, 0));
                in_port.insert((n.id.clone(), "amp".into()), Wire::mono(id, 1));
                in_port.insert((n.id.clone(), "pwm".into()), Wire::mono(id, 2));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Guitar => {
                let mut gtr = KarplusStrong::new(sample_rate);
                gtr.set_adsr(voice_adsr(n));
                let id = graph.add_node(Box::new(gtr));
                voices.insert(n.id.clone(), id);
                dsp.insert(n.id.clone(), id);
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::AudioIn => {
                let tap = patch
                    .captures
                    .get(&n.id)
                    .cloned()
                    .unwrap_or_else(CaptureTap::new);
                let id = graph.add_node(Box::new(CaptureSource::new(tap)));
                dsp.insert(n.id.clone(), id);
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Osc => {
                let wf = WAVEFORMS.get(n.waveform).map(|w| w.1).unwrap_or(Waveform::Saw);
                let mut osc = Oscillator::new(wf, n.freq.max(1.0));
                osc.pulse_width = n.pulse_width.clamp(0.02, 0.98);
                let id = graph.add_node(Box::new(osc));
                dsp.insert(n.id.clone(), id);
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
                in_port.insert((n.id.clone(), "fm".into()), Wire::mono(id, 0));
                in_port.insert((n.id.clone(), "pwm".into()), Wire::mono(id, 1));
            }
            NodeKind::Lfo => {
                let osc = graph.add_node(Box::new(Oscillator::new(
                    Waveform::Sine,
                    n.lfo_rate.max(0.01),
                )));
                let amp = graph.add_node(Box::new(GainCv::new(n.lfo_depth.max(0.0))));
                dsp.insert(n.id.clone(), osc);
                lfo_mix_ids.insert(n.id.clone(), amp);
                graph.connect(osc, 0, amp, 0);
                out_port.insert((n.id.clone(), "out".into()), Wire::mono(amp, 0));
                in_port.insert((n.id.clone(), "rate".into()), Wire::mono(osc, 0));
                in_port.insert((n.id.clone(), "depth".into()), Wire::mono(amp, 1));
            }
            NodeKind::Filter => {
                let mut filt = BiquadFilter::new(
                    biquad_kind(n.filter_kind),
                    n.cutoff.max(20.0),
                    n.q.max(0.1),
                    sample_rate,
                );
                filt.cutoff_from_cv = audio_in_wired(patch, &n.id, "cutoff");
                let id = graph.add_node(Box::new(filt));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                in_port.insert((n.id.clone(), "cutoff".into()), Wire::mono(id, 2));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Eq => {
                let mut eq = CurveEq::new(sample_rate);
                eq.set_curve(&n.eq_pairs());
                let id = graph.add_node(Box::new(eq));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::TranceGate => {
                let mut g = TranceGate::new(sample_rate);
                g.pattern = n.gate_pattern;
                g.smooth = n.gate_smooth.clamp(0.0, 0.08);
                g.mix = n.gate_mix.clamp(0.0, 1.0);
                g.step_beats = n.gate_step_beats();
                let id = graph.add_node(Box::new(g));
                dsp.insert(n.id.clone(), id);
                gates.push(id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Gain => {
                let id = graph.add_node(Box::new(StereoGain::new(n.gain)));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Pan => {
                let mut pan = StereoPan::new(n.pan);
                pan.pan_from_cv = port_wired(patch, &n.id, "pan");
                let id = graph.add_node(Box::new(pan));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                in_port.insert((n.id.clone(), "pan".into()), Wire::mono(id, 2));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Mix => {
                let mut mix = StereoJoin::new(2);
                mix.gains[0] = n.mix_a;
                mix.gains[1] = n.mix_b;
                let id = graph.add_node(Box::new(mix));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "a".into()), Wire::stereo(id, 0, 1));
                in_port.insert((n.id.clone(), "b".into()), Wire::stereo(id, 2, 3));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Mixer => {
                let n_strips = MIX_INS.len();
                let mut mix = StereoMixer::new(n_strips);
                apply_mixer_strips(&mut mix, n, patch);
                let id = graph.add_node(Box::new(mix));
                dsp.insert(n.id.clone(), id);
                for (i, p) in MIX_INS.iter().enumerate() {
                    in_port.insert(
                        (n.id.clone(), (*p).into()),
                        Wire::stereo(id, i, n_strips + i),
                    );
                    in_port.insert(
                        (n.id.clone(), MIX_VOL_INS[i].into()),
                        Wire::mono(id, n_strips * 2 + i),
                    );
                    in_port.insert(
                        (n.id.clone(), MIX_PAN_INS[i].into()),
                        Wire::mono(id, n_strips * 3 + i),
                    );
                }
                in_port.insert((n.id.clone(), "a".into()), Wire::stereo(id, 0, n_strips));
                in_port.insert((n.id.clone(), "b".into()), Wire::stereo(id, 1, n_strips + 1));
                out_port.insert((n.id.clone(), "L".into()), Wire::mono(id, 0));
                out_port.insert((n.id.clone(), "R".into()), Wire::mono(id, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Scope => {
                let tap = ScopeTap {
                    buf: monitor.scope_buf(&n.id),
                };
                let id = graph.add_node(Box::new(tap));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Spectrum | NodeKind::Spectrogram => {
                let tap = FftTap::new(monitor.fft_buf(&n.id), sample_rate);
                let id = graph.add_node(Box::new(tap));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Delay => {
                let mut delay = Delay::new(2.0, sample_rate);
                delay.delay_time = n.delay_time.clamp(0.02, 1.8);
                delay.feedback = n.delay_feedback.clamp(0.0, 0.92);
                delay.mix = n.delay_mix.clamp(0.0, 1.0);
                let id = graph.add_node(Box::new(delay));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Distortion => {
                let id = graph.add_node(Box::new(Distortion::new(n.drive.max(0.05))));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Chorus => {
                let mut ch = Chorus::new(sample_rate);
                ch.rate = n.chorus_rate.max(0.01);
                ch.depth = n.chorus_depth.clamp(0.0, 1.0);
                ch.mix = n.chorus_mix.clamp(0.0, 1.0);
                let id = graph.add_node(Box::new(ch));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Flanger => {
                let mut fl = Flanger::new(sample_rate);
                fl.rate = n.flange_rate.max(0.01);
                fl.depth = n.flange_depth.clamp(0.0, 1.0);
                fl.feedback = n.flange_feedback.clamp(0.0, 0.95);
                fl.mix = n.flange_mix.clamp(0.0, 1.0);
                let id = graph.add_node(Box::new(fl));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Reverb => {
                let mut rv = Reverb::new(sample_rate);
                rv.room = n.rev_room.clamp(0.0, 1.0);
                rv.damp = n.rev_damp.clamp(0.0, 1.0);
                rv.mix = n.rev_mix.clamp(0.0, 1.0);
                let id = graph.add_node(Box::new(rv));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Compressor => {
                let mut c = Compressor::new(sample_rate);
                c.threshold = n.comp_thresh.clamp(0.02, 1.0);
                c.ratio = n.comp_ratio.clamp(1.0, 20.0);
                c.attack = n.comp_attack.clamp(0.0005, 0.2);
                c.release = n.comp_release.clamp(0.01, 1.5);
                c.makeup = n.comp_makeup.clamp(0.25, 8.0);
                let id = graph.add_node(Box::new(c));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::stereo(id, 0, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::stereo(id, 0, 1));
            }
            NodeKind::Value => {
                let id = graph.add_node(Box::new(Const::new(n.value)));
                dsp.insert(n.id.clone(), id);
                out_port.insert((n.id.clone(), "out".into()), Wire::mono(id, 0));
            }
            NodeKind::Mul => {
                let id = graph.add_node(Box::new(Mul));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "a".into()), Wire::mono(id, 0));
                in_port.insert((n.id.clone(), "b".into()), Wire::mono(id, 1));
                out_port.insert((n.id.clone(), "out".into()), Wire::mono(id, 0));
            }
            NodeKind::Clamp => {
                let id = graph.add_node(Box::new(Clamp::new(n.clamp_min, n.clamp_max)));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::mono(id, 0));
                out_port.insert((n.id.clone(), "out".into()), Wire::mono(id, 0));
            }
            NodeKind::Remap => {
                let id = graph.add_node(Box::new(Remap::new(
                    n.map_in_min,
                    n.map_in_max,
                    n.map_out_min,
                    n.map_out_max,
                )));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), Wire::mono(id, 0));
                out_port.insert((n.id.clone(), "out".into()), Wire::mono(id, 0));
            }
        }
    }

    for (from, from_p, to, to_p) in &patch.links {
        if to == &patch.output_id {
            continue;
        }
        let Some(&src) = out_port.get(&(from.clone(), from_p.clone())) else {
            continue;
        };
        let Some(&dst) = in_port.get(&(to.clone(), to_p.clone())) else {
            continue;
        };
        connect_wire(&mut graph, src, dst);
    }

    let master = StereoGain::new(if patch_audible(patch) { MASTER_GAIN } else { 0.0 });
    let master_id = graph.add_node(Box::new(master));

    if let Some((from, from_p, _, _)) = patch
        .links
        .iter()
        .find(|(_, _, to, to_p)| to == &patch.output_id && to_p == "in")
    {
        if let Some(&src) = out_port.get(&(from.clone(), from_p.clone())) {
            connect_wire(&mut graph, src, Wire::stereo(master_id, 0, 1));
        }
    }

    graph.set_master_output(master_id, 0);
    Build {
        graph,
        voices,
        dsp,
        lfo_mix: lfo_mix_ids,
        master: master_id,
        gates,
    }
}

struct ClockRun {
    out: Arc<AtomicU32>,
}

struct SeqRun {
    notes: Vec<SeqNote>,
    clock_id: String,
    windows: Vec<(f64, f64)>,
    loop_beats: f64,
    pos: f64,
    playhead_out: Arc<AtomicU32>,
    targets: Vec<SeqTarget>,
    taps: Vec<TapRun>,
    events: Vec<SeqEv>,
    next_ev: usize,
    counts: HashMap<(NodeId, u8), u32>,
    tap_counts: HashMap<(u32, u8), u32>,
    held: Vec<(NodeId, u8)>,
    was_active: bool,
    need_init: bool,
}

#[derive(Clone, Copy)]
struct SeqEv {
    t: f64,
    on: bool,
    voice: Option<NodeId>,
    tap: Option<u32>,
    pitch: u8,
}

pub struct Live {
    sample_rate: f32,
    playing: bool,
    topo: u64,
    voices: HashMap<String, NodeId>,
    dsp: HashMap<String, NodeId>,
    lfo_mix: HashMap<String, NodeId>,
    master: NodeId,
    gates: Vec<NodeId>,
    preview: NodeId,
    preview_targets: Vec<SeqTarget>,
    clocks: HashMap<String, ClockRun>,
    seqs: HashMap<String, SeqRun>,
    taps: HashMap<String, Arc<crate::monitor::PitchSet>>,
    voice_holds: HashMap<NodeId, HashMap<u8, u32>>,
    tap_holds: HashMap<(usize, u8), u32>,
    monitor: Arc<Monitor>,
    song_beats: f64,
    bpm: f32,
    seek_gen: u64,
    block_size: usize,
}

impl Live {
    pub fn new(patch: &Patch, sample_rate: f32, monitor: Arc<Monitor>) -> (Self, Graph) {
        Self::new_at(patch, sample_rate, monitor, 1)
    }

    pub fn new_at(
        patch: &Patch,
        sample_rate: f32,
        monitor: Arc<Monitor>,
        block_size: usize,
    ) -> (Self, Graph) {
        let block_size = block_size.max(1);
        let build = build_graph_at(patch, sample_rate, &monitor, block_size);
        let mut graph = build.graph;
        let preview = attach_preview(&mut graph, build.master, sample_rate, block_size);
        let mut live = Self {
            sample_rate,
            playing: patch.playing,
            topo: patch.topo_hash(),
            voices: build.voices,
            dsp: build.dsp,
            lfo_mix: build.lfo_mix,
            master: build.master,
            gates: build.gates,
            preview,
            preview_targets: Vec::new(),
            clocks: HashMap::new(),
            seqs: HashMap::new(),
            taps: HashMap::new(),
            voice_holds: HashMap::new(),
            tap_holds: HashMap::new(),
            monitor,
            song_beats: patch.seek_beats.max(0.0),
            bpm: patch.bpm.max(1.0),
            seek_gen: patch.seek_gen,
            block_size,
        };
        live.rebuild_clocks(patch);
        live.rebuild_taps(patch);
        let _ = live.rebuild_seqs(patch, false);
        live.rebuild_preview(patch);
        live.sync_dsp(&mut graph, patch);
        (live, graph)
    }

    fn rebuild_clocks(&mut self, patch: &Patch) {
        let mut next = HashMap::new();
        for n in &patch.nodes {
            if n.kind != NodeKind::Clock || n.bypass {
                continue;
            }
            let prev = self.clocks.remove(&n.id);
            next.insert(
                n.id.clone(),
                ClockRun {
                    out: prev
                        .map(|c| c.out)
                        .unwrap_or_else(|| self.monitor.playhead_slot(&n.id)),
                },
            );
        }
        self.clocks = next;
    }

    fn rebuild_seqs(&mut self, patch: &Patch, keep_held: bool) -> Vec<(NodeId, u8)> {
        for seq in self.seqs.values_mut() {
            seq_release_taps(&mut self.tap_holds, seq);
        }
        let mut next = HashMap::new();
        for n in &patch.nodes {
            if n.kind != NodeKind::Sequencer || n.bypass {
                continue;
            }
            let Some(clock_id) = seq_clock_id(patch, &n.id) else {
                continue;
            };
            if !self.clocks.contains_key(&clock_id) {
                continue;
            }
            let (targets, tap_ids) = seq_routes(patch, &n.id, &self.voices);
            let mut taps = Vec::new();
            for t in tap_ids {
                let Some(slot) = self.taps.get(&t.id) else {
                    continue;
                };
                taps.push(TapRun {
                    slot: slot.clone(),
                    pitch: t.pitch,
                    delay: t.delay,
                    gate: t.gate,
                });
            }
            let prev = self.seqs.remove(&n.id);
            let playhead_out = prev
                .as_ref()
                .map(|p| p.playhead_out.clone())
                .unwrap_or_else(|| self.monitor.playhead_slot(&n.id));
            let pos = prev.as_ref().map(|p| p.pos).unwrap_or(0.0);
            let (held, was_active) = if keep_held {
                prev.map(|p| (p.held, p.was_active))
                    .unwrap_or((Vec::new(), false))
            } else {
                (Vec::new(), false)
            };
            let loop_beats = n.loop_beats();
            let events = compile_seq_events(&n.notes, &targets, &taps, loop_beats);
            next.insert(
                n.id.clone(),
                SeqRun {
                    notes: n.notes.clone(),
                    clock_id,
                    windows: parse_seq_when(&n.seq_when),
                    loop_beats,
                    pos,
                    playhead_out,
                    targets,
                    taps,
                    events,
                    next_ev: 0,
                    counts: HashMap::new(),
                    tap_counts: HashMap::new(),
                    held,
                    was_active,
                    need_init: true,
                },
            );
        }
        let mut dropped = Vec::new();
        for seq in self.seqs.values() {
            dropped.extend(seq.held.iter().copied());
        }
        self.seqs = next;
        dropped
    }

    fn rebuild_taps(&mut self, patch: &Patch) {
        let mut next = HashMap::new();
        for n in &patch.nodes {
            if n.kind != NodeKind::NoteScope {
                continue;
            }
            let slot = self
                .taps
                .remove(&n.id)
                .unwrap_or_else(|| self.monitor.note_slot(&n.id));
            next.insert(n.id.clone(), slot);
        }
        self.taps = next;
    }

    fn clear_taps(&self) {
        for slot in self.taps.values() {
            slot.clear();
        }
    }

    fn sync_dsp(&self, graph: &mut Graph, patch: &Patch) {
        for n in &patch.nodes {
            let Some(&id) = self.dsp.get(&n.id) else {
                continue;
            };
            match n.kind {
                NodeKind::Voice => {
                    if let Some(inst) = graph.node_mut::<PolyphonicInstrument>(id) {
                        inst.set_adsr(voice_adsr(n));
                        inst.set_pulse_width(n.pulse_width);
                        inst.set_unison(n.unison.round().clamp(1.0, 16.0) as usize);
                        inst.set_detune(n.detune);
                        inst.set_unison_pan(n.unison_pan);
                    }
                }
                NodeKind::Guitar => {
                    if let Some(gtr) = graph.node_mut::<KarplusStrong>(id) {
                        gtr.set_adsr(voice_adsr(n));
                    }
                }
                NodeKind::Osc => {
                    if let Some(osc) = graph.node_mut::<Oscillator>(id) {
                        osc.frequency = n.freq.max(1.0);
                        osc.waveform = WAVEFORMS.get(n.waveform).map(|w| w.1).unwrap_or(Waveform::Saw);
                        osc.pulse_width = n.pulse_width.clamp(0.02, 0.98);
                    }
                }
                NodeKind::Lfo => {
                    if let Some(osc) = graph.node_mut::<Oscillator>(id) {
                        osc.frequency = n.lfo_rate.max(0.01);
                    }
                    if let Some(&amp_id) = self.lfo_mix.get(&n.id) {
                        if let Some(g) = graph.node_mut::<GainCv>(amp_id) {
                            g.gain = n.lfo_depth.max(0.0);
                        }
                    }
                }
                NodeKind::Value => {
                    if let Some(c) = graph.node_mut::<Const>(id) {
                        c.value = n.value;
                    }
                }
                NodeKind::TranceGate => {
                    if let Some(g) = graph.node_mut::<TranceGate>(id) {
                        g.pattern = n.gate_pattern;
                        g.smooth = n.gate_smooth.clamp(0.0, 0.08);
                        g.mix = n.gate_mix.clamp(0.0, 1.0);
                        g.step_beats = n.gate_step_beats();
                        g.armed = seq_clock_id(patch, &n.id).is_some();
                    }
                }
                NodeKind::Clamp => {
                    if let Some(c) = graph.node_mut::<Clamp>(id) {
                        c.min = n.clamp_min;
                        c.max = n.clamp_max;
                    }
                }
                NodeKind::Remap => {
                    if let Some(r) = graph.node_mut::<Remap>(id) {
                        r.in_min = n.map_in_min;
                        r.in_max = n.map_in_max;
                        r.out_min = n.map_out_min;
                        r.out_max = n.map_out_max;
                    }
                }
                NodeKind::Delay => {
                    if let Some(d) = graph.node_mut::<Delay>(id) {
                        d.delay_time = n.delay_time.clamp(0.02, 1.8);
                        d.feedback = n.delay_feedback.clamp(0.0, 0.92);
                        d.mix = n.delay_mix.clamp(0.0, 1.0);
                    }
                }
                NodeKind::Filter => {
                    if let Some(f) = graph.node_mut::<BiquadFilter>(id) {
                        f.kind = biquad_kind(n.filter_kind);
                        f.cutoff = n.cutoff.max(20.0);
                        f.q = n.q.max(0.1);
                        f.cutoff_from_cv = audio_in_wired(patch, &n.id, "cutoff");
                    }
                }
                NodeKind::Eq => {
                    if let Some(eq) = graph.node_mut::<CurveEq>(id) {
                        eq.set_curve(&n.eq_pairs());
                    }
                }
                NodeKind::Gain => {
                    if let Some(g) = graph.node_mut::<StereoGain>(id) {
                        g.gain = n.gain;
                    }
                }
                NodeKind::Pan => {
                    if let Some(p) = graph.node_mut::<StereoPan>(id) {
                        p.pan = n.pan.clamp(-1.0, 1.0);
                        p.pan_from_cv = port_wired(patch, &n.id, "pan");
                    }
                }
                NodeKind::Distortion => {
                    if let Some(d) = graph.node_mut::<Distortion>(id) {
                        d.drive = n.drive.max(0.05);
                    }
                }
                NodeKind::Chorus => {
                    if let Some(c) = graph.node_mut::<Chorus>(id) {
                        c.rate = n.chorus_rate.max(0.01);
                        c.depth = n.chorus_depth.clamp(0.0, 1.0);
                        c.mix = n.chorus_mix.clamp(0.0, 1.0);
                    }
                }
                NodeKind::Flanger => {
                    if let Some(f) = graph.node_mut::<Flanger>(id) {
                        f.rate = n.flange_rate.max(0.01);
                        f.depth = n.flange_depth.clamp(0.0, 1.0);
                        f.feedback = n.flange_feedback.clamp(0.0, 0.95);
                        f.mix = n.flange_mix.clamp(0.0, 1.0);
                    }
                }
                NodeKind::Reverb => {
                    if let Some(r) = graph.node_mut::<Reverb>(id) {
                        r.room = n.rev_room.clamp(0.0, 1.0);
                        r.damp = n.rev_damp.clamp(0.0, 1.0);
                        r.mix = n.rev_mix.clamp(0.0, 1.0);
                    }
                }
                NodeKind::Compressor => {
                    if let Some(c) = graph.node_mut::<Compressor>(id) {
                        c.threshold = n.comp_thresh.clamp(0.02, 1.0);
                        c.ratio = n.comp_ratio.clamp(1.0, 20.0);
                        c.attack = n.comp_attack.clamp(0.0005, 0.2);
                        c.release = n.comp_release.clamp(0.01, 1.5);
                        c.makeup = n.comp_makeup.clamp(0.25, 8.0);
                    }
                }
                NodeKind::Mix => {
                    if let Some(mix) = graph.node_mut::<StereoJoin>(id) {
                        mix.gains[0] = n.mix_a;
                        mix.gains[1] = n.mix_b;
                    }
                }
                NodeKind::Mixer => {
                    if let Some(mix) = graph.node_mut::<StereoMixer>(id) {
                        apply_mixer_strips(mix, n, patch);
                    }
                }
                _ => {}
            }
        }
        self.sync_bypass(graph, patch);
    }

    fn sync_bypass(&self, graph: &mut Graph, patch: &Patch) {
        for n in &patch.nodes {
            let mode = if n.bypass {
                dsp_bypass(n.kind)
            } else {
                Bypass::Off
            };
            if let Some(&id) = self.dsp.get(&n.id) {
                graph.set_bypass(id, mode);
            }
            if n.kind == NodeKind::Lfo {
                if let Some(&amp_id) = self.lfo_mix.get(&n.id) {
                    graph.set_bypass(amp_id, mode);
                }
            }
        }
    }

    pub fn apply(&mut self, graph: &mut Graph, patch: Patch) {
        let topo = patch.topo_hash();
        let keep_held = topo == self.topo;
        if !keep_held {
            let build = build_graph_at(&patch, self.sample_rate, &self.monitor, self.block_size);
            *graph = build.graph;
            self.voices = build.voices;
            self.dsp = build.dsp;
            self.lfo_mix = build.lfo_mix;
            self.master = build.master;
            self.gates = build.gates;
            self.preview = attach_preview(graph, self.master, self.sample_rate, self.block_size);
            self.topo = topo;
            self.voice_holds.clear();
        }
        self.rebuild_clocks(&patch);
        self.rebuild_taps(&patch);
        self.rebuild_preview(&patch);
        let dropped = self.rebuild_seqs(&patch, keep_held);
        if keep_held {
            for (id, pitch) in dropped {
                voice_note(
                    graph,
                    &mut self.voice_holds,
                    id,
                    NoteEvent::NoteOff { note: pitch },
                );
            }
        }
        self.sync_dsp(graph, &patch);

        if let Some(gain) = graph.node_mut::<StereoGain>(self.master) {
            gain.gain = if patch_audible(&patch) { MASTER_GAIN } else { 0.0 };
        }

        self.bpm = patch.bpm.max(1.0);
        if patch.seek_gen != self.seek_gen {
            self.song_beats = patch.seek_beats.max(0.0);
            self.seek_gen = patch.seek_gen;
            self.silence_seqs(graph);
        } else if self.playing && !patch.playing {
            self.silence_seqs(graph);
        }
        self.playing = patch.playing;
    }

    fn rebuild_preview(&mut self, patch: &Patch) {
        self.preview_targets = patch
            .preview_seq
            .as_deref()
            .map(|id| seq_routes(patch, id, &self.voices).0)
            .unwrap_or_default();
    }

    pub fn preview_event(&mut self, graph: &mut Graph, event: NoteEvent) {
        if self.preview_targets.is_empty() {
            if let Some(inst) = graph.node_mut::<PolyphonicInstrument>(self.preview) {
                inst.handle_event(event);
            }
            return;
        }
        let targets = self.preview_targets.clone();
        match event {
            NoteEvent::NoteOn { note, velocity } => {
                for t in targets {
                    voice_note(
                        graph,
                        &mut self.voice_holds,
                        t.voice,
                        NoteEvent::NoteOn {
                            note: midi_shift(note, t.pitch),
                            velocity,
                        },
                    );
                }
            }
            NoteEvent::NoteOff { note } => {
                for t in targets {
                    voice_note(
                        graph,
                        &mut self.voice_holds,
                        t.voice,
                        NoteEvent::NoteOff {
                            note: midi_shift(note, t.pitch),
                        },
                    );
                }
            }
        }
    }

    fn silence_seqs(&mut self, graph: &mut Graph) {
        let held: Vec<(NodeId, u8)> = self
            .seqs
            .values()
            .flat_map(|seq| seq.held.iter().copied())
            .collect();
        for (id, pitch) in held {
            voice_note(graph, &mut self.voice_holds, id, NoteEvent::NoteOff { note: pitch });
        }
        for seq in self.seqs.values_mut() {
            seq_release_taps(&mut self.tap_holds, seq);
            seq.held.clear();
            seq.counts.clear();
            seq.was_active = false;
            seq.need_init = true;
        }
        self.voice_holds.clear();
    }

    pub fn tick(&mut self, graph: &mut Graph) {
        self.tick_block(graph, 1);
    }

    pub fn tick_block(&mut self, graph: &mut Graph, samples: usize) {
        let samples = samples.max(1);
        let start = self.song_beats;
        if self.playing {
            let step = self.bpm as f64 / (self.sample_rate as f64 * 60.0);
            self.song_beats += step * samples as f64;
            let song = self.song_beats;
            for seq in self.seqs.values_mut() {
                tick_seq(
                    graph,
                    &self.clocks,
                    &mut self.voice_holds,
                    &mut self.tap_holds,
                    seq,
                    song,
                );
            }
        } else {
            self.clear_taps();
        }
        let gates = self.gates.clone();
        let bpm = self.bpm;
        let playing = self.playing;
        for id in gates {
            if let Some(g) = graph.node_mut::<TranceGate>(id) {
                g.beats = start;
                g.bpm = bpm;
                g.playing = playing;
            }
        }
        self.publish_time();
    }

    fn publish_time(&self) {
        self.monitor.set_song_beats(self.song_beats);
        let bits = (self.song_beats as f32).to_bits();
        for clock in self.clocks.values() {
            clock.out.store(bits, Ordering::Relaxed);
        }
        let song = self.song_beats;
        for seq in self.seqs.values() {
            if seq_window(&seq.windows, song).is_some() {
                let period = seq.loop_beats.max(1e-9);
                let now = song.rem_euclid(period) as f32;
                seq.playhead_out.store(now.to_bits(), Ordering::Relaxed);
            } else {
                seq.playhead_out.store(f32::NAN.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

fn tick_seq(
    graph: &mut Graph,
    clocks: &HashMap<String, ClockRun>,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    tap_holds: &mut HashMap<(usize, u8), u32>,
    seq: &mut SeqRun,
    song: f64,
) {
    if !clocks.contains_key(&seq.clock_id) {
        if seq.was_active {
            seq_release(graph, holds, tap_holds, seq);
        }
        return;
    }
    let win = seq_window(&seq.windows, song);
    if seq.was_active && win.is_none() {
        seq_release(graph, holds, tap_holds, seq);
        return;
    }
    if win.is_none() {
        return;
    }
    let loop_len = seq.loop_beats.max(BEATS_PER_BAR as f64);
    if seq.need_init || !seq.was_active {
        seq_snapshot(graph, holds, tap_holds, seq, song, loop_len);
        seq.was_active = true;
        seq.need_init = false;
        seq.pos = song;
        return;
    }
    seq_catch_up(graph, holds, tap_holds, seq, seq.pos, song, loop_len);
    seq.pos = song;
    seq.was_active = true;
}

fn seq_release(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    tap_holds: &mut HashMap<(usize, u8), u32>,
    seq: &mut SeqRun,
) {
    let held: Vec<(NodeId, u8)> = seq.held.drain(..).collect();
    for (id, pitch) in held {
        voice_note(graph, holds, id, NoteEvent::NoteOff { note: pitch });
    }
    seq_release_taps(tap_holds, seq);
    seq.counts.clear();
    seq.was_active = false;
    seq.need_init = true;
}

fn seq_release_taps(tap_holds: &mut HashMap<(usize, u8), u32>, seq: &mut SeqRun) {
    let tap_counts = std::mem::take(&mut seq.tap_counts);
    for ((idx, pitch), n) in tap_counts {
        let Some(tap) = seq.taps.get(idx as usize) else {
            continue;
        };
        for _ in 0..n {
            tap_note(&tap.slot, tap_holds, pitch, false);
        }
    }
}

fn seq_snapshot(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    tap_holds: &mut HashMap<(usize, u8), u32>,
    seq: &mut SeqRun,
    song: f64,
    loop_len: f64,
) {
    let mut counts = HashMap::new();
    voice_spans(&seq.notes, &seq.targets, loop_len, |voice, pitch, dur, on, off| {
        if sounding_at(song, on, off, loop_len, dur) {
            *counts.entry((voice, pitch)).or_insert(0) += 1;
        }
    });
    let new_keys: HashSet<(NodeId, u8)> = counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, _)| *k)
        .collect();
    let mut i = 0;
    while i < seq.held.len() {
        let key = seq.held[i];
        if new_keys.contains(&key) {
            i += 1;
            continue;
        }
        voice_note(graph, holds, key.0, NoteEvent::NoteOff { note: key.1 });
        seq.held.swap_remove(i);
    }
    for &key in &new_keys {
        if seq.held.contains(&key) {
            continue;
        }
        voice_note(
            graph,
            holds,
            key.0,
            NoteEvent::NoteOn {
                note: key.1,
                velocity: 0.8,
            },
        );
        seq.held.push(key);
    }
    seq.counts = counts;

    seq_release_taps(tap_holds, seq);
    let mut tap_counts = HashMap::new();
    tap_spans(&seq.notes, &seq.taps, loop_len, |idx, pitch, dur, on, off| {
        if sounding_at(song, on, off, loop_len, dur) {
            *tap_counts.entry((idx, pitch)).or_insert(0) += 1;
        }
    });
    for (&(idx, pitch), &n) in &tap_counts {
        let Some(tap) = seq.taps.get(idx as usize) else {
            continue;
        };
        for _ in 0..n {
            tap_note(&tap.slot, tap_holds, pitch, true);
        }
    }
    seq.tap_counts = tap_counts;

    let now = song.rem_euclid(loop_len);
    seq.next_ev = seq.events.partition_point(|e| e.t <= now);
}

fn seq_catch_up(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    tap_holds: &mut HashMap<(usize, u8), u32>,
    seq: &mut SeqRun,
    from_song: f64,
    to_song: f64,
    loop_len: f64,
) {
    if to_song <= from_song || loop_len <= 0.0 {
        return;
    }
    let last_loop = from_song.rem_euclid(loop_len);
    let now_loop = to_song.rem_euclid(loop_len);
    let wraps = ((last_loop + (to_song - from_song)) / loop_len).floor() as u32;
    if wraps == 0 {
        seq_fire_until(graph, holds, tap_holds, seq, now_loop);
        return;
    }
    seq_fire_rest(graph, holds, tap_holds, seq);
    for _ in 1..wraps {
        seq.next_ev = 0;
        seq_fire_rest(graph, holds, tap_holds, seq);
    }
    seq.next_ev = 0;
    seq_fire_until(graph, holds, tap_holds, seq, now_loop);
}

fn seq_fire_until(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    tap_holds: &mut HashMap<(usize, u8), u32>,
    seq: &mut SeqRun,
    until: f64,
) {
    while seq.next_ev < seq.events.len() && seq.events[seq.next_ev].t <= until {
        let ev = seq.events[seq.next_ev];
        seq.next_ev += 1;
        apply_seq_ev(graph, holds, tap_holds, seq, ev);
    }
}

fn seq_fire_rest(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    tap_holds: &mut HashMap<(usize, u8), u32>,
    seq: &mut SeqRun,
) {
    while seq.next_ev < seq.events.len() {
        let ev = seq.events[seq.next_ev];
        seq.next_ev += 1;
        apply_seq_ev(graph, holds, tap_holds, seq, ev);
    }
}

fn apply_seq_ev(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    tap_holds: &mut HashMap<(usize, u8), u32>,
    seq: &mut SeqRun,
    ev: SeqEv,
) {
    if let Some(id) = ev.voice {
        let key = (id, ev.pitch);
        let count = seq.counts.entry(key).or_insert(0);
        if ev.on {
            *count += 1;
            if *count == 1 {
                voice_note(
                    graph,
                    holds,
                    id,
                    NoteEvent::NoteOn {
                        note: ev.pitch,
                        velocity: 0.8,
                    },
                );
                if !seq.held.contains(&key) {
                    seq.held.push(key);
                }
            }
        } else if *count > 0 {
            *count -= 1;
            if *count == 0 {
                seq.counts.remove(&key);
                voice_note(graph, holds, id, NoteEvent::NoteOff { note: ev.pitch });
                seq.held.retain(|k| *k != key);
            }
        }
    }
    if let Some(idx) = ev.tap {
        let key = (idx, ev.pitch);
        let slot = seq.taps.get(idx as usize).map(|t| t.slot.clone());
        let Some(slot) = slot else {
            return;
        };
        let count = seq.tap_counts.entry(key).or_insert(0);
        if ev.on {
            *count += 1;
            tap_note(&slot, tap_holds, ev.pitch, true);
        } else if *count > 0 {
            *count -= 1;
            tap_note(&slot, tap_holds, ev.pitch, false);
            if *count == 0 {
                seq.tap_counts.remove(&key);
            }
        }
    }
}

fn tap_note(slot: &crate::monitor::PitchSet, holds: &mut HashMap<(usize, u8), u32>, pitch: u8, on: bool) {
    let k = (slot as *const crate::monitor::PitchSet as usize, pitch);
    let count = holds.entry(k).or_insert(0);
    if on {
        *count += 1;
        if *count == 1 {
            slot.insert(pitch);
        }
    } else {
        if *count > 0 {
            *count -= 1;
        }
        if *count == 0 {
            holds.remove(&k);
            slot.remove(pitch);
        }
    }
}

fn compile_seq_events(
    notes: &[SeqNote],
    targets: &[SeqTarget],
    taps: &[TapRun],
    loop_beats: f64,
) -> Vec<SeqEv> {
    let loop_len = loop_beats.max(BEATS_PER_BAR as f64);
    let mut events = Vec::new();
    voice_spans(notes, targets, loop_len, |voice, pitch, dur, on, off| {
        if dur >= loop_len - 1e-9 {
            return;
        }
        events.push(SeqEv {
            t: on,
            on: true,
            voice: Some(voice),
            tap: None,
            pitch,
        });
        events.push(SeqEv {
            t: off,
            on: false,
            voice: Some(voice),
            tap: None,
            pitch,
        });
    });
    tap_spans(notes, taps, loop_len, |idx, pitch, dur, on, off| {
        if dur >= loop_len - 1e-9 {
            return;
        }
        events.push(SeqEv {
            t: on,
            on: true,
            voice: None,
            tap: Some(idx),
            pitch,
        });
        events.push(SeqEv {
            t: off,
            on: false,
            voice: None,
            tap: Some(idx),
            pitch,
        });
    });
    events.sort_by(|a, b| a.t.total_cmp(&b.t).then_with(|| a.on.cmp(&b.on)));
    events
}

fn voice_spans(
    notes: &[SeqNote],
    targets: &[SeqTarget],
    loop_len: f64,
    mut emit: impl FnMut(NodeId, u8, f64, f64, f64),
) {
    let max_step = (loop_len / BEATS_PER_STEP as f64).round().max(1.0) as u32;
    for note in notes {
        if note.step >= max_step {
            continue;
        }
        let note_dur = note.len.max(1) as f64 * BEATS_PER_STEP as f64;
        for t in targets {
            let dur = if t.gate > 1e-9 { t.gate } else { note_dur };
            let on = (note.step as f64 * BEATS_PER_STEP as f64 + t.delay).rem_euclid(loop_len);
            let off = (on + dur).rem_euclid(loop_len);
            emit(t.voice, midi_shift(note.pitch, t.pitch), dur, on, off);
        }
    }
}

fn tap_spans(
    notes: &[SeqNote],
    taps: &[TapRun],
    loop_len: f64,
    mut emit: impl FnMut(u32, u8, f64, f64, f64),
) {
    let max_step = (loop_len / BEATS_PER_STEP as f64).round().max(1.0) as u32;
    for note in notes {
        if note.step >= max_step {
            continue;
        }
        let note_dur = note.len.max(1) as f64 * BEATS_PER_STEP as f64;
        for (idx, t) in taps.iter().enumerate() {
            let dur = if t.gate > 1e-9 { t.gate } else { note_dur };
            let on = (note.step as f64 * BEATS_PER_STEP as f64 + t.delay).rem_euclid(loop_len);
            let off = (on + dur).rem_euclid(loop_len);
            emit(idx as u32, midi_shift(note.pitch, t.pitch), dur, on, off);
        }
    }
}

fn seq_desired(
    notes: &[SeqNote],
    targets: &[SeqTarget],
    song: f64,
    loop_len: f64,
) -> HashSet<(NodeId, u8)> {
    let mut out = HashSet::new();
    voice_spans(notes, targets, loop_len, |voice, pitch, dur, on, off| {
        if sounding_at(song, on, off, loop_len, dur) {
            out.insert((voice, pitch));
        }
    });
    out
}

fn sounding_at(song: f64, on: f64, off: f64, period: f64, dur: f64) -> bool {
    if period <= 0.0 {
        return false;
    }
    if dur >= period - 1e-9 {
        return true;
    }
    let now = song.rem_euclid(period);
    if on < off {
        now >= on && now < off
    } else {
        now >= on || now < off
    }
}

fn voice_note(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    id: NodeId,
    event: NoteEvent,
) {
    let slot = holds.entry(id).or_default();
    match event {
        NoteEvent::NoteOn { note, velocity } => {
            let count = slot.entry(note).or_insert(0);
            *count += 1;
            if *count == 1 {
                if let Some(inst) = graph.node_mut::<PolyphonicInstrument>(id) {
                    inst.handle_event(NoteEvent::NoteOn { note, velocity });
                } else if let Some(gtr) = graph.node_mut::<KarplusStrong>(id) {
                    gtr.handle_event(NoteEvent::NoteOn { note, velocity });
                }
            }
        }
        NoteEvent::NoteOff { note } => {
            let count = slot.entry(note).or_insert(0);
            if *count > 0 {
                *count -= 1;
            }
            if *count == 0 {
                slot.remove(&note);
                if let Some(inst) = graph.node_mut::<PolyphonicInstrument>(id) {
                    inst.handle_event(NoteEvent::NoteOff { note });
                } else if let Some(gtr) = graph.node_mut::<KarplusStrong>(id) {
                    gtr.handle_event(NoteEvent::NoteOff { note });
                }
            }
        }
    }
}

struct ScopeTap {
    buf: Arc<ScopeBuf>,
}

impl Node for ScopeTap {
    fn num_inputs(&self) -> usize {
        2
    }

    fn num_outputs(&self) -> usize {
        2
    }

    fn process(&mut self, ctx: &ProcessContext, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        let n = ctx.block_size;
        let left = inputs.first().copied().unwrap_or(&[]);
        let right = inputs.get(1).copied().unwrap_or(left);
        if let Some(out) = outputs.get_mut(0) {
            let k = n.min(left.len()).min(out.len());
            out[..k].copy_from_slice(&left[..k]);
        }
        if let Some(out) = outputs.get_mut(1) {
            let src = if right.len() >= n { right } else { left };
            let k = n.min(src.len()).min(out.len());
            out[..k].copy_from_slice(&src[..k]);
        }
        for i in 0..n {
            let l = left.get(i).copied().unwrap_or(0.0);
            let r = right.get(i).copied().unwrap_or(l);
            self.buf.push((l + r) * 0.5);
        }
    }

    fn name(&self) -> &'static str {
        "Scope"
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

struct FftTap {
    buf: Arc<FftBuf>,
    sr: f32,
    ring: Vec<f32>,
    write: usize,
    seen: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    bins: Vec<f32>,
}

impl FftTap {
    fn new(buf: Arc<FftBuf>, sample_rate: f32) -> Self {
        Self {
            buf,
            sr: sample_rate.max(1.0),
            ring: vec![0.0; TAP_FFT_N],
            write: 0,
            seen: 0,
            re: vec![0.0; TAP_FFT_N],
            im: vec![0.0; TAP_FFT_N],
            bins: vec![0.0; SPEC_BINS],
        }
    }

    fn hop(&mut self) {
        let start = self.write;
        for i in 0..TAP_FFT_N {
            self.re[i] = self.ring[(start + i) % TAP_FFT_N] * hann(i, TAP_FFT_N);
            self.im[i] = 0.0;
        }
        crate::fft::fft_radix2(&mut self.re, &mut self.im);
        fold_log_bins(&self.re, &self.im, self.sr, &mut self.bins);
        self.buf.push_bins(&self.bins);
    }
}

impl Node for FftTap {
    fn num_inputs(&self) -> usize {
        2
    }

    fn num_outputs(&self) -> usize {
        2
    }

    fn process(&mut self, ctx: &ProcessContext, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        let n = ctx.block_size;
        let left = inputs.first().copied().unwrap_or(&[]);
        let right = inputs.get(1).copied().unwrap_or(left);
        if let Some(out) = outputs.get_mut(0) {
            let k = n.min(left.len()).min(out.len());
            out[..k].copy_from_slice(&left[..k]);
        }
        if let Some(out) = outputs.get_mut(1) {
            let src = if right.len() >= n { right } else { left };
            let k = n.min(src.len()).min(out.len());
            out[..k].copy_from_slice(&src[..k]);
        }
        for i in 0..n {
            let l = left.get(i).copied().unwrap_or(0.0);
            let r = right.get(i).copied().unwrap_or(l);
            self.ring[self.write] = (l + r) * 0.5;
            self.write = (self.write + 1) % TAP_FFT_N;
            self.seen += 1;
            if self.seen >= TAP_FFT_N && (self.seen - TAP_FFT_N) % TAP_HOP == 0 {
                self.hop();
            }
        }
    }

    fn name(&self) -> &'static str {
        "FftTap"
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

struct EqChan {
    ring: Vec<f32>,
    write: usize,
    seen: usize,
    ola: Vec<f32>,
    pending: Vec<f32>,
    pend_r: usize,
    pend_w: usize,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl EqChan {
    fn new() -> Self {
        Self {
            ring: vec![0.0; FFT_N],
            write: 0,
            seen: 0,
            ola: vec![0.0; FFT_N],
            pending: vec![0.0; EQ_HOP * 4],
            pend_r: 0,
            pend_w: 0,
            re: vec![0.0; FFT_N],
            im: vec![0.0; FFT_N],
        }
    }

    fn hop(&mut self, gain: &[f32]) {
        let start = self.write;
        for i in 0..FFT_N {
            self.re[i] = self.ring[(start + i) % FFT_N] * hann(i, FFT_N);
            self.im[i] = 0.0;
        }
        fft_radix2(&mut self.re, &mut self.im);
        multiply_gains(&mut self.re, &mut self.im, gain);
        ifft_radix2(&mut self.re, &mut self.im);
        for i in 0..FFT_N {
            self.ola[i] += self.re[i];
        }
        let cap = self.pending.len();
        for i in 0..EQ_HOP {
            self.pending[self.pend_w % cap] = self.ola[i];
            self.pend_w += 1;
        }
        self.ola.copy_within(EQ_HOP..FFT_N, 0);
        self.ola[FFT_N - EQ_HOP..].fill(0.0);
    }

    fn push_sample(&mut self, x: f32, gain: &[f32]) -> f32 {
        self.ring[self.write] = x;
        self.write = (self.write + 1) % FFT_N;
        self.seen += 1;
        if self.seen >= FFT_N && (self.seen - FFT_N) % EQ_HOP == 0 {
            self.hop(gain);
        }
        if self.pend_r == self.pend_w {
            return 0.0;
        }
        let cap = self.pending.len();
        let y = self.pending[self.pend_r % cap];
        self.pend_r += 1;
        y
    }
}

struct CurveEq {
    sr: f32,
    ch: [EqChan; 2],
    gain: Vec<f32>,
}

impl CurveEq {
    fn new(sample_rate: f32) -> Self {
        let mut s = Self {
            sr: sample_rate.max(1.0),
            ch: [EqChan::new(), EqChan::new()],
            gain: vec![1.0; FFT_N],
        };
        s.set_curve(&[(0.0, 1.0), (1.0, 1.0)]);
        s
    }

    fn set_curve(&mut self, pts: &[(f32, f32)]) {
        eq_bin_gains(pts, self.sr, &mut self.gain);
    }
}

fn multiply_gains(re: &mut [f32], im: &mut [f32], gain: &[f32]) {
    for k in 0..re.len() {
        re[k] *= gain[k];
        im[k] *= gain[k];
    }
}

impl Node for CurveEq {
    fn num_inputs(&self) -> usize {
        2
    }

    fn num_outputs(&self) -> usize {
        2
    }

    fn process(&mut self, ctx: &ProcessContext, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        let n = ctx.block_size;
        let left = inputs.first().copied().unwrap_or(&[]);
        let right = inputs.get(1).copied().unwrap_or(left);
        for i in 0..n {
            let xl = left.get(i).copied().unwrap_or(0.0);
            let xr = right.get(i).copied().unwrap_or(xl);
            let yl = self.ch[0].push_sample(xl, &self.gain);
            let yr = self.ch[1].push_sample(xr, &self.gain);
            if let Some(out) = outputs.get_mut(0).and_then(|o| o.get_mut(i)) {
                *out = yl;
            }
            if let Some(out) = outputs.get_mut(1).and_then(|o| o.get_mut(i)) {
                *out = yr;
            }
        }
    }

    fn name(&self) -> &'static str {
        "Eq"
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn expand_instruments(
    nodes: &mut Vec<crate::graph::GraphNode>,
    links: &mut Vec<(String, String, String, String)>,
    instruments: &[crate::graph::Instrument],
    keep_ids: bool,
) {
    let hosts: Vec<_> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Instrument)
        .cloned()
        .collect();
    for host in hosts {
        nodes.retain(|n| n.id != host.id);
        let incoming: Vec<_> = links
            .iter()
            .filter(|l| l.2 == host.id && l.3 == "notes")
            .cloned()
            .collect();
        let outgoing: Vec<_> = links
            .iter()
            .filter(|l| l.0 == host.id && l.1 == "out")
            .cloned()
            .collect();
        links.retain(|l| l.0 != host.id && l.2 != host.id);

        if host.bypass {
            continue;
        }
        let Some(inst) = instruments.iter().find(|i| i.id == host.inst_id) else {
            continue;
        };
        let prefix = if keep_ids {
            String::new()
        } else {
            format!("{}__", host.id)
        };
        let input_id = inst.graph.input_id.as_str();
        let output_id = inst.graph.output_id.as_str();

        for n in &inst.graph.nodes {
            if n.kind == NodeKind::Input || n.kind == NodeKind::Output {
                continue;
            }
            let mut c = n.clone();
            c.id = format!("{prefix}{}", n.id);
            nodes.push(c);
        }

        let mut note_dsts = Vec::new();
        let mut audio_srcs = Vec::new();
        for l in &inst.graph.space.links {
            if l.from_node == input_id && l.from_port == "notes" {
                note_dsts.push((l.to_node.clone(), l.to_port.clone()));
                continue;
            }
            if l.to_node == output_id && l.to_port == "in" {
                audio_srcs.push((l.from_node.clone(), l.from_port.clone()));
                continue;
            }
            if l.from_node == input_id
                || l.to_node == input_id
                || l.from_node == output_id
                || l.to_node == output_id
            {
                continue;
            }
            links.push((
                format!("{prefix}{}", l.from_node),
                l.from_port.clone(),
                format!("{prefix}{}", l.to_node),
                l.to_port.clone(),
            ));
        }
        for inc in &incoming {
            for (to, port) in &note_dsts {
                links.push((inc.0.clone(), inc.1.clone(), format!("{prefix}{to}"), port.clone()));
            }
        }
        for out in &outgoing {
            for (from, port) in &audio_srcs {
                links.push((format!("{prefix}{from}"), port.clone(), out.2.clone(), out.3.clone()));
            }
        }
    }
}

fn attach_preview(graph: &mut Graph, master: NodeId, sample_rate: f32, block_size: usize) -> NodeId {
    let voice = graph.add_node(Box::new(PolyphonicInstrument::new(
        8,
        sample_rate,
        block_size.max(1),
        Waveform::Sine,
        AdsrParams {
            attack: 0.005,
            decay: 0.04,
            sustain: 0.55,
            release: 0.08,
        },
        0.5,
    )));
    let gain = graph.add_node(Box::new(StereoGain::new(MASTER_GAIN)));
    graph.connect(voice, 0, gain, 0);
    graph.connect(voice, 1, gain, 1);
    let mix = graph.add_node(Box::new(StereoJoin::new(2)));
    graph.connect(master, 0, mix, 0);
    graph.connect(master, 1, mix, 1);
    graph.connect(gain, 0, mix, 2);
    graph.connect(gain, 1, mix, 3);
    graph.set_master_output(mix, 0);
    voice
}

fn seq_clock_id(patch: &Patch, seq_id: &str) -> Option<String> {
    let kind_of = |id: &str| {
        patch
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.kind)
    };
    let mut cur = seq_id.to_string();
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(cur.clone()) {
            return None;
        }
        let (from, from_p) = patch.links.iter().find_map(|(from, from_p, to, to_p)| {
            (to == &cur && to_p == "clock").then(|| (from.clone(), from_p.clone()))
        })?;
        match (kind_of(&from), from_p.as_str()) {
            (Some(NodeKind::Clock), "clock") => return Some(from),
            (Some(NodeKind::Sequencer), "clock") => cur = from,
            _ => return None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct SeqTarget {
    voice: NodeId,
    pitch: i32,
    delay: f64,
    /// >0: note length in beats (arp). 0 = sequencer cell length.
    gate: f64,
}

#[derive(Clone)]
struct TapRun {
    slot: Arc<crate::monitor::PitchSet>,
    pitch: i32,
    delay: f64,
    gate: f64,
}

#[derive(Clone, PartialEq)]
struct TapTarget {
    id: String,
    pitch: i32,
    delay: f64,
    gate: f64,
}

fn seq_routes(
    patch: &Patch,
    seq_id: &str,
    voices: &HashMap<String, NodeId>,
) -> (Vec<SeqTarget>, Vec<TapTarget>) {
    let node = |id: &str| patch.nodes.iter().find(|n| n.id == id);
    let kind_of = |id: &str| node(id).map(|n| n.kind);
    let mut out = Vec::new();
    let mut taps = Vec::new();
    let mut stack = vec![(seq_id.to_string(), "notes".to_string(), 0_i32, 0.0_f64, 0.0_f64)];
    let mut seen = HashSet::new();
    while let Some((from, from_p, pitch, delay, gate)) = stack.pop() {
        if !seen.insert((from.clone(), from_p.clone(), pitch, delay.to_bits(), gate.to_bits())) {
            continue;
        }
        for (f, fp, to, to_p) in &patch.links {
            if f != &from || fp != &from_p {
                continue;
            }
            match (kind_of(to), to_p.as_str()) {
                (Some(NodeKind::Voice | NodeKind::Guitar), "notes") => {
                    if let Some(&id) = voices.get(to) {
                        out.push(SeqTarget {
                            voice: id,
                            pitch,
                            delay,
                            gate,
                        });
                    }
                }
                (Some(NodeKind::NoteJoin), p) if NOTE_JOIN_INS.contains(&p) => {
                    stack.push((to.clone(), "out".into(), pitch, delay, gate));
                }
                (Some(NodeKind::NoteScope), "in") => {
                    let bypass = node(to).is_some_and(|n| n.bypass);
                    if !bypass {
                        taps.push(TapTarget {
                            id: to.clone(),
                            pitch,
                            delay,
                            gate,
                        });
                    }
                    stack.push((to.clone(), "out".into(), pitch, delay, gate));
                }
                (Some(NodeKind::Transpose), "in") => {
                    let n = node(to);
                    let bypass = n.is_some_and(|n| n.bypass);
                    let pitch = if bypass {
                        pitch
                    } else {
                        pitch + n.map(|n| n.pitch_shift()).unwrap_or(0)
                    };
                    let delay = if bypass {
                        delay
                    } else {
                        delay + n.map(|n| n.time_shift_beats()).unwrap_or(0.0)
                    };
                    stack.push((to.clone(), "out".into(), pitch, delay, gate));
                }
                (Some(NodeKind::Chord), "in") => {
                    let n = node(to);
                    let bypass = n.is_some_and(|n| n.bypass);
                    let ivs: &[i32] = if bypass {
                        &[0]
                    } else {
                        n.map(|n| n.chord_intervals()).unwrap_or(&[0])
                    };
                    for &iv in ivs {
                        stack.push((to.clone(), "out".into(), pitch + iv, delay, gate));
                    }
                }
                (Some(NodeKind::Arp), "in") => {
                    let n = node(to);
                    let bypass = n.is_some_and(|n| n.bypass);
                    if bypass {
                        stack.push((to.clone(), "out".into(), pitch, delay, gate));
                    } else if let Some(n) = n {
                        let step = n.arp_step_beats();
                        for (p, d) in n.arp_events(pitch, delay) {
                            stack.push((to.clone(), "out".into(), p, d, step));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut uniq = Vec::new();
    for t in out {
        if !uniq.contains(&t) {
            uniq.push(t);
        }
    }
    let mut tap_uniq = Vec::new();
    for t in taps {
        if !tap_uniq.contains(&t) {
            tap_uniq.push(t);
        }
    }
    (uniq, tap_uniq)
}

fn crossed(prev: f64, now: f64, t: f64, period: f64) -> bool {
    if now <= prev || period <= 0.0 {
        return false;
    }
    let lo = (prev - t) / period;
    let hi = (now - t) / period;
    lo.ceil() < hi
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: f64 = 4.0;
    const DT: f64 = 120.0 / 48_000.0 / 60.0;

    #[test]
    fn first_step_fires_on_play() {
        assert!(crossed(0.0, DT, 0.0, BAR));
        assert!(!crossed(DT, DT * 2.0, 0.0, BAR));
    }

    #[test]
    fn wrap_fires_step0_once() {
        assert!(crossed(BAR - DT, BAR + DT, 0.0, BAR));
        assert!(!crossed(BAR + DT, BAR + DT * 2.0, 0.0, BAR));
    }

    #[test]
    fn mid_grid_still_fires() {
        assert!(crossed(0.24, 0.26, 0.25, BAR));
        assert!(!crossed(0.26, 0.28, 0.25, BAR));
    }

    #[test]
    fn long_play_still_advances() {
        let sr = 48_000.0_f64;
        let bpm = 120.0_f64;
        let step = bpm / (sr * 60.0);
        let mut beats = 4_096.0_f64;
        beats += step;
        assert!(beats > 4_096.0, "f64 still moves after many beats");
        let mut f = 4_096.0_f32;
        let before = f;
        f += step as f32;
        assert_eq!(f, before, "f32 already drops the sample step");
    }

    #[test]
    fn sounding_inside_and_at_off() {
        assert!(sounding_at(0.1, 0.0, 0.25, BAR, 0.25));
        assert!(!sounding_at(0.25, 0.0, 0.25, BAR, 0.25));
        assert!(!sounding_at(0.3, 0.0, 0.25, BAR, 0.25));
    }

    #[test]
    fn sounding_wraps_bar() {
        assert!(sounding_at(3.9, 3.75, 0.25, BAR, 0.5));
        assert!(sounding_at(0.1, 3.75, 0.25, BAR, 0.5));
        assert!(!sounding_at(1.0, 3.75, 0.25, BAR, 0.5));
    }

    #[test]
    fn sounding_full_bar() {
        assert!(sounding_at(2.0, 0.0, 0.0, BAR, BAR));
    }

    #[test]
    fn deleted_note_leaves_desired() {
        let notes = [SeqNote {
            step: 0,
            pitch: 60,
            len: 4,
        }];
        let song = 0.1;
        assert!(sounding_at(
            song,
            0.0,
            4.0 * BEATS_PER_STEP as f64,
            BAR,
            4.0 * BEATS_PER_STEP as f64
        ));
        assert!(
            seq_desired(&[], &[], song, BAR).is_empty(),
            "empty notes must not keep sounding"
        );
        let _ = notes;
    }

    #[test]
    fn event_cursor_matches_scan() {
        let mut g = Graph::new(48_000.0, 1);
        let voice = g.add_node(Box::new(Oscillator::new(Waveform::Sine, 440.0)));
        let mut notes: Vec<SeqNote> = (0..250)
            .map(|i| SeqNote {
                step: (i * 3) % 32,
                pitch: 40 + (i % 20) as u8,
                len: 1 + (i % 5) as u32,
            })
            .collect();
        notes.push(SeqNote {
            step: 30,
            pitch: 72,
            len: 8,
        });
        let targets = [SeqTarget {
            voice,
            pitch: 0,
            delay: 0.0,
            gate: 0.0,
        }];
        let loop_len = 8.0;
        let mut seq = SeqRun {
            notes: notes.clone(),
            clock_id: "clk".into(),
            windows: Vec::new(),
            loop_beats: loop_len,
            pos: 0.0,
            playhead_out: Arc::new(AtomicU32::new(0)),
            targets: targets.to_vec(),
            taps: Vec::new(),
            events: compile_seq_events(&notes, &targets, &[], loop_len),
            next_ev: 0,
            counts: HashMap::new(),
            tap_counts: HashMap::new(),
            held: Vec::new(),
            was_active: false,
            need_init: true,
        };
        let clocks = HashMap::from([(
            "clk".into(),
            ClockRun {
                out: Arc::new(AtomicU32::new(0)),
            },
        )]);
        let mut holds = HashMap::new();
        let mut tap_holds = HashMap::new();
        let mut song = 0.0;
        let dt = 0.02;
        for i in 0..900 {
            song += dt;
            tick_seq(&mut g, &clocks, &mut holds, &mut tap_holds, &mut seq, song);
            if i % 15 != 0 {
                continue;
            }
            let want = seq_desired(&notes, &targets, song, loop_len);
            let got: HashSet<_> = seq.held.iter().copied().collect();
            assert_eq!(got, want, "held diverged at song={song}");
        }
    }

    fn patch_with(nodes: Vec<crate::graph::GraphNode>, links: Vec<(&str, &str, &str, &str)>) -> Patch {
        Patch {
            playing: false,
            bpm: 120.0,
            seek_gen: 0,
            seek_beats: 0.0,
            output_id: "out".into(),
            nodes,
            links: links
                .into_iter()
                .map(|(a, b, c, d)| (a.into(), b.into(), c.into(), d.into()))
                .collect(),
            captures: HashMap::new(),
            preview_seq: None,
        }
    }

    #[test]
    fn notes_tap_does_not_duplicate_voice() {
        let seq = crate::graph::GraphNode::new("seq".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
        let join = crate::graph::GraphNode::new("join".into(), NodeKind::NoteJoin, glam::Vec2::ZERO);
        let notes = crate::graph::GraphNode::new("notes".into(), NodeKind::NoteScope, glam::Vec2::ZERO);
        let voice = crate::graph::GraphNode::new("voice".into(), NodeKind::Voice, glam::Vec2::ZERO);
        let patch = patch_with(
            vec![seq, join, notes, voice],
            vec![
                ("seq", "notes", "join", "1"),
                ("join", "out", "voice", "notes"),
                ("join", "out", "notes", "in"),
            ],
        );
        let mon = Monitor::default();
        let voices = build_graph(&patch, 48_000.0, &mon).voices;
        let (targets, taps) = seq_routes(&patch, "seq", &voices);
        assert_eq!(targets.len(), 1);
        assert_eq!(taps.len(), 1);
        let thru = patch_with(
            patch.nodes.clone(),
            vec![
                ("seq", "notes", "join", "1"),
                ("join", "out", "notes", "in"),
                ("notes", "out", "voice", "notes"),
            ],
        );
        let (thru_t, thru_tap) = seq_routes(&thru, "seq", &voices);
        assert_eq!(thru_t.len(), 1);
        assert_eq!(thru_tap.len(), 1);
    }

    #[test]
    fn guitar_is_seq_target() {
        let seq = crate::graph::GraphNode::new("seq".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
        let gtr = crate::graph::GraphNode::new("gtr".into(), NodeKind::Guitar, glam::Vec2::ZERO);
        let patch = patch_with(vec![seq, gtr], vec![("seq", "notes", "gtr", "notes")]);
        let mon = Monitor::default();
        let build = build_graph(&patch, 48_000.0, &mon);
        assert!(build.dsp.contains_key("gtr"));
        let (targets, _) = seq_routes(&patch, "seq", &build.voices);
        assert_eq!(targets.len(), 1);
    }

    #[test]
    fn notes_link_does_not_rebuild_dsp_topo() {
        let voice = crate::graph::GraphNode::new("voice".into(), NodeKind::Voice, glam::Vec2::ZERO);
        let seq = crate::graph::GraphNode::new("seq".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
        let notes = crate::graph::GraphNode::new("notes".into(), NodeKind::NoteScope, glam::Vec2::ZERO);
        let a = patch_with(vec![voice.clone(), seq.clone()], vec![("seq", "notes", "voice", "notes")]);
        let b = patch_with(
            vec![voice, seq, notes],
            vec![
                ("seq", "notes", "voice", "notes"),
                ("seq", "notes", "notes", "in"),
            ],
        );
        assert_eq!(a.topo_hash(), b.topo_hash());
    }

    #[test]
    fn bypass_does_not_rebuild_dsp_topo() {
        let mut a = crate::graph::GraphNode::new("f".into(), NodeKind::Filter, glam::Vec2::ZERO);
        let b = a.clone();
        a.bypass = true;
        let pa = patch_with(vec![a], vec![]);
        let pb = patch_with(vec![b], vec![]);
        assert_eq!(pa.topo_hash(), pb.topo_hash());
        let mut da = crate::graph::GraphDoc::blank();
        da.nodes = pa.nodes;
        let mut db = crate::graph::GraphDoc::blank();
        db.nodes = pb.nodes;
        assert_ne!(da.fingerprint(), db.fingerprint());
    }

    #[test]
    fn transpose_bypass_skips_shift() {
        let seq = crate::graph::GraphNode::new("seq".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
        let mut tr = crate::graph::GraphNode::new("tr".into(), NodeKind::Transpose, glam::Vec2::ZERO);
        tr.transpose_octaves = 1;
        let voice = crate::graph::GraphNode::new("voice".into(), NodeKind::Voice, glam::Vec2::ZERO);
        let patch = patch_with(
            vec![seq, tr, voice],
            vec![
                ("seq", "notes", "tr", "in"),
                ("tr", "out", "voice", "notes"),
            ],
        );
        let mon = Monitor::default();
        let voices = build_graph(&patch, 48_000.0, &mon).voices;
        let (targets, _) = seq_routes(&patch, "seq", &voices);
        assert_eq!(targets[0].pitch, 12);
        let mut bypassed = patch.clone();
        bypassed.nodes.iter_mut().find(|n| n.id == "tr").unwrap().bypass = true;
        let (t2, _) = seq_routes(&bypassed, "seq", &voices);
        assert_eq!(t2[0].pitch, 0);
    }

    #[test]
    fn notescope_bypass_drops_taps() {
        let seq = crate::graph::GraphNode::new("seq".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
        let mut notes = crate::graph::GraphNode::new("notes".into(), NodeKind::NoteScope, glam::Vec2::ZERO);
        notes.bypass = true;
        let voice = crate::graph::GraphNode::new("voice".into(), NodeKind::Voice, glam::Vec2::ZERO);
        let patch = patch_with(
            vec![seq, notes, voice],
            vec![
                ("seq", "notes", "notes", "in"),
                ("notes", "out", "voice", "notes"),
            ],
        );
        let mon = Monitor::default();
        let voices = build_graph(&patch, 48_000.0, &mon).voices;
        let (targets, taps) = seq_routes(&patch, "seq", &voices);
        assert_eq!(targets.len(), 1);
        assert!(taps.is_empty());
    }

    #[test]
    fn chord_fans_root_into_triad() {
        let seq = crate::graph::GraphNode::new("seq".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
        let chord = crate::graph::GraphNode::new("ch".into(), NodeKind::Chord, glam::Vec2::ZERO);
        let voice = crate::graph::GraphNode::new("voice".into(), NodeKind::Voice, glam::Vec2::ZERO);
        let patch = patch_with(
            vec![seq, chord, voice],
            vec![
                ("seq", "notes", "ch", "in"),
                ("ch", "out", "voice", "notes"),
            ],
        );
        let mon = Monitor::default();
        let voices = build_graph(&patch, 48_000.0, &mon).voices;
        let (mut targets, _) = seq_routes(&patch, "seq", &voices);
        targets.sort_by_key(|t| t.pitch);
        let pitches: Vec<i32> = targets.iter().map(|t| t.pitch).collect();
        assert_eq!(pitches, vec![0, 4, 7]);
        let mut bypassed = patch.clone();
        bypassed.nodes.iter_mut().find(|n| n.id == "ch").unwrap().bypass = true;
        let (t2, _) = seq_routes(&bypassed, "seq", &voices);
        assert_eq!(t2.len(), 1);
        assert_eq!(t2[0].pitch, 0);
    }

    #[test]
    fn arp_staggers_voice_targets() {
        let seq = crate::graph::GraphNode::new("seq".into(), NodeKind::Sequencer, glam::Vec2::ZERO);
        let arp = crate::graph::GraphNode::new("arp".into(), NodeKind::Arp, glam::Vec2::ZERO);
        let voice = crate::graph::GraphNode::new("voice".into(), NodeKind::Voice, glam::Vec2::ZERO);
        let patch = patch_with(
            vec![seq, arp, voice],
            vec![
                ("seq", "notes", "arp", "in"),
                ("arp", "out", "voice", "notes"),
            ],
        );
        let mon = Monitor::default();
        let voices = build_graph(&patch, 48_000.0, &mon).voices;
        let (mut targets, _) = seq_routes(&patch, "seq", &voices);
        targets.sort_by_key(|t| t.delay.to_bits());
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].pitch, 0);
        assert_eq!(targets[1].pitch, 4);
        assert_eq!(targets[2].pitch, 7);
        assert!(targets[1].delay > targets[0].delay);
        assert!(targets[0].gate > 0.0);
    }

    #[test]
    fn filter_node_wires_highpass() {
        let mut n = crate::graph::GraphNode::new("f".into(), NodeKind::Filter, glam::Vec2::ZERO);
        n.filter_kind = 1;
        let patch = patch_with(vec![n], vec![]);
        let mon = Monitor::default();
        let mut build = build_graph(&patch, 48_000.0, &mon);
        let id = *build.dsp.get("f").expect("filter dsp");
        let f = build.graph.node_mut::<BiquadFilter>(id).expect("biquad");
        assert_eq!(f.kind, FilterKind::HighPass);
        assert!(!f.cutoff_from_cv);
    }

    #[test]
    fn filter_cutoff_cable_replaces_knob() {
        let osc = crate::graph::GraphNode::new("osc".into(), NodeKind::Osc, glam::Vec2::ZERO);
        let mut filt = crate::graph::GraphNode::new("f".into(), NodeKind::Filter, glam::Vec2::ZERO);
        filt.cutoff = 8000.0;
        let patch = patch_with(
            vec![osc, filt],
            vec![("osc", "out", "f", "cutoff")],
        );
        let mon = Monitor::default();
        let mut build = build_graph(&patch, 48_000.0, &mon);
        let id = *build.dsp.get("f").expect("filter dsp");
        let f = build.graph.node_mut::<BiquadFilter>(id).expect("biquad");
        assert!(f.cutoff_from_cv);
    }

    #[test]
    fn spectrum_nodes_are_thru_dsp() {
        assert!(dsp_kind(NodeKind::Spectrum));
        assert!(dsp_kind(NodeKind::Spectrogram));
        assert_eq!(dsp_bypass(NodeKind::Spectrum), Bypass::Thru);
        assert_eq!(dsp_bypass(NodeKind::Spectrogram), Bypass::Thru);
        let spec = crate::graph::GraphNode::new("s".into(), NodeKind::Spectrum, glam::Vec2::ZERO);
        let gram = crate::graph::GraphNode::new("g".into(), NodeKind::Spectrogram, glam::Vec2::ZERO);
        let patch = patch_with(vec![spec, gram], vec![]);
        let mon = Monitor::default();
        let build = build_graph(&patch, 48_000.0, &mon);
        assert!(build.dsp.contains_key("s"));
        assert!(build.dsp.contains_key("g"));
    }

    #[test]
    fn audio_in_is_capture_dsp() {
        assert!(dsp_kind(NodeKind::AudioIn));
        assert_eq!(dsp_bypass(NodeKind::AudioIn), Bypass::Mute);
        let n = crate::graph::GraphNode::new("mic".into(), NodeKind::AudioIn, glam::Vec2::ZERO);
        let patch = patch_with(vec![n], vec![]);
        let mon = Monitor::default();
        let build = build_graph(&patch, 48_000.0, &mon);
        assert!(build.dsp.contains_key("mic"));
    }

    #[test]
    fn fft_tap_copies_and_peaks_on_a440() {
        let mon = Monitor::default();
        let mut tap = FftTap::new(mon.fft_buf("spec"), 48_000.0);
        let sr = 48_000.0;
        let mut phase = 0.0f32;
        let step = 2.0 * std::f32::consts::PI * 440.0 / sr;
        let ctx = ProcessContext {
            sample_rate: sr,
            block_size: 256,
        };
        for _ in 0..8 {
            let mut block = vec![0.0f32; 256];
            for s in block.iter_mut() {
                *s = phase.sin();
                phase += step;
            }
            let mut out = vec![0.0f32; 256];
            tap.process(&ctx, &[&block], &mut [&mut out[..]]);
            assert_eq!(out, block);
        }
        let bins = mon.spectrum("spec");
        let peak = crate::fft::peak_bin(&bins);
        assert!(peak < crate::fft::SPEC_BINS / 2);
        assert!(bins[peak] > 0.15);
        let gram = mon.spectrogram("spec");
        assert_eq!(gram.len(), crate::fft::SPEC_COLS * crate::fft::SPEC_BINS);
        assert!(gram.iter().any(|v| *v > 0.15));
    }

    #[test]
    fn eq_curve_cuts_when_gain_zero() {
        let sr = 48_000.0;
        let ctx = ProcessContext {
            sample_rate: sr,
            block_size: 1,
        };
        let mut pass = CurveEq::new(sr);
        pass.set_curve(&[(0.0, 1.0), (1.0, 1.0)]);
        let mut mute = CurveEq::new(sr);
        mute.set_curve(&[(0.0, 0.0), (1.0, 0.0)]);
        let step = 2.0 * std::f32::consts::PI * 440.0 / sr;
        let mut phase = 0.0f32;
        let mut pass_e = 0.0f32;
        let mut mute_e = 0.0f32;
        let n = FFT_N + EQ_HOP * 4;
        for _ in 0..n {
            let x = phase.sin();
            phase += step;
            let mut po = [0.0];
            let mut mo = [0.0];
            pass.process(&ctx, &[&[x]], &mut [&mut po]);
            mute.process(&ctx, &[&[x]], &mut [&mut mo]);
            pass_e += po[0] * po[0];
            mute_e += mo[0] * mo[0];
        }
        assert!(pass_e > 1.0, "flat EQ should pass energy, got {pass_e}");
        assert!(
            mute_e < pass_e * 0.05,
            "zero curve should kill the tone ({mute_e} vs {pass_e})"
        );
    }

    #[test]
    fn mixer_hard_pan_left_reaches_master() {
        let osc = crate::graph::GraphNode::new("osc".into(), NodeKind::Osc, glam::Vec2::ZERO);
        let mut mix = crate::graph::GraphNode::new("mix".into(), NodeKind::Mixer, glam::Vec2::ZERO);
        let out = crate::graph::GraphNode::new("out".into(), NodeKind::Output, glam::Vec2::ZERO);
        mix.ensure_mix_strips();
        mix.mix_strips[0].pan = -1.0;
        let mut patch = patch_with(
            vec![osc, mix, out],
            vec![
                ("osc", "out", "mix", "1"),
                ("mix", "out", "out", "in"),
            ],
        );
        patch.playing = true;
        let mon = Monitor::default();
        let mut build = build_graph(&patch, 48_000.0, &mon);
        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        for _ in 0..512 {
            let _ = build.graph.process();
            let (l, r) = build.graph.master_lr();
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
        }
        assert!(peak_l > 0.01, "left silent {peak_l}");
        assert!(
            peak_r < peak_l * 0.05,
            "right leaked {peak_r} vs left {peak_l}"
        );
    }

    #[test]
    fn stereo_pan_survives_delay() {
        let osc = crate::graph::GraphNode::new("osc".into(), NodeKind::Osc, glam::Vec2::ZERO);
        let mut mix = crate::graph::GraphNode::new("mix".into(), NodeKind::Mixer, glam::Vec2::ZERO);
        let mut delay = crate::graph::GraphNode::new("dl".into(), NodeKind::Delay, glam::Vec2::ZERO);
        delay.delay_mix = 0.0;
        let out = crate::graph::GraphNode::new("out".into(), NodeKind::Output, glam::Vec2::ZERO);
        mix.ensure_mix_strips();
        mix.mix_strips[0].pan = -1.0;
        let mut patch = patch_with(
            vec![osc, mix, delay, out],
            vec![
                ("osc", "out", "mix", "1"),
                ("mix", "out", "dl", "in"),
                ("dl", "out", "out", "in"),
            ],
        );
        patch.playing = true;
        let mon = Monitor::default();
        let mut build = build_graph(&patch, 48_000.0, &mon);
        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        for _ in 0..512 {
            let _ = build.graph.process();
            let (l, r) = build.graph.master_lr();
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
        }
        assert!(peak_l > 0.01, "left silent {peak_l}");
        assert!(
            peak_r < peak_l * 0.05,
            "delay collapsed stereo {peak_r} vs {peak_l}"
        );
    }

    #[test]
    fn mixer_wired_vol_cv_ignores_slider_flag() {
        let osc = crate::graph::GraphNode::new("osc".into(), NodeKind::Osc, glam::Vec2::ZERO);
        let mix = crate::graph::GraphNode::new("mix".into(), NodeKind::Mixer, glam::Vec2::ZERO);
        let out = crate::graph::GraphNode::new("out".into(), NodeKind::Output, glam::Vec2::ZERO);
        let patch = patch_with(
            vec![osc, mix, out],
            vec![
                ("osc", "out", "mix", "1"),
                ("osc", "out", "mix", "v1"),
                ("mix", "out", "out", "in"),
            ],
        );
        let mon = Monitor::default();
        let mut build = build_graph(&patch, 48_000.0, &mon);
        let id = *build.dsp.get("mix").unwrap();
        let mix = build.graph.node_mut::<StereoMixer>(id).unwrap();
        assert!(mix.vol_from_cv[0]);
        assert!(!mix.pan_from_cv[0]);
    }

    #[test]
    fn pan_node_cv_replaces_knob() {
        let osc = crate::graph::GraphNode::new("osc".into(), NodeKind::Osc, glam::Vec2::ZERO);
        let mut pan = crate::graph::GraphNode::new("p".into(), NodeKind::Pan, glam::Vec2::ZERO);
        pan.pan = -1.0;
        let patch = patch_with(vec![osc, pan], vec![("osc", "out", "p", "pan")]);
        let mon = Monitor::default();
        let mut build = build_graph(&patch, 48_000.0, &mon);
        let id = *build.dsp.get("p").expect("pan dsp");
        let p = build.graph.node_mut::<StereoPan>(id).expect("stereo pan");
        assert!(p.pan_from_cv);
    }

    #[test]
    fn reverb_and_comp_are_dsp() {
        assert!(dsp_kind(NodeKind::Reverb));
        assert!(dsp_kind(NodeKind::Compressor));
        assert_eq!(dsp_bypass(NodeKind::Reverb), Bypass::Thru);
        assert_eq!(dsp_bypass(NodeKind::Compressor), Bypass::Thru);
        let rv = crate::graph::GraphNode::new("rv".into(), NodeKind::Reverb, glam::Vec2::ZERO);
        let cp = crate::graph::GraphNode::new("cp".into(), NodeKind::Compressor, glam::Vec2::ZERO);
        let patch = patch_with(vec![rv, cp], vec![]);
        let mon = Monitor::default();
        let build = build_graph(&patch, 48_000.0, &mon);
        assert!(build.dsp.contains_key("rv"));
        assert!(build.dsp.contains_key("cp"));
    }

    #[test]
    fn instrument_flattens_into_voice() {
        let mut p = crate::graph::Project::new_default();
        let inst_id = p.instruments[0].id.clone();
        let host = p.main.spawn_node(NodeKind::Instrument, glam::Vec2::ZERO);
        if let Some(n) = p.main.nodes.iter_mut().find(|n| n.id == host) {
            n.inst_id = inst_id;
        }
        let seq = p
            .main
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Sequencer)
            .unwrap()
            .id
            .clone();
        let _ = p.main.connect(&seq, "notes", &host, "notes");
        let gain = p
            .main
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Gain)
            .unwrap()
            .id
            .clone();
        let _ = p.main.connect(&host, "out", &gain, "in");
        let patch = Patch::from_project(&p, false);
        assert!(!patch.nodes.iter().any(|n| n.kind == NodeKind::Instrument));
        assert!(patch.nodes.iter().any(|n| n.kind == NodeKind::Voice && n.id.starts_with(&format!("{host}__"))));
        assert!(patch.links.iter().any(|l| l.0 == seq && l.2.starts_with(&format!("{host}__"))));
    }

    #[test]
    fn sequence_view_plays_only_that_seq() {
        let mut p = crate::graph::Project::new_default();
        p.view = crate::graph::EditorView::Sequence("s1".into());
        let patch = Patch::from_project(&p, true);
        let seqs: Vec<_> = patch
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Sequencer)
            .collect();
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0].notes, p.sequences[0].notes);
        assert!(patch
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Voice && n.waveform == 0));
    }

    #[test]
    fn sequence_view_uses_chosen_instrument() {
        let mut p = crate::graph::Project::new_default();
        p.sequences[0].play_inst = p.instruments[0].id.clone();
        p.view = crate::graph::EditorView::Sequence("s1".into());
        let patch = Patch::from_project(&p, true);
        assert!(!patch.nodes.iter().any(|n| n.kind == NodeKind::Instrument));
        assert!(patch.nodes.iter().any(|n| n.kind == NodeKind::Voice && n.id.starts_with("play__")));
        assert_eq!(patch.preview_seq.as_deref(), Some("seq"));
        let mon = Monitor::default();
        let (live, _) = Live::new(&patch, 48_000.0, std::sync::Arc::new(mon));
        assert!(!live.preview_targets.is_empty());
    }

    #[test]
    fn sequence_view_preview_uses_play_voice() {
        let mut p = crate::graph::Project::new_default();
        p.view = crate::graph::EditorView::Sequence("s1".into());
        let patch = Patch::from_project(&p, false);
        assert_eq!(patch.preview_seq.as_deref(), Some("seq"));
        assert!(!patch.playing);
        let mon = Monitor::default();
        let (live, _) = Live::new(&patch, 48_000.0, std::sync::Arc::new(mon));
        assert!(!live.preview_targets.is_empty());
    }

    #[test]
    fn instrument_view_plays_that_graph() {
        let mut p = crate::graph::Project::new_default();
        let inst_id = p.instruments[0].id.clone();
        p.view = crate::graph::EditorView::Instrument(inst_id);
        let patch = Patch::from_project(&p, false);
        assert_eq!(patch.preview_seq.as_deref(), Some("keys"));
        assert!(patch.playing);
        assert!(!patch.nodes.iter().any(|n| n.kind == NodeKind::Sequencer && n.id != "keys"));
        assert!(patch.nodes.iter().any(|n| n.kind == NodeKind::Voice && !n.id.starts_with("play__")));
        let mon = Monitor::default();
        let (live, _) = Live::new(&patch, 48_000.0, std::sync::Arc::new(mon));
        assert!(!live.preview_targets.is_empty());
        assert!(!patch.nodes.iter().any(|n| n.kind == NodeKind::Clock));

        let heard = Patch::from_project(&p, true);
        let keys = heard
            .nodes
            .iter()
            .find(|n| n.id == "keys")
            .unwrap();
        assert!(heard.nodes.iter().any(|n| n.kind == NodeKind::Clock));
        assert_eq!(
            keys.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            vec![48, 60, 72]
        );
    }

    #[test]
    fn instrument_view_keeps_scope_id() {
        let mut p = crate::graph::Project::new_default();
        let inst_id = p.instruments[0].id.clone();
        let g = &mut p.instruments[0].graph;
        let voice = g
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Voice)
            .unwrap()
            .id
            .clone();
        let scope = g.spawn_node(NodeKind::Scope, glam::Vec2::ZERO);
        let out = g.output_id.clone();
        g.space.links.retain(|l| l.to_node != out);
        let _ = g.connect(&voice, "out", &scope, "in");
        let _ = g.connect(&scope, "out", &out, "in");
        p.view = crate::graph::EditorView::Instrument(inst_id);
        let patch = Patch::from_project(&p, false);
        assert!(
            patch
                .nodes
                .iter()
                .any(|n| n.kind == NodeKind::Scope && n.id == scope),
            "scope id must match the editor node so Waveform can read the tap"
        );
    }

    #[test]
    fn instrument_clock_feeds_trance_gate() {
        let mut p = crate::graph::Project::new_default();
        let inst_id = p.instruments[0].id.clone();
        let g = &mut p.instruments[0].graph;
        let gate = g.spawn_node(NodeKind::TranceGate, glam::Vec2::ZERO);
        p.view = crate::graph::EditorView::Instrument(inst_id);
        p.preview_clock = true;
        let patch = Patch::from_project(&p, false);
        assert!(patch.nodes.iter().any(|n| n.kind == NodeKind::Clock));
        assert!(
            patch
                .links
                .iter()
                .any(|l| l.2 == gate && l.3 == "clock"),
            "preview clock must reach Trance Gate"
        );
        let silent = {
            p.preview_clock = false;
            Patch::from_project(&p, false)
        };
        assert!(!silent.nodes.iter().any(|n| n.kind == NodeKind::Clock));
    }
}
