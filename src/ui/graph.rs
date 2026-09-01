use std::sync::Arc;

use glam::Vec2;
use mega_ui::{NodePortSide, PlotView, Ui};

use crate::compile::WAVEFORMS;
use crate::graph::{port, GraphDoc, GraphNode, NodeKind, BEATS_PER_BAR};
use crate::monitor::Monitor;

use super::piano;

pub fn draw(ui: &mut Ui, doc: &mut GraphDoc, monitor: &Arc<Monitor>, playing: bool) -> bool {
    let mut keep = false;
    let size = ui.available_size();
    let size = Vec2::new(size.x, size.y.max(120.0));

    let mut spawn_at: Option<(NodeKind, Vec2)> = None;

    {
        let space = &mut doc.space;
        let nodes = &mut doc.nodes;

        ui.node_space("music_graph", size, space, |ui| {
            let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
            for id in ids {
                let Some(idx) = nodes.iter().position(|n| n.id == id) else {
                    continue;
                };
                let title = nodes[idx].kind.title().to_string();
                let mut pos = nodes[idx].pos;
                ui.node(&id, &title, &mut pos, |ui| {
                    draw_body(ui, &mut nodes[idx], monitor, playing);
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
    kind
}

fn draw_body(ui: &mut Ui, node: &mut GraphNode, monitor: &Monitor, playing: bool) {
    let names: Vec<&str> = WAVEFORMS.iter().map(|(n, _)| *n).collect();
    match node.kind {
        NodeKind::Clock => {
            ui.node_port(NodePortSide::Output, "clock", port::CLOCK);
            labeled_slider(ui, "BPM", &mut node.bpm, 40.0..=200.0);
            let bar = if playing {
                monitor
                    .playhead(&node.id)
                    .filter(|b| b.is_finite())
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            ui.label(&format!("Bar {}", (bar / BEATS_PER_BAR).floor() as i32));
        }
        NodeKind::Sequencer => {
            ui.node_port(NodePortSide::Input, "clock", port::CLOCK);
            ui.label("Start bar");
            ui.drag_float("start", &mut node.seq_start, 1.0);
            node.seq_start = node.seq_start.max(0.0);
            ui.label("Bars (0 = forever)");
            ui.drag_float("bars", &mut node.seq_bars, 1.0);
            node.seq_bars = node.seq_bars.max(0.0);
            let id = node.id.clone();
            let playhead = if playing {
                monitor.playhead(&id).filter(|p| p.is_finite())
            } else {
                None
            };
            piano::draw_in_node(ui, &id, node, playhead);
            ui.node_port(NodePortSide::Output, "notes", port::NOTES);
        }
        NodeKind::Voice => {
            ui.node_port(NodePortSide::Input, "notes", port::NOTES);
            ui.label("Waveform");
            ui.select("wave", &mut node.waveform, &names);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Osc => {
            ui.node_port(NodePortSide::Input, "fm", port::AUDIO);
            ui.label("Waveform");
            ui.select("wave", &mut node.waveform, &names);
            labeled_slider(ui, "Frequency, Hz", &mut node.freq, 40.0..=880.0);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Lfo => {
            labeled_slider(ui, "Rate, Hz", &mut node.lfo_rate, 0.1..=20.0);
            labeled_slider(ui, "Depth, Hz", &mut node.lfo_depth, 0.0..=80.0);
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
            ui.node_port(NodePortSide::Input, "a", port::NOTES);
            ui.node_port(NodePortSide::Input, "b", port::NOTES);
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
        NodeKind::Delay => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
            labeled_slider(ui, "Delay time, sec", &mut node.delay_time, 0.05..=1.2);
            labeled_slider(ui, "Feedback", &mut node.delay_feedback, 0.0..=0.9);
            labeled_slider(ui, "Dry / Wet", &mut node.delay_mix, 0.0..=0.8);
            ui.node_port(NodePortSide::Output, "out", port::AUDIO);
        }
        NodeKind::Output => {
            ui.node_port(NodePortSide::Input, "in", port::AUDIO);
        }
    }
}

fn labeled_slider(ui: &mut Ui, name: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.label(name);
    ui.slider(name, value, range);
}
