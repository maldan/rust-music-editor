use mega_ui::Ui;

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

    ui.tree_scope(&mut project.tree_sel, |ui| {
        ui.tree_leaf_icon("graph", "file", "Graph");
        ui.tree_node_icon_open("seqs", "folder", "Sequences", true, |ui| {
            for (id, name) in &seqs {
                ui.tree_leaf_icon(&format!("seq:{id}"), "file", name);
            }
        });
        ui.tree_node_icon_open("insts", "folder", "Instruments", true, |ui| {
            for (id, name) in &insts {
                ui.tree_leaf_icon(&format!("inst:{id}"), "file", name);
            }
        });
    });
    project.apply_tree_sel();

    ui.separator();
    if ui.button("New sequence").clicked {
        project.add_sequence();
    }
    if ui.button("New instrument").clicked {
        project.add_instrument();
    }
}
