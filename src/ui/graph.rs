use std::sync::Arc;

use glam::Vec2;
use mega_ui::{
    AnimationCurve, CurvePoint, CurvePreset, CursorIcon, NodePortSide, PlotView, Ui, flat_pass_curve,
};

use crate::compile::{wave_shape_of, WAVEFORMS};
use crate::fft::{
    freq_ticks, spec_window, t_to_freq, view_freq_ticks, view_note_ticks, SPEC_BINS, SPEC_COLS,
};
use crate::graph::{
    port, ARP_NAMES, CHORD_NAMES, FILTER_NAMES, GATE_DIV_NAMES, EqPt, GraphDoc, GraphNode, NodeKind, MIX_INS,
    NOTE_JOIN_INS, SEQ_OCTAVE_MIN,
};
use crate::monitor::Monitor;

use super::piano;

pub struct DeviceLists {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
}

impl DeviceLists {
    pub fn fetch() -> Self {
        Self {
            outputs: mega_audio::output_device_names(),
            inputs: mega_audio::input_device_names(),
        }
    }

    pub fn refresh(&mut self) {
        *self = Self::fetch();
    }
}

pub enum Spawn {
    Kind(NodeKind),
    Seq(String),
    Inst(String),
    Sample(String),
}

pub fn draw(
    ui: &mut Ui,
    doc: &mut GraphDoc,
    monitor: &Arc<Monitor>,
    sequences: &[(String, String)],
    instruments: &[(String, String)],
    samples: &[crate::graph::Sample],
    devices: &mut DeviceLists,
    instrument_graph: bool,
    bpm: f32,
) -> Option<f64> {
    let mut seek_beats = None;
    let size = ui.available_size();
    let size = Vec2::new(size.x, size.y.max(120.0));

    let mut spawn_at: Option<(Spawn, Vec2)> = None;

    let (bg, world, opened) = {
        let space = &mut doc.space;
        let nodes = &mut doc.nodes;

        space.bypassed_nodes.clear();
        for n in nodes.iter() {
            if n.bypass {
                space.bypassed_nodes.insert(n.id.clone());
            }
        }

        let cutoff_cv: Vec<String> = space
            .links
            .iter()
            .filter(|l| l.to_port == "cutoff")
            .map(|l| l.to_node.clone())
            .collect();
        let q_cv: Vec<String> = space
            .links
            .iter()
            .filter(|l| l.to_port == "q")
            .map(|l| l.to_node.clone())
            .collect();
        let pan_cv: Vec<String> = space
            .links
            .iter()
            .filter(|l| l.to_port == "pan")
            .map(|l| l.to_node.clone())
            .collect();
        let gain_cv: Vec<String> = space
            .links
            .iter()
            .filter(|l| l.to_port == "gain")
            .map(|l| l.to_node.clone())
            .collect();
        let lfo_rate_cv: Vec<String> = space
            .links
            .iter()
            .filter(|l| l.to_port == "rate")
            .map(|l| l.to_node.clone())
            .collect();
        let lfo_depth_cv: Vec<String> = space
            .links
            .iter()
            .filter(|l| l.to_port == "depth")
            .map(|l| l.to_node.clone())
            .collect();

        ui.node_space("music_graph", size, space, |ui| {
            let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
            for id in ids {
                let Some(idx) = nodes.iter().position(|n| n.id == id) else {
                    continue;
                };
                let title = node_title(&nodes[idx], sequences, instruments, samples);
                let mut pos = nodes[idx].pos;
                let cutoff_from_cv = cutoff_cv.iter().any(|n| n == &id);
                let q_from_cv = q_cv.iter().any(|n| n == &id);
                let pan_from_cv = pan_cv.iter().any(|n| n == &id);
                let gain_from_cv = gain_cv.iter().any(|n| n == &id);
                let lfo_rate_from_cv = lfo_rate_cv.iter().any(|n| n == &id);
                let lfo_depth_from_cv = lfo_depth_cv.iter().any(|n| n == &id);
                ui.node(&id, &title, &mut pos, |ui| {
                    draw_body(
                        ui,
                        &mut nodes[idx],
                        monitor,
                        sequences,
                        instruments,
                        samples,
                        devices,
                        cutoff_from_cv,
                        q_from_cv,
                        pan_from_cv,
                        gain_from_cv,
                        lfo_rate_from_cv,
                        lfo_depth_from_cv,
                        instrument_graph,
                        bpm,
                        &mut seek_beats,
                    );
                });
                nodes[idx].pos = pos;
            }
        });

        let bg = space.background_hovered;
        let world = space.context_world.unwrap_or(Vec2::new(80.0, 80.0));
        let opened = space.context_menu_request;
        (bg, world, opened)
    };

    if opened {
        doc.spawn_menu_page = 0;
    }
    let mut spawn_kind = None;
    ui.context_menu("music_spawn", bg, |ui| {
        spawn_kind = spawn_menu(ui, &mut doc.spawn_menu_page, sequences, instruments, samples, instrument_graph);
    });
    if !ui.context_menu_open() {
        doc.spawn_menu_page = 0;
    }
    if let Some(kind) = spawn_kind {
        spawn_at = Some((kind, world));
    }

    if let Some((spawn, world)) = spawn_at {
        match spawn {
            Spawn::Kind(kind) => {
                doc.spawn_node(kind, world);
            }
            Spawn::Seq(seq_id) => {
                let id = doc.spawn_node(NodeKind::Sequencer, world);
                if let Some(n) = doc.nodes.iter_mut().find(|n| n.id == id) {
                    n.seq_id = seq_id;
                }
            }
            Spawn::Inst(inst_id) => {
                let id = doc.spawn_node(NodeKind::Instrument, world);
                if let Some(n) = doc.nodes.iter_mut().find(|n| n.id == id) {
                    n.inst_id = inst_id;
                }
            }
            Spawn::Sample(sample_id) => {
                let id = doc.spawn_node(NodeKind::Sample, world);
                if let Some(n) = doc.nodes.iter_mut().find(|n| n.id == id) {
                    n.sample_id = sample_id;
                }
            }
        }
    }

    seek_beats
}

