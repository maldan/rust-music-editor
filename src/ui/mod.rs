mod adsr;
mod env;
mod explorer;
mod export;
mod graph;
mod groups;
mod inspector;
mod piano;
mod sample;

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
            DockNode::split_v(
                0.62,
                DockNode::leaf(&["Inspector"]),
                DockNode::leaf(&["Groups"]),
            ),
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
            ui.menu("File", |ui| {
                if ui.menu_item("New").clicked() {
                    state.new_project();
                }
                if ui.menu_item("Open...").clicked() {
                    state.open_dialog();
                }
                if ui.menu_item("Save").clicked() {
                    state.save();
                }
                if ui.menu_item("Save As...").clicked() {
                    state.save_dialog();
                }
                if ui.menu_item("Export MP3...").clicked() {
                    state.export_open = true;
                }
            });
            ui.menu("Graph", |ui| {
                if ui.menu_item("New sequence").clicked() {
                    state.project.add_sequence();
                }
                if ui.menu_item("New instrument").clicked() {
                    state.project.add_instrument();
                }
                if ui.menu_item("Import MIDI...").clicked() {
                    state.import_midi_dialog();
                }
                if ui.menu_item("Import Sample...").clicked() {
                    state.import_sample_dialog();
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
            "Project" => explorer::draw(ui, project),
            "Graph" => draw_editor(ui, project, monitor, preview_tx, &seqs, &insts, devices),
            "Inspector" => {
                inspector::draw(ui, project, playing, monitor, status, preview_tx);
            }
            "Groups" => groups::draw(ui, project),
            _ => {}
        });

        if let Some(doc) = project.active_graph() {
            doc.apply_deletes();
            doc.apply_clones();
        }
        explorer::confirm_delete(ui, project);
        groups::draw_modals(ui, project);
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
    let bpm = project.main.bpm;
    let seek = match project.view.clone() {
        EditorView::Graph => graph::draw(
            ui,
            &mut project.main,
            monitor,
            seqs,
            insts,
            &project.samples,
            devices,
            false,
            bpm,
        ),
        EditorView::Instrument(id) => {
            if let Some(inst) = project.instruments.iter_mut().find(|i| i.id == id) {
                let _ = graph::draw(
                    ui,
                    &mut inst.graph,
                    monitor,
                    seqs,
                    insts,
                    &[],
                    devices,
                    true,
                    bpm,
                );
            }
            None
        }
        EditorView::Sequence(id) => {
            let song = monitor.song_beats();
            if let Some(seq) = project.sequence_mut(&id) {
                let bars = seq.loop_bars();
                let groups = seq.groups.clone();
                piano::draw_editor(ui, &id, &mut seq.notes, &groups, bars, song, preview_tx)
            } else {
                None
            }
        }
        EditorView::Sample(id) => {
            let play_t = monitor.song_beats() as f32;
            if let Some(smp) = project.samples.iter().find(|s| s.id == id) {
                sample::draw_editor(ui, &id, smp, play_t)
            } else {
                None
            }
        }
    };
    if let Some(beats) = seek {
        match &project.view {
            EditorView::Sample(_) => project.seek_sample(beats),
            _ => project.main.seek_to(beats),
        }
    }
    if matches!(project.view, EditorView::Sample(_)) {
        ui.request_repaint();
    }
}
