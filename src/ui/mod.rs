mod adsr;
mod explorer;
mod export;
mod graph;
mod inspector;
mod piano;

pub use graph::DeviceLists;

use glam::Vec2;
use mega_audio::events::EventSender;
use mega_audio::note::NoteEvent;
use mega_ui::{DockNode, DockState, Ui};

use crate::app::App;
use crate::framework::{DrawStats, KeyEvents, Scene};
use crate::graph::EditorView;

pub fn default_dock() -> DockState {
    DockState::new(DockNode::split_h(
        0.16,
        DockNode::leaf(&["Project"]),
        DockNode::split_h(
            0.80,
            DockNode::leaf(&["Graph"]),
            DockNode::leaf(&["Inspector"]),
        ),
    ))
}

impl Scene for App {
    fn title() -> &'static str {
        "music-editor"
    }

    fn window_size() -> (f64, f64) {
        (1280.0, 720.0)
    }

    fn init(ui: &mut Ui) {
        ui.load_builtin_icons();
    }

    fn build(
        ui: &mut Ui,
        state: &mut Self,
        _viewport: Vec2,
        _dt: f32,
        _stats: DrawStats,
        keys: &KeyEvents,
    ) -> bool {
        if keys.save {
            state.save();
        }
        ui.menu_bar(|ui| {
            ui.menu("Graph", |ui| {
                if ui.menu_item("Open...").clicked() {
                    state.open_dialog();
                }
                if ui.menu_item("Save").clicked() {
                    state.save();
                }
                if ui.menu_item("Save As...").clicked() {
                    state.save_dialog();
                }
                if ui.menu_item("Import MIDI...").clicked() {
                    state.import_midi_dialog();
                }
                if ui.menu_item("Export MP3...").clicked() {
                    state.export_open = true;
                }
                ui.separator();
                if ui.menu_item("Fit view").clicked() {
                    state.project.main.space.fit_view = true;
                }
            });
        });

        let seqs: Vec<(String, String)> = state
            .project
            .sequences
            .iter()
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let insts: Vec<(String, String)> = state
            .project
            .instruments
            .iter()
            .map(|i| (i.id.clone(), i.name.clone()))
            .collect();

        let mut import_midi = false;
        let App {
            dock,
            project,
            playing,
            monitor,
            status,
            preview_tx,
            devices,
            ..
        } = state;

        let dock_size = ui.available_size();
        let dock_size = Vec2::new(dock_size.x.max(1.0), dock_size.y.max(120.0));

        ui.dock_space("main", dock_size, dock, |ui, tab| match tab {
            "Project" => explorer::draw(ui, project, &mut import_midi),
            "Graph" => draw_editor(ui, project, monitor, preview_tx, &seqs, &insts, devices),
            "Inspector" => {
                inspector::draw(ui, project, playing, monitor, status, preview_tx);
            }
            _ => {}
        });

        if let Some(doc) = project.active_graph() {
            doc.apply_deletes();
            doc.apply_clones();
        }
        explorer::confirm_delete(ui, project);
        if import_midi {
            state.import_midi_dialog();
        }
        export::draw(ui, state);
        state.sync_audio();
        true
    }
}

fn draw_editor(
    ui: &mut Ui,
    project: &mut crate::graph::Project,
    monitor: &std::sync::Arc<crate::monitor::Monitor>,
    preview_tx: &mut EventSender<NoteEvent>,
    seqs: &[(String, String)],
    insts: &[(String, String)],
    devices: &mut DeviceLists,
) {
    let seek = match project.view.clone() {
        EditorView::Graph => {
            graph::draw(ui, &mut project.main, monitor, seqs, insts, devices, false);
            None
        }
        EditorView::Instrument(id) => {
            if let Some(inst) = project.instruments.iter_mut().find(|i| i.id == id) {
                graph::draw(ui, &mut inst.graph, monitor, seqs, insts, devices, true);
            }
            None
        }
        EditorView::Sequence(id) => {
            let song = monitor.song_beats();
            if let Some(seq) = project.sequence_mut(&id) {
                let bars = seq.loop_bars();
                piano::draw_editor(ui, &id, &mut seq.notes, bars, song, preview_tx)
            } else {
                None
            }
        }
    };
    if let Some(beats) = seek {
        project.main.seek_to(beats);
    }
}
