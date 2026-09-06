use std::cell::RefCell;
use std::collections::HashMap;

use glam::Vec2;
use mega_ui::{CursorIcon, LayoutOpts, Rect, ScrollAxes, Ui};

use crate::graph::Sample;

const ZOOM_MIN: f32 = 0.12;
const ZOOM_MAX: f32 = 8.0;
const PX_PER_SEC: f32 = 80.0;
const HEAD_H: f32 = 22.0;
const WAVE: [f32; 4] = [0.32, 0.72, 0.40, 1.0];
const HEAD: [f32; 4] = [1.0, 0.42, 0.18, 0.9];

thread_local! {
    static VIEWS: RefCell<HashMap<String, WaveView>> = RefCell::new(HashMap::new());
}

#[derive(Clone)]
struct WaveView {
    zoom: f32,
    header: Rect,
    wave: Rect,
}

impl Default for WaveView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            header: Rect {
                min: Vec2::ZERO,
                max: Vec2::ZERO,
            },
            wave: Rect {
                min: Vec2::ZERO,
                max: Vec2::ZERO,
            },
        }
    }
}

fn clamp_zoom(z: f32) -> f32 {
    z.clamp(ZOOM_MIN, ZOOM_MAX)
}

fn px_per_sec(zoom: f32, scale: f32) -> f32 {
    PX_PER_SEC * clamp_zoom(zoom) * scale.max(1e-6)
}

fn content_width(duration: f32, zoom: f32, scale: f32) -> f32 {
    (duration.max(0.001) * px_per_sec(zoom, scale)).max(1.0)
}

fn zoom_anchor_x(old: f32, new: f32, offset: f32, local: f32) -> f32 {
    let content = (offset + local).max(0.0);
    let ratio = new / old.max(1e-6);
    (content * ratio - local).max(0.0)
}

pub fn draw_editor(ui: &mut Ui, id: &str, sample: &Sample, play_t: f32) -> Option<f64> {
    let duration = sample.duration().max(0.001);
    let mut view = VIEWS.with(|m| m.borrow().get(id).cloned().unwrap_or_default());
    let mut seek_to = None;
    let scale = ui.scale().max(1.0);
    let head_h = HEAD_H * scale;
    apply_zoom(ui, &mut view, duration);
    let px = px_per_sec(view.zoom, scale);

    ui.column_with(LayoutOpts { spacing: Some(0.0), ..Default::default() }, |ui| {
        let w = ui.available_size().x.max(1.0);
        let header = ui.area("smp_bars", Vec2::new(w, head_h));
        view.header = header.rect;
        let off_x = ui.scroll_offset("smp_wave").x;
        draw_time_header(ui, header.rect, off_x, view.zoom, duration, scale);
        let ptr = ui.pointer();
        if header.hovered {
            ui.set_mouse_cursor(CursorIcon::Pointer);
            if !ptr.ctrl {
                let wheel = ui.take_scroll();
                if wheel.y.abs() > 0.0 || wheel.x.abs() > 0.0 {
                    let mut off = ui.scroll_offset("smp_wave");
                    off.x = (off.x - wheel.y - wheel.x).max(0.0);
                    ui.set_scroll_target("smp_wave", off);
                    ui.request_repaint();
                }
            }
        }
        if (header.hovered && ptr.pressed) || (header.active && ptr.down) {
            seek_to = Some(time_at(header.rect, off_x, ptr.pos.x, view.zoom, duration, scale));
            ui.request_repaint();
        }

        ui.flex(1.0, |ui| {
            let size = ui.available_size();
            let size = Vec2::new(size.x.max(80.0), size.y.max(80.0));
            let content_w = content_width(duration, view.zoom, scale);
            ui.scroll_area("smp_wave", size, ScrollAxes::Horizontal, |ui| {
                let area = ui.area("wave", Vec2::new(content_w, size.y.max(1.0)));
                let rect = area.rect;
                ui.fill_rect(rect, [0.08, 0.08, 0.10, 1.0]);
                let left = if view.header.width() > 1.0 {
                    view.header.min.x
                } else {
                    rect.min.x
                };
                let right = if view.header.width() > 1.0 {
                    view.header.max.x
                } else {
                    rect.min.x + size.x
                };
                let vis = Rect {
                    min: Vec2::new(left.max(rect.min.x), rect.min.y),
                    max: Vec2::new(right.min(rect.max.x), rect.max.y),
                };
                if vis.width() >= 1.0 {
                    let t0 = ((vis.min.x - rect.min.x) / px.max(1e-6)).max(0.0);
                    let t1 = ((vis.max.x - rect.min.x) / px.max(1e-6)).min(duration);
                    draw_peaks(ui, vis, &sample.peaks, t0, t1, duration);
                    let x = rect.min.x + play_t.clamp(0.0, duration) * px;
                    if x >= vis.min.x && x <= vis.max.x {
                        ui.line(Vec2::new(x, vis.min.y), Vec2::new(x, vis.max.y), 1.5, HEAD);
                    }
                    view.wave = vis;
                } else {
                    view.wave = rect;
                }
                let ptr = ui.pointer();
                if (area.hovered && ptr.pressed) || (area.active && ptr.down) {
                    let t = ((ptr.pos.x - rect.min.x) / px.max(1e-6)).clamp(0.0, duration);
                    seek_to = Some(t as f64);
                    ui.request_repaint();
                }
            });
        });
    });

    VIEWS.with(|m| {
        m.borrow_mut().insert(id.to_string(), view);
    });
    seek_to
}

