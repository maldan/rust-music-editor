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
        viewport: Vec2,
        _dt: f32,
        _stats: DrawStats,
        _keys: &KeyEvents,
    ) -> bool {
        ui.menu_bar(|ui| {
            ui.menu("Graph", |ui| {
                if ui.menu_item("Open JSON...").clicked() {
                    state.open_dialog();
                }
                if ui.menu_item("Save JSON...").clicked() {
                    state.save_dialog();
                }
                ui.separator();
                if ui.menu_item("Fit view").clicked() {
                    state.graph.space.fit_view = true;
                }
            });
        });

        let menu_h = 26.0 * ui.scale();
        let dock_size = Vec2::new(viewport.x, (viewport.y - menu_h).max(1.0));

        let App {
            dock,
            graph: doc,
            playing,
            monitor,
            status,
            ..
        } = state;

        ui.dock_space("main", dock_size, dock, |ui, tab| match tab {
            "Graph" => {
                graph::draw(ui, doc, monitor, *playing);
            }
            "Inspector" => {
                inspector::draw(ui, doc, playing, status);
            }
            _ => {}
        });

        doc.apply_deletes();
        doc.apply_clones();
        state.sync_audio();
        true
    }
}
