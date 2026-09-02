use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use glam::Vec2;
use mega_ui::{CursorIcon, Rect, Ui};

use crate::graph::{
    GraphNode, SeqNote, BEATS_PER_STEP, SEQ_PITCHES, SEQ_STEPS,
};
use crate::monitor::Monitor;

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

const LABEL_W: f32 = 28.0;
const CELL_W: f32 = 12.0;
const CELL_H: f32 = 11.0;
const HEAD_H: f32 = 4.0;
const GAP: f32 = 1.0;
const HANDLE: f32 = 6.0;
const FADE_SEC: f32 = 0.45;

thread_local! {
    static ROLLS: RefCell<HashMap<String, Roll>> = RefCell::new(HashMap::new());
    static PREVIEWS: RefCell<HashMap<String, Preview>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy)]
struct Roll {
    drag: Option<Drag>,
    last_len: u8,
}

impl Default for Roll {
    fn default() -> Self {
        Self {
            drag: None,
            last_len: 1,
        }
    }
}

#[derive(Clone, Copy)]
enum Drag {
    Paint { idx: usize, origin: u8 },
    Move { idx: usize, grab_step: i32, grab_pitch: i32 },
    Resize { idx: usize },
    Erase,
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

fn cell_at(grid: Rect, stride: Vec2, pos: Vec2, steps: u32, base: u8) -> Option<(u8, u8)> {
    if pos.x < grid.min.x || pos.y < grid.min.y || pos.x >= grid.max.x || pos.y >= grid.max.y {
        return None;
    }
    let col = ((pos.x - grid.min.x) / stride.x).floor() as i32;
    let row = ((pos.y - grid.min.y) / stride.y).floor() as i32;
    if col < 0 || row < 0 || col >= steps as i32 || row >= SEQ_PITCHES as i32 {
        return None;
    }
    Some((col as u8, pitch_of_row(base, row as u32, SEQ_PITCHES)))
}

fn note_rect(grid: Rect, stride: Vec2, note: SeqNote, base: u8) -> Option<Rect> {
    let row = row_of_pitch(base, note.pitch, SEQ_PITCHES)?;
    let x = grid.min.x + note.step as f32 * stride.x;
    let y = grid.min.y + row as f32 * stride.y;
    let w = (note.len.max(1) as f32 * stride.x - GAP).max(2.0);
    let h = (stride.y - GAP).max(2.0);
    Some(Rect::from_min_size(Vec2::new(x, y), Vec2::new(w, h)))
}

fn hit_note(
    notes: &[SeqNote],
    grid: Rect,
    stride: Vec2,
    pos: Vec2,
    base: u8,
) -> Option<(usize, bool)> {
    for (i, n) in notes.iter().enumerate().rev() {
        let Some(r) = note_rect(grid, stride, *n, base) else {
            continue;
        };
        if !r.contains(pos) {
            continue;
        }
        let handle = HANDLE.min(r.width() * 0.45).max(3.0);
        let on_handle = pos.x >= r.max.x - handle;
        return Some((i, on_handle));
    }
    None
}

fn erase_at(notes: &mut Vec<SeqNote>, grid: Rect, stride: Vec2, pos: Vec2, base: u8) -> bool {
    if let Some((i, _)) = hit_note(notes, grid, stride, pos, base) {
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
}

fn geom(s: f32, steps: u32, rows: u32) -> RollGeom {
    let gap = GAP.max(s * 0.5);
    let stride = Vec2::new(CELL_W * s + gap, CELL_H * s + gap);
    let label_w = LABEL_W * s;
    let head_h = HEAD_H * s;
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
    }
}

fn draw_grid(
    ui: &mut Ui,
    rect: Rect,
    g: &RollGeom,
    base: u8,
    play_step: Option<u32>,
) -> Rect {
    let grid = Rect {
        min: Vec2::new(rect.min.x + g.label_w, rect.min.y + g.head_h),
        max: Vec2::new(rect.max.x, rect.max.y),
    };
    ui.fill_rect(rect, [0.07, 0.07, 0.08, 1.0]);
    for step in 0..g.steps {
        let x = grid.min.x + step as f32 * g.stride.x;
        let on = play_step == Some(step);
        let bar = step % SEQ_STEPS == 0;
        let color = if on {
            [1.0, 0.38, 0.16, 1.0]
        } else if bar {
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
        let px = (8.0 * g.s).min(row_h);
        ui.text_at_size(
            Vec2::new(rect.min.x + 2.0, y + (row_h - px) * 0.5),
            &pitch_name(pitch),
            px,
        );
        for step in 0..g.steps {
            let x = grid.min.x + step as f32 * g.stride.x;
            let mut color = if step % SEQ_STEPS == 0 {
                if sharp {
                    [0.18, 0.18, 0.24, 1.0]
                } else {
                    [0.22, 0.22, 0.28, 1.0]
                }
            } else if step % 4 == 0 {
                if sharp {
                    [0.16, 0.16, 0.20, 1.0]
                } else {
                    [0.20, 0.20, 0.24, 1.0]
                }
            } else if sharp {
                [0.10, 0.10, 0.12, 1.0]
            } else {
                [0.13, 0.13, 0.15, 1.0]
            };
            if play_step == Some(step) {
                color = [0.42, 0.22, 0.14, 1.0];
            }
            ui.fill_rect(
                Rect::from_min_size(
                    Vec2::new(x, y),
                    Vec2::new((g.stride.x - g.gap).max(1.0), (g.stride.y - g.gap).max(1.0)),
                ),
                color,
            );
        }
    }
    grid
}

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
        if let Some((_, on_handle)) = hit_note(&node.notes, grid, g.stride, ptr.pos, base) {
            ui.set_mouse_cursor(if on_handle && !matches!(roll.drag, Some(Drag::Move { .. })) {
                CursorIcon::ResizeEw
            } else {
                CursorIcon::Move
            });
        } else if area.hovered {
            ui.set_mouse_cursor(CursorIcon::Pointer);
        }
    }

