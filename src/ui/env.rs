use glam::Vec2;
use mega_audio::dsp::sample_env;
use mega_ui::{CursorIcon, Rect, Ui};

use crate::graph::{default_env_pts, EnvEdit, EnvPt, EnvSeg, GraphNode, ENV_SEG_NAMES};

const PLOT: Vec2 = Vec2::new(260.0, 120.0);
const BG: [f32; 4] = [0.05, 0.05, 0.05, 1.0];
const GRID: [f32; 4] = [0.14, 0.14, 0.16, 1.0];
const LINE: [f32; 4] = [0.12, 0.32, 0.72, 1.0];
const TEN: [f32; 4] = [0.95, 0.45, 0.28, 1.0];

const HEAD: [f32; 4] = [0.92, 0.22, 0.22, 1.0];

struct EnvView<'a> {
    pts: &'a mut Vec<EnvPt>,
    time: &'a mut f32,
    lo: &'a mut f32,
    hi: &'a mut f32,
    sel: &'a mut Option<usize>,
    drag_ten: &'a mut bool,
    ten0: &'a mut f32,
    drag_v0: &'a mut f32,
}

pub fn draw(ui: &mut Ui, node: &mut GraphNode, playhead: Option<f32>) {
    let mut view = EnvView {
        pts: &mut node.env_pts,
        time: &mut node.env_time,
        lo: &mut node.env_lo,
        hi: &mut node.env_hi,
        sel: &mut node.env_sel,
        drag_ten: &mut node.env_drag_ten,
        ten0: &mut node.env_ten0,
        drag_v0: &mut node.env_drag_v0,
    };
    draw_view(ui, "env", &mut view, playhead);
}

pub fn draw_edit(ui: &mut Ui, id: &str, edit: &mut EnvEdit, playhead: Option<f32>) {
    ui.checkbox("On", &mut edit.enabled);
    let playhead = if edit.enabled { playhead } else { None };
    let mut view = EnvView {
        pts: &mut edit.pts,
        time: &mut edit.time,
        lo: &mut edit.lo,
        hi: &mut edit.hi,
        sel: &mut edit.sel,
        drag_ten: &mut edit.drag_ten,
        ten0: &mut edit.ten0,
        drag_v0: &mut edit.drag_v0,
    };
    draw_view(ui, id, &mut view, playhead);
}

fn draw_view(ui: &mut Ui, id: &str, node: &mut EnvView, playhead: Option<f32>) {
    if node.pts.len() < 2 {
        *node.pts = default_env_pts();
    }
    ensure_ends(node.pts);

    let z = ui.scale();
    let axis_h = 14.0 * z;
    let area = ui.area(&format!("{id}_plot"), Vec2::new(PLOT.x * z, PLOT.y * z + axis_h));
    let plot = Rect {
        min: area.rect.min,
        max: Vec2::new(area.rect.max.x, area.rect.max.y - axis_h),
    };
    ui.fill_round(plot, 4.0 * z, BG);
    draw_grid(ui, plot);
    draw_markers(ui, plot, node.pts, z);
    draw_curve(ui, plot, node.pts, z);
    draw_playhead(ui, plot, playhead, z);
    draw_handles(ui, plot, node.pts, *node.sel, z);

    let lo = format!("{:.4}", *node.lo);
    let hi = format!("{:.4}", *node.hi);
    ui.text_at_size(
        Vec2::new(plot.min.x + 2.0, plot.min.y + 2.0),
        &hi,
        10.0 * z,
    );
    ui.text_at_size(
        Vec2::new(plot.min.x + 2.0, plot.max.y - 12.0 * z),
        &lo,
        10.0 * z,
    );

    let t0 = format!("0 s");
    let t1 = format!("{:.2} s", *node.time);
    ui.text_at_size(
        Vec2::new(plot.min.x + 2.0, plot.max.y + 1.0),
        &t0,
        10.0 * z,
    );
    let tw = t1.len() as f32 * 5.5 * z;
    ui.text_at_size(
        Vec2::new((plot.max.x - tw - 2.0).max(plot.min.x), plot.max.y + 1.0),
        &t1,
        10.0 * z,
    );

    let ptr = ui.pointer();
    if area.hovered {
        ui.set_mouse_cursor(CursorIcon::Pointer);
    }
    interact(ui, node, plot, area.active, area.hovered, z);

    if let Some(i) = *node.sel {
        if i < node.pts.len() {
            ui.label("Segment");
            let mut seg = node.pts[i].seg as usize;
            ui.select(&format!("{id}_seg"), &mut seg, &ENV_SEG_NAMES);
            node.pts[i].seg = EnvSeg::from_index(seg);
            ui.horizontal(|ui| {
                let mut d = node.pts[i].decay;
                let mut s = node.pts[i].sustain;
                ui.checkbox("Decay", &mut d);
                ui.checkbox("Sustain", &mut s);
                if d != node.pts[i].decay {
                    exclusive_flag(node.pts, i, true, d);
                }
                if s != node.pts[i].sustain {
                    exclusive_flag(node.pts, i, false, s);
                }
            });
        }
    }

    ui.label("Time, sec");
    ui.drag_float(&format!("{id}_time"), node.time, 0.01);
    *node.time = node.time.clamp(0.02, 8.0);
    ui.label("Out min / max");
    let mut out = Vec2::new(*node.lo, *node.hi);
    ui.vec2(&format!("{id}_out"), &mut out, 0.1, Vec2::new(0.0, 1.0));
    *node.lo = out.x;
    *node.hi = out.y;
    ui.request_repaint();

    if ptr.delete && (area.hovered || node.sel.is_some()) {
        delete_sel(node);
        ui.consume_delete();
    }
}

