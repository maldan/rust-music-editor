use mega_ui::Ui;

use crate::graph::{beats_to_tick, GraphDoc};
use crate::monitor::Monitor;

pub fn draw(
    ui: &mut Ui,
    doc: &mut GraphDoc,
    playing: &mut bool,
    monitor: &Monitor,
    status: &str,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(&format!("Tick {}", beats_to_tick(monitor.song_beats())));
        ui.label("From");
        ui.drag_int("from", &mut doc.play_from, 1);
        doc.play_from = doc.play_from.max(1);
        ui.label("BPM");
        ui.drag_float("bpm", &mut doc.bpm, 1.0);
        doc.bpm = doc.bpm.clamp(40.0, 300.0);
    });
    ui.horizontal(|ui| {
        let play = if *playing { "Stop" } else { "Play" };
        if ui.button(play).clicked {
            if *playing {
                *playing = false;
            } else {
                doc.cue_play();
                *playing = true;
            }
        }
        if ui.button("Reset").clicked {
            doc.reset_tick();
        }
    });

    ui.separator();
    ui.label("Selection");

    let selected = doc.space.selected_nodes.clone();
    let actionable: Vec<String> = selected
        .iter()
        .filter(|id| {
            doc.nodes
                .iter()
                .find(|n| n.id == **id)
                .is_some_and(|n| n.kind.can_delete())
        })
        .cloned()
        .collect();
    let can_act = !actionable.is_empty();

    if selected.is_empty() {
        ui.label("No selection");
    } else if selected.len() == 1 {
        let id = selected[0].clone();
        if let Some(n) = doc.nodes.iter_mut().find(|n| n.id == id) {
            ui.label(n.kind.title());
            if n.kind.can_bypass() {
                ui.checkbox("Bypass", &mut n.bypass);
            }
        }
    } else {
        ui.label(&format!("{} nodes", selected.len()));
    }

    ui.add_enabled(can_act, |ui| {
        if ui.button("Clone").clicked() {
            doc.space.request_clone_nodes = actionable.clone();
        }
        if ui.button("Delete").clicked() {
            doc.space.request_delete_nodes = actionable.clone();
        }
    });

    if !can_act && selected.iter().any(|id| id == &doc.output_id) {
        ui.label("Output cannot be deleted.");
    }

    if !status.is_empty() {
        ui.separator();
        ui.label(status);
    }

    false
}
