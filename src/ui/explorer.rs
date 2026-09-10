use std::cell::RefCell;

use glam::Vec2;
use mega_ui::{Ui, Window};

use crate::graph::Project;

#[derive(Clone)]
struct SeqEdit {
    id: String,
    name: String,
    bars: i32,
}

#[derive(Clone)]
struct InstEdit {
    id: String,
    name: String,
}

thread_local! {
    static SEQ_EDIT: RefCell<Option<SeqEdit>> = const { RefCell::new(None) };
    static INST_EDIT: RefCell<Option<InstEdit>> = const { RefCell::new(None) };
}

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

    let mut edit_seq = None;
    let mut del_seq = None;
    let mut edit_inst = None;
    let mut del_inst = None;

    ui.tree_scope(&mut project.tree_sel, |ui| {
        ui.tree_leaf_icon("graph", "file", "Graph");
        ui.tree_node_icon_open("seqs", "folder", "Sequences", true, |ui| {
            for (id, name) in &seqs {
                ui.tree_leaf_icon_with(&format!("seq:{id}"), "music/piano", name, |row| {
                    row.spacer();
                    if row.icon_button(&format!("seq_del:{id}"), "delete", false) {
                        del_seq = Some(id.clone());
                    }
                    if row.icon_button(&format!("seq_edit:{id}"), "edit", false) {
                        edit_seq = Some(id.clone());
                    }
                });
            }
        });
        ui.tree_node_icon_open("insts", "folder", "Instruments", true, |ui| {
            for (id, name) in &insts {
                ui.tree_leaf_icon_with(&format!("inst:{id}"), "file", name, |row| {
                    row.spacer();
                    if row.icon_button(&format!("inst_del:{id}"), "delete", false) {
                        del_inst = Some(id.clone());
                    }
                    if row.icon_button(&format!("inst_edit:{id}"), "edit", false) {
                        edit_inst = Some(id.clone());
                    }
                });
            }
        });
        ui.tree_node_icon_open("smps", "folder", "Samples", true, |ui| {
            for (id, name) in &smps {
                ui.tree_leaf_icon(&format!("smp:{id}"), "file", name);
            }
        });
    });
    if let Some(id) = edit_seq {
        if let Some(seq) = project.sequences.iter().find(|s| s.id == id) {
            SEQ_EDIT.with(|e| {
                *e.borrow_mut() = Some(SeqEdit {
                    id,
                    name: seq.name.clone(),
                    bars: seq.seq_loop_bars.max(1) as i32,
                });
            });
        }
    }
    if let Some(id) = del_seq {
        project.pending_delete_seq = Some(id);
    }
    if let Some(id) = edit_inst {
        if let Some(inst) = project.instruments.iter().find(|i| i.id == id) {
            INST_EDIT.with(|e| {
                *e.borrow_mut() = Some(InstEdit {
                    id,
                    name: inst.name.clone(),
                });
            });
        }
    }
    if let Some(id) = del_inst {
        project.pending_delete_inst = Some(id);
    }
    project.apply_tree_sel();
}

pub(crate) fn confirm_delete(ui: &mut Ui, project: &mut Project) {
    edit_seq(ui, project);
    edit_inst(ui, project);
    confirm_seq(ui, project);
    confirm_inst(ui, project);
    confirm_sample(ui, project);
}

fn edit_seq(ui: &mut Ui, project: &mut Project) {
    let Some(mut edit) = SEQ_EDIT.with(|e| e.borrow().clone()) else {
        return;
    };
    let mut open = true;
    let mut apply = false;
    ui.modal(
        Window::new("Edit sequence")
            .size(Vec2::new(320.0, 180.0))
            .open(&mut open),
        |ui| {
            ui.label("Name");
            ui.text_input("seq_edit_name", &mut edit.name);
            ui.horizontal(|ui| {
                ui.label("Bars");
                ui.drag_int("seq_edit_bars", &mut edit.bars, 1);
                edit.bars = edit.bars.max(1);
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked {
                    ui.close_modal();
                }
                if ui.button("Save").clicked {
                    apply = true;
                    ui.close_modal();
                }
            });
        },
    );
    if apply {
        if let Some(seq) = project.sequence_mut(&edit.id) {
            seq.name = edit.name;
            seq.seq_loop_bars = edit.bars.max(1) as u32;
        }
        SEQ_EDIT.with(|e| *e.borrow_mut() = None);
    } else if !open {
        SEQ_EDIT.with(|e| *e.borrow_mut() = None);
    } else {
        SEQ_EDIT.with(|e| *e.borrow_mut() = Some(edit));
    }
}

fn edit_inst(ui: &mut Ui, project: &mut Project) {
    let Some(mut edit) = INST_EDIT.with(|e| e.borrow().clone()) else {
        return;
    };
    let mut open = true;
    let mut apply = false;
    ui.modal(
        Window::new("Edit instrument")
            .size(Vec2::new(320.0, 150.0))
            .open(&mut open),
        |ui| {
            ui.label("Name");
            ui.text_input("inst_edit_name", &mut edit.name);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked {
                    ui.close_modal();
                }
                if ui.button("Save").clicked {
                    apply = true;
                    ui.close_modal();
                }
            });
        },
    );
    if apply {
        if let Some(inst) = project.instrument_mut(&edit.id) {
            inst.name = edit.name;
        }
        INST_EDIT.with(|e| *e.borrow_mut() = None);
    } else if !open {
        INST_EDIT.with(|e| *e.borrow_mut() = None);
    } else {
        INST_EDIT.with(|e| *e.borrow_mut() = Some(edit));
    }
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
