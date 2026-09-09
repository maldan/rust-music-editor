use std::cell::RefCell;
use std::collections::HashMap;

use glam::Vec2;
use mega_ui::{Rect, Ui};

const SEGS: usize = 14;
const HOLD_FRAMES: u8 = 28;
const BG: [f32; 4] = [0.07, 0.07, 0.08, 1.0];
const GREEN: [f32; 4] = [0.22, 0.82, 0.32, 1.0];
const YELLOW: [f32; 4] = [0.92, 0.82, 0.18, 1.0];
const RED: [f32; 4] = [0.92, 0.22, 0.18, 1.0];
const DIM: [f32; 4] = [0.14, 0.16, 0.14, 1.0];
const PEAK: [f32; 4] = [0.95, 0.95, 0.88, 0.9];

struct State {
    l: f32,
    r: f32,
    hl: f32,
    hr: f32,
    al: u8,
    ar: u8,
}

thread_local! {
    static HOLD: RefCell<HashMap<String, State>> = RefCell::new(HashMap::new());
}

/// Map linear amplitude to 0..1 meter (−48 dB … +3 dB).
pub fn level_t(amp: f32) -> f32 {
    let a = amp.abs();
    if a < 1e-5 {
        0.0
    } else {
        ((a.log10() * 20.0 + 48.0) / 51.0).clamp(0.0, 1.0)
    }
}

/// Stereo peak meter: two vertical LED columns (L / R).
pub fn stereo(ui: &mut Ui, id: &str, left: f32, right: f32) {
    let z = ui.scale().max(0.05);
    let w = ui.available_size().x.max(48.0 * z);
    let area = ui.area(id, Vec2::new(w, 56.0 * z));
    let (disp_l, disp_r, hold_l, hold_r) = envelope(id, left, right);
    draw_pair(ui, area.rect, disp_l, disp_r, hold_l, hold_r);
    ui.request_repaint();
}

fn envelope(id: &str, left: f32, right: f32) -> (f32, f32, f32, f32) {
    let tl = level_t(left);
    let tr = level_t(right);
    HOLD.with(|h| {
        let mut map = h.borrow_mut();
        let e = map.entry(id.to_string()).or_insert(State {
            l: 0.0,
            r: 0.0,
            hl: 0.0,
            hr: 0.0,
            al: 0,
            ar: 0,
        });
        step(&mut e.l, &mut e.hl, &mut e.al, tl);
        step(&mut e.r, &mut e.hr, &mut e.ar, tr);
        (e.l, e.r, e.hl, e.hr)
    })
}

fn step(disp: &mut f32, hold: &mut f32, age: &mut u8, now: f32) {
    *disp = now.max(*disp * 0.78);
    if now + 0.002 >= *hold {
        *hold = now;
        *age = 0;
    } else if *age < HOLD_FRAMES {
        *age += 1;
    } else {
        *hold = hold.max(now) * 0.92;
        if *hold < now {
            *hold = now;
        }
    }
}

fn draw_pair(ui: &mut Ui, rect: Rect, l: f32, r: f32, hl: f32, hr: f32) {
    ui.fill_rect(rect, BG);
    let gap = rect.width() * 0.12;
    let col_w = (rect.width() - gap * 3.0) * 0.5;
    let y0 = rect.min.y + 2.0;
    let y1 = rect.max.y - 2.0;
    let xl = rect.min.x + gap;
    let xr = xl + col_w + gap;
    draw_col(ui, xl, y0, col_w, y1 - y0, l, hl);
    draw_col(ui, xr, y0, col_w, y1 - y0, r, hr);
}

fn draw_col(ui: &mut Ui, x: f32, y: f32, w: f32, h: f32, t: f32, hold: f32) {
    let gap = (h / SEGS as f32) * 0.18;
    let seg_h = (h - gap * (SEGS as f32 - 1.0)) / SEGS as f32;
    let lit = (t * SEGS as f32).ceil() as usize;
    for i in 0..SEGS {
        let top = y + h - (i as f32 + 1.0) * (seg_h + gap) + gap;
        let r = Rect {
            min: Vec2::new(x, top),
            max: Vec2::new(x + w, top + seg_h),
        };
        ui.fill_rect(r, if i < lit { seg_color(i) } else { DIM });
    }
    if hold > 0.02 {
        let hy = y + h * (1.0 - hold.clamp(0.0, 1.0));
        ui.fill_rect(
            Rect {
                min: Vec2::new(x, hy - 1.0),
                max: Vec2::new(x + w, hy + 1.0),
            },
            PEAK,
        );
    }
}

fn seg_color(i: usize) -> [f32; 4] {
    let t = i as f32 / (SEGS.saturating_sub(1) as f32).max(1.0);
    if t < 0.62 {
        GREEN
    } else if t < 0.82 {
        YELLOW
    } else {
        RED
    }
}

#[cfg(test)]
mod tests {
    use super::level_t;

    #[test]
    fn silence_is_empty() {
        assert_eq!(level_t(0.0), 0.0);
    }

    #[test]
    fn unity_is_near_top() {
        let t = level_t(1.0);
        assert!(t > 0.9 && t < 1.0, "{t}");
    }

    #[test]
    fn clip_is_full() {
        assert!((level_t(1.5) - 1.0).abs() < 1e-5);
    }
}
