use std::cell::RefCell;

use glam::Vec2;
use mega_ui::{CursorIcon, LayoutOpts, Rect, ScrollAxes, Ui, Window};

use crate::graph::{EditorView, NoteGroup, Project, DEFAULT_GROUP_ID};
use crate::ui::piano;

#[derive(Clone)]
struct Edit {
    seq: String,
    id: u32,
    name: String,
    color: [f32; 4],
    inst: String,
}

thread_local! {
    static EDIT: RefCell<Option<Edit>> = const { RefCell::new(None) };
}

pub fn draw(ui: &mut Ui, project: &mut Project) {
    match project.view.clone() {
        EditorView::Sequence(seq_id) => draw_list(ui, project, &seq_id),
        _ => ui.label("Open a sequence"),
    }
}

pub fn draw_modals(ui: &mut Ui, project: &mut Project) {
    edit_modal(ui, project);
}

fn draw_list(ui: &mut Ui, project: &mut Project, seq_id: &str) {
    let sel = piano::editor_selection(seq_id);
    if let Some(seq) = project.sequence_mut(seq_id) {
        seq.ensure_groups();
    }

    if ui.button("New group").clicked {
        if let Some(seq) = project.sequence_mut(seq_id) {
            let gid = seq.add_group();
            for i in &sel {
                if let Some(n) = seq.notes.get_mut(*i) {
                    n.group = gid;
                }
            }
            piano::set_editor_selection(seq_id, sel.clone(), gid);
        }
        ui.request_repaint();
    }
    ui.separator();

    let groups = project
        .sequences
        .iter()
        .find(|s| s.id == seq_id)
        .map(|s| s.groups.clone())
        .unwrap_or_default();

    let size = ui.available_size();
    ui.scroll_area(
        "groups",
        Vec2::new(size.x.max(1.0), size.y.max(1.0)),
        ScrollAxes::Vertical,
        |ui| {
            draw_rows(ui, seq_id, project, &groups, &sel);
        },
    );
}

fn draw_rows(
    ui: &mut Ui,
    seq_id: &str,
    project: &mut Project,
    groups: &[NoteGroup],
    sel: &[usize],
) {

    let mut toggle = None;
    let mut edit_id = None;
    let mut delete_id = None;
    let mut assign_id = None;
    let mut pick_id = None;

    for g in groups {
        ui.selectable(&format!("row{}", g.id), false, |ui| {
            ui.row_with(
                LayoutOpts {
                    spacing: Some(4.0 * ui.scale()),
                    ..Default::default()
                },
                |ui| {
                    let vis = if g.visible {
                        "visibility"
                    } else {
                        "visibility_off"
                    };
                    if ui.icon_button(&format!("vis{}", g.id), vis, g.visible) {
                        toggle = Some(g.id);
                    }
                    swatch(ui, g.color);
                    ui.label(&g.name);
                    ui.spacer();
                    if letter_chip(ui, &format!("sel{}", g.id), "S") {
                        pick_id = Some(g.id);
                    }
                    ui.add_enabled(!sel.is_empty(), |ui| {
                        if letter_chip(ui, &format!("asn{}", g.id), "G") {
                            assign_id = Some(g.id);
                        }
                    });
                    if ui.icon_button(&format!("edit{}", g.id), "edit", false) {
                        edit_id = Some(g.id);
                    }
                    ui.add_enabled(g.id != DEFAULT_GROUP_ID, |ui| {
                        if ui.icon_button(&format!("del{}", g.id), "delete", false) {
                            delete_id = Some(g.id);
                        }
                    });
                },
            );
        });
    }

    if let Some(id) = toggle {
        if let Some(seq) = project.sequence_mut(seq_id) {
            if let Some(g) = seq.groups.iter_mut().find(|g| g.id == id) {
                g.visible = !g.visible;
            }
        }
        ui.request_repaint();
    }
    if let Some(id) = pick_id {
        if let Some(seq) = project.sequences.iter().find(|s| s.id == seq_id) {
            let indices: Vec<usize> = seq
                .notes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.group == id)
                .map(|(i, _)| i)
                .collect();
            piano::set_editor_selection(seq_id, indices, id);
        }
        ui.request_repaint();
    }
    if let Some(id) = assign_id {
        if let Some(seq) = project.sequence_mut(seq_id) {
            for i in sel {
                if let Some(n) = seq.notes.get_mut(*i) {
                    n.group = id;
                }
            }
            piano::set_editor_selection(seq_id, sel.to_vec(), id);
        }
        ui.request_repaint();
    }
    if let Some(id) = delete_id {
        if let Some(seq) = project.sequence_mut(seq_id) {
            seq.remove_group(id);
        }
        project.forget_play_group(seq_id, id);
        ui.request_repaint();
    }
    if let Some(id) = edit_id {
        if let Some(seq) = project.sequences.iter().find(|s| s.id == seq_id) {
            if let Some(g) = seq.group(id) {
                EDIT.with(|e| {
                    *e.borrow_mut() = Some(Edit {
                        seq: seq_id.to_string(),
                        id,
                        name: g.name.clone(),
                        color: g.color,
                        inst: g.play_inst.clone(),
                    });
                });
            }
        }
    }
}

