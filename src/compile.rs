//! Editor graph → mega-audio graph, plus sequencer tick on the audio thread.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use mega_audio::dsp::{
    AdsrParams, BiquadFilter, Delay, FilterKind, Mixer, Oscillator, Waveform,
};
use mega_audio::graph::{Graph, Node, NodeId, ProcessContext};
use mega_audio::instrument::PolyphonicInstrument;
use mega_audio::note::NoteEvent;

use crate::graph::{GraphDoc, NodeKind, SeqNote, BEATS_PER_BAR, BEATS_PER_STEP};
use crate::monitor::{Monitor, ScopeBuf};

pub const WAVEFORMS: [(&str, Waveform); 5] = [
    ("Sine", Waveform::Sine),
    ("Saw", Waveform::Saw),
    ("Square", Waveform::Square),
    ("Triangle", Waveform::Triangle),
    ("Noise", Waveform::Noise),
];

pub const MASTER_GAIN: f32 = 0.18;

#[derive(Clone)]
pub struct Patch {
    pub playing: bool,
    pub output_id: String,
    pub nodes: Vec<crate::graph::GraphNode>,
    pub links: Vec<(String, String, String, String)>,
}

impl Patch {
    pub fn from_doc(doc: &GraphDoc, playing: bool) -> Self {
        Self {
            playing,
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
            n.id.hash(&mut h);
            n.kind.hash(&mut h);
            if n.kind == NodeKind::Voice {
                n.waveform.hash(&mut h);
            }
        }
        for l in &self.links {
            l.0.hash(&mut h);
            l.1.hash(&mut h);
            l.2.hash(&mut h);
            l.3.hash(&mut h);
        }
        h.finish()
    }
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
            NodeKind::Output | NodeKind::Clock | NodeKind::Sequencer | NodeKind::NoteJoin => {}
            NodeKind::Voice => {
                let wf = WAVEFORMS.get(n.waveform).map(|w| w.1).unwrap_or(Waveform::Saw);
                let id = graph.add_node(Box::new(PolyphonicInstrument::new(
                    8,
                    sample_rate,
                    1,
                    wf,
                    AdsrParams::default(),
                )));
                voices.insert(n.id.clone(), id);
                dsp.insert(n.id.clone(), id);
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
            }
            NodeKind::Osc => {
                let wf = WAVEFORMS.get(n.waveform).map(|w| w.1).unwrap_or(Waveform::Saw);
                let id = graph.add_node(Box::new(Oscillator::new(wf, n.freq.max(1.0))));
                dsp.insert(n.id.clone(), id);
                out_port.insert((n.id.clone(), "out".into()), (id, 0));
                in_port.insert((n.id.clone(), "fm".into()), (id, 0));
            }
            NodeKind::Lfo => {
                let osc = graph.add_node(Box::new(Oscillator::new(
                    Waveform::Sine,
                    n.lfo_rate.max(0.01),
                )));
                let mut mix = Mixer::new(1);
                mix.gains[0] = n.lfo_depth;
                let mix_id = graph.add_node(Box::new(mix));
                dsp.insert(n.id.clone(), osc);
                lfo_mix_ids.insert(n.id.clone(), mix_id);
                graph.connect(osc, 0, mix_id, 0);
                out_port.insert((n.id.clone(), "out".into()), (mix_id, 0));
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
    bpm: f32,
    song_beats: f32,
    out: Arc<AtomicU32>,
}

struct SeqRun {
    notes: Vec<SeqNote>,
    clock_id: String,
    start_beats: f32,
    end_beats: f32,
    playhead: f32,
    playhead_out: Arc<AtomicU32>,
    voices: Vec<NodeId>,
    held: Vec<(u8, f32)>,
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
    monitor: Arc<Monitor>,
}

impl Live {
    pub fn new(patch: &Patch, sample_rate: f32, monitor: Arc<Monitor>) -> (Self, Graph) {
        let build = build_graph(patch, sample_rate, &monitor);
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
            monitor,
        };
        live.rebuild_clocks(patch);
        live.rebuild_seqs(patch);
        (live, build.graph)
    }

    fn rebuild_clocks(&mut self, patch: &Patch) {
        let mut next = HashMap::new();
        for n in &patch.nodes {
            if n.kind != NodeKind::Clock {
                continue;
            }
            let prev = self.clocks.remove(&n.id);
            next.insert(
                n.id.clone(),
                ClockRun {
                    bpm: n.bpm.max(1.0),
                    song_beats: prev.as_ref().map(|c| c.song_beats).unwrap_or(0.0),
                    out: prev
                        .map(|c| c.out)
                        .unwrap_or_else(|| self.monitor.playhead_slot(&n.id)),
                },
            );
        }
        self.clocks = next;
    }