fn node_title(
    node: &GraphNode,
    sequences: &[(String, String)],
    instruments: &[(String, String)],
    samples: &[crate::graph::Sample],
) -> String {
    match node.kind {
        NodeKind::Sequencer => sequences
            .iter()
            .find(|(id, _)| *id == node.seq_id)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| node.kind.title().into()),
        NodeKind::Instrument => instruments
            .iter()
            .find(|(id, _)| *id == node.inst_id)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| node.kind.title().into()),
        NodeKind::Sample => samples
            .iter()
            .find(|s| s.id == node.sample_id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| node.kind.title().into()),
        _ => node.kind.title().into(),
    }
}

fn spawn_menu(
    ui: &mut Ui,
    page: &mut u8,
    sequences: &[(String, String)],
    instruments: &[(String, String)],
    samples: &[crate::graph::Sample],
    instrument_graph: bool,
) -> Option<Spawn> {
    const ROOT: u8 = 0;
    const NOTES: u8 = 1;
    const SYNTH: u8 = 2;
    const SOUND: u8 = 3;
    const FX: u8 = 4;
    const MATH: u8 = 5;
    const SEQS: u8 = 6;
    const INSTS: u8 = 7;
    const SMPS: u8 = 8;

    fn leaf(ui: &mut Ui, label: &str, kind: NodeKind) -> Option<Spawn> {
        ui.menu_item(label).clicked().then_some(Spawn::Kind(kind))
    }
    fn go(ui: &mut Ui, label: &str, page: &mut u8, to: u8) {
        if ui.menu_item_submenu(label).clicked() {
            *page = to;
        }
    }
    fn back(ui: &mut Ui, page: &mut u8) {
        if ui.menu_item_keep_open("Back").clicked() {
            *page = ROOT;
        }
    }

    match *page {
        SEQS => {
            back(ui, page);
            ui.separator();
            let mut hit = None;
            for (id, name) in sequences {
                if ui.menu_item(name).clicked() {
                    hit = Some(Spawn::Seq(id.clone()));
                }
            }
            hit
        }
        INSTS => {
            back(ui, page);
            ui.separator();
            let mut hit = None;
            for (id, name) in instruments {
                if ui.menu_item(name).clicked() {
                    hit = Some(Spawn::Inst(id.clone()));
                }
            }
            hit
        }
        SMPS => {
            back(ui, page);
            ui.separator();
            let mut hit = None;
            for s in samples {
                if ui.menu_item(&s.name).clicked() {
                    hit = Some(Spawn::Sample(s.id.clone()));
                }
            }
            hit
        }
        NOTES => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Clock", NodeKind::Clock)
                .or_else(|| leaf(ui, "Join Notes", NodeKind::NoteJoin))
                .or_else(|| leaf(ui, "Transpose", NodeKind::Transpose))
                .or_else(|| leaf(ui, "Chord", NodeKind::Chord))
                .or_else(|| leaf(ui, "Arp", NodeKind::Arp))
                .or_else(|| leaf(ui, "Notes", NodeKind::NoteScope))
                .or_else(|| leaf(ui, "Note Gate", NodeKind::NoteGate))
                .or_else(|| leaf(ui, "Note Hold", NodeKind::NoteHold))
                .or_else(|| leaf(ui, "Note Freq", NodeKind::NoteFreq))
                .or_else(|| leaf(ui, "Envelope", NodeKind::Envelope))
        }
        SYNTH => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Oscillator", NodeKind::Osc)
                .or_else(|| leaf(ui, "Simple Synth", NodeKind::Tone))
                .or_else(|| leaf(ui, "Shape Synth", NodeKind::Shape))
                .or_else(|| leaf(ui, "Basic Synth", NodeKind::Voice))
                .or_else(|| leaf(ui, "Guitar", NodeKind::Guitar))
                .or_else(|| leaf(ui, "Piano", NodeKind::Piano))
                .or_else(|| leaf(ui, "Drum Kit", NodeKind::Drums))
                .or_else(|| leaf(ui, "LFO", NodeKind::Lfo))
        }
        SOUND => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Filter", NodeKind::Filter)
                .or_else(|| leaf(ui, "Gain", NodeKind::Gain))
                .or_else(|| leaf(ui, "Pan", NodeKind::Pan))
                .or_else(|| leaf(ui, "Join Audio", NodeKind::Mix))
                .or_else(|| leaf(ui, "Morph", NodeKind::Morph))
                .or_else(|| leaf(ui, "Mixer", NodeKind::Mixer))
                .or_else(|| leaf(ui, "Audio In", NodeKind::AudioIn))
                .or_else(|| leaf(ui, "Waveform", NodeKind::Scope))
                .or_else(|| leaf(ui, "Spectrum", NodeKind::Spectrum))
                .or_else(|| leaf(ui, "Spectrogram", NodeKind::Spectrogram))
        }
        FX => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Delay / Echo", NodeKind::Delay)
                .or_else(|| leaf(ui, "Distortion", NodeKind::Distortion))
                .or_else(|| leaf(ui, "Chorus", NodeKind::Chorus))
                .or_else(|| leaf(ui, "Flanger", NodeKind::Flanger))
                .or_else(|| leaf(ui, "Reverb", NodeKind::Reverb))
                .or_else(|| leaf(ui, "Comp / Limit", NodeKind::Compressor))
                .or_else(|| leaf(ui, "EQ Curve", NodeKind::Eq))
                .or_else(|| leaf(ui, "Trance Gate", NodeKind::TranceGate))
        }
        MATH => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Value", NodeKind::Value)
                .or_else(|| leaf(ui, "Readout", NodeKind::Readout))
                .or_else(|| leaf(ui, "Multiply", NodeKind::Mul))
                .or_else(|| leaf(ui, "Clamp", NodeKind::Clamp))
                .or_else(|| leaf(ui, "Remap", NodeKind::Remap))
                .or_else(|| leaf(ui, "Smooth", NodeKind::Smooth))
        }
        _ => {
            if !instrument_graph {
                go(ui, "Sequences", page, SEQS);
                go(ui, "Instruments", page, INSTS);
                go(ui, "Samples", page, SMPS);
            }
            go(ui, "Notes", page, NOTES);
            go(ui, "Synth", page, SYNTH);
            go(ui, "Sound", page, SOUND);
            go(ui, "Effects", page, FX);
            go(ui, "Math", page, MATH);
            None
        }
    }
}