fn cell(ui: &mut Ui) -> f32 {
    22.0 * ui.scale()
}

fn swatch(ui: &mut Ui, color: [f32; 4]) {
    let cell = cell(ui);
    let a = ui.area("swatch", Vec2::splat(cell));
    let sw = 12.0 * ui.scale();
    let pad = (cell - sw) * 0.5;
    ui.fill_round(
        Rect::from_min_size(a.rect.min + Vec2::splat(pad), Vec2::splat(sw)),
        2.0 * ui.scale(),
        color,
    );
}

fn letter_chip(ui: &mut Ui, id: &str, letter: &str) -> bool {
    let enabled = ui.enabled();
    let s = ui.scale();
    let cell = 22.0 * s;
    let a = ui.area(id, Vec2::splat(cell));
    if enabled && a.hovered {
        ui.set_mouse_cursor(CursorIcon::Pointer);
        ui.fill_round(a.rect, 4.0 * s, [0.18, 0.18, 0.18, 1.0]);
    }
    ui.text_at(a.rect.min + Vec2::new(6.5 * s, 3.0 * s), letter);
    enabled && a.hovered && a.active && ui.pointer().released
}

fn edit_modal(ui: &mut Ui, project: &mut Project) {
    let Some(mut edit) = EDIT.with(|e| e.borrow().clone()) else {
        return;
    };
    let inst_ids: Vec<String> = project.instruments.iter().map(|i| i.id.clone()).collect();
    let inst_names: Vec<String> = project.instruments.iter().map(|i| i.name.clone()).collect();
    let mut open = true;
    let mut apply = false;
    ui.modal(
        Window::new("Edit group")
            .size(Vec2::new(360.0, 280.0))
            .open(&mut open),
        |ui| {
            ui.label("Name");
            ui.text_input("grp_name", &mut edit.name);
            ui.label("Color");
            ui.color_edit("grp_color", &mut edit.color);
            ui.label("Instrument");
            let mut labels: Vec<&str> = vec!["Default"];
            for n in &inst_names {
                labels.push(n.as_str());
            }
            let mut sel = inst_ids
                .iter()
                .position(|i| *i == edit.inst)
                .map(|i| i + 1)
                .unwrap_or(0);
            ui.select("grp_inst", &mut sel, &labels);
            edit.inst = if sel == 0 {
                String::new()
            } else {
                inst_ids[sel - 1].clone()
            };
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
        if let Some(seq) = project.sequence_mut(&edit.seq) {
            if let Some(g) = seq.groups.iter_mut().find(|g| g.id == edit.id) {
                g.name = edit.name.clone();
                g.color = edit.color;
            }
            seq.set_group_play_inst(edit.id, edit.inst.clone());
        }
        EDIT.with(|e| *e.borrow_mut() = None);
    } else if !open {
        EDIT.with(|e| *e.borrow_mut() = None);
    } else {
        EDIT.with(|e| *e.borrow_mut() = Some(edit));
    }
}
