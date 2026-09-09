use std::cell::RefCell;
use std::ops::RangeInclusive;

use glam::Vec2;
use mega_ui::{CursorIcon, LayoutOpts, Rect, TextStyle, Ui};

use crate::graph::GraphNode;

pub const ATTACK: RangeInclusive<f32> = 0.001..=2.0;
pub const DECAY: RangeInclusive<f32> = 0.01..=2.0;
pub const SUSTAIN: RangeInclusive<f32> = 0.0..=1.0;
pub const RELEASE: RangeInclusive<f32> = 0.01..=4.0;

const HOLD: f32 = 0.24;
const COL_A: [f32; 4] = [0.28, 0.55, 0.95, 1.0];
const COL_D: [f32; 4] = [0.90, 0.32, 0.28, 1.0];
const COL_S: [f32; 4] = [0.92, 0.78, 0.22, 1.0];
const COL_R: [f32; 4] = [0.28, 0.78, 0.42, 1.0];
const LINE: [f32; 4] = [0.82, 0.90, 1.0, 1.0];
const FILL: [f32; 4] = [0.20, 0.38, 0.72, 0.28];
const BG: [f32; 4] = [0.05, 0.05, 0.05, 1.0];
const ZONE_A: [f32; 4] = [0.12, 0.22, 0.42, 0.35];
const ZONE_D: [f32; 4] = [0.42, 0.14, 0.12, 0.35];
const ZONE_S: [f32; 4] = [0.38, 0.32, 0.08, 0.35];
const ZONE_R: [f32; 4] = [0.10, 0.32, 0.16, 0.35];
const CAPTION: [f32; 4] = [0.50, 0.50, 0.50, 1.0];
const KNOB_DIAL: f32 = 52.0;

