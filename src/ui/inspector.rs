use std::cell::RefCell;

use mega_audio::events::EventSender;
use mega_audio::note::NoteEvent;
use mega_ui::Ui;

use crate::graph::{beats_to_tick, EditorView, GraphDoc, Project};
use crate::monitor::Monitor;
use crate::ui::piano::{self, set_preview};

thread_local! {
    static TEST_HELD: RefCell<Option<u8>> = const { RefCell::new(None) };
    static MERGE_SEL: RefCell<usize> = const { RefCell::new(0) };
}

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
                if matches!(project.view, EditorView::Sample(_)) {
                    project.sample_seek = monitor.song_beats();
                }
                *playing = false;
            } else if matches!(project.view, EditorView::Sample(_)) {
                *playing = true;
            } else {
                project.main.cue_play();
                *playing = true;
            }
        }
        if ui.button("Reset").clicked {
            if matches!(project.view, EditorView::Sample(_)) {
                project.seek_sample(0.0);
            } else {
                project.main.reset_tick();
            }
        }
    });

    ui.separator();

    match project.view.clone() {
        EditorView::Sequence(id) => {
            let others: Vec<(String, String)> = project
                .sequences
                .iter()
                .filter(|s| s.id != id)
                .map(|s| (s.id.clone(), s.name.clone()))
                .collect();
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
                if ui.button("Delete sequence").clicked {
                    project.pending_delete_seq = Some(id.clone());
                }
            }
            if !others.is_empty() {
                ui.separator();
                ui.label("Merge with");
                let mut sel = MERGE_SEL.with(|s| *s.borrow());
                if sel >= others.len() {
                    sel = 0;
                }
                let labels: Vec<&str> = others.iter().map(|(_, n)| n.as_str()).collect();
                ui.select("seq_merge", &mut sel, &labels);
                MERGE_SEL.with(|s| *s.borrow_mut() = sel);
                if ui.button("Merge with").clicked {
                    if let Some((from, _)) = others.get(sel) {
                        project.merge_sequence(&id, from);
                    }
                }
            }
        }
        EditorView::Instrument(id) => {
            for s in &mut project.sequences {
                s.ensure_groups();
            }
            let seqs: Vec<(String, String)> = project
                .sequences
                .iter()
                .map(|s| (s.id.clone(), s.name.clone()))
                .collect();
            let seq_groups: Vec<(String, Vec<(u32, String)>)> = project
                .sequences
                .iter()
                .map(|s| {
                    (
                        s.id.clone(),
                        s.groups.iter().map(|g| (g.id, g.name.clone())).collect(),
                    )
                })
                .collect();
            ui.label("Instrument");
            if let Some(inst) = project.instrument_mut(&id) {
                ui.label("Name");
                ui.text_input("inst_name", &mut inst.name);
                ui.label("Play sequence");
                let mut labels: Vec<&str> = vec!["Default 3 notes"];
                for (_, name) in &seqs {
                    labels.push(name.as_str());
                }
                let mut sel = seqs
                    .iter()
                    .position(|(sid, _)| *sid == inst.play_seq)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                ui.select("inst_play_seq", &mut sel, &labels);
                inst.play_seq = if sel == 0 {
                    String::new()
                } else {
                    seqs[sel - 1].0.clone()
                };
                if inst.play_seq.is_empty() {
                    inst.play_group = None;
                } else {
                    let groups: Vec<(u32, String)> = seq_groups
                        .iter()
                        .find(|(sid, _)| *sid == inst.play_seq)
                        .map(|(_, g)| g.clone())
                        .unwrap_or_default();
                    if inst
                        .play_group
                        .is_some_and(|gid| !groups.iter().any(|(id, _)| *id == gid))
                    {
                        inst.play_group = None;
                    }
                    ui.label("Play group");
                    let names: Vec<String> = groups.iter().map(|(_, n)| n.clone()).collect();
                    let mut glabels: Vec<&str> = vec!["All"];
                    for n in &names {
                        glabels.push(n.as_str());
                    }
                    let mut gsel = inst
                        .play_group
                        .and_then(|gid| groups.iter().position(|(id, _)| *id == gid))
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    ui.select("inst_play_group", &mut gsel, &glabels);
                    inst.play_group = if gsel == 0 {
                        None
                    } else {
                        Some(groups[gsel - 1].0)
                    };
                }
                if ui.button("Delete instrument").clicked {
                    project.pending_delete_inst = Some(id.clone());
                }
            }
            ui.separator();
            let Some(doc) = project.active_graph() else {
                return false;
            };
            draw_graph_sel(ui, doc);
        }
        EditorView::Sample(id) => {
            ui.label("Sample");
            if let Some(smp) = project.sample_mut(&id) {
                ui.label("Name");
                ui.text_input("smp_name", &mut smp.name);
                ui.label(&format!("{:.2} s", smp.duration()));
                if !smp.path.is_empty() {
                    ui.label(&smp.path);
                }
                if ui.button("Delete sample").clicked {
                    project.pending_delete_sample = Some(id.clone());
                }
            }
        }
        EditorView::Graph => {
            let Some(doc) = project.active_graph() else {
                return false;
            };
            draw_graph_sel(ui, doc);
        }
    }

    if matches!(project.view, EditorView::Instrument(_)) {
        ui.separator();
        TEST_HELD.with(|h| piano::draw_test_keyboard(ui, preview_tx, &mut h.borrow_mut()));
    } else {
        TEST_HELD.with(|h| set_preview(preview_tx, &mut h.borrow_mut(), None));
    }

    if !status.is_empty() {
        ui.separator();
        ui.label(status);
    }

    false
}

fn draw_graph_sel(ui: &mut Ui, doc: &mut GraphDoc) {
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