fn draw_body(
    ui: &mut Ui,
    node: &mut GraphNode,
    monitor: &Monitor,
    sequences: &[(String, String)],
    instruments: &[(String, String)],
    samples: &[crate::graph::Sample],
    devices: &mut DeviceLists,
    cutoff_from_cv: bool,
    q_from_cv: bool,
    pan_from_cv: bool,
    gain_from_cv: bool,
    lfo_rate_from_cv: bool,
    lfo_depth_from_cv: bool,
    instrument_graph: bool,
    bpm: f32,
    seek_beats: &mut Option<f64>,
) {
    let names: Vec<&str> = WAVEFORMS.iter().map(|(n, _)| *n).collect();
    match node.kind {
        NodeKind::Clock => {
            ui.node_port(NodePortSide::Output, "clock", port::CLOCK);
        }
        NodeKind::Sequencer => {
            ui.node_port(NodePortSide::Input, "clock", port::CLOCK);
            ui.node_port(NodePortSide::Output, "clock", port::CLOCK);
            ui.label("Sequence");
            let mut labels: Vec<&str> = vec!["None"];
            for (_, name) in sequences {
                labels.push(name.as_str());
            }
            let mut sel = sequences
                .iter()
                .position(|(id, _)| *id == node.seq_id)
                .map(|i| i + 1)
                .unwrap_or(0);
            ui.select(&format!("{}_seq", node.id), &mut sel, &labels);
            node.seq_id = if sel == 0 {
                String::new()
            } else {
                sequences[sel - 1].0.clone()
            };
            ui.label("When");
            ui.text_input("when", &mut node.seq_when);
            ui.node_port(NodePortSide::Output, "notes", port::NOTES);
        }
        NodeKind::Instrument => {
            ui.node_port(NodePortSide::Input, "clock", port::CLOCK);
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            if let Some((_, name)) = instruments.iter().find(|(id, _)| *id == node.inst_id) {
                ui.label(name);
            }
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Input => {
            ui.node_port(NodePortSide::Output, "clock", port::CLOCK);
            ui.node_port(NodePortSide::Output, "notes", port::NOTES);
        }
        NodeKind::AudioIn => {
            device_picker(ui, node, &devices.inputs.clone(), devices);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Sample => {
            let (name, peaks, dur) = samples
                .iter()
                .find(|s| s.id == node.sample_id)
                .map(|s| (s.name.as_str(), s.peaks.as_slice(), s.duration()))
                .unwrap_or(("", &[], 0.0));
            if !name.is_empty() {
                ui.label(name);
            }
            let secs = monitor.song_beats() as f32 * 60.0 / bpm.max(1.0);
            if let Some(t) = super::sample::draw_preview(ui, peaks, dur, secs) {
                *seek_beats = Some(t * bpm.max(1.0) as f64 / 60.0);
            }
            ui.request_repaint();
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Voice => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            ui.node_port(NodePortSide::Input, "freq", port::AUDIO);
            ui.node_port(NodePortSide::Input, "amp", port::AUDIO);
            ui.node_port(NodePortSide::Input, "pwm", port::AUDIO);
            ui.label("Waveform");
            ui.select("wave", &mut node.waveform, &names);
            ui.label("Pulse width");
            ui.drag_float("pw", &mut node.pulse_width, 0.01);
            node.pulse_width = node.pulse_width.clamp(0.02, 0.98);
            ui.label("Pitch");
            ui.drag_float("pitch", &mut node.pitch, 0.01);
            node.pitch = node.pitch.max(0.01);
            ui.group("Unison", |ui| {
                ui.horizontal(|ui| {
                    ui.knob("Amount", &mut node.unison, 1.0..=16.0);
                    ui.knob("Detune", &mut node.detune, 0.0..=100.0);
                    ui.knob("Pan", &mut node.unison_pan, 0.0..=1.0);
                });
            });
            super::adsr::draw(ui, node);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Tone => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            ui.label("Waveform");
            ui.select("wave", &mut node.waveform, &names);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Shape => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            ui.node_port(NodePortSide::Input, "freq", port::AUDIO);
            let mut preview = [0.0f32; 128];
            wave_shape_of(node).fill_preview(&mut preview);
            let view = PlotView::new(0.0, 1.0, -1.0, 1.0);
            ui.plot_with_view("shape_wave", Vec2::new(220.0, 72.0), &preview, &view);
            ui.label("OSC  (1 = note, 2 = octave, …)");
            node.ensure_wave_harms();
            let osc_ticks = [(0.0, "1"), (7.0 / 15.0, "8"), (1.0, "16")];
            ui.plot_bars_edit(
                "shape_osc",
                Vec2::new(220.0, 64.0),
                &mut node.wave_harms,
                &osc_ticks,
            );
            ui.horizontal(|ui| {
                ui.checkbox("Half", &mut node.wave_half);
                ui.checkbox("Pulse", &mut node.wave_pulse);
                ui.checkbox("Abs", &mut node.wave_abs);
            });
            ui.group("Wave", |ui| {
                ui.horizontal(|ui| {
                    ui.knob("SH", &mut node.wave_shape, 0.0..=1.0);
                    ui.knob("TN", &mut node.wave_tension, -1.0..=1.0);
                    ui.knob("SK", &mut node.wave_skew, 0.0..=1.0);
                });
                ui.horizontal(|ui| {
                    ui.knob("SN", &mut node.wave_sine, 0.0..=1.0);
                    ui.knob("FL", &mut node.wave_flip, 0.0..=1.0);
                    ui.knob("NS", &mut node.wave_noise, 0.0..=1.0);
                });
            });
            ui.label("Pitch");
            ui.drag_float("pitch", &mut node.pitch, 0.01);
            node.pitch = node.pitch.max(0.01);
            ui.group("Unison", |ui| {
                ui.horizontal(|ui| {
                    ui.knob("Amount", &mut node.unison, 1.0..=16.0);
                    ui.knob("Detune", &mut node.detune, 0.0..=100.0);
                    ui.knob("Pan", &mut node.unison_pan, 0.0..=1.0);
                });
            });
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Guitar => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            super::adsr::draw(ui, node);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Piano => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            super::adsr::draw(ui, node);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Drums => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            ui.label("C2 kick  D2 snare  F#2 hat");
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Osc => {
            ui.node_port(NodePortSide::Input, "fm", port::AUDIO);
            ui.node_port(NodePortSide::Input, "pwm", port::AUDIO);
            ui.label("Waveform");
            ui.select("wave", &mut node.waveform, &names);
            labeled_slider(ui, "Frequency, Hz", &mut node.freq, 40.0..=880.0);
            ui.label("Pulse width");
            ui.drag_float("pw", &mut node.pulse_width, 0.01);
            node.pulse_width = node.pulse_width.clamp(0.02, 0.98);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Lfo => {
            ui.node_port(NodePortSide::Input, "rate", port::AUDIO);
            if !lfo_rate_from_cv {
                ui.label("Rate, Hz");
                ui.drag_float("lfo_rate", &mut node.lfo_rate, 0.1);
                node.lfo_rate = node.lfo_rate.max(0.01);
            }
            ui.node_port(NodePortSide::Input, "depth", port::AUDIO);
            if !lfo_depth_from_cv {
                ui.label("Depth");
                ui.drag_float("lfo_depth", &mut node.lfo_depth, 1.0);
                node.lfo_depth = node.lfo_depth.max(0.0);
            }
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Filter => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.node_port(NodePortSide::Input, "cutoff", port::AUDIO);
            ui.node_port(NodePortSide::Input, "q", port::AUDIO);
            ui.select("filter_kind", &mut node.filter_kind, &FILTER_NAMES);
            if !cutoff_from_cv {
                labeled_slider(ui, "Cutoff, Hz", &mut node.cutoff, 20.0..=16_000.0);
            }
            if !q_from_cv {
                labeled_slider(ui, "Resonance", &mut node.q, 0.3..=8.0);
            }
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Eq => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label("Pass (top) / cut (bottom)");
            let mut curve = eq_to_curve(&node.eq_pts);
            let ticks = freq_ticks(48_000.0);
            let resp = ui.eq_curve_editor("eq", &mut curve, Vec2::new(320.0, 140.0), &ticks);
            if resp.changed {
                node.eq_pts = curve_to_eq(&curve);
            }
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::TranceGate => {
            ui.node_port(NodePortSide::Input, "clock", port::CLOCK);
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label("Steps");
            let ptr = ui.pointer();
            let z = ui.scale();
            let cell = 18.0 * z;
            for row in 0..2 {
                ui.horizontal(|ui| {
                    for col in 0..8 {
                        let i = row * 8 + col;
                        let on = node.gate_pattern & (1u16 << i) != 0;
                        let area = ui.area(&format!("gs{i}"), Vec2::new(cell, cell + 4.0 * z));
                        let color = if on {
                            [0.95, 0.72, 0.22, 1.0]
                        } else {
                            [0.16, 0.16, 0.18, 1.0]
                        };
                        ui.fill_round(area.rect.inset(1.0 * z), 2.0 * z, color);
                        if area.hovered {
                            ui.set_mouse_cursor(CursorIcon::Pointer);
                            if ptr.pressed {
                                node.gate_pattern ^= 1u16 << i;
                            }
                        }
                    }
                });
            }
            ui.label("Rate");
            ui.select("gate_div", &mut node.gate_div, &GATE_DIV_NAMES);
            ui.label("Smooth, sec");
            ui.drag_float("gate_smooth", &mut node.gate_smooth, 0.001);
            node.gate_smooth = node.gate_smooth.clamp(0.0, 0.08);
            ui.knob("Wet", &mut node.gate_mix, 0.0..=1.0);
            node.gate_mix = node.gate_mix.clamp(0.0, 1.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Gain => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.node_port(NodePortSide::Input, "gain", port::AUDIO);
            if !gain_from_cv {
                labeled_slider(ui, "Volume", &mut node.gain, 0.0..=1.5);
            }
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Pan => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.node_port(NodePortSide::Input, "pan", port::AUDIO);
            if !pan_from_cv {
                labeled_slider(ui, "Pan", &mut node.pan, -1.0..=1.0);
            }
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::NoteJoin => {
            for p in NOTE_JOIN_INS {
                ui.node_port(NodePortSide::Input, p, port::NOTES);
            }
            ui.node_port(NodePortSide::Output, "out", port::NOTES);
        }
        NodeKind::Transpose => {
            ui.node_port(NodePortSide::Input, "in", port::NOTES);
            ui.label("Notes");
            ui.drag_int("t_notes", &mut node.transpose_notes, 1);
            ui.label("Octaves");
            ui.drag_int("t_oct", &mut node.transpose_octaves, 1);
            ui.label("Steps");
            ui.drag_float("t_steps", &mut node.transpose_steps, 0.25);
            ui.node_port(NodePortSide::Output, "out", port::NOTES);
        }
        NodeKind::Chord => {
            ui.node_port(NodePortSide::Input, "in", port::NOTES);
            ui.select("chord", &mut node.chord_kind, &CHORD_NAMES);
            node.chord_kind = node.chord_kind.min(CHORD_NAMES.len() - 1);
            ui.node_port(NodePortSide::Output, "out", port::NOTES);
        }
        NodeKind::Arp => {
            ui.node_port(NodePortSide::Input, "in", port::NOTES);
            ui.select("chord", &mut node.chord_kind, &CHORD_NAMES);
            node.chord_kind = node.chord_kind.min(CHORD_NAMES.len() - 1);
            ui.select("arp_dir", &mut node.arp_mode, &ARP_NAMES);
            node.arp_mode = node.arp_mode.min(ARP_NAMES.len() - 1);
            ui.label("Rate, 16ths");
            ui.drag_float("arp_rate", &mut node.arp_rate, 0.25);
            node.arp_rate = node.arp_rate.clamp(0.25, 8.0);
            ui.node_port(NodePortSide::Output, "out", port::NOTES);
        }
        NodeKind::Mix => {
            ui.node_port(NodePortSide::Input, "a", port::AUDIO);
            ui.node_port(NodePortSide::Input, "b", port::AUDIO);
            labeled_slider(ui, "Volume A", &mut node.mix_a, 0.0..=1.5);
            labeled_slider(ui, "Volume B", &mut node.mix_b, 0.0..=1.5);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Morph => {
            ui.node_port(NodePortSide::Input, "a", port::AUDIO);
            ui.node_port(NodePortSide::Input, "b", port::AUDIO);
            labeled_slider(ui, "Morph", &mut node.morph, 0.0..=1.0);
            node.morph = node.morph.clamp(0.0, 1.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Mixer => {
            node.ensure_mix_strips();
            for name in MIX_INS {
                ui.node_port(NodePortSide::Input, name, port::AUDIO);
            }
            for row in 0..2 {
                ui.horizontal(|ui| {
                    for col in 0..4 {
                        let i = row * 4 + col;
                        ui.group(MIX_INS[i], |ui| {
                            ui.checkbox("Mute", &mut node.mix_strips[i].mute);
                            ui.horizontal(|ui| {
                                ui.knob("Vol", &mut node.mix_strips[i].vol, 0.0..=1.5);
                                ui.knob("Pan", &mut node.mix_strips[i].pan, -1.0..=1.0);
                            });
                        });
                    }
                });
            }
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Scope => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            let samples = monitor.scope_samples(&node.id);
            let view = PlotView::new(0.0, 1.0, -1.0, 1.0);
            ui.plot_with_view("wave", Vec2::new(0.0, 72.0), &samples, &view);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Spectrum => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            let bins = monitor.spectrum(&node.id);
            let ticks = freq_ticks(48_000.0);
            ui.plot_line("fft", Vec2::new(480.0, 110.0), &bins, &ticks);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Spectrogram => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            labeled_slider(ui, "Freq pos", &mut node.spec_pos, 0.0..=1.0);
            labeled_slider(ui, "Freq span", &mut node.spec_span, 0.04..=1.0);
            labeled_slider(ui, "Gate", &mut node.spec_floor, 0.0..=1.0);
            node.spec_span = node.spec_span.clamp(0.04, 1.0);
            node.spec_pos = node.spec_pos.clamp(0.0, 1.0 - node.spec_span);
            node.spec_floor = node.spec_floor.clamp(0.0, 1.0);
            let sr = 48_000.0;
            let t0 = node.spec_pos;
            let span = node.spec_span;
            ui.label(&format!(
                "{:.0}–{:.0} Hz",
                t_to_freq(t0, sr),
                t_to_freq(t0 + span, sr)
            ));
            let cells = spec_window(
                &monitor.spectrogram(&node.id),
                SPEC_COLS,
                SPEC_BINS,
                t0,
                span,
                node.spec_floor,
            );
            let ticks = view_freq_ticks(sr, t0, span);
            let notes = view_note_ticks(sr, t0, span);
            let note_refs: Vec<(f32, &str)> = notes.iter().map(|(t, s)| (*t, s.as_str())).collect();
            ui.plot_heatmap(
                "gram",
                Vec2::new(420.0, 384.0),
                SPEC_COLS,
                SPEC_BINS,
                &cells,
                &ticks,
                &note_refs,
            );
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::NoteScope => {
            ui.node_port(NodePortSide::Input, "in", port::NOTES);
            roll_chrome(ui, node);
            piano::draw_preview(ui, node, monitor);
            ui.node_port(NodePortSide::Output, "out", port::NOTES);
        }
        NodeKind::Delay => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            labeled_slider(ui, "Delay time, sec", &mut node.delay_time, 0.05..=1.2);
            labeled_slider(ui, "Feedback", &mut node.delay_feedback, 0.0..=0.9);
            labeled_slider(ui, "Dry / Wet", &mut node.delay_mix, 0.0..=0.8);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Distortion => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label("Drive");
            ui.drag_float("drive", &mut node.drive, 0.5);
            node.drive = node.drive.max(0.05);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Chorus => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label("Rate, Hz");
            ui.drag_float("ch_rate", &mut node.chorus_rate, 0.05);
            node.chorus_rate = node.chorus_rate.max(0.01);
            ui.label("Depth");
            ui.drag_float("ch_depth", &mut node.chorus_depth, 0.05);
            node.chorus_depth = node.chorus_depth.clamp(0.0, 1.0);
            ui.label("Mix");
            ui.drag_float("ch_mix", &mut node.chorus_mix, 0.05);
            node.chorus_mix = node.chorus_mix.clamp(0.0, 1.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Flanger => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label("Rate, Hz");
            ui.drag_float("fl_rate", &mut node.flange_rate, 0.05);
            node.flange_rate = node.flange_rate.max(0.01);
            ui.label("Depth");
            ui.drag_float("fl_depth", &mut node.flange_depth, 0.05);
            node.flange_depth = node.flange_depth.clamp(0.0, 1.0);
            ui.label("Feedback");
            ui.drag_float("fl_fb", &mut node.flange_feedback, 0.05);
            node.flange_feedback = node.flange_feedback.clamp(0.0, 0.95);
            ui.label("Mix");
            ui.drag_float("fl_mix", &mut node.flange_mix, 0.05);
            node.flange_mix = node.flange_mix.clamp(0.0, 1.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Reverb => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            labeled_slider(ui, "Room", &mut node.rev_room, 0.0..=1.0);
            labeled_slider(ui, "Damp", &mut node.rev_damp, 0.0..=1.0);
            labeled_slider(ui, "Dry / Wet", &mut node.rev_mix, 0.0..=1.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Compressor => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            labeled_slider(ui, "Threshold", &mut node.comp_thresh, 0.05..=1.0);
            labeled_slider(ui, "Ratio (20 = limit)", &mut node.comp_ratio, 1.0..=20.0);
            labeled_slider(ui, "Attack, sec", &mut node.comp_attack, 0.001..=0.15);
            labeled_slider(ui, "Release, sec", &mut node.comp_release, 0.02..=0.8);
            labeled_slider(ui, "Makeup", &mut node.comp_makeup, 0.5..=4.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Value => {
            ui.label("Value");
            ui.drag_float("value", &mut node.value, 0.01);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Readout => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label(&format!("{:.4}", monitor.meter_value(&node.id)));
            ui.request_repaint();
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::NoteGate | NodeKind::NoteHold | NodeKind::NoteFreq => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Envelope => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            super::env::draw(ui, node, monitor.playhead(&node.id));
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Smooth => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label("Time, ms");
            ui.drag_float("smooth_ms", &mut node.smooth_ms, 1.0);
            node.smooth_ms = node.smooth_ms.clamp(0.0, 5_000.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Mul => {
            ui.node_port(NodePortSide::Input, "a", port::AUDIO);
            ui.node_port(NodePortSide::Input, "b", port::AUDIO);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Clamp => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label("Min");
            ui.drag_float("cmin", &mut node.clamp_min, 0.1);
            ui.label("Max");
            ui.drag_float("cmax", &mut node.clamp_max, 0.1);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Remap => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.label("In min / max");
            let mut inn = Vec2::new(node.map_in_min, node.map_in_max);
            ui.vec2("map_in", &mut inn, 0.1, Vec2::new(-1.0, 1.0));
            node.map_in_min = inn.x;
            node.map_in_max = inn.y;
            ui.label("Out min / max");
            let mut out = Vec2::new(node.map_out_min, node.map_out_max);
            ui.vec2("map_out", &mut out, 0.1, Vec2::new(-1.0, 1.0));
            node.map_out_min = out.x;
            node.map_out_max = out.y;
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Output => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            if !instrument_graph {
                device_picker(ui, node, &devices.outputs.clone(), devices);
            }
        }
    }
}

fn roll_chrome(ui: &mut Ui, node: &mut GraphNode) {
    ui.horizontal(|ui| {
        ui.label("Bars");
        let mut bars = node.seq_loop_bars as i32;
        ui.drag_int("bars", &mut bars, 1);
        node.seq_loop_bars = bars.max(1) as u32;
    });
    ui.label("Octave");
    let mut oct = node.seq_octave as f32;
    let max = node.view_octave_max() as f32;
    ui.slider_stepped("oct", &mut oct, SEQ_OCTAVE_MIN as f32..=max, 1.0);
    node.seq_octave = oct.round() as i32;
}

fn labeled_slider(ui: &mut Ui, name: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.label(name);
    ui.slider(name, value, range);
}

fn device_picker(ui: &mut Ui, node: &mut GraphNode, names: &[String], lists: &mut DeviceLists) {
    ui.label("Device");
    let mut labels = vec!["Default".to_string()];
    labels.extend(names.iter().cloned());
    if !node.audio_device.is_empty() && !names.iter().any(|n| *n == node.audio_device) {
        labels.push(node.audio_device.clone());
    }
    let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let mut sel = if node.audio_device.is_empty() {
        0
    } else {
        labels
            .iter()
            .position(|l| l == &node.audio_device)
            .unwrap_or(0)
    };
    ui.select(&format!("{}_dev", node.id), &mut sel, &refs);
    node.audio_device = if sel == 0 {
        String::new()
    } else {
        labels.get(sel).cloned().unwrap_or_default()
    };
    if ui
        .button_with(&format!("{}_ref", node.id), |ui| {
            ui.label("Refresh");
        })
        .clicked
    {
        lists.refresh();
    }
}

fn eq_to_curve(pts: &[EqPt]) -> AnimationCurve {
    if pts.len() < 2 {
        return flat_pass_curve();
    }
    AnimationCurve {
        points: pts
            .iter()
            .map(|p| CurvePoint {
                t: p.t.clamp(0.0, 1.0),
                v: p.v.clamp(0.0, 1.0),
                tangent_out: 0.0,
            })
            .collect(),
        preset: CurvePreset::Custom,
    }
}

fn curve_to_eq(curve: &AnimationCurve) -> Vec<EqPt> {
    curve
        .points
        .iter()
        .map(|p| EqPt {
            t: p.t.clamp(0.0, 1.0),
            v: p.v.clamp(0.0, 1.0),
        })
        .collect()
}
