//! Editor graph → mega-audio graph, plus sequencer tick on the audio thread.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use mega_audio::dsp::{
    AdsrParams, BiquadFilter, Chorus, Clamp, Delay, Distortion, FilterKind, GainCv, Mixer, Mul,
    Oscillator, Remap, Waveform,
};
use mega_audio::graph::{Bypass, Graph, Node, NodeId, ProcessContext};
use mega_audio::instrument::PolyphonicInstrument;
use mega_audio::note::NoteEvent;

use crate::graph::{
    midi_shift, output_port_type, parse_seq_when, port, seq_window, GraphDoc, NodeKind, SeqNote,
    BEATS_PER_BAR, BEATS_PER_STEP, NOTE_JOIN_INS,
};
use crate::monitor::{Monitor, ScopeBuf};

pub const WAVEFORMS: [(&str, Waveform); 5] = [
    ("Sine", Waveform::Sine),
    ("Saw", Waveform::Saw),
    ("Square", Waveform::Square),
    ("Triangle", Waveform::Triangle),
    ("Noise", Waveform::Noise),
];

pub const MASTER_GAIN: f32 = 0.18;

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

fn dsp_bypass(kind: NodeKind) -> Bypass {
    match kind {
        NodeKind::Osc | NodeKind::Voice | NodeKind::Lfo => Bypass::Mute,
        NodeKind::Filter
        | NodeKind::Gain
        | NodeKind::Mix
        | NodeKind::Delay
        | NodeKind::Distortion
        | NodeKind::Chorus
        | NodeKind::Mul
        | NodeKind::Clamp
        | NodeKind::Remap
        | NodeKind::Scope => Bypass::Thru,
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
            | NodeKind::Mix
            | NodeKind::Delay
            | NodeKind::Distortion
            | NodeKind::Chorus
            | NodeKind::Mul
            | NodeKind::Clamp
            | NodeKind::Remap
            | NodeKind::Scope
            | NodeKind::Voice
    )
}

pub struct Build {
    pub graph: Graph,
    pub voices: HashMap<String, NodeId>,
    pub dsp: HashMap<String, NodeId>,
    pub lfo_mix: HashMap<String, NodeId>,
    pub master: NodeId,
}