    if ptr.right_pressed && (area.hovered || area.active) {
        erase_at(&mut node.notes, grid, g.stride, ptr.pos, base);
        roll.drag = Some(Drag::Erase);
        ui.request_repaint();
    } else if matches!(roll.drag, Some(Drag::Erase)) && ptr.right_down && area.active {
        erase_at(&mut node.notes, grid, g.stride, ptr.pos, base);
        ui.request_repaint();
    } else if ptr.pressed && area.hovered {
        match hit_note(&node.notes, grid, g.stride, ptr.pos, base) {
            Some((idx, true)) => {
                roll.drag = Some(Drag::Resize { idx });
            }
            Some((idx, false)) => {
                let n = node.notes[idx];
                if let Some((step, pitch)) = cell_at(grid, g.stride, ptr.pos, steps, base) {
                    roll.drag = Some(Drag::Move {
                        idx,
                        grab_step: step as i32 - n.step as i32,
                        grab_pitch: pitch as i32 - n.pitch as i32,
                    });
                }
            }
            None => {
                if let Some((step, pitch)) = cell_at(grid, g.stride, ptr.pos, steps, base) {
                    let len = roll.last_len.max(1).min((steps - step as u32) as u8);
                    node.notes.push(SeqNote { step, pitch, len });
                    roll.drag = Some(Drag::Paint {
                        idx: node.notes.len() - 1,
                        origin: step,
                    });
                }
            }
        }
        ui.request_repaint();
    } else if area.active && ptr.down {
        match roll.drag {
            Some(Drag::Paint { idx, origin }) => {
                if let Some(n) = node.notes.get_mut(idx) {
                    if let Some((step, _)) = cell_at(grid, g.stride, ptr.pos, steps, base) {
                        if step != origin {
                            let a = origin.min(step);
                            let b = origin.max(step);
                            n.step = a;
                            n.len = (b - a + 1).max(1);
                            clamp_note(n, steps);
                            roll.last_len = n.len;
                        }
                    }
                }
                ui.request_repaint();
            }
            Some(Drag::Move {
                idx,
                grab_step,
                grab_pitch,
            }) => {
                if let Some((step, pitch)) = cell_at(grid, g.stride, ptr.pos, steps, base) {
                    if let Some(n) = node.notes.get_mut(idx) {
                        n.step = (step as i32 - grab_step).clamp(0, steps as i32 - 1) as u8;
                        n.pitch = (pitch as i32 - grab_pitch).clamp(0, 127) as u8;
                        clamp_note(n, steps);
                    }
                }
                ui.request_repaint();
            }
            Some(Drag::Resize { idx }) => {
                if let Some(n) = node.notes.get_mut(idx) {
                    if let Some((step, _)) = cell_at(grid, g.stride, ptr.pos, steps, base) {
                        n.len = step.saturating_sub(n.step).saturating_add(1);
                        clamp_note(n, steps);
                        roll.last_len = n.len;
                    }
                }
                ui.request_repaint();
            }
            Some(Drag::Erase) | None => {}
        }
    }

