use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use glam::Vec2;
use mega_ui::{CursorIcon, LayoutOpts, Rect, ScrollAxes, Ui};

use mega_audio::events::EventSender;
use mega_audio::note::NoteEvent;
use crate::graph::{GraphNode, SeqNote, BEATS_PER_STEP, SEQ_PITCHES, SEQ_STEPS};
use crate::monitor::Monitor;

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

const LABEL_W: f32 = 28.0;
const CELL_W: f32 = 12.0;
const CELL_H: f32 = 11.0;
const HEAD_H: f32 = 4.0;
const ED_LABEL_W: f32 = 52.0;
const ED_CELL_W: f32 = 20.0;
const ED_CELL_H: f32 = 18.0;
const ED_HEAD_H: f32 = 10.0;
const GAP: f32 = 1.0;
const HANDLE: f32 = 8.0;
const FADE_SEC: f32 = 0.45;

thread_local! {
    static ROLLS: RefCell<HashMap<String, Roll>> = RefCell::new(HashMap::new());
    static PREVIEWS: RefCell<HashMap<String, Preview>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy)]
struct Roll {
    drag: Option<Drag>,
    last_len: u8,
    preview: Option<u8>,
    key: Option<u8>,
    scrolled: bool,
}

impl Default for Roll {
    fn default() -> Self {
        Self {
            drag: None,
            last_len: 1,
            preview: None,
            key: None,
            scrolled: false,
        }
    }
}