fn ensure_ends(pts: &mut [EnvPt]) {
    if let Some(p) = pts.first_mut() {
        p.t = 0.0;
    }
    if let Some(p) = pts.last_mut() {
        p.t = 1.0;
    }
}

fn exclusive_flag(pts: &mut [EnvPt], i: usize, decay: bool, on: bool) {
    if decay {
        for p in pts.iter_mut() {
            p.decay = false;
        }
        pts[i].decay = on;
    } else {
        for p in pts.iter_mut() {
            p.sustain = false;
        }
        pts[i].sustain = on;
    }
}

fn to_screen(plot: Rect, t: f32, v: f32) -> Vec2 {
    Vec2::new(
        plot.min.x + plot.width() * t.clamp(0.0, 1.0),
        plot.max.y - plot.height() * v.clamp(0.0, 1.0),
    )
}

fn from_screen(plot: Rect, p: Vec2) -> (f32, f32) {
    let t = ((p.x - plot.min.x) / plot.width().max(1.0)).clamp(0.0, 1.0);
    let v = ((plot.max.y - p.y) / plot.height().max(1.0)).clamp(0.0, 1.0);
    (t, v)
}

fn draw_grid(ui: &mut Ui, plot: Rect) {
    for i in 1..4 {
        let x = plot.min.x + plot.width() * (i as f32 / 4.0);
        ui.line(
            Vec2::new(x, plot.min.y),
            Vec2::new(x, plot.max.y),
            1.0,
            GRID,
        );
        let y = plot.min.y + plot.height() * (i as f32 / 4.0);
        ui.line(
            Vec2::new(plot.min.x, y),
            Vec2::new(plot.max.x, y),
            1.0,
            GRID,
        );
    }
}

fn draw_markers(ui: &mut Ui, plot: Rect, pts: &[EnvPt], z: f32) {
    for p in pts {
        if !p.decay && !p.sustain {
            continue;
        }
        let x = to_screen(plot, p.t, 0.0).x;
        let col = if p.sustain {
            [0.28, 0.78, 0.42, 0.7]
        } else {
            [0.7, 0.7, 0.72, 0.55]
        };
        ui.line(
            Vec2::new(x, plot.min.y),
            Vec2::new(x, plot.max.y),
            1.0,
            col,
        );
        let label = if p.sustain { "S" } else { "D" };
        ui.text_at_size(
            Vec2::new(x + 2.0 * z, plot.min.y + 2.0 * z),
            label,
            10.0 * z,
        );
    }
}

fn draw_curve(ui: &mut Ui, plot: Rect, pts: &[EnvPt], z: f32) {
    let knots = pts
        .iter()
        .map(|p| mega_audio::dsp::EnvKnot {
            t: p.t,
            v: p.v,
            tension: p.tension,
            kind: p.seg.to_dsp(),
            sustain: p.sustain,
        })
        .collect::<Vec<_>>();
    let n = 128;
    let mut prev = to_screen(plot, 0.0, sample_env(&knots, 0.0));
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let cur = to_screen(plot, t, sample_env(&knots, t));
        ui.line(prev, cur, 1.6 * z, LINE);
        prev = cur;
    }
}

fn handle_pos(plot: Rect, a: EnvPt, b: EnvPt) -> Vec2 {
    let t = (a.t + b.t) * 0.5;
    // Stairs / Pulse / Wave sample at 0.5 lands on a plateau and snaps tension to 0.
    let v = match a.seg {
        EnvSeg::Stairs | EnvSeg::Pulse | EnvSeg::Wave => {
            ((a.v + b.v) * 0.5 + a.tension * 0.4).clamp(0.0, 1.0)
        }
        _ => sample_env(
            &[
                mega_audio::dsp::EnvKnot {
                    t: 0.0,
                    v: a.v,
                    tension: a.tension,
                    kind: a.seg.to_dsp(),
                    sustain: false,
                },
                mega_audio::dsp::EnvKnot {
                    t: 1.0,
                    v: b.v,
                    tension: a.tension,
                    kind: a.seg.to_dsp(),
                    sustain: false,
                },
            ],
            0.5,
        ),
    };
    to_screen(plot, t, v)
}

fn draw_playhead(ui: &mut Ui, plot: Rect, playhead: Option<f32>, z: f32) {
    let Some(t) = playhead.filter(|t| *t >= 0.0) else {
        return;
    };
    let x = plot.min.x + plot.width() * t.clamp(0.0, 1.0);
    ui.line(
        Vec2::new(x, plot.min.y),
        Vec2::new(x, plot.max.y),
        1.5 * z,
        HEAD,
    );
}