thread_local! {
    static DRAG: RefCell<Option<(String, Handle)>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Handle {
    Attack,
    Decay,
    Off,
    SustainY,
}

#[derive(Clone, Copy)]
struct Plot {
    peak: Vec2,
    decay: Vec2,
    off: Vec2,
    end: Vec2,
}

fn clamp_range(v: f32, r: RangeInclusive<f32>) -> f32 {
    v.clamp(*r.start(), *r.end())
}

fn plot(attack: f32, decay: f32, sustain: f32, release: f32) -> Plot {
    let a = attack.max(1e-4);
    let d = decay.max(1e-4);
    let r = release.max(1e-4);
    let s = sustain.clamp(0.0, 1.0);
    let rest = (1.0 - HOLD).max(0.2);
    let sum = a + d + r;
    let xa = a / sum * rest;
    let xd = d / sum * rest;
    Plot {
        peak: Vec2::new(xa, 1.0),
        decay: Vec2::new(xa + xd, s),
        off: Vec2::new(xa + xd + HOLD, s),
        end: Vec2::new(1.0, 0.0),
    }
}

fn set_attack(a: &mut f32, d: f32, r: f32, nx: f32) {
    let rest = (1.0 - HOLD).max(0.2);
    let k = (nx / rest).clamp(0.02, 0.92);
    *a = clamp_range(k * (d.max(1e-4) + r.max(1e-4)) / (1.0 - k), ATTACK);
}

fn set_decay(a: f32, d: &mut f32, r: f32, nx: f32) {
    let rest = (1.0 - HOLD).max(0.2);
    let k = (nx / rest).clamp(0.02, 0.92);
    let sum_ad = k * r.max(1e-4) / (1.0 - k);
    *d = clamp_range(sum_ad - a, DECAY);
}

fn set_release(a: f32, d: f32, r: &mut f32, off_x: f32) {
    let rest = (1.0 - HOLD).max(0.2);
    let u = ((off_x - HOLD) / rest).clamp(0.08, 0.95);
    let ad = (a + d).max(1e-4);
    *r = clamp_range(ad * (1.0 - u) / u, RELEASE);
}

fn inner(rect: Rect, pad: f32) -> Rect {
    Rect {
        min: rect.min + Vec2::splat(pad),
        max: rect.max - Vec2::splat(pad),
    }
}

fn to_screen(inner: Rect, n: Vec2) -> Vec2 {
    Vec2::new(
        inner.min.x + n.x * inner.width(),
        inner.max.y - n.y * inner.height(),
    )
}

fn from_screen(inner: Rect, p: Vec2) -> Vec2 {
    let w = inner.width().max(1.0);
    let h = inner.height().max(1.0);
    Vec2::new(
        ((p.x - inner.min.x) / w).clamp(0.0, 1.0),
        ((inner.max.y - p.y) / h).clamp(0.0, 1.0),
    )
}

pub fn draw(ui: &mut Ui, node: &mut GraphNode) {
    ui.group("Volume", |ui| {
        draw_fields(
            ui,
            &node.id,
            "vol",
            &mut node.adsr_attack,
            &mut node.adsr_decay,
            &mut node.adsr_sustain,
            &mut node.adsr_release,
        );
    });
}

pub fn draw_fields(
    ui: &mut Ui,
    node_id: &str,
    key: &str,
    attack: &mut f32,
    decay: &mut f32,
    sustain: &mut f32,
    release: &mut f32,
) {
    *attack = clamp_range(*attack, ATTACK);
    *decay = clamp_range(*decay, DECAY);
    *sustain = clamp_range(*sustain, SUSTAIN);
    *release = clamp_range(*release, RELEASE);
    draw_body(ui, node_id, key, attack, decay, sustain, release);
    *attack = clamp_range(*attack, ATTACK);
    *decay = clamp_range(*decay, DECAY);
    *sustain = clamp_range(*sustain, SUSTAIN);
    *release = clamp_range(*release, RELEASE);
}

fn draw_body(
    ui: &mut Ui,
    node_id: &str,
    key: &str,
    attack: &mut f32,
    decay: &mut f32,
    sustain: &mut f32,
    release: &mut f32,
) {
    let z = ui.scale();
    let graph_h = 96.0 * z;
    let graph_w = 252.0 * z;
    let pad = 8.0 * z;
    let hit_r = 14.0 * z;

    let area = ui.area(&format!("{key}_plot"), Vec2::new(graph_w, graph_h));
    let rect = area.rect;
    ui.fill_round(rect, 4.0 * z, BG);
    let plot_r = inner(rect, pad);

    let p = plot(*attack, *decay, *sustain, *release);
    let start = to_screen(plot_r, Vec2::new(0.0, 0.0));
    let peak_pt = to_screen(plot_r, p.peak);
    let decay_pt = to_screen(plot_r, p.decay);
    let off_pt = to_screen(plot_r, p.off);
    let end_pt = to_screen(plot_r, p.end);

    fill_zone(ui, plot_r, 0.0, p.peak.x, ZONE_A);
    fill_zone(ui, plot_r, p.peak.x, p.decay.x, ZONE_D);
    fill_zone(ui, plot_r, p.decay.x, p.off.x, ZONE_S);
    fill_zone(ui, plot_r, p.off.x, 1.0, ZONE_R);
    fill_under(ui, plot_r, &[start, peak_pt, decay_pt, off_pt, end_pt]);

    let thick = 2.0 * z;
    ui.line(start, peak_pt, thick, COL_A);
    ui.line(peak_pt, decay_pt, thick, COL_D);
    ui.line(decay_pt, off_pt, thick, COL_S);
    ui.line(off_pt, end_pt, thick, COL_R);
    ui.line(start, peak_pt, thick * 0.35, LINE);
    ui.line(peak_pt, decay_pt, thick * 0.35, LINE);
    ui.line(decay_pt, off_pt, thick * 0.35, LINE);
    ui.line(off_pt, end_pt, thick * 0.35, LINE);

    draw_handle(ui, peak_pt, COL_A, z);
    draw_handle(ui, decay_pt, COL_D, z);
    draw_handle(ui, off_pt, COL_S, z);
    draw_handle(ui, end_pt, COL_R, z);

    let ptr = ui.pointer();
    let handles = [
        (Handle::Attack, peak_pt),
        (Handle::Decay, decay_pt),
        (Handle::Off, off_pt),
    ];
    let mut over = None;
    let mut best = hit_r * hit_r;
    for (h, pos) in handles {
        let d = (ptr.pos - pos).length_squared();
        if d < best {
            best = d;
            over = Some(h);
        }
    }
    if over.is_none() && area.hovered {
        let n = from_screen(plot_r, ptr.pos);
        if n.x >= p.decay.x && n.x <= p.off.x {
            over = Some(Handle::SustainY);
        }
    }

    if over.is_some() && (area.hovered || area.active) {
        ui.set_mouse_cursor(CursorIcon::Move);
    }

    let id = format!("{node_id}/{key}");
    if ptr.pressed && area.hovered {
        if let Some(h) = over {
            DRAG.with(|d| *d.borrow_mut() = Some((id.clone(), h)));
            ui.request_repaint();
        }
    }

    let drag = DRAG.with(|d| {
        d.borrow()
            .as_ref()
            .filter(|(nid, _)| *nid == id)
            .map(|(_, h)| *h)
    });

    if let Some(h) = drag {
        if area.active && ptr.down {
            let n = from_screen(plot_r, ptr.pos);
            match h {
                Handle::Attack => set_attack(attack, *decay, *release, n.x),
                Handle::Decay => {
                    set_decay(*attack, decay, *release, n.x);
                    *sustain = clamp_range(n.y, SUSTAIN);
                }
                Handle::Off => {
                    *sustain = clamp_range(n.y, SUSTAIN);
                    set_release(*attack, *decay, release, n.x);
                }
                Handle::SustainY => *sustain = clamp_range(n.y, SUSTAIN),
            }
            ui.request_repaint();
        } else if !ptr.down {
            DRAG.with(|d| *d.borrow_mut() = None);
        }
    }

    ui.row_with(
        LayoutOpts {
            spacing: Some(0.0),
            ..LayoutOpts::default()
        },
        |ui| {
            let cell = rect.width() / 4.0;
            let cap_a = fmt_ms(*attack);
            let cap_d = fmt_ms(*decay);
            let cap_s = fmt_pct(*sustain);
            let cap_r = fmt_ms(*release);
            knob_cell(ui, cell, "A", attack, ATTACK, COL_A, &cap_a);
            knob_cell(ui, cell, "D", decay, DECAY, COL_D, &cap_d);
            knob_cell(ui, cell, "S", sustain, SUSTAIN, COL_S, &cap_s);
            knob_cell(ui, cell, "R", release, RELEASE, COL_R, &cap_r);
        },
    );
}

fn knob_cell(
    ui: &mut Ui,
    cell_w: f32,
    id: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    color: [f32; 4],
    caption: &str,
) {
    ui.flex_width(cell_w, |ui| {
        let z = ui.scale();
        let dial = KNOB_DIAL * z;
        let pad = ((cell_w - dial) * 0.5).max(0.0);
        ui.row_with(
            LayoutOpts {
                spacing: Some(0.0),
                ..LayoutOpts::default()
            },
            |ui| {
                if pad > 0.5 {
                    let _ = ui.area(&format!("{id}_pad"), Vec2::new(pad, dial));
                }
                ui.knob_colored(id, value, range, color);
            },
        );
        caption_centered(ui, id, cell_w, caption);
    });
}

fn caption_centered(ui: &mut Ui, id: &str, width: f32, text: &str) {
    let z = ui.scale();
    let px = 12.0 * z;
    let h = 16.0 * z;
    let tw = approx_text_w(text, px);
    let left = ((width - tw) * 0.5).max(0.0);
    ui.row_with(
        LayoutOpts {
            spacing: Some(0.0),
            ..LayoutOpts::default()
        },
        |ui| {
            if left > 0.5 {
                let _ = ui.area(&format!("{id}_cap"), Vec2::new(left, h));
            }
            ui.label_styled(
                text,
                TextStyle {
                    color: CAPTION,
                    size: 12.0,
                },
            );
        },
    );
}

fn approx_text_w(text: &str, px: f32) -> f32 {
    text.chars()
        .map(|c| {
            let k = match c {
                '1' | 'i' | '.' => 0.36,
                ' ' => 0.32,
                '%' | 'm' | 'w' => 0.82,
                _ => 0.56,
            };
            k * px
        })
        .sum()
}

fn fill_zone(ui: &mut Ui, inner: Rect, x0: f32, x1: f32, color: [f32; 4]) {
    let a = x0.clamp(0.0, 1.0);
    let b = x1.clamp(0.0, 1.0);
    if b <= a {
        return;
    }
    let r = Rect {
        min: Vec2::new(inner.min.x + a * inner.width(), inner.min.y),
        max: Vec2::new(inner.min.x + b * inner.width(), inner.max.y),
    };
    ui.fill_rect(r, color);
}

fn fill_under(ui: &mut Ui, inner: Rect, pts: &[Vec2]) {
    let n = 48;
    let w = inner.width();
    let step = w / n as f32;
    for i in 0..n {
        let x = inner.min.x + (i as f32 + 0.5) * step;
        let mut y = inner.max.y;
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if (x >= a.x && x <= b.x) || (x >= b.x && x <= a.x) {
                let t = if (b.x - a.x).abs() < 0.001 {
                    0.0
                } else {
                    (x - a.x) / (b.x - a.x)
                };
                y = a.y + (b.y - a.y) * t;
                break;
            }
        }
        let r = Rect {
            min: Vec2::new(x - step * 0.5, y),
            max: Vec2::new(x + step * 0.5, inner.max.y),
        };
        if r.height() > 0.5 {
            ui.fill_rect(r, FILL);
        }
    }
}