    fn rebuild_seqs(&mut self, patch: &Patch) {
        let mut next = HashMap::new();
        for n in &patch.nodes {
            if n.kind != NodeKind::Sequencer {
                continue;
            }
            let Some(clock_id) = seq_clock_id(patch, &n.id) else {
                continue;
            };
            if !self.clocks.contains_key(&clock_id) {
                continue;
            }
            let start = n.seq_start.max(0.0).floor() * BEATS_PER_BAR;
            let bars = n.seq_bars.max(0.0).floor();
            let end = if bars < 1.0 {
                f32::INFINITY
            } else {
                start + bars * BEATS_PER_BAR
            };
            let voices = seq_voices(patch, &n.id, &self.voices);
            let prev = self.seqs.remove(&n.id);
            let playhead_out = prev
                .as_ref()
                .map(|p| p.playhead_out.clone())
                .unwrap_or_else(|| self.monitor.playhead_slot(&n.id));
            next.insert(
                n.id.clone(),
                SeqRun {
                    notes: n.notes.clone(),
                    clock_id,
                    start_beats: start,
                    end_beats: end,
                    playhead: prev.as_ref().map(|p| p.playhead).unwrap_or(0.0),
                    playhead_out,
                    voices,
                    held: prev.as_ref().map(|p| p.held.clone()).unwrap_or_default(),
                    was_active: prev.map(|p| p.was_active).unwrap_or(false),
                },
            );
        }
        self.seqs = next;
    }

