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

use crate::graph::{
    midi_shift, parse_seq_when, seq_window, GraphDoc, NodeKind, SeqNote, BEATS_PER_BAR,
    BEATS_PER_STEP, NOTE_JOIN_INS,
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
            NodeKind::Output
            | NodeKind::Clock
            | NodeKind::Sequencer
            | NodeKind::NoteJoin
            | NodeKind::Transpose => {}
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
    out: Arc<AtomicU32>,
}

struct SeqRun {
    notes: Vec<SeqNote>,
    clock_id: String,
    windows: Vec<(f64, f64)>,
    pos: f64,
    playhead_out: Arc<AtomicU32>,
    targets: Vec<SeqTarget>,
    held: Vec<(NodeId, u8, f32)>,
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
    voice_holds: HashMap<NodeId, HashMap<u8, u32>>,
    monitor: Arc<Monitor>,
    song_beats: f64,
    bpm: f32,
    seek_gen: u64,
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
            voice_holds: HashMap::new(),
            monitor,
            song_beats: patch.seek_beats.max(0.0),
            bpm: patch.bpm.max(1.0),
            seek_gen: patch.seek_gen,
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
            let targets = seq_targets(patch, &n.id, &self.voices);
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
                    windows: parse_seq_when(&n.seq_when),
                    pos: prev.as_ref().map(|p| p.pos).unwrap_or(0.0),
                    playhead_out,
                    targets,
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
            self.voice_holds.clear();
        }
        self.rebuild_clocks(&patch);
        self.rebuild_seqs(&patch);
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
            .flat_map(|seq| seq.held.iter().map(|(id, p, _)| (*id, *p)))
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
        let loop_len = BEATS_PER_BAR as f64;
        if self.playing {
            let sr = self.sample_rate as f64;
            let step = self.bpm as f64 / (sr * 60.0);
            self.song_beats += step;
            let song = self.song_beats;
            let seq_ids: Vec<String> = self.seqs.keys().cloned().collect();
            for sid in seq_ids {
                let Some(seq) = self.seqs.get_mut(&sid) else {
                    continue;
                };
                if !self.clocks.contains_key(&seq.clock_id) {
                    continue;
                }
                let win = seq_window(&seq.windows, song);
                if seq.was_active && win.is_none() {
                    seq_release(graph, &mut self.voice_holds, seq);
                }
                if win.is_none() {
                    continue;
                }
                let now_pos = song;
                let prev_pos = if !seq.was_active { song - step } else { seq.pos };
                seq.pos = now_pos;
                seq.was_active = true;
                if seq.targets.is_empty() {
                    continue;
                }
                let targets = seq.targets.clone();
                for note in &seq.notes {
                    let dur = note.len.max(1) as f64 * BEATS_PER_STEP as f64;
                    for t in &targets {
                        let on = (note.step as f64 * BEATS_PER_STEP as f64 + t.delay)
                            .rem_euclid(loop_len);
                        let off = (on + dur).rem_euclid(loop_len);
                        let pitch = midi_shift(note.pitch, t.pitch);
                        if crossed(prev_pos, now_pos, on, loop_len) {
                            voice_note(
                                graph,
                                &mut self.voice_holds,
                                t.voice,
                                NoteEvent::NoteOn {
                                    note: pitch,
                                    velocity: 0.8,
                                },
                            );
                            seq.held.push((t.voice, pitch, off as f32));
                        }
                        if crossed(prev_pos, now_pos, off, loop_len) {
                            voice_note(
                                graph,
                                &mut self.voice_holds,
                                t.voice,
                                NoteEvent::NoteOff { note: pitch },
                            );
                            let off_f = off as f32;
                            if let Some(i) = seq.held.iter().position(|(vid, p, tm)| {
                                *vid == t.voice && *p == pitch && (*tm - off_f).abs() < 1e-5
                            }) {
                                seq.held.remove(i);
                            }
                        }
                    }
                }
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
        let loop_len = BEATS_PER_BAR as f64;
        let song = self.song_beats;
        for seq in self.seqs.values() {
            if seq_window(&seq.windows, song).is_some() {
                let now = song.rem_euclid(loop_len) as f32;
                seq.playhead_out.store(now.to_bits(), Ordering::Relaxed);
            } else {
                seq.playhead_out.store(f32::NAN.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

fn seq_release(
    graph: &mut Graph,
    holds: &mut HashMap<NodeId, HashMap<u8, u32>>,
    seq: &mut SeqRun,
) {
    let held: Vec<(NodeId, u8)> = seq.held.iter().map(|(id, p, _)| (*id, *p)).collect();
    for (id, pitch) in held {
        voice_note(graph, holds, id, NoteEvent::NoteOff { note: pitch });
    }
    seq.held.clear();
    seq.was_active = false;
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

fn seq_targets(patch: &Patch, seq_id: &str, voices: &HashMap<String, NodeId>) -> Vec<SeqTarget> {
    let node = |id: &str| patch.nodes.iter().find(|n| n.id == id);
    let kind_of = |id: &str| node(id).map(|n| n.kind);
    let mut out = Vec::new();
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
                (Some(NodeKind::Transpose), "in") => {
                    let n = node(to);
                    let pitch = pitch + n.map(|n| n.pitch_shift()).unwrap_or(0);
                    let delay = delay + n.map(|n| n.time_shift_beats()).unwrap_or(0.0);
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
    uniq
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
}