fn fmt_ms(sec: f32) -> String {
    format!("{} ms", (sec * 1000.0).round() as i32)
}

fn fmt_pct(level: f32) -> String {
    format!("{}%", (level * 100.0).round() as i32)
}

fn draw_handle(ui: &mut Ui, c: Vec2, color: [f32; 4], z: f32) {
    let r = 5.0 * z;
    ui.fill_round(
        Rect::from_min_size(c - Vec2::splat(r), Vec2::splat(r * 2.0)),
        r,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_moves_right_when_attack_grows() {
        let short = plot(0.05, 0.2, 0.5, 0.2);
        let long = plot(1.0, 0.2, 0.5, 0.2);
        assert!(long.peak.x > short.peak.x);
        assert!((short.decay.y - 0.5).abs() < 1e-5);
        assert!((long.end.x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn set_attack_roundtrip_near_layout() {
        let d = 0.2;
        let r = 0.4;
        let mut a = 0.1;
        let p = plot(a, d, 0.5, r);
        set_attack(&mut a, d, r, p.peak.x);
        let p2 = plot(a, d, 0.5, r);
        assert!((p2.peak.x - p.peak.x).abs() < 0.02);
    }

    #[test]
    fn set_release_from_off_x_roundtrip() {
        let a = 0.1;
        let d = 0.2;
        let mut r = 0.4;
        let p = plot(a, d, 0.5, r);
        set_release(a, d, &mut r, p.off.x);
        let p2 = plot(a, d, 0.5, r);
        assert!((p2.off.x - p.off.x).abs() < 0.03);
    }

    #[test]
    fn formats_ms_and_sustain_percent() {
        assert_eq!(fmt_ms(0.12), "120 ms");
        assert_eq!(fmt_ms(0.001), "1 ms");
        assert_eq!(fmt_pct(0.55), "55%");
        assert_eq!(fmt_pct(0.0), "0%");
        assert!(approx_text_w("38 ms", 12.0) > approx_text_w("A", 12.0));
    }
}
