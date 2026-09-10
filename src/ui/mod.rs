mod adsr;
mod env;
mod explorer;
mod export;
mod graph;
mod groups;
mod inspector;
mod meter;
mod piano;
mod sample;

pub use graph::DeviceLists;

use glam::Vec2;
use mega_audio::events::EventSender;
use mega_audio::note::NoteEvent;
use mega_ui::{DockNode, DockState, Ui, Window};

use crate::app::App;
use crate::framework::{DrawStats, KeyEvents, Scene};
use crate::graph::EditorView;
use crate::viz;

pub fn default_dock() -> DockState {
    DockState::new(DockNode::split_h(
        0.16,
        DockNode::leaf(&["Project"]),
        DockNode::split_h(
            0.80,
            DockNode::leaf(&["Graph", "Visual"]),
            DockNode::leaf(&["Inspector"]),
        ),
    ))
}

fn leaf_tabs(node: &DockNode) -> Option<&[String]> {
    match node {
        DockNode::Leaf { tabs, .. } => Some(tabs),
        _ => None,
    }
}

fn has_tab(node: &DockNode, name: &str) -> bool {
    match node {
        DockNode::Leaf { tabs, .. } => tabs.iter().any(|t| t == name),
        DockNode::Split { first, second, .. } => has_tab(first, name) || has_tab(second, name),
    }
}

fn ensure_groups_dock(node: &mut DockNode, show: bool) {
    if show {
        show_groups_dock(node);
    } else {
        hide_groups_dock(node);
    }
}

fn hide_groups_dock(node: &mut DockNode) {
    match node {
        DockNode::Split { first, second, .. } => {
            let groups_second = leaf_tabs(second).is_some_and(|t| t.len() == 1 && t[0] == "Groups");
            let inspector_first = has_tab(first, "Inspector");
            let groups_first = leaf_tabs(first).is_some_and(|t| t.len() == 1 && t[0] == "Groups");
            let inspector_second = has_tab(second, "Inspector");
            if groups_second && inspector_first {
                *node = (**first).clone();
                return;
            }
            if groups_first && inspector_second {
                *node = (**second).clone();
                return;
            }
            hide_groups_dock(first);
            hide_groups_dock(second);
        }
        DockNode::Leaf { tabs, active } => {
            tabs.retain(|t| t != "Groups");
            if !tabs.is_empty() {
                *active = (*active).min(tabs.len() - 1);
            }
        }
    }
}

fn show_groups_dock(node: &mut DockNode) -> bool {
    if has_tab(node, "Groups") {
        return true;
    }
    match node {
        DockNode::Leaf { tabs, .. }
            if tabs.iter().any(|t| t == "Inspector") && !tabs.iter().any(|t| t == "Groups") =>
        {
            let inspector = node.clone();
            *node = DockNode::split_v(0.62, inspector, DockNode::leaf(&["Groups"]));
            true
        }
        DockNode::Split { first, second, .. } => show_groups_dock(first) || show_groups_dock(second),
        _ => false,
    }
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
                if ui.menu_item("Track settings...").clicked() {
                    state.track_settings_open = true;
                }
                if ui.menu_item("Export MP3...").clicked() {
                    state.export_open = true;
                }
                if ui.menu_item("Export MP4...").clicked() {
                    state.export_video_open = true;
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
        let seq_groups: Vec<(String, Vec<(u32, String)>)> = state
            .project
            .sequences
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    s.groups.iter().map(|g| (g.id, g.name.clone())).collect(),
                )
            })
            .collect();
        let insts: Vec<(String, String)> = state
            .project
            .instruments
            .iter()
            .map(|i| (i.id.clone(), i.name.clone()))
            .collect();

        ensure_groups_dock(
            &mut state.dock.root,
            matches!(state.project.view, EditorView::Sequence(_)),
        );

        let App {
            dock,
            project,
            playing,
            monitor,
            status,
            preview_tx,
            devices,
            viz_px,
            ..
        } = state;

        let dock_size = ui.available_size();
        let dock_size = Vec2::new(dock_size.x.max(1.0), dock_size.y.max(120.0));

        *viz_px = None;
        ui.dock_space("main", dock_size, dock, |ui, tab| match tab {
            "Project" => explorer::draw(ui, project),
            "Graph" => draw_editor(
                ui,
                project,
                monitor,
                *playing,
                preview_tx,
                &seqs,
                &seq_groups,
                &insts,
                devices,
            ),
            "Visual" => {
                let size = ui.available_size();
                let w = size.x.max(1.0).floor() as u32;
                let h = size.y.max(1.0).floor() as u32;
                ui.texture(viz::TEX_SLOT, Vec2::new(w as f32, h as f32));
                ui.request_repaint();
                *viz_px = Some((w.max(1), h.max(1)));
            }
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
        draw_track_settings(ui, state);
        draw_midi_import(ui, state);
        export::draw(ui, state);
        state.sync_audio();
        true
    }

    fn viz_frame(&self) -> Option<viz::Frame> {
        let (w, h) = self.viz_px?;
        let notes = self.monitor.horizon();
        let waves = self.monitor.viz_waves(&notes);
        Some(viz::Frame {
            now_beats: self.monitor.song_beats(),
            window_beats: viz::WINDOW_BEATS,
            notes,
            gonio: Vec::new(),
            waves,
            reset: false,
            width: w,
            height: h,
            title: self.project.title.clone(),
            credit: self.project.viz_credit(),
        })
    }
}