    fn sync_dsp(&self, graph: &mut Graph, patch: &Patch) {
        for n in &patch.nodes {
            let Some(&id) = self.dsp.get(&n.id) else {
                continue;
            };
            match n.kind {
                NodeKind::Delay => {
                    if let Some(d) = graph.node_mut::<Delay>(id) {
                        d.delay_time = n.delay_time.clamp(0.02, 1.8);
                        d.feedback = n.delay_feedback.clamp(0.0, 0.92);
                        d.mix = n.delay_mix.clamp(0.0, 1.0);
                    }
                }
                NodeKind::Osc => {
                    if let Some(osc) = graph.node_mut::<Oscillator>(id) {
                        osc.frequency = n.freq.max(1.0);
                        osc.waveform = WAVEFORMS.get(n.waveform).map(|w| w.1).unwrap_or(Waveform::Saw);
                    }
                }
                NodeKind::Lfo => {
                    if let Some(osc) = graph.node_mut::<Oscillator>(id) {
                        osc.frequency = n.lfo_rate.max(0.01);
                    }
                    if let Some(&mix_id) = self.lfo_mix.get(&n.id) {
                        if let Some(mix) = graph.node_mut::<Mixer>(mix_id) {
                            mix.gains[0] = n.lfo_depth;
                        }
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
                NodeKind::Mix => {
                    if let Some(mix) = graph.node_mut::<Mixer>(id) {
                        mix.gains[0] = n.mix_a;
                        mix.gains[1] = n.mix_b;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn apply(&mut self, graph: &mut Graph, patch: Patch) {
        let topo = patch.topo_hash();
        if topo != self.topo {
            let build = build_graph(&patch, self.sample_rate, &self.monitor);
            *graph = build.graph;
            self.voices = build.voices;
            self.dsp = build.dsp;
            self.lfo_mix = build.lfo_mix;
            self.master = build.master;
            self.topo = topo;
        }
        self.rebuild_clocks(&patch);
        self.rebuild_seqs(&patch);
        self.sync_dsp(graph, &patch);

        if let Some(mixer) = graph.node_mut::<Mixer>(self.master) {
            mixer.gains[0] = if patch.playing { MASTER_GAIN } else { 0.0 };
        }

        if self.playing && !patch.playing {
            for seq in self.seqs.values_mut() {
                for &id in &seq.voices {
                    if let Some(inst) = graph.node_mut::<PolyphonicInstrument>(id) {
                        for &(pitch, _) in &seq.held {
                            inst.handle_event(NoteEvent::NoteOff { note: pitch });
                        }
                    }
                }
                seq.held.clear();
                seq.playhead = 0.0;
                seq.was_active = false;
            }
            for clock in self.clocks.values_mut() {
                clock.song_beats = 0.0;
            }
        }
        self.playing = patch.playing;
    }

    pub fn tick(&mut self, graph: &mut Graph) {
        if !self.playing {
            for seq in self.seqs.values() {
                seq.playhead_out.store(f32::NAN.to_bits(), Ordering::Relaxed);
            }
            for clock in self.clocks.values() {
                clock.out.store(0, Ordering::Relaxed);
            }
            return;
        }
        let dt = 1.0 / self.sample_rate / 60.0;
        let loop_len = BEATS_PER_BAR;
        for clock in self.clocks.values_mut() {
            clock.song_beats += clock.bpm * dt;
            clock.out.store(clock.song_beats.to_bits(), Ordering::Relaxed);
        }
        let seq_ids: Vec<String> = self.seqs.keys().cloned().collect();
        for sid in seq_ids {
            let Some(seq) = self.seqs.get_mut(&sid) else {
                continue;
            };
            let Some(song) = self.clocks.get(&seq.clock_id).map(|c| c.song_beats) else {
                continue;
            };
            let active = song >= seq.start_beats && song < seq.end_beats;
            if seq.was_active && !active {
                for &id in &seq.voices {
                    if let Some(inst) = graph.node_mut::<PolyphonicInstrument>(id) {
                        for &(pitch, _) in &seq.held {
                            inst.handle_event(NoteEvent::NoteOff { note: pitch });
                        }
                    }
                }
                seq.held.clear();
                seq.playhead_out.store(f32::NAN.to_bits(), Ordering::Relaxed);
                seq.was_active = false;
                continue;
            }
            if !active {
                seq.playhead_out.store(f32::NAN.to_bits(), Ordering::Relaxed);
                continue;
            }
            let now = (song - seq.start_beats) % loop_len;
            let prev = if !seq.was_active {
                (now - 1.0e-6 + loop_len) % loop_len
            } else {
                seq.playhead
            };
            seq.playhead = now;
            seq.playhead_out.store(now.to_bits(), Ordering::Relaxed);
            seq.was_active = true;
            if seq.voices.is_empty() {
                continue;
            }
            let mut events = Vec::new();
            for note in &seq.notes {
                let on = note.step as f32 * BEATS_PER_STEP;
                let off = (on + note.len.max(1) as f32 * BEATS_PER_STEP) % loop_len;
                if crossed(prev, now, on) {
                    events.push(NoteEvent::NoteOn {
                        note: note.pitch,
                        velocity: 0.8,
                    });
                    seq.held.push((note.pitch, off));
                }
                if crossed(prev, now, off) {
                    events.push(NoteEvent::NoteOff { note: note.pitch });
                    seq.held.retain(|(p, _)| *p != note.pitch);
                }
            }
            let voice_ids = seq.voices.clone();
            for id in voice_ids {
                if let Some(inst) = graph.node_mut::<PolyphonicInstrument>(id) {
                    for &e in &events {
                        inst.handle_event(e);
                    }
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
    let clock_id = patch.links.iter().find_map(|(from, _, to, to_p)| {
        (to == seq_id && to_p == "clock").then(|| from.clone())
    })?;
    patch
        .nodes
        .iter()
        .find(|n| n.id == clock_id && n.kind == NodeKind::Clock)
        .map(|n| n.id.clone())
}

fn seq_voices(patch: &Patch, seq_id: &str, voices: &HashMap<String, NodeId>) -> Vec<NodeId> {
    let kind_of = |id: &str| {
        patch
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.kind)
    };
    let mut out = Vec::new();
    let mut stack = vec![(seq_id.to_string(), "notes".to_string())];
    let mut seen = HashSet::new();
    while let Some((from, from_p)) = stack.pop() {
        if !seen.insert((from.clone(), from_p.clone())) {
            continue;
        }
        for (f, fp, to, to_p) in &patch.links {
            if f != &from || fp != &from_p {
                continue;
            }
            match (kind_of(to), to_p.as_str()) {
                (Some(NodeKind::Voice), "notes") => {
                    if let Some(&id) = voices.get(to) {
                        out.push(id);
                    }
                }
                (Some(NodeKind::NoteJoin), "a" | "b") => {
                    stack.push((to.clone(), "out".into()));
                }
                _ => {}
            }
        }
    }
    let mut uniq = Vec::new();
    for id in out {
        if !uniq.contains(&id) {
            uniq.push(id);
        }
    }
    uniq
}

fn crossed(prev: f32, now: f32, t: f32) -> bool {
    if now >= prev {
        t > prev && t <= now
    } else {
        t > prev || t <= now
    }
}