pub fn build_graph(patch: &Patch, sample_rate: f32, monitor: &Monitor) -> Build {
    let mut graph = Graph::new(sample_rate, 1);
    let mut out_port: HashMap<(String, String), (NodeId, usize)> = HashMap::new();
    let mut in_port: HashMap<(String, String), (NodeId, usize)> = HashMap::new();
    let mut voices = HashMap::new();
    let mut dsp = HashMap::new();
    let mut lfo_mix_ids = HashMap::new();

    for n in &patch.nodes {
        match n.kind {
            NodeKind::Output
            | NodeKind::Clock
            | NodeKind::Sequencer
            | NodeKind::NoteJoin
            | NodeKind::Transpose
            | NodeKind::NoteScope => {}
            NodeKind::Voice => {
                let wf = WAVEFORMS.get(n.waveform).map(|w| w.1).unwrap_or(Waveform::Saw);
                let id = graph.add_node(Box::new(PolyphonicInstrument::new(
                    8,
                    sample_rate,
                    1,
                    wf,
                    voice_adsr(n),
                    n.pulse_width,
                )));
                voices.insert(n.id.clone(), id);
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "pitch".into()), (id, 0));
                in_port.insert((n.id.clone(), "amp".into()), (id, 1));
                in_port.insert((n.id.clone(), "pwm".into()), (id, 2));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Osc => {
                let wf = WAVEFORMS.get(n.waveform).map(|w| w.1).unwrap_or(Waveform::Saw);
                let mut osc = Oscillator::new(wf, n.freq.max(1.0));
                osc.pulse_width = n.pulse_width.clamp(0.02, 0.98);
                let id = graph.add_node(Box::new(osc));
                dsp.insert(n.id.clone(), id);
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
                in_port.insert((n.id.clone(), "fm".into()), (id, 0));
                in_port.insert((n.id.clone(), "pwm".into()), (id, 1));
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
                out_port.insert((n.id.clone(), "out".into()), (amp, 0));
                in_port.insert((n.id.clone(), "rate".into()), (osc, 0));
                in_port.insert((n.id.clone(), "depth".into()), (amp, 1));
            }
            NodeKind::Filter => {
                let id = graph.add_node(Box::new(BiquadFilter::new(
                    FilterKind::LowPass,
                    n.cutoff.max(20.0),
                    n.q.max(0.1),
                    sample_rate,
                )));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), (id, 0));
                in_port.insert((n.id.clone(), "cutoff".into()), (id, 1));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Gain => {
                let mut mix = Mixer::new(1);
                mix.gains[0] = n.gain;
                let id = graph.add_node(Box::new(mix));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), (id, 0));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Mix => {
                let mut mix = Mixer::new(2);
                mix.gains[0] = n.mix_a;
                mix.gains[1] = n.mix_b;
                let id = graph.add_node(Box::new(mix));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "a".into()), (id, 0));
                in_port.insert((n.id.clone(), "b".into()), (id, 1));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Scope => {
                let tap = ScopeTap {
                    buf: monitor.scope_buf(&n.id),
                };
                let id = graph.add_node(Box::new(tap));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), (id, 0));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Delay => {
                let mut delay = Delay::new(2.0, sample_rate);
                delay.delay_time = n.delay_time.clamp(0.02, 1.8);
                delay.feedback = n.delay_feedback.clamp(0.0, 0.92);
                delay.mix = n.delay_mix.clamp(0.0, 1.0);
                let id = graph.add_node(Box::new(delay));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), (id, 0));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Distortion => {
                let id = graph.add_node(Box::new(Distortion::new(n.drive.max(0.05))));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), (id, 0));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Chorus => {
                let mut ch = Chorus::new(sample_rate);
                ch.rate = n.chorus_rate.max(0.01);
                ch.depth = n.chorus_depth.clamp(0.0, 1.0);
                ch.mix = n.chorus_mix.clamp(0.0, 1.0);
                let id = graph.add_node(Box::new(ch));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), (id, 0));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Mul => {
                let id = graph.add_node(Box::new(Mul));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "a".into()), (id, 0));
                in_port.insert((n.id.clone(), "b".into()), (id, 1));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Clamp => {
                let id = graph.add_node(Box::new(Clamp::new(n.clamp_min, n.clamp_max)));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), (id, 0));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Remap => {
                let id = graph.add_node(Box::new(Remap::new(
                    n.map_in_min,
                    n.map_in_max,
                    n.map_out_min,
                    n.map_out_max,
                )));
                dsp.insert(n.id.clone(), id);
                in_port.insert((n.id.clone(), "in".into()), (id, 0));
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
        }
    }

    for (from, from_p, to, to_p) in &patch.links {
        if to == &patch.output_id {
            continue;
        }
        let Some(&(src, sp)) = out_port.get(&(from.clone(), from_p.clone())) else {
            continue;
        };
        let Some(&(dst, dp)) = in_port.get(&(to.clone(), to_p.clone())) else {
            continue;
        };
        graph.connect(src, sp, dst, dp);
    }

    let mut master = Mixer::new(1);
    master.gains[0] = if patch.playing { MASTER_GAIN } else { 0.0 };
    let master_id = graph.add_node(Box::new(master));

    if let Some((from, from_p, _, _)) = patch
        .links
        .iter()
        .find(|(_, _, to, to_p)| to == &patch.output_id && to_p == "in")
    {
        if let Some(&(src, sp)) = out_port.get(&(from.clone(), from_p.clone())) {
            graph.connect(src, sp, master_id, 0);
        }
    }

    graph.set_master_output(master_id, 0);
    Build {
        graph,
        voices,
        dsp,
        lfo_mix: lfo_mix_ids,
        master: master_id,
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
    held: Vec<(NodeId, u8)>,
    want: Vec<(NodeId, u8)>,
    was_active: bool,
}

pub struct Live {
    sample_rate: f32,
    playing: bool,
    topo: u64,
    voices: HashMap<String, NodeId>,
    dsp: HashMap<String, NodeId>,
    lfo_mix: HashMap<String, NodeId>,
    master: NodeId,
    clocks: HashMap<String, ClockRun>,
    seqs: HashMap<String, SeqRun>,
    taps: HashMap<String, Arc<crate::monitor::PitchSet>>,
    voice_holds: HashMap<NodeId, HashMap<u8, u32>>,
    monitor: Arc<Monitor>,
    song_beats: f64,
    bpm: f32,
    seek_gen: u64,
}