fn draw_editor(
    ui: &mut Ui,
    project: &mut crate::graph::Project,
    monitor: &std::sync::Arc<crate::monitor::Monitor>,
    playing: bool,
    preview_tx: &mut EventSender<NoteEvent>,
    seqs: &[(String, String)],
    seq_groups: &[(String, Vec<(u32, String)>)],
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
            seq_groups,
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
                    seq_groups,
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
                piano::draw_editor(ui, &id, &mut seq.notes, &groups, bars, song, playing, preview_tx)
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

fn draw_track_settings(ui: &mut Ui, app: &mut App) {
    if !app.track_settings_open {
        return;
    }
    let mut open = true;
    ui.modal(
        Window::new("Track settings")
            .size(Vec2::new(380.0, 280.0))
            .open(&mut open),
        |ui| {
            ui.label("Title");
            ui.text_input("track_title", &mut app.project.title);
            ui.label("Author");
            ui.text_input("track_author", &mut app.project.author);
            ui.label("Original author");
            ui.text_input("track_orig_author", &mut app.project.original_author);
            ui.checkbox("Remix", &mut app.project.remix);
            ui.horizontal(|ui| {
                ui.label("BPM");
                ui.drag_float("track_bpm", &mut app.project.main.bpm, 1.0);
                app.project.main.bpm = app.project.main.bpm.clamp(40.0, 300.0);
            });
            ui.separator();
            if ui.button("Close").clicked {
                ui.close_modal();
            }
        },
    );
    if !open {
        app.track_settings_open = false;
    }
}

fn draw_midi_import(ui: &mut Ui, app: &mut App) {
    let Some(mut pending) = app.midi_import.take() else {
        return;
    };
    let mut open = true;
    let mut go = false;
    ui.modal(
        Window::new("Import MIDI")
            .size(Vec2::new(380.0, 180.0))
            .open(&mut open),
        |ui| {
            ui.label(&pending.name);
            ui.label("Import as");
            let opts = ["Separate sequences", "One sequence, tracks as groups"];
            ui.select("midi_mode", &mut pending.mode, &opts);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked {
                    ui.close_modal();
                }
                if ui.button("Import").clicked {
                    go = true;
                    ui.close_modal();
                }
            });
        },
    );
    if go {
        match app
            .project
            .import_midi(&pending.bytes, pending.mode == 1, &pending.name)
        {
            Ok(n) => {
                app.status = if pending.mode == 1 {
                    format!("Imported MIDI as one sequence ({n})")
                } else {
                    format!(
                        "Imported {n} sequence{}",
                        if n == 1 { "" } else { "s" }
                    )
                };
            }
            Err(e) => app.status = format!("MIDI import failed: {e}"),
        }
    } else if open {
        app.midi_import = Some(pending);
    }
}
