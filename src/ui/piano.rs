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
const ED_HEAD_H: f32 = 22.0;
const GAP: f32 = 1.0;
const HANDLE: f32 = 8.0;
const FADE_SEC: f32 = 0.45;
const ZOOM_MIN: f32 = 0.12;
const ZOOM_MAX: f32 = 4.0;
const NOTE_FILL: [f32; 4] = [0.32, 0.72, 0.40, 1.0];
const NOTE_PLAY: [f32; 4] = [0.48, 0.88, 0.52, 1.0];
const NOTE_SEL: [f32; 4] = [0.82, 0.28, 0.32, 1.0];
const NOTE_SEL_PLAY: [f32; 4] = [0.95, 0.42, 0.44, 1.0];

thread_local! {
    static ROLLS: RefCell<HashMap<String, Roll>> = RefCell::new(HashMap::new());
    static PREVIEWS: RefCell<HashMap<String, Preview>> = RefCell::new(HashMap::new());
    static CLIP: RefCell<Vec<SeqNote>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
struct Roll {
    drag: Option<Drag>,
    last_len: u32,
    preview: Option<u8>,
    key: Option<u8>,
    scrolled: bool,
    zoom: f32,
    sel: Vec<usize>,
    header: Rect,
    view: Rect,
}

impl Default for Roll {
    fn default() -> Self {
        Self {
            drag: None,
            last_len: 1,
            preview: None,
            key: None,
            scrolled: false,
            zoom: 1.0,
            sel: Vec::new(),
            header: Rect {
                min: Vec2::ZERO,
                max: Vec2::ZERO,
            },
            view: Rect {
                min: Vec2::ZERO,
                max: Vec2::ZERO,
            },
        }
    }
}

#[derive(Clone)]
enum Drag {
    Move {
        press_step: u32,
        press_pitch: u8,
        orig: Vec<(usize, u32, u8)>,
    },
    Resize { idx: usize, left: bool },
    Erase,
    Box { a: Vec2, b: Vec2 },
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
    let max_step = steps.saturating_sub(1);
    n.step = n.step.min(max_step);
    let room = (steps - n.step).max(1);
    n.len = n.len.max(1).min(room);
    n.pitch = n.pitch.min(127);
}

fn clamp_zoom(z: f32) -> f32 {
    z.clamp(ZOOM_MIN, ZOOM_MAX)
}

fn zoom_anchor_x(old_stride: f32, new_stride: f32, offset: f32, local: f32) -> f32 {
    let content = (offset + local).max(0.0);
    let ratio = new_stride / old_stride.max(1e-6);
    (content * ratio - local).max(0.0)
}

fn header_step(rect: Rect, stride_x: f32, off_x: f32, pos: Vec2, steps: u32) -> u32 {
    let x = (pos.x - rect.min.x + off_x).max(0.0);
    let step = (x / stride_x.max(1e-6)).floor() as i32;
    step.clamp(0, steps.saturating_sub(1) as i32) as u32
}

fn sel_has(sel: &[usize], i: usize) -> bool {
    sel.contains(&i)
}

fn toggle_sel(sel: &mut Vec<usize>, i: usize) {
    if let Some(p) = sel.iter().position(|&x| x == i) {
        sel.remove(p);
    } else {
        sel.push(i);
    }
}

fn shift_sel(notes: &mut [SeqNote], orig: &[(usize, u32, u8)], d_step: i32, d_pitch: i32, steps: u32) {
    if orig.is_empty() {
        return;
    }
    let max_s = steps.saturating_sub(1) as i32;
    let min_step = orig.iter().map(|o| o.1 as i32).min().unwrap_or(0);
    let max_step = orig.iter().map(|o| o.1 as i32).max().unwrap_or(0);
    let min_pitch = orig.iter().map(|o| o.2 as i32).min().unwrap_or(0);
    let max_pitch = orig.iter().map(|o| o.2 as i32).max().unwrap_or(0);
    let ds = d_step.clamp(-min_step, max_s - max_step);
    let dp = d_pitch.clamp(-min_pitch, 127 - max_pitch);
    for &(i, step, pitch) in orig {
        if let Some(n) = notes.get_mut(i) {
            n.step = (step as i32 + ds) as u32;
            n.pitch = (pitch as i32 + dp) as u8;
            clamp_note(n, steps);
        }
    }
}

fn remove_sel(notes: &mut Vec<SeqNote>, sel: &mut Vec<usize>) {
    sel.sort_unstable();
    sel.dedup();
    for i in sel.iter().rev().copied() {
        if i < notes.len() {
            notes.remove(i);
        }
    }
    sel.clear();
}

fn clone_sel(notes: &mut Vec<SeqNote>, sel: &[usize], steps: u32) -> Vec<usize> {
    let mut add = Vec::new();
    for &i in sel {
        let Some(src) = notes.get(i).copied() else {
            continue;
        };
        let mut n = src;
        n.step = n.step.saturating_add(1);
        clamp_note(&mut n, steps);
        add.push(n);
    }
    let start = notes.len();
    notes.extend(add);
    (start..notes.len()).collect()
}

fn clip_from_sel(notes: &[SeqNote], sel: &[usize]) -> Vec<SeqNote> {
    sel.iter().filter_map(|&i| notes.get(i).copied()).collect()
}

fn paste_at(notes: &mut Vec<SeqNote>, clip: &[SeqNote], step: u32, pitch: u8, steps: u32) -> Vec<usize> {
    if clip.is_empty() {
        return Vec::new();
    }
    let min_step = clip.iter().map(|n| n.step).min().unwrap_or(0);
    let min_pitch = clip.iter().map(|n| n.pitch).min().unwrap_or(0);
    let mut add = Vec::new();
    for n in clip {
        let mut c = *n;
        c.step = (step as i32 + (n.step as i32 - min_step as i32)).max(0) as u32;
        c.pitch = (pitch as i32 + (n.pitch as i32 - min_pitch as i32)).clamp(0, 127) as u8;
        clamp_note(&mut c, steps);
        add.push(c);
    }
    let start = notes.len();
    notes.extend(add);
    (start..notes.len()).collect()
}

fn notes_in_box(
    notes: &[SeqNote],
    grid: Rect,
    stride: Vec2,
    a: Vec2,
    b: Vec2,
    base: u8,
    rows: u32,
) -> Vec<usize> {
    let min = Vec2::new(a.x.min(b.x), a.y.min(b.y));
    let max = Vec2::new(a.x.max(b.x), a.y.max(b.y));
    let boxr = Rect { min, max };
    let mut out = Vec::new();
    for (i, n) in notes.iter().copied().enumerate() {
        let Some(r) = note_rect(grid, stride, n, base, rows) else {
            continue;
        };
        if r.intersect(boxr).is_some() {
            out.push(i);
        }
    }
    out
}

pub(crate) fn set_preview(tx: &mut EventSender<NoteEvent>, held: &mut Option<u8>, pitch: Option<u8>) {
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

fn cell_at(grid: Rect, stride: Vec2, pos: Vec2, steps: u32, rows: u32, base: u8) -> Option<(u32, u8)> {
    if pos.x < grid.min.x || pos.y < grid.min.y || pos.x >= grid.max.x || pos.y >= grid.max.y {
        return None;
    }
    let col = ((pos.x - grid.min.x) / stride.x).floor() as i32;
    let row = ((pos.y - grid.min.y) / stride.y).floor() as i32;
    if col < 0 || row < 0 || col as u32 >= steps || row as u32 >= rows {
        return None;
    }
    Some((col as u32, pitch_of_row(base, row as u32, rows)))
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

fn resize_note(n: &mut SeqNote, step: u32, left: bool, steps: u32) {
    if left {
        let end = n.step + n.len.max(1);
        n.step = step.min(end.saturating_sub(1));
        n.len = (end - n.step).max(1);
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

#[derive(Clone, Copy)]
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

fn geom_editor(s: f32, steps: u32, rows: u32, zoom: f32) -> RollGeom {
    geom_ex(
        s,
        steps,
        rows,
        ED_LABEL_W,
        ED_CELL_W * clamp_zoom(zoom),
        ED_CELL_H,
        ED_HEAD_H,
        true,
    )
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

fn draw_grid(ui: &mut Ui, rect: Rect, g: &RollGeom, base: u8, with_keys: bool, head: bool) -> Rect {
    let label_w = if with_keys { g.label_w } else { 0.0 };
    let head_h = if head { g.head_h } else { 0.0 };
    let grid = Rect {
        min: Vec2::new(rect.min.x + label_w, rect.min.y + head_h),
        max: Vec2::new(rect.max.x, rect.max.y),
    };
    ui.fill_rect(rect, [0.07, 0.07, 0.08, 1.0]);
    if head {
        draw_bar_header(ui, Rect {
            min: Vec2::new(grid.min.x, rect.min.y),
            max: Vec2::new(grid.max.x, grid.min.y),
        }, g, 0.0);
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

fn draw_bar_header(ui: &mut Ui, rect: Rect, g: &RollGeom, off_x: f32) {
    ui.fill_rect(rect, [0.08, 0.08, 0.10, 1.0]);
    let bars = (g.steps / SEQ_STEPS).max(1);
    let bar_w = SEQ_STEPS as f32 * g.stride.x;
    let px = (12.0 * g.s).min(rect.height() * 0.8);
    for bar in 0..bars {
        let x = rect.min.x + bar as f32 * bar_w - off_x;
        let cell = Rect::from_min_size(Vec2::new(x, rect.min.y), Vec2::new(bar_w.max(1.0), rect.height()));
        let Some(vis) = cell.intersect(rect) else {
            continue;
        };
        let color = if bar % 2 == 0 {
            [0.16, 0.16, 0.19, 1.0]
        } else {
            [0.13, 0.13, 0.16, 1.0]
        };
        ui.fill_rect(vis, color);
        let tx = x + 6.0 * g.s.min(1.5);
        if tx >= rect.min.x + 2.0 && tx < rect.max.x - 8.0 {
            ui.text_at_size(
                Vec2::new(tx, rect.min.y + (rect.height() - px) * 0.5),
                &format!("{}", bar + 1),
                px,
            );
        }
        if x >= rect.min.x && x <= rect.max.x {
            ui.line(
                Vec2::new(x, rect.min.y),
                Vec2::new(x, rect.max.y),
                1.2,
                [0.02, 0.02, 0.03, 1.0],
            );
        }
    }
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

fn draw_keys(ui: &mut Ui, rect: Rect, g: &RollGeom, base: u8, offset_y: f32, lit: Option<u8>, head_h: f32) {
    ui.fill_rect(rect, [0.07, 0.07, 0.08, 1.0]);
    let y0 = rect.min.y + head_h - offset_y;
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

    let mut roll = ROLLS.with(|m| m.borrow().get(node_id).cloned().unwrap_or_default());

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
                        press_step: step,
                        press_pitch: pitch,
                        orig: vec![(idx, n.step, n.pitch)],
                    });
                }
            }
            None => {
                if let Some((step, pitch)) = cell_at(grid, g.stride, ptr.pos, steps, g.rows, base) {
                    let len = roll.last_len.max(1).min(steps - step);
                    node.notes.push(SeqNote { step, pitch, len });
                    roll.drag = Some(Drag::Move {
                        press_step: step,
                        press_pitch: pitch,
                        orig: vec![(node.notes.len() - 1, step, pitch)],
                    });
                }
            }
        }
        ui.request_repaint();
    } else if area.active && ptr.down {
        match roll.drag.clone() {
            Some(Drag::Move {
                press_step,
                press_pitch,
                orig,
            }) => {
                if let Some((step, pitch)) = cell_at(grid, g.stride, ptr.pos, steps, g.rows, base) {
                    shift_sel(
                        &mut node.notes,
                        &orig,
                        step as i32 - press_step as i32,
                        pitch as i32 - press_pitch as i32,
                        steps,
                    );
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
            Some(Drag::Erase | Drag::Box { .. }) | None => {}
        }
    }

    if ptr.released {
        roll.drag = None;
    }

    let drag = roll.drag.clone();
    ROLLS.with(|m| {
        m.borrow_mut().insert(node_id.to_string(), roll);
    });

    let grid = draw_grid(ui, rect, &g, base, true, true);

    let active = match &drag {
        Some(Drag::Move { orig, .. }) => orig.iter().map(|o| o.0).collect::<Vec<_>>(),
        Some(Drag::Resize { idx, .. }) => vec![*idx],
        _ => Vec::new(),
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
        if active.iter().any(|&j| j == i) {
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
) -> Option<f64> {
    let steps = SEQ_STEPS * loop_bars.max(1);
    let base = EDITOR_BASE;
    let mut roll = ROLLS.with(|m| m.borrow().get(id).cloned().unwrap_or_default());
    let loop_beats = steps as f32 * BEATS_PER_STEP;
    let play_step = {
        let t = (song_beats.rem_euclid(loop_beats as f64) / loop_beats.max(0.001) as f64)
            .clamp(0.0, 0.999);
        Some((t * steps as f64) as u32)
    };
    let mut jump_c4 = false;
    if !roll.scrolled {
        jump_c4 = true;
        roll.scrolled = true;
    }
    let mut seek_to = None;
    let scale = ui.scale().max(1.0);
    let mut g = geom_editor(scale, steps, EDITOR_ROWS, roll.zoom);
    apply_zoom_at_cursor(ui, &mut roll, &mut g, scale, steps);
    let grid_h = (g.size.y - g.head_h).max(1.0);
    let grid_size = Vec2::new((g.size.x - g.label_w).max(1.0), grid_h);
    let mut header_rect = roll.header;

    ui.column_with(LayoutOpts { spacing: Some(0.0), ..Default::default() }, |ui| {
        ui.row_with(LayoutOpts { spacing: Some(0.0), ..Default::default() }, |ui| {
            let corner = ui.area("corner", Vec2::new(g.label_w, g.head_h));
            ui.fill_rect(corner.rect, [0.08, 0.08, 0.10, 1.0]);
            ui.flex(1.0, |ui| {
                let w = ui.available_size().x.max(1.0);
                let header = ui.area("bars", Vec2::new(w, g.head_h));
                header_rect = header.rect;
                let off_x = ui.scroll_offset("seq_roll").x;
                draw_bar_header(ui, header.rect, &g, off_x);
                let ptr = ui.pointer();
                if header.hovered {
                    ui.set_mouse_cursor(CursorIcon::Pointer);
                    if !ptr.ctrl {
                        let wheel = ui.take_scroll();
                        if wheel.y.abs() > 0.0 || wheel.x.abs() > 0.0 {
                            let mut off = ui.scroll_offset("seq_roll");
                            off.x = (off.x - wheel.y - wheel.x).max(0.0);
                            ui.set_scroll_target("seq_roll", off);
                            ui.request_repaint();
                        }
                    }
                }
                if (header.hovered && ptr.pressed) || (header.active && ptr.down) {
                    let step = header_step(header.rect, g.stride.x, off_x, ptr.pos, steps);
                    seek_to = Some(step as f64 * BEATS_PER_STEP as f64);
                    ui.request_repaint();
                }
            });
        });
        ui.flex(1.0, |ui| {
            ui.row_with(LayoutOpts { spacing: Some(0.0), ..Default::default() }, |ui| {
                let keys_h = ui.available_size().y.max(80.0);
                let keys = ui.area("keys", Vec2::new(g.label_w, keys_h));
                ui.flex(1.0, |ui| {
                    let size = ui.available_size();
                    let size = Vec2::new(size.x.max(80.0), size.y.max(80.0));
                    ui.scroll_area("seq_roll", size, ScrollAxes::Both, |ui| {
                        let area = ui.area("roll", grid_size);
                        let rect = area.rect;
                        let ptr = ui.pointer();
                        let grid = Rect {
                            min: rect.min,
                            max: rect.max,
                        };

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

                        editor_interact(
                            ui,
                            notes,
                            &mut roll,
                            preview,
                            area,
                            grid,
                            g.stride,
                            ptr,
                            steps,
                            base,
                            g.rows,
                        );

                        let grid = draw_grid(ui, rect, &g, base, false, false);
                        for (i, note) in notes.iter().copied().enumerate() {
                            let Some(r) = note_rect(grid, g.stride, note, base, g.rows) else {
                                continue;
                            };
                            let playing = play_step.map(|s| {
                                s >= note.step && s < note.step + note.len.max(1)
                            }) == Some(true);
                            let selected = sel_has(&roll.sel, i);
                            let color = match (selected, playing) {
                                (true, true) => NOTE_SEL_PLAY,
                                (true, false) => NOTE_SEL,
                                (false, true) => NOTE_PLAY,
                                (false, false) => NOTE_FILL,
                            };
                            ui.fill_round(r, 2.0 * g.s.min(1.5), color);
                            draw_note_handles(ui, r);
                        }
                        if let Some(Drag::Box { a, b }) = &roll.drag {
                            let min = Vec2::new(a.x.min(b.x), a.y.min(b.y));
                            let max = Vec2::new(a.x.max(b.x), a.y.max(b.y));
                            ui.fill_rect(
                                Rect { min, max },
                                [0.82, 0.28, 0.32, 0.18],
                            );
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
                    });
                });

                if jump_c4 {
                    if let Some(row) = row_of_pitch(base, 60, EDITOR_ROWS) {
                        let y = (row as f32 * g.stride.y - keys_h * 0.4).max(0.0);
                        ui.set_scroll_target("seq_roll", Vec2::new(0.0, y));
                    }
                    ui.request_repaint();
                }

                let mut off = ui.scroll_offset("seq_roll");
                let ptr = ui.pointer();
                if keys.hovered {
                    ui.set_mouse_cursor(CursorIcon::Pointer);
                    if ptr.select_all {
                        roll.sel = (0..notes.len()).collect();
                        ui.request_repaint();
                    }
                    let wheel = ui.take_scroll();
                    if wheel.y.abs() > 0.0 {
                        off.y = (off.y - wheel.y).max(0.0);
                        ui.set_scroll_target("seq_roll", off);
                        ui.request_repaint();
                    }
                }
                if ptr.pressed && keys.hovered {
                    if let Some(pitch) = key_at(&g, keys.rect, ptr.pos, base, off.y, 0.0) {
                        set_preview(preview, &mut roll.key, Some(pitch));
                    }
                    ui.request_repaint();
                } else if roll.key.is_some() && ptr.down {
                    if let Some(pitch) = key_at(&g, keys.rect, ptr.pos, base, off.y, 0.0) {
                        set_preview(preview, &mut roll.key, Some(pitch));
                    }
                    ui.request_repaint();
                }
                if ptr.released {
                    set_preview(preview, &mut roll.key, None);
                }
                draw_keys(ui, keys.rect, &g, base, off.y, roll.key, 0.0);
                if roll.key.is_some() {
                    ui.request_repaint();
                }
                roll.header = header_rect;
                roll.view = Rect {
                    min: Vec2::new(header_rect.min.x, keys.rect.min.y),
                    max: Vec2::new(header_rect.max.x, keys.rect.max.y),
                };
            });
        });
    });

    ROLLS.with(|m| {
        m.borrow_mut().insert(id.to_string(), roll);
    });
    seek_to
}

fn apply_zoom_at_cursor(
    ui: &mut Ui,
    roll: &mut Roll,
    g: &mut RollGeom,
    scale: f32,
    steps: u32,
) {
    let ptr = ui.pointer();
    if !ptr.ctrl {
        return;
    }
    let over_header = roll.header.contains(ptr.pos);
    let over_view = roll.view.contains(ptr.pos);
    if !over_header && !over_view {
        return;
    }
    let wheel = ui.take_scroll();
    if wheel.y.abs() <= 0.0 {
        return;
    }
    let next = clamp_zoom(roll.zoom * if wheel.y > 0.0 { 1.1 } else { 0.9 });
    if (next - roll.zoom).abs() <= 1e-6 {
        return;
    }
    let min_x = if over_header {
        roll.header.min.x
    } else {
        roll.view.min.x
    };
    let local_x = ptr.pos.x - min_x;
    let old = *g;
    roll.zoom = next;
    *g = geom_editor(scale, steps, EDITOR_ROWS, roll.zoom);
    apply_h_zoom(ui, &old, scale, steps, roll.zoom, local_x);
    ui.request_repaint();
}

fn apply_h_zoom(ui: &mut Ui, old: &RollGeom, scale: f32, steps: u32, zoom: f32, local_x: f32) {
    let next = geom_editor(scale, steps, EDITOR_ROWS, zoom);
    let mut off = ui.scroll_offset("seq_roll");
    off.x = zoom_anchor_x(old.stride.x, next.stride.x, off.x, local_x);
    ui.set_scroll_target("seq_roll", off);
}

fn editor_interact(
    ui: &mut Ui,
    notes: &mut Vec<SeqNote>,
    roll: &mut Roll,
    preview: &mut EventSender<NoteEvent>,
    area: mega_ui::Area,
    grid: Rect,
    stride: Vec2,
    ptr: mega_ui::Pointer,
    steps: u32,
    base: u8,
    rows: u32,
) {
    let hot = area.hovered || area.active;
    if ptr.select_all && hot {
        roll.sel = (0..notes.len()).collect();
        ui.request_repaint();
    }
    if ptr.delete && hot && !roll.sel.is_empty() {
        remove_sel(notes, &mut roll.sel);
        set_preview(preview, &mut roll.preview, None);
        ui.request_repaint();
    }
    if ptr.copy && hot && !roll.sel.is_empty() {
        let clip = clip_from_sel(notes, &roll.sel);
        CLIP.with(|c| *c.borrow_mut() = clip);
    }
    if ptr.cut && hot && !roll.sel.is_empty() {
        let clip = clip_from_sel(notes, &roll.sel);
        CLIP.with(|c| *c.borrow_mut() = clip);
        remove_sel(notes, &mut roll.sel);
        set_preview(preview, &mut roll.preview, None);
        ui.request_repaint();
    }
    if ptr.paste && hot {
        let clip = CLIP.with(|c| c.borrow().clone());
        if !clip.is_empty() {
            if let Some((step, pitch)) = cell_at(grid, stride, ptr.pos, steps, rows, base) {
                roll.sel = paste_at(notes, &clip, step, pitch, steps);
                ui.request_repaint();
            }
        }
    }
    if ptr.duplicate && hot && !roll.sel.is_empty() {
        let next = clone_sel(notes, &roll.sel, steps);
        roll.sel = next;
        ui.request_repaint();
    }

    if ptr.right_pressed && hot {
        if let Some((idx, _)) = hit_note(notes, grid, stride, ptr.pos, base, rows) {
            if sel_has(&roll.sel, idx) {
                remove_sel(notes, &mut roll.sel);
            } else {
                notes.remove(idx);
                roll.sel.clear();
            }
        }
        roll.drag = Some(Drag::Erase);
        set_preview(preview, &mut roll.preview, None);
        ui.request_repaint();
    } else if matches!(roll.drag, Some(Drag::Erase)) && ptr.right_down && area.active {
        erase_at(notes, grid, stride, ptr.pos, base, rows);
        ui.request_repaint();
    } else if ptr.pressed && area.hovered {
        match hit_note(notes, grid, stride, ptr.pos, base, rows) {
            Some((idx, edge)) if edge != Edge::Body && !ptr.ctrl => {
                roll.last_len = notes[idx].len.max(1);
                roll.sel = vec![idx];
                roll.drag = Some(Drag::Resize {
                    idx,
                    left: edge == Edge::Left,
                });
                set_preview(preview, &mut roll.preview, Some(notes[idx].pitch));
            }
            Some((idx, _)) if ptr.ctrl => {
                toggle_sel(&mut roll.sel, idx);
                set_preview(preview, &mut roll.preview, Some(notes[idx].pitch));
            }
            Some((idx, _)) => {
                let n = notes[idx];
                roll.last_len = n.len.max(1);
                if !sel_has(&roll.sel, idx) {
                    roll.sel = vec![idx];
                }
                if let Some((step, pitch)) = cell_at(grid, stride, ptr.pos, steps, rows, base) {
                    let orig = roll
                        .sel
                        .iter()
                        .filter_map(|&i| notes.get(i).map(|n| (i, n.step, n.pitch)))
                        .collect();
                    roll.drag = Some(Drag::Move {
                        press_step: step,
                        press_pitch: pitch,
                        orig,
                    });
                }
                set_preview(preview, &mut roll.preview, Some(n.pitch));
            }
            None if ptr.ctrl => {
                roll.drag = Some(Drag::Box {
                    a: ptr.pos,
                    b: ptr.pos,
                });
            }
            None => {
                roll.sel.clear();
                if let Some((step, pitch)) = cell_at(grid, stride, ptr.pos, steps, rows, base)
                {
                    let len = roll.last_len.max(1).min(steps - step);
                    notes.push(SeqNote { step, pitch, len });
                    let idx = notes.len() - 1;
                    roll.sel = vec![idx];
                    roll.drag = Some(Drag::Move {
                        press_step: step,
                        press_pitch: pitch,
                        orig: vec![(idx, step, pitch)],
                    });
                    set_preview(preview, &mut roll.preview, Some(pitch));
                }
            }
        }
        ui.request_repaint();
    } else if area.active && ptr.down {
        match roll.drag.clone() {
            Some(Drag::Move {
                press_step,
                press_pitch,
                orig,
            }) => {
                if let Some((step, pitch)) = cell_at(grid, stride, ptr.pos, steps, rows, base)
                {
                    shift_sel(
                        notes,
                        &orig,
                        step as i32 - press_step as i32,
                        pitch as i32 - press_pitch as i32,
                        steps,
                    );
                    if let Some((_, p)) = orig.first().and_then(|o| notes.get(o.0)).map(|n| (n.step, n.pitch))
                    {
                        set_preview(preview, &mut roll.preview, Some(p));
                    }
                }
                ui.request_repaint();
            }
            Some(Drag::Resize { idx, left }) => {
                if let Some(n) = notes.get_mut(idx) {
                    if let Some((step, _)) = cell_at(grid, stride, ptr.pos, steps, rows, base) {
                        resize_note(n, step, left, steps);
                        roll.last_len = n.len.max(1);
                    }
                }
                ui.request_repaint();
            }
            Some(Drag::Box { a, .. }) => {
                roll.drag = Some(Drag::Box { a, b: ptr.pos });
                ui.request_repaint();
            }
            Some(Drag::Erase) | None => {}
        }
    }

    if ptr.released {
        if let Some(Drag::Box { a, b }) = roll.drag.take() {
            let hit = notes_in_box(notes, grid, stride, a, b, base, rows);
            if ptr.ctrl {
                for i in hit {
                    if !sel_has(&roll.sel, i) {
                        roll.sel.push(i);
                    }
                }
            } else {
                roll.sel = hit;
            }
        } else {
            roll.drag = None;
        }
        set_preview(preview, &mut roll.preview, None);
    }
}

fn key_at(g: &RollGeom, keys: Rect, pos: Vec2, base: u8, offset_y: f32, head_h: f32) -> Option<u8> {
    if pos.x < keys.min.x || pos.x >= keys.max.x {
        return None;
    }
    let y = pos.y - keys.min.y + offset_y - head_h;
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
    let grid = draw_grid(ui, area.rect, &g, base, true, true);

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
        let g = geom_editor(1.0, 16, EDITOR_ROWS, 1.0);
        let keys = Rect::from_min_size(Vec2::ZERO, Vec2::new(g.label_w, 200.0));
        let row = row_of_pitch(EDITOR_BASE, 60, EDITOR_ROWS).unwrap();
        let pos = Vec2::new(g.label_w * 0.5, 4.0);
        let pitch = key_at(&g, keys, pos, EDITOR_BASE, row as f32 * g.stride.y, 0.0).unwrap();
        assert_eq!(pitch, 60);
    }

    #[test]
    fn group_shift_stays_in_grid() {
        let mut notes = vec![
            SeqNote { step: 0, pitch: 60, len: 2 },
            SeqNote { step: 2, pitch: 64, len: 2 },
        ];
        let orig = vec![(0, 0, 60), (1, 2, 64)];
        shift_sel(&mut notes, &orig, -4, -2, 16);
        assert_eq!(notes[0].step, 0);
        assert_eq!(notes[0].pitch, 58);
        assert_eq!(notes[1].step, 2);
        assert_eq!(notes[1].pitch, 62);
        let orig = vec![
            (0, notes[0].step, notes[0].pitch),
            (1, notes[1].step, notes[1].pitch),
        ];
        shift_sel(&mut notes, &orig, 20, 0, 16);
        assert_eq!(notes[1].step, 15);
        assert_eq!(notes[0].step, 13);
    }

    #[test]
    fn paste_at_cursor_keeps_shape() {
        let clip = vec![
            SeqNote { step: 4, pitch: 60, len: 2 },
            SeqNote { step: 6, pitch: 64, len: 1 },
        ];
        let mut notes = Vec::new();
        let sel = paste_at(&mut notes, &clip, 8, 72, 32);
        assert_eq!(sel, vec![0, 1]);
        assert_eq!(notes[0], SeqNote { step: 8, pitch: 72, len: 2 });
        assert_eq!(notes[1], SeqNote { step: 10, pitch: 76, len: 1 });
    }

    #[test]
    fn zoom_clamps() {
        assert_eq!(clamp_zoom(0.01), ZOOM_MIN);
        assert_eq!(clamp_zoom(9.0), ZOOM_MAX);
        assert!((clamp_zoom(1.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_keeps_cursor_content() {
        let off = zoom_anchor_x(10.0, 20.0, 40.0, 80.0);
        assert!((off - 160.0).abs() < 0.01);
        let off = zoom_anchor_x(20.0, 10.0, 160.0, 80.0);
        assert!((off - 40.0).abs() < 0.01);
        let off = zoom_anchor_x(16.0, 17.6, 0.0, 120.0);
        let content = 120.0 * (17.6 / 16.0);
        assert!((off - (content - 120.0)).abs() < 0.01);
    }

    #[test]
    fn header_click_maps_to_step() {
        let rect = Rect::from_min_size(Vec2::new(100.0, 0.0), Vec2::new(200.0, 20.0));
        assert_eq!(header_step(rect, 10.0, 0.0, Vec2::new(100.0, 5.0), 32), 0);
        assert_eq!(header_step(rect, 10.0, 0.0, Vec2::new(125.0, 5.0), 32), 2);
        assert_eq!(header_step(rect, 10.0, 40.0, Vec2::new(100.0, 5.0), 32), 4);
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