impl Live {
    pub fn new(patch: &Patch, sample_rate: f32, monitor: Arc<Monitor>) -> (Self, Graph) {
        let build = build_graph(patch, sample_rate, &monitor);
        let mut graph = build.graph;
        let mut live = Self {
            sample_rate,
            playing: patch.playing,
            topo: patch.topo_hash(),
            voices: build.voices,
            dsp: build.dsp,
            lfo_mix: build.lfo_mix,
            master: build.master,
            clocks: HashMap::new(),
            seqs: HashMap::new(),
            taps: HashMap::new(),
            voice_holds: HashMap::new(),
            monitor,
            song_beats: patch.seek_beats.max(0.0),
            bpm: patch.bpm.max(1.0),
            seek_gen: patch.seek_gen,
        };
        live.rebuild_clocks(patch);
        live.rebuild_taps(patch);
        let _ = live.rebuild_seqs(patch, false);
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
            next.insert(
                n.id.clone(),
                SeqRun {
                    notes: n.notes.clone(),
                    clock_id,
                    windows: parse_seq_when(&n.seq_when),
                    loop_beats: n.loop_beats(),
                    pos,
                    playhead_out,
                    targets,
                    taps,
                    held,
                    want: Vec::new(),
                    was_active,
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
                        f.cutoff = n.cutoff.max(20.0);
                        f.q = n.q.max(0.1);
                    }
                }
                NodeKind::Gain => {
                    if let Some(mix) = graph.node_mut::<Mixer>(id) {
                        mix.gains[0] = n.gain;
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
                NodeKind::Mix => {
                    if let Some(mix) = graph.node_mut::<Mixer>(id) {
                        mix.gains[0] = n.mix_a;
                        mix.gains[1] = n.mix_b;
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
            let build = build_graph(&patch, self.sample_rate, &self.monitor);
            *graph = build.graph;
            self.voices = build.voices;
            self.dsp = build.dsp;
            self.lfo_mix = build.lfo_mix;
            self.master = build.master;
            self.topo = topo;
            self.voice_holds.clear();
        }
        self.rebuild_clocks(&patch);
        self.rebuild_taps(&patch);
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

        if let Some(mixer) = graph.node_mut::<Mixer>(self.master) {
            mixer.gains[0] = if patch.playing { MASTER_GAIN } else { 0.0 };
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
            seq.held.clear();
            seq.was_active = false;
        }
        self.voice_holds.clear();
    }

    pub fn tick(&mut self, graph: &mut Graph) {
        self.clear_taps();
        if self.playing {
            let step = self.bpm as f64 / (self.sample_rate as f64 * 60.0);
            self.song_beats += step;
            let song = self.song_beats;
            for seq in self.seqs.values_mut() {
                tick_seq(graph, &self.clocks, &mut self.voice_holds, seq, song);
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
    seq: &mut SeqRun,
    song: f64,
) {
    if !clocks.contains_key(&seq.clock_id) {
        if seq.was_active {
            seq_release(graph, holds, seq);
        }
        return;
    }
    let win = seq_window(&seq.windows, song);
    if seq.was_active && win.is_none() {
        seq_release(graph, holds, seq);
        return;
    }
    if win.is_none() {
        return;
    }
    seq.pos = song;
    seq.was_active = true;
    let loop_len = seq.loop_beats.max(BEATS_PER_BAR as f64);
    collect_now(seq, song, loop_len);
    let mut i = 0;
    while i < seq.held.len() {
        let (id, pitch) = seq.held[i];
        if seq.want.contains(&(id, pitch)) {
            i += 1;
            continue;
        }
        voice_note(graph, holds, id, NoteEvent::NoteOff { note: pitch });
        seq.held.swap_remove(i);
    }
    for &(id, pitch) in &seq.want {
        if seq.held.contains(&(id, pitch)) {
            continue;
        }
        voice_note(
            graph,
            holds,
            id,
            NoteEvent::NoteOn {
                note: pitch,
                velocity: 0.8,
            },
        );
        seq.held.push((id, pitch));
    }
}

fn seq_release(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    seq: &mut SeqRun,
) {
    let held = seq.held.clone();
    for (id, pitch) in held {
        voice_note(graph, holds, id, NoteEvent::NoteOff { note: pitch });
    }
    seq.held.clear();
    seq.was_active = false;
}

fn collect_now(seq: &mut SeqRun, song: f64, loop_len: f64) {
    seq.want.clear();
    let max_step = (loop_len / BEATS_PER_STEP as f64).round().max(1.0) as u32;
    for note in &seq.notes {
        if note.step as u32 >= max_step {
            continue;
        }
        let dur = note.len.max(1) as f64 * BEATS_PER_STEP as f64;
        for t in &seq.targets {
            let on = (note.step as f64 * BEATS_PER_STEP as f64 + t.delay).rem_euclid(loop_len);
            let off = (on + dur).rem_euclid(loop_len);
            if sounding_at(song, on, off, loop_len, dur) {
                let key = (t.voice, midi_shift(note.pitch, t.pitch));
                if !seq.want.contains(&key) {
                    seq.want.push(key);
                }
            }
        }
        for t in &seq.taps {
            let on = (note.step as f64 * BEATS_PER_STEP as f64 + t.delay).rem_euclid(loop_len);
            let off = (on + dur).rem_euclid(loop_len);
            if sounding_at(song, on, off, loop_len, dur) {
                t.slot.insert(midi_shift(note.pitch, t.pitch));
            }
        }
    }
}

fn seq_desired(
    notes: &[SeqNote],
    targets: &[SeqTarget],
    song: f64,
    loop_len: f64,
) -> HashSet<(NodeId, u8)> {
    let mut seq = SeqRun {
        notes: notes.to_vec(),
        clock_id: String::new(),
        windows: Vec::new(),
        loop_beats: loop_len,
        pos: 0.0,
        playhead_out: Arc::new(AtomicU32::new(0)),
        targets: targets.to_vec(),
        taps: Vec::new(),
        held: Vec::new(),
        want: Vec::new(),
        was_active: false,
    };
    collect_now(&mut seq, song, loop_len);
    seq.want.into_iter().collect()
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
        1
    }

    fn num_outputs(&self) -> usize {
        1
    }

    fn process(&mut self, ctx: &ProcessContext, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        let n = ctx.block_size;
        outputs[0][..n].copy_from_slice(&inputs[0][..n]);
        for &s in &inputs[0][..n] {
            self.buf.push(s);
        }
    }

    fn name(&self) -> &'static str {
        "Scope"
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
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
}

#[derive(Clone)]
struct TapRun {
    slot: Arc<crate::monitor::PitchSet>,
    pitch: i32,
    delay: f64,
}

#[derive(Clone, PartialEq)]
struct TapTarget {
    id: String,
    pitch: i32,
    delay: f64,
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
    let mut stack = vec![(seq_id.to_string(), "notes".to_string(), 0_i32, 0.0_f64)];
    let mut seen = HashSet::new();
    while let Some((from, from_p, pitch, delay)) = stack.pop() {
        if !seen.insert((from.clone(), from_p.clone(), pitch, delay.to_bits())) {
            continue;
        }
        for (f, fp, to, to_p) in &patch.links {
            if f != &from || fp != &from_p {
                continue;
            }
            match (kind_of(to), to_p.as_str()) {
                (Some(NodeKind::Voice), "notes") => {
                    if let Some(&id) = voices.get(to) {
                        out.push(SeqTarget {
                            voice: id,
                            pitch,
                            delay,
                        });
                    }
                }
                (Some(NodeKind::NoteJoin), p) if NOTE_JOIN_INS.contains(&p) => {
                    stack.push((to.clone(), "out".into(), pitch, delay));
                }
                (Some(NodeKind::NoteScope), "in") => {
                    let bypass = node(to).is_some_and(|n| n.bypass);
                    if !bypass {
                        taps.push(TapTarget {
                            id: to.clone(),
                            pitch,
                            delay,
                        });
                    }
                    stack.push((to.clone(), "out".into(), pitch, delay));
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
                    stack.push((to.clone(), "out".into(), pitch, delay));
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
}
