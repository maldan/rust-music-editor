use std::sync::Arc;

use glam::Vec2;
use mega_ui::{NodePortSide, PlotView, Ui};

use crate::compile::WAVEFORMS;
use crate::graph::{
    port, GraphDoc, GraphNode, NodeKind, NOTE_JOIN_INS, SEQ_MAX_BARS, SEQ_OCTAVE_MIN,
};
use crate::monitor::Monitor;

use super::piano;

pub fn draw(ui: &mut Ui, doc: &mut GraphDoc, monitor: &Arc<Monitor>) -> bool {
    let mut keep = false;
    let size = ui.available_size();
    let size = Vec2::new(size.x, size.y.max(120.0));

    let mut spawn_at: Option<(NodeKind, Vec2)> = None;

    {
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
                let title = nodes[idx].kind.title().to_string();
                let mut pos = nodes[idx].pos;
                ui.node(&id, &title, &mut pos, |ui| {
                    draw_body(ui, &mut nodes[idx], monitor);
                });
                nodes[idx].pos = pos;
            }
        });

        let bg = space.background_hovered;
        let world = space.context_world.unwrap_or(Vec2::new(80.0, 80.0));
        let mut spawn_kind = None;
        ui.context_menu("music_spawn", bg, |ui| {
            spawn_kind = spawn_menu(ui);
        });
        if let Some(kind) = spawn_kind {
            spawn_at = Some((kind, world));
        }
    }

    if let Some((kind, world)) = spawn_at {
        doc.spawn_node(kind, world);
        keep = true;
    }

    keep
}

fn spawn_menu(ui: &mut Ui) -> Option<NodeKind> {
    let mut kind = None;
    if ui.menu_item("Clock").clicked() {
        kind = Some(NodeKind::Clock);
    }
    if ui.menu_item("Sequencer").clicked() {
        kind = Some(NodeKind::Sequencer);
    }
    if ui.menu_item("Voice").clicked() {
        kind = Some(NodeKind::Voice);
    }
    if ui.menu_item("Join Notes").clicked() {
        kind = Some(NodeKind::NoteJoin);
    }
    if ui.menu_item("Transpose").clicked() {
        kind = Some(NodeKind::Transpose);
    }
    if ui.menu_item("Notes").clicked() {
        kind = Some(NodeKind::NoteScope);
    }
    ui.separator();
    if ui.menu_item("Oscillator").clicked() {
        kind = Some(NodeKind::Osc);
    }
    if ui.menu_item("LFO").clicked() {
        kind = Some(NodeKind::Lfo);
    }
    if ui.menu_item("Filter").clicked() {
        kind = Some(NodeKind::Filter);
    }
    if ui.menu_item("Gain").clicked() {
        kind = Some(NodeKind::Gain);
    }
    if ui.menu_item("Join Audio").clicked() {
        kind = Some(NodeKind::Mix);
    }
    if ui.menu_item("Waveform").clicked() {
        kind = Some(NodeKind::Scope);
    }
    if ui.menu_item("Delay / Echo").clicked() {
        kind = Some(NodeKind::Delay);
    }
    if ui.menu_item("Distortion").clicked() {
        kind = Some(NodeKind::Distortion);
    }
    if ui.menu_item("Chorus").clicked() {
        kind = Some(NodeKind::Chorus);
    }
    if ui.menu_item("Multiply").clicked() {
        kind = Some(NodeKind::Mul);
    }
    if ui.menu_item("Clamp").clicked() {
        kind = Some(NodeKind::Clamp);
    }
    if ui.menu_item("Remap").clicked() {
        kind = Some(NodeKind::Remap);
    }
    kind
}

fn draw_body(ui: &mut Ui, node: &mut GraphNode, monitor: &Monitor) {
    let names: Vec<&str> = WAVEFORMS.iter().map(|(n, _)| *n).collect();
    match node.kind {
        NodeKind::Clock => {
            ui.node_port(NodePortSide::Output, "clock", port::CLOCK);
        }
        NodeKind::Sequencer => {
            ui.node_port(NodePortSide::Input, "clock", port::CLOCK);
            ui.node_port(NodePortSide::Output, "clock", port::CLOCK);
            ui.label("When");
            ui.text_input("when", &mut node.seq_when);
            roll_chrome(ui, node);
            let id = node.id.clone();
            let playhead = monitor.playhead(&id).filter(|p| p.is_finite());
            piano::draw_in_node(ui, &id, node, playhead);
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
        NodeKind::Mix => {
            ui.node_port(NodePortSide::Input, "a", port::AUDIO);
            ui.node_port(NodePortSide::Input, "b", port::AUDIO);
            labeled_slider(ui, "Volume A", &mut node.mix_a, 0.0..=1.5);
            labeled_slider(ui, "Volume B", &mut node.mix_b, 0.0..=1.5);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Scope => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            let samples = monitor.scope_samples(&node.id);
            let view = PlotView::new(0.0, 1.0, -1.0, 1.0);
            ui.plot_with_view("wave", Vec2::new(0.0, 72.0), &samples, &view);
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
