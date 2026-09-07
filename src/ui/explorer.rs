use glam::Vec2;
use mega_ui::{Ui, Window};

use crate::graph::Project;

pub fn draw(ui: &mut Ui, project: &mut Project) {
    let seqs: Vec<(String, String)> = project
        .sequences
        .iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    let insts: Vec<(String, String)> = project
        .instruments
        .iter()
        .map(|i| (i.id.clone(), i.name.clone()))
        .collect();
    let smps: Vec<(String, String)> = project
        .samples
        .iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();

    ui.tree_scope(&mut project.tree_sel, |ui| {
        ui.tree_leaf_icon("graph", "file", "Graph");
        ui.tree_node_icon_open("seqs", "folder", "Sequences", true, |ui| {
            for (id, name) in &seqs {
                ui.tree_leaf_icon(&format!("seq:{id}"), "music/piano", name);
            }
        });
        ui.tree_node_icon_open("insts", "folder", "Instruments", true, |ui| {
            for (id, name) in &insts {
                ui.tree_leaf_icon(&format!("inst:{id}"), "file", name);
            }
        });
        ui.tree_node_icon_open("smps", "folder", "Samples", true, |ui| {
            for (id, name) in &smps {
                ui.tree_leaf_icon(&format!("smp:{id}"), "file", name);
            }
        });
    });
    project.apply_tree_sel();
}

pub(crate) fn confirm_delete(ui: &mut Ui, project: &mut Project) {
    confirm_seq(ui, project);
    confirm_inst(ui, project);
    confirm_sample(ui, project);
}

fn confirm_seq(ui: &mut Ui, project: &mut Project) {
    let Some(id) = project.pending_delete_seq.clone() else {
        return;
    };
    let name = project
        .sequences
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| id.clone());
    let mut open = true;
    let mut confirmed = false;
    ui.modal(
        Window::new("Delete sequence")
            .size(Vec2::new(320.0, 140.0))
            .open(&mut open),
        |ui| {
            ui.label(&format!("Delete \"{name}\"?"));
            ui.label("This cannot be undone.");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked {
                    ui.close_modal();
                }
                if ui.button("Delete").clicked {
                    confirmed = true;
                    ui.close_modal();
                }
            });
        },
    );
    if confirmed {
        project.remove_sequence(&id);
    } else if !open {
        project.pending_delete_seq = None;
    }
}

fn confirm_inst(ui: &mut Ui, project: &mut Project) {
    let Some(id) = project.pending_delete_inst.clone() else {
        return;
    };
    let name = project
        .instruments
        .iter()
        .find(|i| i.id == id)
        .map(|i| i.name.clone())
        .unwrap_or_else(|| id.clone());
    let mut open = true;
    let mut confirmed = false;
    ui.modal(
        Window::new("Delete instrument")
            .size(Vec2::new(320.0, 140.0))
            .open(&mut open),
        |ui| {
            ui.label(&format!("Delete \"{name}\"?"));
            ui.label("This cannot be undone.");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked {
                    ui.close_modal();
                }
                if ui.button("Delete").clicked {
                    confirmed = true;
                    ui.close_modal();
                }
            });
        },
    );
    if confirmed {
        project.remove_instrument(&id);
    } else if !open {
        project.pending_delete_inst = None;
    }
}

fn confirm_sample(ui: &mut Ui, project: &mut Project) {
    let Some(id) = project.pending_delete_sample.clone() else {
        return;
    };
    let name = project
        .samples
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| id.clone());
    let mut open = true;
    let mut confirmed = false;
    ui.modal(
        Window::new("Delete sample")
            .size(Vec2::new(320.0, 140.0))
            .open(&mut open),
        |ui| {
            ui.label(&format!("Delete \"{name}\"?"));
            ui.label("This cannot be undone.");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked {
                    ui.close_modal();
                }
                if ui.button("Delete").clicked {
                    confirmed = true;
                    ui.close_modal();
                }
            });
        },
    );
    if confirmed {
        project.remove_sample(&id);
    } else if !open {
        project.pending_delete_sample = None;
    }
}
