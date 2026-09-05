use std::cell::RefCell;

use mega_audio::events::EventSender;
use mega_audio::note::NoteEvent;
use mega_ui::Ui;

use crate::graph::{beats_to_tick, EditorView, Project};
use crate::monitor::Monitor;
use crate::ui::piano::set_preview;

thread_local! {
    static TEST_HELD: RefCell<Option<u8>> = const { RefCell::new(None) };
}

const TEST_KEYS: [(&str, u8); 8] = [
    ("C4", 60),
    ("D4", 62),
    ("E4", 64),
    ("F4", 65),
    ("G4", 67),
    ("A4", 69),
    ("B4", 71),
    ("C5", 72),
];

pub fn draw(
    ui: &mut Ui,
    project: &mut Project,
    playing: &mut bool,
    monitor: &Monitor,
    status: &str,
    preview_tx: &mut EventSender<NoteEvent>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(&format!("Tick {}", beats_to_tick(monitor.song_beats())));
        ui.label("From");
        ui.drag_int("from", &mut project.main.play_from, 1);
        project.main.play_from = project.main.play_from.max(1);
        ui.label("BPM");
        ui.drag_float("bpm", &mut project.main.bpm, 1.0);
        project.main.bpm = project.main.bpm.clamp(40.0, 300.0);
    });
    ui.horizontal(|ui| {
        let play = if *playing { "Stop" } else { "Play" };
        if ui.button(play).clicked {
            if *playing {
                *playing = false;
            } else {
                project.main.cue_play();
                *playing = true;
            }
        }
        if ui.button("Reset").clicked {
            project.main.reset_tick();
        }
    });

    ui.separator();

    match project.view.clone() {
        EditorView::Sequence(id) => {
            let inst_ids: Vec<String> = project.instruments.iter().map(|i| i.id.clone()).collect();
            let inst_names: Vec<String> = project.instruments.iter().map(|i| i.name.clone()).collect();
            ui.label("Sequence");
            if let Some(seq) = project.sequence_mut(&id) {
                ui.label("Name");
                ui.text_input("seq_name", &mut seq.name);
                ui.horizontal(|ui| {
                    ui.label("Bars");
                    let mut bars = seq.seq_loop_bars as i32;
                    ui.drag_int("seq_bars", &mut bars, 1);
                    seq.seq_loop_bars = bars.max(1) as u32;
                });
                ui.label("Play with");
                let mut labels: Vec<&str> = vec!["Default"];
                for n in &inst_names {
                    labels.push(n.as_str());
                }
                let mut sel = inst_ids
                    .iter()
                    .position(|i| *i == seq.play_inst)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                ui.select("seq_play_inst", &mut sel, &labels);
                seq.play_inst = if sel == 0 {
                    String::new()
                } else {
                    inst_ids[sel - 1].clone()
                };
                if ui.button("Delete sequence").clicked {
                    project.pending_delete_seq = Some(id.clone());
                }
            }
        }
        EditorView::Graph | EditorView::Instrument(_) => {
            let Some(doc) = project.active_graph() else {
                return false;
            };
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
                if ui.button("Clone").clicked {
                    doc.space.request_clone_nodes = actionable.clone();
                }
                if ui.button("Delete").clicked {
                    doc.space.request_delete_nodes = actionable.clone();
                }
            });

            if !can_act && selected.iter().any(|id| id == &doc.output_id || id == &doc.input_id) {
                ui.label("In/Out cannot be deleted.");
            }
        }
    }

    if matches!(project.view, EditorView::Instrument(_)) {
        draw_test_keys(ui, preview_tx);
    } else {
        TEST_HELD.with(|h| set_preview(preview_tx, &mut h.borrow_mut(), None));
    }

    if !status.is_empty() {
        ui.separator();
        ui.label(status);
    }

    false
}

fn draw_test_keys(ui: &mut Ui, tx: &mut EventSender<NoteEvent>) {
    ui.separator();
    ui.label("Test notes");
    let down = ui.pointer().down;
    let mut want = None;
    ui.horizontal(|ui| {
        for (label, pitch) in &TEST_KEYS[..4] {
            if ui.button(label).hovered && down {
                want = Some(*pitch);
            }
        }
    });
    ui.horizontal(|ui| {
        for (label, pitch) in &TEST_KEYS[4..] {
            if ui.button(label).hovered && down {
                want = Some(*pitch);
            }
        }
    });
    TEST_HELD.with(|h| set_preview(tx, &mut h.borrow_mut(), want));
}
