mod graph;
mod inspector;
mod piano;

use glam::Vec2;
use mega_ui::{DockNode, DockState, Ui};

use crate::app::App;
use crate::framework::{DrawStats, KeyEvents, Scene};

pub fn default_dock() -> DockState {
    DockState::new(DockNode::split_h(
        0.82,
        DockNode::leaf(&["Graph"]),
        DockNode::leaf(&["Inspector"]),
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
                ui.separator();
                if ui.menu_item("Fit view").clicked() {
                    state.graph.space.fit_view = true;
                }
            });
        });

        let App {
            dock,
            graph: doc,
            playing,
            monitor,
            status,
            ..
        } = state;

        let dock_size = ui.available_size();
        let dock_size = Vec2::new(dock_size.x.max(1.0), dock_size.y.max(120.0));

        ui.dock_space("main", dock_size, dock, |ui, tab| match tab {
            "Graph" => {
                graph::draw(ui, doc, monitor);
            }
            "Inspector" => {
                inspector::draw(ui, doc, playing, monitor, status);
            }
            _ => {}
        });

        doc.apply_deletes();
        doc.apply_clones();
        state.sync_audio();
        true
    }
}