fn playhead_x(rect: Rect, play_t: f32, duration: f32) -> f32 {
    let t = if duration <= 1e-6 {
        0.0
    } else {
        (play_t / duration).clamp(0.0, 1.0)
    };
    rect.min.x + t * rect.width()
}

pub fn draw_preview(ui: &mut Ui, peaks: &[f32], duration: f32, play_t: f32) -> Option<f64> {
    let z = ui.scale().max(0.05);
    let w = ui.available_size().x.max(80.0 * z);
    let h = 56.0 * z;
    let area = ui.area("smp_prev", Vec2::new(w, h));
    ui.fill_rect(area.rect, [0.08, 0.08, 0.10, 1.0]);
    let mut seek = None;
    if duration > 0.0 && area.rect.width() >= 1.0 {
        draw_peaks(ui, area.rect, peaks, 0.0, duration, duration);
        let x = playhead_x(area.rect, play_t, duration);
        ui.line(
            Vec2::new(x, area.rect.min.y),
            Vec2::new(x, area.rect.max.y),
            1.5,
            HEAD,
        );
        let ptr = ui.pointer();
        if area.hovered {
            ui.set_mouse_cursor(CursorIcon::Pointer);
        }
        if (area.hovered && ptr.pressed) || (area.active && ptr.down) {
            seek = Some(seek_secs_at(area.rect, ptr.pos.x, duration));
            ui.request_repaint();
        }
    }
    seek
}