#[derive(Clone, Copy)]
enum Drag {
    Move { idx: usize, grab_step: i32, grab_pitch: i32 },
    Resize { idx: usize, left: bool },
    Erase,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edge {
    Body,
    Left,
    Right,
}

struct Ghost {
    pitch: u8,
    on: f64,
    off: f64,
    live: bool,
    fade: f32,
}

struct Preview {
    ghosts: Vec<Ghost>,
    last: Instant,
}

fn pitch_name(pitch: u8) -> String {
    format!("{}{}", NAMES[(pitch % 12) as usize], pitch / 12 - 1)
}

fn pitch_of_row(base: u8, row: u32, rows: u32) -> u8 {
    base.saturating_add((rows.saturating_sub(1).saturating_sub(row)) as u8)
        .min(127)
}

fn row_of_pitch(base: u8, pitch: u8, rows: u32) -> Option<u32> {
    let max = base.saturating_add(rows.saturating_sub(1) as u8).min(127);
    if pitch < base || pitch > max {
        return None;
    }
    Some((max - pitch) as u32)
}

fn clamp_note(n: &mut SeqNote, steps: u32) {
    let max_step = steps.saturating_sub(1).min(u8::MAX as u32) as u8;
    n.step = n.step.min(max_step);
    let room = (steps - n.step as u32).max(1).min(u8::MAX as u32) as u8;
    n.len = n.len.max(1).min(room);
    n.pitch = n.pitch.min(127);
}

fn set_preview(tx: &mut EventSender<NoteEvent>, held: &mut Option<u8>, pitch: Option<u8>) {
    if *held == pitch {
        return;
    }
    if let Some(p) = *held {
        let _ = tx.send(NoteEvent::NoteOff { note: p });
    }
    if let Some(p) = pitch {
        let _ = tx.send(NoteEvent::NoteOn { note: p, velocity: 0.85 });
    }
    *held = pitch;
}

fn cell_at(grid: Rect, stride: Vec2, pos: Vec2, steps: u32, rows: u32, base: u8) -> Option<(u8, u8)> {
    if pos.x < grid.min.x || pos.y < grid.min.y || pos.x >= grid.max.x || pos.y >= grid.max.y {
        return None;
    }
    let col = ((pos.x - grid.min.x) / stride.x).floor() as i32;
    let row = ((pos.y - grid.min.y) / stride.y).floor() as i32;
    if col < 0 || row < 0 || col >= steps as i32 || row >= rows as i32 {
        return None;
    }
    Some((col as u8, pitch_of_row(base, row as u32, rows)))
}

fn note_rect(grid: Rect, stride: Vec2, note: SeqNote, base: u8, rows: u32) -> Option<Rect> {
    let row = row_of_pitch(base, note.pitch, rows)?;
    let x = grid.min.x + note.step as f32 * stride.x;
    let y = grid.min.y + row as f32 * stride.y;
    let w = (note.len.max(1) as f32 * stride.x - GAP).max(2.0);
    let h = (stride.y - GAP).max(2.0);
    Some(Rect::from_min_size(Vec2::new(x, y), Vec2::new(w, h)))
}

fn handle_w(r: Rect) -> f32 {
    HANDLE.min(r.width() * 0.45).max(3.0)
}

fn edge_at(r: Rect, pos: Vec2) -> Edge {
    let h = handle_w(r);
    if pos.x >= r.max.x - h {
        Edge::Right
    } else if pos.x <= r.min.x + h {
        Edge::Left
    } else {
        Edge::Body
    }
}

fn hit_note(
    notes: &[SeqNote],
    grid: Rect,
    stride: Vec2,
    pos: Vec2,
    base: u8,
    rows: u32,
) -> Option<(usize, Edge)> {
    for (i, n) in notes.iter().enumerate().rev() {
        let Some(r) = note_rect(grid, stride, *n, base, rows) else {
            continue;
        };
        if r.contains(pos) {
            return Some((i, edge_at(r, pos)));
        }
    }
    None
}

fn draw_note_handles(ui: &mut Ui, r: Rect) {
    let h = handle_w(r);
    let y = r.min.y + 1.0;
    let hh = (r.height() - 2.0).max(1.0);
    let color = [1.0, 0.92, 0.55, 0.35];
    ui.fill_rect(
        Rect::from_min_size(Vec2::new(r.min.x, y), Vec2::new(h, hh)),
        color,
    );
    ui.fill_rect(
        Rect::from_min_size(Vec2::new(r.max.x - h, y), Vec2::new(h, hh)),
        color,
    );
}

fn resize_note(n: &mut SeqNote, step: u8, left: bool, steps: u32) {
    if left {
        let end = n.step as u32 + n.len.max(1) as u32;
        n.step = step.min(end.saturating_sub(1) as u8);
        n.len = (end - n.step as u32).max(1) as u8;
    } else {
        n.len = step.saturating_sub(n.step).saturating_add(1);
    }
    clamp_note(n, steps);
}

fn erase_at(
    notes: &mut Vec<SeqNote>,
    grid: Rect,
    stride: Vec2,
    pos: Vec2,
    base: u8,
    rows: u32,
) -> bool {
    if let Some((i, _)) = hit_note(notes, grid, stride, pos, base, rows) {
        notes.remove(i);
        true
    } else {
        false
    }
}

struct RollGeom {
    s: f32,
    gap: f32,
    stride: Vec2,
    label_w: f32,
    head_h: f32,
    size: Vec2,
    steps: u32,
    rows: u32,
    editor: bool,
}

fn geom(s: f32, steps: u32, rows: u32) -> RollGeom {
    geom_ex(s, steps, rows, LABEL_W, CELL_W, CELL_H, HEAD_H, false)
}

fn geom_editor(s: f32, steps: u32, rows: u32) -> RollGeom {
    geom_ex(s, steps, rows, ED_LABEL_W, ED_CELL_W, ED_CELL_H, ED_HEAD_H, true)
}

fn geom_ex(
    s: f32,
    steps: u32,
    rows: u32,
    label_w: f32,
    cell_w: f32,
    cell_h: f32,
    head_h: f32,
    editor: bool,
) -> RollGeom {
    let gap = GAP.max(s * 0.5);
    let stride = Vec2::new(cell_w * s + gap, cell_h * s + gap);
    let label_w = label_w * s;
    let head_h = head_h * s;
    let grid_w = steps as f32 * stride.x - gap;
    let grid_h = rows as f32 * stride.y - gap;
    RollGeom {
        s,
        gap,
        stride,
        label_w,
        head_h,
        size: Vec2::new(label_w + grid_w, head_h + grid_h),
        steps,
        rows,
        editor,
    }
}

fn draw_grid(ui: &mut Ui, rect: Rect, g: &RollGeom, base: u8, with_keys: bool) -> Rect {
    let label_w = if with_keys { g.label_w } else { 0.0 };
    let grid = Rect {
        min: Vec2::new(rect.min.x + label_w, rect.min.y + g.head_h),
        max: Vec2::new(rect.max.x, rect.max.y),
    };
    ui.fill_rect(rect, [0.07, 0.07, 0.08, 1.0]);
    for step in 0..g.steps {
        let x = grid.min.x + step as f32 * g.stride.x;
        let bar = step % SEQ_STEPS == 0;
        let color = if bar {
            [0.28, 0.28, 0.34, 1.0]
        } else if step % 4 == 0 {
            [0.22, 0.22, 0.26, 1.0]
        } else {
            [0.14, 0.14, 0.16, 1.0]
        };
        ui.fill_rect(
            Rect::from_min_size(
                Vec2::new(x, rect.min.y),
                Vec2::new((g.stride.x - g.gap).max(1.0), g.head_h - 1.0),
            ),
            color,
        );
    }
    for row in 0..g.rows {
        let pitch = pitch_of_row(base, row, g.rows);
        let sharp = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
        let y = grid.min.y + row as f32 * g.stride.y;
        let row_h = (g.stride.y - g.gap).max(1.0);
        if with_keys {
            draw_key(ui, rect.min.x, y, g, pitch, row_h, false);
        }
        for step in 0..g.steps {
            let x = grid.min.x + step as f32 * g.stride.x;
            let color = if sharp {
                [0.10, 0.10, 0.12, 1.0]
            } else {
                [0.13, 0.13, 0.15, 1.0]
            };
            ui.fill_rect(
                Rect::from_min_size(
                    Vec2::new(x, y),
                    Vec2::new((g.stride.x - g.gap).max(1.0), (g.stride.y - g.gap).max(1.0)),
                ),
                color,
            );
        }
        if pitch % 12 == 0 {
            let y1 = y + row_h;
            ui.line(
                Vec2::new(rect.min.x, y1),
                Vec2::new(grid.max.x, y1),
                if g.editor { 1.6 } else { 1.0 },
                [0.02, 0.02, 0.03, 1.0],
            );
        }
    }
    draw_time_lines(ui, grid, g);
    grid
}

fn draw_time_lines(ui: &mut Ui, grid: Rect, g: &RollGeom) {
    for step in 0..g.steps {
        let beat = step % 4 == 0;
        let bar = step % SEQ_STEPS == 0;
        if !beat {
            continue;
        }
        let x = grid.min.x + step as f32 * g.stride.x;
        let (w, a) = if bar {
            (2.4 * g.s.min(1.5), 1.0)
        } else {
            (1.15 * g.s.min(1.5), 0.85)
        };
        ui.line(
            Vec2::new(x, grid.min.y),
            Vec2::new(x, grid.max.y),
            w,
            [0.02, 0.02, 0.03, a],
        );
    }
}

fn draw_key(ui: &mut Ui, x: f32, y: f32, g: &RollGeom, pitch: u8, row_h: f32, lit: bool) {
    let sharp = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
    let mut color = if sharp {
        [0.11, 0.12, 0.15, 1.0]
    } else {
        [0.26, 0.27, 0.30, 1.0]
    };
    if lit {
        color = [0.95, 0.72, 0.22, 0.85];
    }
    ui.fill_rect(
        Rect::from_min_size(Vec2::new(x, y), Vec2::new(g.label_w - 1.0, row_h)),
        color,
    );
    let px = if g.editor {
        (13.0 * g.s).min(row_h * 0.85)
    } else {
        (8.0 * g.s).min(row_h)
    };
    ui.text_at_size(
        Vec2::new(x + 4.0 * g.s.min(1.5), y + (row_h - px) * 0.5),
        &pitch_name(pitch),
        px,
    );
}

fn draw_keys(ui: &mut Ui, rect: Rect, g: &RollGeom, base: u8, offset_y: f32, lit: Option<u8>) {
    ui.fill_rect(rect, [0.07, 0.07, 0.08, 1.0]);
    let y0 = rect.min.y + g.head_h - offset_y;
    let row_h = (g.stride.y - g.gap).max(1.0);
    for row in 0..g.rows {
        let y = y0 + row as f32 * g.stride.y;
        if y + row_h < rect.min.y || y > rect.max.y {
            continue;
        }
        let pitch = pitch_of_row(base, row, g.rows);
        draw_key(ui, rect.min.x, y, g, pitch, row_h, lit == Some(pitch));
        if pitch % 12 == 0 {
            let y1 = y + row_h;
            ui.line(
                Vec2::new(rect.min.x, y1),
                Vec2::new(rect.max.x, y1),
                1.6,
                [0.02, 0.02, 0.03, 1.0],
            );
        }
    }
}

#[allow(dead_code)]
pub fn draw_in_node(ui: &mut Ui, node_id: &str, node: &mut GraphNode, playhead: Option<f32>) {
    let steps = node.loop_steps();
    let base = node.view_base_pitch();
    let g = geom(ui.scale(), steps, node.view_pitch_count());
    let loop_beats = steps as f32 * BEATS_PER_STEP;
    let play_step = playhead.map(|p| {
        let t = (p / loop_beats.max(0.001)).clamp(0.0, 0.999);
        (t * steps as f32) as u32
    });

    let area = ui.area("roll", g.size);
    let rect = area.rect;
    let ptr = ui.pointer();
    let grid = Rect {
        min: Vec2::new(rect.min.x + g.label_w, rect.min.y + g.head_h),
        max: Vec2::new(rect.max.x, rect.max.y),
    };

    let mut roll = ROLLS.with(|m| m.borrow().get(node_id).copied().unwrap_or_default());

    if area.hovered || area.active {
        match hit_note(&node.notes, grid, g.stride, ptr.pos, base, g.rows) {
            Some((_, Edge::Left | Edge::Right)) if !matches!(roll.drag, Some(Drag::Move { .. })) => {
                ui.set_mouse_cursor(CursorIcon::ResizeEw);
            }
            Some(_) => ui.set_mouse_cursor(CursorIcon::Move),
            None if area.hovered => ui.set_mouse_cursor(CursorIcon::Pointer),
            None => {}
        }
    }

    if ptr.right_pressed && (area.hovered || area.active) {
        erase_at(&mut node.notes, grid, g.stride, ptr.pos, base, g.rows);
        roll.drag = Some(Drag::Erase);
        ui.request_repaint();
    } else if matches!(roll.drag, Some(Drag::Erase)) && ptr.right_down && area.active {
        erase_at(&mut node.notes, grid, g.stride, ptr.pos, base, g.rows);
        ui.request_repaint();
    } else if ptr.pressed && area.hovered {
        match hit_note(&node.notes, grid, g.stride, ptr.pos, base, g.rows) {
            Some((idx, edge)) if edge != Edge::Body => {
                roll.last_len = node.notes[idx].len.max(1);
                roll.drag = Some(Drag::Resize {
                    idx,
                    left: edge == Edge::Left,
                });
            }
            Some((idx, _)) => {
                let n = node.notes[idx];
                roll.last_len = n.len.max(1);
                if let Some((step, pitch)) = cell_at(grid, g.stride, ptr.pos, steps, g.rows, base) {
                    roll.drag = Some(Drag::Move {
                        idx,
                        grab_step: step as i32 - n.step as i32,
                        grab_pitch: pitch as i32 - n.pitch as i32,
                    });
                }
            }
            None => {
                if let Some((step, pitch)) = cell_at(grid, g.stride, ptr.pos, steps, g.rows, base) {
                    let len = roll.last_len.max(1).min((steps - step as u32) as u8);
                    node.notes.push(SeqNote { step, pitch, len });
                    roll.drag = Some(Drag::Move {
                        idx: node.notes.len() - 1,
                        grab_step: 0,
                        grab_pitch: 0,
                    });
                }
            }
        }
        ui.request_repaint();
    } else if area.active && ptr.down {
        match roll.drag {
            Some(Drag::Move {
                idx,
                grab_step,
                grab_pitch,
            }) => {
                if let Some((step, pitch)) = cell_at(grid, g.stride, ptr.pos, steps, g.rows, base) {
                    if let Some(n) = node.notes.get_mut(idx) {
                        n.step = (step as i32 - grab_step).clamp(0, steps as i32 - 1) as u8;
                        n.pitch = (pitch as i32 - grab_pitch).clamp(0, 127) as u8;
                        clamp_note(n, steps);
                    }
                }
                ui.request_repaint();
            }
            Some(Drag::Resize { idx, left }) => {
                if let Some(n) = node.notes.get_mut(idx) {
                    if let Some((step, _)) = cell_at(grid, g.stride, ptr.pos, steps, g.rows, base) {
                        resize_note(n, step, left, steps);
                        roll.last_len = n.len.max(1);
                    }
                }
                ui.request_repaint();
            }
            Some(Drag::Erase) | None => {}
        }
    }

    if ptr.released {
        roll.drag = None;
    }

    ROLLS.with(|m| {
        m.borrow_mut().insert(node_id.to_string(), roll);
    });

    let grid = draw_grid(ui, rect, &g, base, true);

    let active_idx = match roll.drag {
        Some(Drag::Move { idx, .. } | Drag::Resize { idx, .. }) => Some(idx),
        _ => None,
    };
    for (i, note) in node.notes.iter().copied().enumerate() {
        let Some(r) = note_rect(grid, g.stride, note, base, g.rows) else {
            continue;
        };
        let mut color = [0.95, 0.72, 0.22, 1.0];
        if play_step.map(|s| s >= note.step as u32 && s < note.step as u32 + note.len.max(1) as u32)
            == Some(true)
        {
            color = [1.0, 0.82, 0.35, 1.0];
        }
        if Some(i) == active_idx {
            color = [1.0, 0.88, 0.45, 1.0];
        }
        ui.fill_round(r, 2.0 * g.s.min(1.5), color);
        draw_note_handles(ui, r);
    }

    if let Some(step) = play_step {
        let x = grid.min.x + step as f32 * g.stride.x;
        ui.line(
            Vec2::new(x, grid.min.y),
            Vec2::new(x, grid.max.y),
            1.5,
            [1.0, 0.42, 0.18, 0.9],
        );
    }
}

const EDITOR_BASE: u8 = 12;
const EDITOR_ROWS: u32 = 108;

pub fn draw_editor(
    ui: &mut Ui,
    id: &str,
    notes: &mut Vec<SeqNote>,
    loop_bars: u32,
    song_beats: f64,
    preview: &mut EventSender<NoteEvent>,
) {
    let steps = SEQ_STEPS * loop_bars.clamp(1, 8);
    let base = EDITOR_BASE;
    let g = geom_editor(ui.scale().max(1.0), steps, EDITOR_ROWS);
    let loop_beats = steps as f32 * BEATS_PER_STEP;
    let play_step = {
        let t = (song_beats.rem_euclid(loop_beats as f64) / loop_beats.max(0.001) as f64).clamp(0.0, 0.999);
        Some((t * steps as f64) as u32)
    };
    let view = ui.available_size();
    let view = Vec2::new(view.x.max(80.0), view.y.max(80.0));
    let grid_size = Vec2::new((g.size.x - g.label_w).max(1.0), g.size.y);
    let mut jump_c4 = false;

    // Zero gap: default row spacing sits on the right of the scroll_area and clips the v-bar.
    ui.row_with(LayoutOpts { spacing: Some(0.0), ..Default::default() }, |ui| {
        let keys = ui.area("keys", Vec2::new(g.label_w, view.y));
        ui.flex(1.0, |ui| {
            let size = ui.available_size();
            let size = Vec2::new(size.x.max(80.0), size.y.max(80.0));
            ui.scroll_area(
            "seq_roll",
            size,
            ScrollAxes::Both,
            |ui| {
                let area = ui.area("roll", grid_size);
                let rect = area.rect;
                let ptr = ui.pointer();
                let grid = Rect {
                    min: Vec2::new(rect.min.x, rect.min.y + g.head_h),
                    max: Vec2::new(rect.max.x, rect.max.y),
                };

                let mut roll = ROLLS.with(|m| m.borrow().get(id).copied().unwrap_or_default());
                if !roll.scrolled {
                    jump_c4 = true;
                    roll.scrolled = true;
                }

                if area.hovered || area.active {
                    match hit_note(notes, grid, g.stride, ptr.pos, base, g.rows) {
                        Some((_, Edge::Left | Edge::Right))
                            if !matches!(roll.drag, Some(Drag::Move { .. })) =>
                        {
                            ui.set_mouse_cursor(CursorIcon::ResizeEw);
                        }
                        Some(_) => ui.set_mouse_cursor(CursorIcon::Move),
                        None if area.hovered => ui.set_mouse_cursor(CursorIcon::Pointer),
                        None => {}
                    }
                }

                if ptr.right_pressed && (area.hovered || area.active) {
                    erase_at(notes, grid, g.stride, ptr.pos, base, g.rows);
                    roll.drag = Some(Drag::Erase);
                    set_preview(preview, &mut roll.preview, None);
                    ui.request_repaint();
                } else if matches!(roll.drag, Some(Drag::Erase)) && ptr.right_down && area.active {
                    erase_at(notes, grid, g.stride, ptr.pos, base, g.rows);
                    ui.request_repaint();
                } else if ptr.pressed && area.hovered {
                    match hit_note(notes, grid, g.stride, ptr.pos, base, g.rows) {
                        Some((idx, edge)) if edge != Edge::Body => {
                            roll.last_len = notes[idx].len.max(1);
                            roll.drag = Some(Drag::Resize {
                                idx,
                                left: edge == Edge::Left,
                            });
                            set_preview(preview, &mut roll.preview, Some(notes[idx].pitch));
                        }
                        Some((idx, _)) => {
                            let n = notes[idx];
                            roll.last_len = n.len.max(1);
                            if let Some((step, pitch)) =
                                cell_at(grid, g.stride, ptr.pos, steps, g.rows, base)
                            {
                                roll.drag = Some(Drag::Move {
                                    idx,
                                    grab_step: step as i32 - n.step as i32,
                                    grab_pitch: pitch as i32 - n.pitch as i32,
                                });
                            }
                            set_preview(preview, &mut roll.preview, Some(n.pitch));
                        }
                        None => {
                            if let Some((step, pitch)) =
                                cell_at(grid, g.stride, ptr.pos, steps, g.rows, base)
                            {
                                let len = roll.last_len.max(1).min((steps - step as u32) as u8);
                                notes.push(SeqNote { step, pitch, len });
                                roll.drag = Some(Drag::Move {
                                    idx: notes.len() - 1,
                                    grab_step: 0,
                                    grab_pitch: 0,
                                });
                                set_preview(preview, &mut roll.preview, Some(pitch));
                            }
                        }
                    }
                    ui.request_repaint();
                } else if area.active && ptr.down {
                    match roll.drag {
                        Some(Drag::Move {
                            idx,
                            grab_step,
                            grab_pitch,
                        }) => {
                            if let Some((step, pitch)) =
                                cell_at(grid, g.stride, ptr.pos, steps, g.rows, base)
                            {
                                if let Some(n) = notes.get_mut(idx) {
                                    n.step = (step as i32 - grab_step).clamp(0, steps as i32 - 1) as u8;
                                    n.pitch = (pitch as i32 - grab_pitch).clamp(0, 127) as u8;
                                    clamp_note(n, steps);
                                    set_preview(preview, &mut roll.preview, Some(n.pitch));
                                }
                            }
                            ui.request_repaint();
                        }
                        Some(Drag::Resize { idx, left }) => {
                            if let Some(n) = notes.get_mut(idx) {
                                if let Some((step, _)) =
                                    cell_at(grid, g.stride, ptr.pos, steps, g.rows, base)
                                {
                                    resize_note(n, step, left, steps);
                                    roll.last_len = n.len.max(1);
                                }
                            }
                            ui.request_repaint();
                        }
                        Some(Drag::Erase) | None => {}
                    }
                }

                if ptr.released {
                    roll.drag = None;
                    set_preview(preview, &mut roll.preview, None);
                }

                ROLLS.with(|m| {
                    m.borrow_mut().insert(id.to_string(), roll);
                });

                let grid = draw_grid(ui, rect, &g, base, false);
                let active_idx = match roll.drag {
                    Some(Drag::Move { idx, .. } | Drag::Resize { idx, .. }) => Some(idx),
                    _ => None,
                };
                for (i, note) in notes.iter().copied().enumerate() {
                    let Some(r) = note_rect(grid, g.stride, note, base, g.rows) else {
                        continue;
                    };
                    let mut color = [0.95, 0.72, 0.22, 1.0];
                    if play_step.map(|s| {
                        s >= note.step as u32 && s < note.step as u32 + note.len.max(1) as u32
                    }) == Some(true)
                    {
                        color = [1.0, 0.82, 0.35, 1.0];
                    }
                    if Some(i) == active_idx {
                        color = [1.0, 0.88, 0.45, 1.0];
                    }
                    ui.fill_round(r, 2.0 * g.s.min(1.5), color);
                    draw_note_handles(ui, r);
                }
                if let Some(step) = play_step {
                    let x = grid.min.x + step as f32 * g.stride.x;
                    ui.line(
                        Vec2::new(x, grid.min.y),
                        Vec2::new(x, grid.max.y),
                        1.5,
                        [1.0, 0.42, 0.18, 0.9],
                    );
                }
                if roll.drag.is_some() {
                    ui.request_repaint();
                }
            },
        );
        });

        if jump_c4 {
            if let Some(row) = row_of_pitch(base, 60, EDITOR_ROWS) {
                let y = (g.head_h + row as f32 * g.stride.y - view.y * 0.4).max(0.0);
                ui.set_scroll_target("seq_roll", Vec2::new(0.0, y));
            }
            ui.request_repaint();
        }

        let mut off = ui.scroll_offset("seq_roll");
        if keys.hovered {
            let wheel = ui.take_scroll();
            if wheel.y.abs() > 0.0 {
                off.y = (off.y - wheel.y).max(0.0);
                ui.set_scroll_target("seq_roll", off);
                ui.request_repaint();
            }
        }

        let ptr = ui.pointer();
        let mut roll = ROLLS.with(|m| m.borrow().get(id).copied().unwrap_or_default());
        if keys.hovered {
            ui.set_mouse_cursor(CursorIcon::Pointer);
        }
        if ptr.pressed && keys.hovered {
            if let Some(pitch) = key_at(&g, keys.rect, ptr.pos, base, off.y) {
                set_preview(preview, &mut roll.key, Some(pitch));
            }
            ui.request_repaint();
        } else if roll.key.is_some() && ptr.down {
            if let Some(pitch) = key_at(&g, keys.rect, ptr.pos, base, off.y) {
                set_preview(preview, &mut roll.key, Some(pitch));
            }
            ui.request_repaint();
        }
        if ptr.released {
            set_preview(preview, &mut roll.key, None);
        }
        ROLLS.with(|m| {
            m.borrow_mut().insert(id.to_string(), roll);
        });
        draw_keys(ui, keys.rect, &g, base, off.y, roll.key);
        if roll.key.is_some() {
            ui.request_repaint();
        }
    });
}

fn key_at(g: &RollGeom, keys: Rect, pos: Vec2, base: u8, offset_y: f32) -> Option<u8> {
    if pos.x < keys.min.x || pos.x >= keys.max.x {
        return None;
    }
    let y = pos.y - keys.min.y + offset_y - g.head_h;
    let row = (y / g.stride.y).floor() as i32;
    if row < 0 || row >= g.rows as i32 {
        return None;
    }
    Some(pitch_of_row(base, row as u32, g.rows))
}

pub fn draw_preview(ui: &mut Ui, node: &GraphNode, monitor: &Monitor) {
    let steps = node.loop_steps();
    let base = node.view_base_pitch();
    let g = geom(ui.scale(), steps, node.view_pitch_count());
    let window = node.loop_beats().max(1e-9);
    let song = monitor.song_beats();
    let play_step = {
        let t = (song.rem_euclid(window) / window).clamp(0.0, 0.999);
        Some((t * steps as f64) as u32)
    };

    let area = ui.area("preview", g.size);
    let grid = draw_grid(ui, area.rect, &g, base, true);

    let sounding = monitor.sounding_notes(&node.id);
    let mut fading = false;
    PREVIEWS.with(|m| {
        let mut map = m.borrow_mut();
        let st = map.entry(node.id.clone()).or_insert_with(|| Preview {
            ghosts: Vec::new(),
            last: Instant::now(),
        });
        let dt = st.last.elapsed().as_secs_f32().clamp(0.0, 0.05);
        st.last = Instant::now();
        sync_ghosts(&mut st.ghosts, &sounding, song, dt);
        fading = st.ghosts.iter().any(|gho| gho.live || gho.fade < 1.0);

        let win0 = (song / window).floor() * window;
        let win1 = win0 + window;
        for gho in &st.ghosts {
            let Some(row) = row_of_pitch(base, gho.pitch, g.rows) else {
                continue;
            };
            let t0 = gho.on.max(win0);
            let t1 = gho.off.min(win1);
            if t1 <= t0 {
                continue;
            }
            let x0 = grid.min.x + ((t0 - win0) / window) as f32 * (grid.width());
            let x1 = grid.min.x + ((t1 - win0) / window) as f32 * (grid.width());
            let y = grid.min.y + row as f32 * g.stride.y;
            let h = (g.stride.y - g.gap).max(2.0);
            let w = (x1 - x0).max(2.0);
            let a = gho.fade.clamp(0.0, 1.0);
            let color = if gho.live {
                [0.45, 0.82, 1.0, 0.92]
            } else {
                [0.45, 0.82, 1.0, 0.92 * a]
            };
            ui.fill_round(
                Rect::from_min_size(Vec2::new(x0, y), Vec2::new(w, h)),
                2.0 * g.s.min(1.5),
                color,
            );
        }
    });

    if let Some(step) = play_step {
        let x = grid.min.x + step as f32 * g.stride.x;
        ui.line(
            Vec2::new(x, grid.min.y),
            Vec2::new(x, grid.max.y),
            1.5,
            [1.0, 0.42, 0.18, 0.9],
        );
    }
    if fading {
        ui.request_repaint();
    }
}

fn sync_ghosts(ghosts: &mut Vec<Ghost>, sounding: &std::collections::HashSet<u8>, song: f64, dt: f32) {
    for gho in ghosts.iter_mut() {
        if gho.live {
            if sounding.contains(&gho.pitch) {
                gho.off = song;
            } else {
                gho.live = false;
                gho.off = song;
            }
        }
        if !gho.live {
            gho.fade = (gho.fade - dt / FADE_SEC).max(0.0);
        }
    }
    for &pitch in sounding {
        if !ghosts.iter().any(|gho| gho.pitch == pitch && gho.live) {
            ghosts.push(Ghost {
                pitch,
                on: song,
                off: song,
                live: true,
                fade: 1.0,
            });
        }
    }
    ghosts.retain(|gho| gho.live || gho.fade > 0.01);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_maps_into_grid() {
        let grid = Rect::from_min_size(Vec2::new(10.0, 20.0), Vec2::new(160.0, 120.0));
        let stride = Vec2::new(10.0, 10.0);
        let base = 60;
        assert_eq!(
            cell_at(grid, stride, Vec2::new(10.0, 20.0), 16, SEQ_PITCHES, base),
            Some((0, pitch_of_row(base, 0, SEQ_PITCHES)))
        );
        assert_eq!(
            cell_at(grid, stride, Vec2::new(25.0, 35.0), 16, SEQ_PITCHES, base),
            Some((1, pitch_of_row(base, 1, SEQ_PITCHES)))
        );
        assert_eq!(cell_at(grid, stride, Vec2::new(9.0, 20.0), 16, SEQ_PITCHES, base), None);
    }

    #[test]
    fn note_hit_edges() {
        let grid = Rect::from_min_size(Vec2::ZERO, Vec2::new(200.0, 200.0));
        let stride = Vec2::new(12.0, 11.0);
        let base = 60;
        let pitch = pitch_of_row(base, 2, SEQ_PITCHES);
        let notes = vec![SeqNote {
            step: 0,
            pitch,
            len: 4,
        }];
        let r = note_rect(grid, stride, notes[0], base, SEQ_PITCHES).unwrap();
        assert_eq!(
            hit_note(&notes, grid, stride, Vec2::new(r.max.x - 1.0, r.min.y + 2.0), base, SEQ_PITCHES),
            Some((0, Edge::Right))
        );
        assert_eq!(
            hit_note(&notes, grid, stride, Vec2::new(r.min.x + 1.0, r.min.y + 2.0), base, SEQ_PITCHES),
            Some((0, Edge::Left))
        );
        assert_eq!(
            hit_note(&notes, grid, stride, r.min + Vec2::new(r.width() * 0.5, 2.0), base, SEQ_PITCHES),
            Some((0, Edge::Body))
        );
    }

    #[test]
    fn resize_keeps_other_edge() {
        let mut n = SeqNote {
            step: 4,
            pitch: 60,
            len: 4,
        };
        resize_note(&mut n, 10, false, 16);
        assert_eq!(n.step, 4);
        assert_eq!(n.len, 7);
        resize_note(&mut n, 2, true, 16);
        assert_eq!(n.step, 2);
        assert_eq!(n.len, 9);
    }

    #[test]
    fn clamp_keeps_note_in_span() {
        let mut n = SeqNote {
            step: 14,
            pitch: 60,
            len: 8,
        };
        clamp_note(&mut n, 16);
        assert_eq!(n.step, 14);
        assert_eq!(n.len, 2);
        n.step = 30;
        n.len = 8;
        clamp_note(&mut n, 32);
        assert_eq!(n.step, 30);
        assert_eq!(n.len, 2);
    }

    #[test]
    fn key_hit_tracks_vertical_scroll() {
        let g = geom_editor(1.0, 16, EDITOR_ROWS);
        let keys = Rect::from_min_size(Vec2::ZERO, Vec2::new(g.label_w, 200.0));
        let row = row_of_pitch(EDITOR_BASE, 60, EDITOR_ROWS).unwrap();
        let pos = Vec2::new(g.label_w * 0.5, g.head_h + 4.0);
        let pitch = key_at(&g, keys, pos, EDITOR_BASE, row as f32 * g.stride.y).unwrap();
        assert_eq!(pitch, 60);
    }

    #[test]
    fn full_roll_shows_c4() {
        let grid = Rect::from_min_size(Vec2::ZERO, Vec2::new(400.0, 2000.0));
        let stride = Vec2::new(20.0, 18.0);
        let note = SeqNote {
            step: 0,
            pitch: 60,
            len: 1,
        };
        let r = note_rect(grid, stride, note, EDITOR_BASE, EDITOR_ROWS).unwrap();
        let row = row_of_pitch(EDITOR_BASE, 60, EDITOR_ROWS).unwrap();
        assert_eq!(r.min.y, row as f32 * stride.y);
        assert!(hit_note(&[note], grid, stride, r.min + Vec2::new(2.0, 2.0), EDITOR_BASE, EDITOR_ROWS).is_some());
        assert!(note_rect(grid, stride, note, EDITOR_BASE, SEQ_PITCHES).is_none());
    }

    #[test]
    fn retrigger_does_not_stretch_old_ghost() {
        let mut ghosts = vec![Ghost {
            pitch: 60,
            on: 0.0,
            off: 0.25,
            live: false,
            fade: 0.8,
        }];
        let mut on = std::collections::HashSet::new();
        on.insert(60);
        sync_ghosts(&mut ghosts, &on, 1.0, 0.016);
        assert_eq!(ghosts.len(), 2);
        assert!(!ghosts[0].live);
        assert_eq!(ghosts[0].on, 0.0);
        assert_eq!(ghosts[0].off, 0.25);
        assert!(ghosts[1].live);
        assert_eq!(ghosts[1].on, 1.0);
    }
}
