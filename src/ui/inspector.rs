use mega_ui::Ui;

use crate::graph::GraphDoc;

pub fn draw(ui: &mut Ui, doc: &mut GraphDoc, playing: &mut bool, status: &str) -> bool {
    let play = if *playing { "Stop" } else { "Play" };
    if ui.button(play).clicked {
        *playing = !*playing;
    }

    ui.separator();
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
        let title = doc
            .nodes
            .iter()
            .find(|n| n.id == selected[0])
            .map(|n| n.kind.title())
            .unwrap_or("Node");
        ui.label(title);
    } else {
        ui.label(&format!("{} nodes", selected.len()));
    }

    ui.add_enabled(can_act, |ui| {
        if ui.button("Clone").clicked() {
            doc.space.request_clone_nodes = actionable.clone();
        }
        if ui.button("Delete").clicked() {
            doc.space.request_delete_nodes = actionable.clone();
        }
    });

    if !can_act && selected.iter().any(|id| id == &doc.output_id) {
        ui.label("Output cannot be deleted.");
    }

    if !status.is_empty() {
        ui.separator();
        ui.label(status);
    }

    false
}