fn seek_secs_at(rect: Rect, x: f32, duration: f32) -> f64 {
    if duration <= 1e-6 || rect.width() < 1.0 {
        return 0.0;
    }
    let t = ((x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
    (t * duration) as f64
}

fn apply_zoom(ui: &mut Ui, view: &mut WaveView, _duration: f32) {
    let ptr = ui.pointer();
    if !ptr.ctrl {
        return;
    }
    let over_header = view.header.contains(ptr.pos);
    let over_wave = view.wave.contains(ptr.pos);
    if !over_header && !over_wave {
        return;
    }
    let wheel = ui.take_scroll();
    if wheel.y.abs() <= 0.0 {
        return;
    }
    let scale = ui.scale().max(1.0);
    let old = px_per_sec(view.zoom, scale);
    let next = clamp_zoom(view.zoom * if wheel.y > 0.0 { 1.1 } else { 0.9 });
    if (next - view.zoom).abs() <= 1e-6 {
        return;
    }
    let min_x = if over_header {
        view.header.min.x
    } else {
        view.wave.min.x
    };
    let local = (ptr.pos.x - min_x).max(0.0);
    let mut off = ui.scroll_offset("smp_wave");
    off.x = zoom_anchor_x(old, px_per_sec(next, scale), off.x, local);
    view.zoom = next;
    ui.set_scroll_target("smp_wave", off);
    ui.request_repaint();
}

fn time_at(rect: Rect, off_x: f32, x: f32, zoom: f32, duration: f32, scale: f32) -> f64 {
    let px = px_per_sec(zoom, scale);
    let t = (x - rect.min.x + off_x) / px.max(1e-6);
    t.clamp(0.0, duration) as f64
}

fn draw_time_header(ui: &mut Ui, rect: Rect, off_x: f32, zoom: f32, duration: f32, scale: f32) {
    ui.fill_rect(rect, [0.08, 0.08, 0.10, 1.0]);
    let px = px_per_sec(zoom, scale);
    let step = if zoom >= 2.0 {
        0.5
    } else if zoom >= 0.6 {
        1.0
    } else {
        5.0
    };
    let t0 = (off_x / px.max(1e-6)).floor() * step;
    let t1 = duration.min((off_x + (rect.max.x - rect.min.x)) / px.max(1e-6) + step);
    let mut t = t0.max(0.0);
    while t <= t1 + 0.001 {
        let x = rect.min.x + t * px - off_x;
        if x >= rect.min.x && x <= rect.max.x {
            ui.line(
                Vec2::new(x, rect.max.y - 6.0 * scale),
                Vec2::new(x, rect.max.y),
                1.0,
                [0.45, 0.45, 0.5, 1.0],
            );
            ui.text_at_size(
                Vec2::new(x + 3.0, rect.min.y + 2.0),
                &format!("{t:.0}s"),
                12.0 * scale,
            );
        }
        t += step;
    }
}

fn peak_range(peaks: &[f32], t0: f32, t1: f32, duration: f32) -> (f32, f32) {
    let n = peaks.len() / 2;
    if n == 0 || duration <= 0.0 {
        return (0.0, 0.0);
    }
    let ia = ((t0 / duration).clamp(0.0, 1.0) * n as f32) as usize;
    let ib = ((t1 / duration).clamp(0.0, 1.0) * n as f32).ceil() as usize;
    let ia = ia.min(n.saturating_sub(1));
    let ib = ib.min(n).max(ia + 1);
    let mut mn = 0.0f32;
    let mut mx = 0.0f32;
    for j in ia..ib {
        mn = mn.min(peaks[j * 2]);
        mx = mx.max(peaks[j * 2 + 1]);
    }
    (mn, mx)
}

pub fn draw_peaks(ui: &mut Ui, rect: Rect, peaks: &[f32], t0: f32, t1: f32, duration: f32) {
    let n = peaks.len() / 2;
    if n == 0 {
        return;
    }
    let w = (rect.max.x - rect.min.x).max(1.0);
    let cols = w.ceil() as i32;
    let mid = (rect.min.y + rect.max.y) * 0.5;
    let amp = (rect.max.y - rect.min.y) * 0.45;
    let span = (t1 - t0).max(1e-6);
    for c in 0..cols {
        let ta = t0 + c as f32 / cols as f32 * span;
        let tb = t0 + (c + 1) as f32 / cols as f32 * span;
        let (mn, mx) = peak_range(peaks, ta, tb, duration);
        let x = rect.min.x + c as f32;
        ui.line(
            Vec2::new(x, mid - mx * amp),
            Vec2::new(x, mid - mn * amp),
            1.0,
            WAVE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_cursor() {
        let off = zoom_anchor_x(10.0, 20.0, 40.0, 80.0);
        assert!((off - 160.0).abs() < 1e-3);
    }

    #[test]
    fn wave_width_tracks_zoom_not_viewport() {
        let dur = 50.0;
        let view = 1000.0;
        let z_out = ZOOM_MIN;
        let w = content_width(dur, z_out, 1.0);
        assert!(w < view, "zoomed-out wave must shrink below the viewport, got {w}");
        let z_in = ZOOM_MAX;
        let wide = content_width(dur, z_in, 1.0);
        let mid = content_width(dur, 1.0, 1.0);
        assert!(wide > mid);
        assert!((wide / mid - z_in).abs() < 1e-4);
        assert!((px_per_sec(2.0, 1.0) / px_per_sec(1.0, 1.0) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn peak_column_uses_time_window() {
        let mut peaks = vec![0.0; 8];
        peaks[2] = -0.5;
        peaks[3] = 0.8;
        let (mn, mx) = peak_range(&peaks, 0.25, 0.5, 1.0);
        assert!((mn + 0.5).abs() < 1e-5);
        assert!((mx - 0.8).abs() < 1e-5);
        let quiet = peak_range(&peaks, 0.0, 0.2, 1.0);
        assert_eq!(quiet, (0.0, 0.0));
    }

    #[test]
    fn preview_playhead_tracks_time_not_zero_width() {
        let r = Rect {
            min: Vec2::new(10.0, 0.0),
            max: Vec2::new(110.0, 40.0),
        };
        assert!((playhead_x(r, 0.0, 10.0) - 10.0).abs() < 1e-4);
        assert!((playhead_x(r, 5.0, 10.0) - 60.0).abs() < 1e-4);
        assert!((playhead_x(r, 50.0, 10.0) - 110.0).abs() < 1e-4);
        let skinny = Rect {
            min: Vec2::new(10.0, 0.0),
            max: Vec2::new(10.0, 40.0),
        };
        assert!((playhead_x(skinny, 5.0, 10.0) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn preview_click_maps_to_time() {
        let r = Rect {
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(100.0, 20.0),
        };
        assert!((seek_secs_at(r, 0.0, 10.0) - 0.0).abs() < 1e-4);
        assert!((seek_secs_at(r, 50.0, 10.0) - 5.0).abs() < 1e-4);
        assert!((seek_secs_at(r, 100.0, 10.0) - 10.0).abs() < 1e-4);
        assert!((seek_secs_at(r, 200.0, 10.0) - 10.0).abs() < 1e-4);
    }
}