    if ptr.released {
        if let Some(Drag::Paint { idx, .. }) = roll.drag {
            if let Some(n) = node.notes.get(idx) {
                roll.last_len = n.len.max(1);
            }
        }
        roll.drag = None;
    }

    ROLLS.with(|m| {
        m.borrow_mut().insert(node_id.to_string(), roll);
    });

    let grid = draw_grid(ui, rect, &g, base, play_step);

    let active_idx = match roll.drag {
        Some(Drag::Paint { idx, .. } | Drag::Move { idx, .. } | Drag::Resize { idx }) => Some(idx),
        _ => None,
    };
    for (i, note) in node.notes.iter().copied().enumerate() {
        let Some(r) = note_rect(grid, g.stride, note, base) else {
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
        let handle = HANDLE.min(r.width() * 0.45).max(2.0);
        ui.fill_rect(
            Rect::from_min_size(
                Vec2::new(r.max.x - handle, r.min.y + 1.0),
                Vec2::new(handle, (r.height() - 2.0).max(1.0)),
            ),
            [1.0, 0.92, 0.55, 0.35],
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
    let grid = draw_grid(ui, area.rect, &g, base, play_step);

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
            cell_at(grid, stride, Vec2::new(10.0, 20.0), 16, base),
            Some((0, pitch_of_row(base, 0, SEQ_PITCHES)))
        );
        assert_eq!(
            cell_at(grid, stride, Vec2::new(25.0, 35.0), 16, base),
            Some((1, pitch_of_row(base, 1, SEQ_PITCHES)))
        );
        assert_eq!(cell_at(grid, stride, Vec2::new(9.0, 20.0), 16, base), None);
    }

    #[test]
    fn note_hit_prefers_handle() {
        let grid = Rect::from_min_size(Vec2::ZERO, Vec2::new(200.0, 200.0));
        let stride = Vec2::new(12.0, 11.0);
        let base = 60;
        let pitch = pitch_of_row(base, 2, SEQ_PITCHES);
        let notes = vec![SeqNote {
            step: 0,
            pitch,
            len: 4,
        }];
        let r = note_rect(grid, stride, notes[0], base).unwrap();
        let (i, handle) =
            hit_note(&notes, grid, stride, Vec2::new(r.max.x - 1.0, r.min.y + 2.0), base).unwrap();
        assert_eq!(i, 0);
        assert!(handle);
        let (_, handle) =
            hit_note(&notes, grid, stride, Vec2::new(r.min.x + 2.0, r.min.y + 2.0), base).unwrap();
        assert!(!handle);
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
