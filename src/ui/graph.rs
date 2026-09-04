use std::sync::Arc;

use glam::Vec2;
use mega_ui::{
    AnimationCurve, CurvePoint, CurvePreset, NodePortSide, PlotView, Ui, flat_pass_curve,
};

use crate::compile::WAVEFORMS;
use crate::fft::{freq_ticks, SPEC_BINS, SPEC_COLS};
use crate::graph::{
    port, ARP_NAMES, CHORD_NAMES, EqPt, GraphDoc, GraphNode, NodeKind, MIX_INS, MIX_PAN_INS,
    MIX_VOL_INS, NOTE_JOIN_INS, SEQ_MAX_BARS, SEQ_OCTAVE_MIN,
};
use crate::monitor::Monitor;

use super::piano;

pub enum Spawn {
    Kind(NodeKind),
    Seq(String),
    Inst(String),
}

pub fn draw(
    ui: &mut Ui,
    doc: &mut GraphDoc,
    monitor: &Arc<Monitor>,
    sequences: &[(String, String)],
    instruments: &[(String, String)],
    instrument_graph: bool,
) -> bool {
    let mut keep = false;
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

        ui.node_space("music_graph", size, space, |ui| {
            let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
            for id in ids {
                let Some(idx) = nodes.iter().position(|n| n.id == id) else {
                    continue;
                };
                let title = node_title(&nodes[idx], sequences, instruments);
                let mut pos = nodes[idx].pos;
                ui.node(&id, &title, &mut pos, |ui| {
                    draw_body(ui, &mut nodes[idx], monitor, sequences, instruments);
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
        spawn_kind = spawn_menu(ui, &mut doc.spawn_menu_page, sequences, instruments, instrument_graph);
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
        }
        keep = true;
    }

    keep
}

fn node_title(
    node: &GraphNode,
    sequences: &[(String, String)],
    instruments: &[(String, String)],
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
        _ => node.kind.title().into(),
    }
}

fn spawn_menu(
    ui: &mut Ui,
    page: &mut u8,
    sequences: &[(String, String)],
    instruments: &[(String, String)],
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
        NOTES => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Clock", NodeKind::Clock)
                .or_else(|| leaf(ui, "Join Notes", NodeKind::NoteJoin))
                .or_else(|| leaf(ui, "Transpose", NodeKind::Transpose))
                .or_else(|| leaf(ui, "Chord", NodeKind::Chord))
                .or_else(|| leaf(ui, "Arp", NodeKind::Arp))
                .or_else(|| leaf(ui, "Notes", NodeKind::NoteScope))
        }
        SYNTH => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Oscillator", NodeKind::Osc)
                .or_else(|| leaf(ui, "Voice", NodeKind::Voice))
                .or_else(|| leaf(ui, "Guitar", NodeKind::Guitar))
                .or_else(|| leaf(ui, "LFO", NodeKind::Lfo))
        }
        SOUND => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Filter", NodeKind::Filter)
                .or_else(|| leaf(ui, "Gain", NodeKind::Gain))
                .or_else(|| leaf(ui, "Join Audio", NodeKind::Mix))
                .or_else(|| leaf(ui, "Mixer", NodeKind::Mixer))
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
        }
        MATH => {
            back(ui, page);
            ui.separator();
            leaf(ui, "Multiply", NodeKind::Mul)
                .or_else(|| leaf(ui, "Clamp", NodeKind::Clamp))
                .or_else(|| leaf(ui, "Remap", NodeKind::Remap))
        }
        _ => {
            if !instrument_graph {
                go(ui, "Sequences", page, SEQS);
                go(ui, "Instruments", page, INSTS);
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
) {
    let names: Vec<&str> = WAVEFORMS.iter().map(|(n, _)| *n).collect();
    match node.kind {
        NodeKind::Clock => {
            ui.node_port(NodePortSide::Output, "clock", port::CLOCK);
        }
        NodeKind::Sequencer => {
            ui.node_port(NodePortSide::Input, "clock", port::CLOCK);
            ui.node_port(NodePortSide::Output, "clock", port::CLOCK);
            if let Some((_, name)) = sequences.iter().find(|(id, _)| *id == node.seq_id) {
                ui.label(name);
            }
            ui.label("When");
            ui.text_input("when", &mut node.seq_when);
            ui.node_port(NodePortSide::Output, "notes", port::NOTES);
        }
        NodeKind::Instrument => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            if let Some((_, name)) = instruments.iter().find(|(id, _)| *id == node.inst_id) {
                ui.label(name);
            }
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Input => {
            ui.node_port(NodePortSide::Output, "notes", port::NOTES);
        }
        NodeKind::Voice => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            ui.node_port(NodePortSide::Input, "pitch", port::AUDIO);
            ui.node_port(NodePortSide::Input, "amp", port::AUDIO);
            ui.node_port(NodePortSide::Input, "pwm", port::AUDIO);
            ui.label("Waveform");
            ui.select("wave", &mut node.waveform, &names);
            ui.label("Pulse width");
            ui.drag_float("pw", &mut node.pulse_width, 0.01);
            node.pulse_width = node.pulse_width.clamp(0.02, 0.98);
            labeled_slider(ui, "Attack, sec", &mut node.adsr_attack, 0.001..=2.0);
            labeled_slider(ui, "Decay, sec", &mut node.adsr_decay, 0.01..=2.0);
            labeled_slider(ui, "Sustain", &mut node.adsr_sustain, 0.0..=1.0);
            labeled_slider(ui, "Release, sec", &mut node.adsr_release, 0.01..=4.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Guitar => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
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
            ui.label("Rate, Hz");
            ui.drag_float("lfo_rate", &mut node.lfo_rate, 0.1);
            node.lfo_rate = node.lfo_rate.max(0.01);
            ui.node_port(NodePortSide::Input, "depth", port::AUDIO);
            ui.label("Depth");
            ui.drag_float("lfo_depth", &mut node.lfo_depth, 1.0);
            node.lfo_depth = node.lfo_depth.max(0.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Filter => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            ui.node_port(NodePortSide::Input, "cutoff", port::AUDIO);
            labeled_slider(ui, "Cutoff, Hz", &mut node.cutoff, 80.0..=8000.0);
            labeled_slider(ui, "Resonance", &mut node.q, 0.3..=8.0);
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
        NodeKind::Gain => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            labeled_slider(ui, "Volume", &mut node.gain, 0.0..=1.5);
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
        NodeKind::Mixer => {
            node.ensure_mix_strips();
            for (i, name) in MIX_INS.iter().enumerate() {
                ui.node_port(NodePortSide::Input, name, port::AUDIO);
                ui.row(|ui| {
                    ui.slider(&format!("mv{i}"), &mut node.mix_strips[i].vol, 0.0..=1.5);
                    ui.slider(&format!("mp{i}"), &mut node.mix_strips[i].pan, -1.0..=1.0);
                });
                ui.node_port(NodePortSide::Input, MIX_VOL_INS[i], port::AUDIO);
                ui.node_port(NodePortSide::Input, MIX_PAN_INS[i], port::AUDIO);
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
            ui.plot_bars("fft", Vec2::new(320.0, 96.0), &bins, &ticks);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Spectrogram => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            let cells = monitor.spectrogram(&node.id);
            let ticks = freq_ticks(48_000.0);
            ui.plot_heatmap(
                "gram",
                Vec2::new(400.0, 140.0),
                SPEC_COLS,
                SPEC_BINS,
                &cells,
                &ticks,
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
            ui.label("In min");
            ui.drag_float("imin", &mut node.map_in_min, 0.1);
            ui.label("In max");
            ui.drag_float("imax", &mut node.map_in_max, 0.1);
            ui.label("Out min");
            ui.drag_float("omin", &mut node.map_out_min, 0.1);
            ui.label("Out max");
            ui.drag_float("omax", &mut node.map_out_max, 0.1);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Output => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
        }
    }
}

fn roll_chrome(ui: &mut Ui, node: &mut GraphNode) {
    ui.horizontal(|ui| {
        ui.label("Bars");
        let mut bars = node.seq_loop_bars as i32;
        ui.drag_int("bars", &mut bars, 1);
        node.seq_loop_bars = bars.clamp(1, SEQ_MAX_BARS as i32) as u32;
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