fn draw_handles(ui: &mut Ui, plot: Rect, pts: &[EnvPt], sel: Option<usize>, z: f32) {
    for i in 0..pts.len().saturating_sub(1) {
        let c = handle_pos(plot, pts[i], pts[i + 1]);
        let r = 4.0 * z;
        let col = TEN;
        ui.fill_round(
            Rect::from_min_size(c - Vec2::splat(r), Vec2::splat(r * 2.0)),
            r,
            BG,
        );
        ui.fill_round(
            Rect::from_min_size(c - Vec2::splat(r * 0.45), Vec2::splat(r * 0.9)),
            r * 0.45,
            col,
        );
    }
    for (i, p) in pts.iter().enumerate() {
        let c = to_screen(plot, p.t, p.v);
        let r = if Some(i) == sel { 6.0 * z } else { 5.0 * z };
        let col = if p.sustain {
            [0.35, 0.92, 0.48, 1.0]
        } else if p.decay {
            [0.85, 0.85, 0.88, 1.0]
        } else {
            [0.98, 0.55, 0.22, 1.0]
        };
        ui.fill_round(
            Rect::from_min_size(c - Vec2::splat(r), Vec2::splat(r * 2.0)),
            r,
            col,
        );
    }
}

fn hit_r(z: f32) -> f32 {
    10.0 * z
}

fn interact(ui: &mut Ui, node: &mut EnvView, plot: Rect, active: bool, hovered: bool, z: f32) {
    let ptr = ui.pointer();
    let hr = hit_r(z);

    if ptr.right_pressed && hovered {
        if let Some(i) = hit_point(node.pts, plot, ptr.pos, hr) {
            *node.sel = Some(i);
            if delete_sel(node) {
                return;
            }
        }
    }

    if ptr.pressed && hovered {
        if let Some(i) = hit_tension(node.pts, plot, ptr.pos, hr) {
            *node.sel = Some(i);
            *node.drag_ten = true;
            *node.ten0 = node.pts[i].tension;
            *node.drag_v0 = from_screen(plot, ptr.pos).1;
        } else if let Some(i) = hit_point(node.pts, plot, ptr.pos, hr) {
            *node.sel = Some(i);
            *node.drag_ten = false;
        } else if plot.contains(ptr.pos) {
            let (t, v) = from_screen(plot, ptr.pos);
            let pt = EnvPt {
                t,
                v,
                tension: 0.0,
                seg: EnvSeg::Curve,
                decay: false,
                sustain: false,
            };
            node.pts.push(pt);
            node.pts.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
            ensure_ends(node.pts);
            *node.sel = node.pts.iter().position(|p| (p.t - t).abs() < 1e-4);
            *node.drag_ten = false;
        }
    }

    if active && ptr.down {
        if *node.drag_ten {
            if let Some(i) = *node.sel {
                if i + 1 < node.pts.len() {
                    let (_, v) = from_screen(plot, ptr.pos);
                    let ten = *node.ten0 + (v - *node.drag_v0) * 2.4;
                    node.pts[i].tension = match node.pts[i].seg {
                        EnvSeg::Stairs | EnvSeg::Pulse | EnvSeg::Wave => ten.clamp(0.0, 1.0),
                        _ => ten.clamp(-1.0, 1.0),
                    };
                }
            }
        } else if let Some(i) = *node.sel {
            if i > 0 && i + 1 < node.pts.len() {
                let (t, v) = from_screen(plot, ptr.pos);
                let lo = node.pts[i - 1].t + 0.01;
                let hi = node.pts[i + 1].t - 0.01;
                node.pts[i].t = t.clamp(lo, hi.max(lo));
                node.pts[i].v = v;
            } else if i == 0 {
                let (_, v) = from_screen(plot, ptr.pos);
                node.pts[0].v = v;
                node.pts[0].t = 0.0;
            } else if i + 1 == node.pts.len() {
                let (_, v) = from_screen(plot, ptr.pos);
                if let Some(p) = node.pts.last_mut() {
                    p.v = v;
                    p.t = 1.0;
                }
            }
        }
    }

    if ptr.released {
        *node.drag_ten = false;
    }
}

fn delete_sel(node: &mut EnvView) -> bool {
    let Some(i) = *node.sel else {
        return false;
    };
    if i == 0 || i + 1 >= node.pts.len() {
        return false;
    }
    node.pts.remove(i);
    *node.sel = None;
    true
}

fn hit_point(pts: &[EnvPt], plot: Rect, pos: Vec2, hr: f32) -> Option<usize> {
    pts.iter().enumerate().rev().find_map(|(i, p)| {
        let c = to_screen(plot, p.t, p.v);
        if (c - pos).length() <= hr {
            Some(i)
        } else {
            None
        }
    })
}

fn hit_tension(pts: &[EnvPt], plot: Rect, pos: Vec2, hr: f32) -> Option<usize> {
    for i in 0..pts.len().saturating_sub(1) {
        let c = handle_pos(plot, pts[i], pts[i + 1]);
        if (c - pos).length() <= hr * 0.85 {
            return Some(i);
        }
    }
    None
}
