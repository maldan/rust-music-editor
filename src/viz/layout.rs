use std::collections::BTreeSet;

use super::frame::{
    in_when, Note, Wave, BEHIND_BEATS, FADE_IN_X, FADE_PAST, HIT_W, HIT_X, NOTE_TOP, PRESS_IN,
    PRESS_OUT, WAVE_BINS,
};

#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 4],
    /// 1 = rounded note, 0 = sharp (hit line / wave).
    pub round: f32,
    /// 1 = SDF glow (notes / playhead).
    pub glow: f32,
}

pub fn gonio_points(lr: &[[f32; 2]], max: usize) -> Vec<[f32; 2]> {
    if lr.is_empty() || max == 0 {
        return Vec::new();
    }
    let n = lr.len().min(max);
    lr[lr.len() - n..].to_vec()
}

/// Independent X/Y so a thin pulse still fills the screen.
pub fn gonio_fit_scale(lr: &[[f32; 2]]) -> [f32; 2] {
    let mut mid_p = 0.0f32;
    let mut side_p = 0.0f32;
    for &[l, r] in lr {
        mid_p = mid_p.max((l + r).abs());
        side_p = side_p.max((r - l).abs());
    }
    let sx = if side_p < 1e-5 { 0.0 } else { 0.80 / side_p };
    let sy = if mid_p < 1e-5 { 0.0 } else { 0.80 / mid_p };
    [sx, sy]
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    if e1 <= e0 {
        return if x >= e1 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn press_amount(now: f64, t0: f64, t1: f64) -> f32 {
    if now < t0 {
        0.0
    } else if now < t1 {
        smoothstep(t0, t0 + PRESS_IN, now) as f32
    } else {
        (1.0 - smoothstep(t1, t1 + PRESS_OUT, now)) as f32
    }
}

fn lane_avg_pitch(notes: &[Note], lane: u32) -> u8 {
    let mut sum = 0u32;
    let mut n = 0u32;
    for note in notes.iter().filter(|n| n.lane == lane) {
        sum += note.pitch as u32;
        n += 1;
    }
    if n == 0 {
        60
    } else {
        (sum / n) as u8
    }
}

fn lane_band(i: usize, n: usize, notes_h: f32) -> (f32, f32) {
    let n = n.max(1);
    let gap = if n > 1 { 0.012 } else { 0.0 };
    let h = ((notes_h - gap * (n - 1) as f32) / n as f32).max(0.04);
    (i as f32 * (h + gap), h)
}

pub fn note_quads(notes: &[Note], now: f64, window: f64, notes_h: f32, top: f32) -> Vec<Quad> {
    let window = window.max(0.25);
    let notes_h = notes_h.clamp(0.2, 1.0);
    let top = top.clamp(0.0, (notes_h - 0.05).max(0.0));
    let body = (notes_h - top).max(0.05);
    let mut lanes: Vec<u32> = notes
        .iter()
        .map(|n| n.lane)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    lanes.sort_by(|&a, &b| {
        lane_avg_pitch(notes, b)
            .cmp(&lane_avg_pitch(notes, a))
            .then(a.cmp(&b))
    });
    let lane_n = lanes.len().max(1);
    let mut out = Vec::new();
    let span_l = now - BEHIND_BEATS.max(window);
    let span_r = now + window;
    let usable = (1.0 - HIT_X).max(0.05);

    for (li, lane) in lanes.iter().enumerate() {
        let lane_notes: Vec<&Note> = notes.iter().filter(|n| n.lane == *lane).collect();
        let (lo, hi) = pitch_range_refs(&lane_notes);
        let span = (hi - lo).max(1) as f32;
        let (band_y, band_h) = lane_band(li, lane_n, body);
        let band_y = band_y + top;
        let row = band_h / span;
        let h = (row * 0.72).max(0.008);
        for n in &lane_notes {
            push_note_quads(
                n, now, window, hi, span, row, h, band_y, band_h, span_l, span_r, usable, &mut out,
            );
        }
    }

    out.push(Quad {
        x: HIT_X - HIT_W * 0.5,
        y: top,
        w: HIT_W,
        h: body,
        color: [1.0, 0.94, 0.82, 0.42],
        round: 0.0,
        glow: 1.0,
    });
    out
}

fn pitch_range_refs(notes: &[&Note]) -> (i32, i32) {
    let mut lo = 127i32;
    let mut hi = 0i32;
    for n in notes {
        lo = lo.min(n.pitch as i32);
        hi = hi.max(n.pitch as i32);
    }
    if lo > hi {
        return (48, 72);
    }
    lo = (lo - 2).max(0);
    hi = (hi + 2).min(127);
    if hi - lo < 12 {
        hi = (lo + 12).min(127);
        lo = (hi - 12).max(0);
    }
    (lo, hi)
}

fn push_note_quads(
    n: &Note,
    now: f64,
    window: f64,
    hi: i32,
    span: f32,
    row: f32,
    h: f32,
    band_y: f32,
    band_h: f32,
    span_l: f64,
    span_r: f64,
    usable: f32,
    out: &mut Vec<Quad>,
) {
    if n.dur <= 1e-9 || n.loop_len <= 1e-9 {
        return;
    }
    let start = n.start.rem_euclid(n.loop_len);
    let k0 = ((span_l - start) / n.loop_len).floor() as i64 - 1;
    let k1 = ((span_r - start) / n.loop_len).ceil() as i64 + 1;
    let y0 = band_y + ((hi - n.pitch as i32) as f32 / span) * band_h + (row - h) * 0.5;
    for k in k0..=k1 {
        let t0 = start + k as f64 * n.loop_len;
        let t1 = t0 + n.dur;
        if t1 + FADE_PAST < span_l || t0 > span_r {
            continue;
        }
        if !in_when(t0, &n.when) {
            continue;
        }
        let x0 = HIT_X + ((t0 - now) / window) as f32 * usable;
        let x1 = HIT_X + ((t1 - now) / window) as f32 * usable;
        if x1 < 0.0 || x0 > 1.0 {
            continue;
        }
        let hit = now >= t0 && now < t1;
        let press = press_amount(now, t0, t1);
        let mut color = n.color;
        let mut a = if hit {
            color[0] = (color[0] + 0.35 * press).min(1.0);
            color[1] = (color[1] + 0.35 * press).min(1.0);
            color[2] = (color[2] + 0.35 * press).min(1.0);
            1.0
        } else {
            0.82 + 0.18 * press
        };
        if now >= t1 {
            a *= 1.0 - smoothstep(t1, t1 + FADE_PAST, now) as f32;
        }
        a *= ((1.0 - x0) / FADE_IN_X).clamp(0.0, 1.0);
        if a < 0.02 {
            continue;
        }
        color[3] = a;
        let y_max = (band_y + band_h - h).max(band_y);
        let y = (y0 + press * h * 0.42).clamp(band_y, y_max);
        out.push(Quad {
            x: x0,
            y,
            w: (x1 - x0).max(0.001),
            h,
            color,
            round: 1.0,
            glow: 1.0,
        });
    }
}

pub fn wave_quads(waves: &[Wave], notes_h: f32) -> Vec<Quad> {
    if waves.is_empty() {
        return Vec::new();
    }
    let top = notes_h.clamp(0.2, 0.95) + 0.014;
    let bot = 0.988;
    let band_h = (bot - top).max(0.04);
    let n = waves.len().max(1);
    let gap = 0.01;
    let box_w = ((0.976 - gap * (n.saturating_sub(1) as f32)) / n as f32).max(0.04);
    let box_h = (box_w * 0.56).min(band_h * 0.92).max(0.03);
    let total_w = n as f32 * box_w + (n - 1) as f32 * gap;
    let x0 = ((1.0 - total_w) * 0.5).max(0.012);
    let mut out = Vec::new();
    for (i, w) in waves.iter().enumerate() {
        let x = x0 + i as f32 * (box_w + gap);
        let y = top + (band_h - box_h) * 0.5;
        out.push(Quad {
            x,
            y,
            w: box_w,
            h: box_h,
            color: [0.16, 0.16, 0.20, 0.72],
            round: 1.0,
            glow: 0.0,
        });
        let samples = downsample(&w.samples, WAVE_BINS);
        if samples.len() < 2 {
            continue;
        }
        let pad_x = box_w * 0.08;
        let pad_y = box_h * 0.14;
        let inner_x = x + pad_x;
        let inner_y = y + pad_y;
        let inner_w = (box_w - pad_x * 2.0).max(0.01);
        let inner_h = (box_h - pad_y * 2.0).max(0.01);
        let mid = inner_y + inner_h * 0.5;
        let half = inner_h * 0.42;
        let th = (box_h * 0.045).max(0.0014);
        let mut c = w.color;
        c[3] = 0.92;
        let n_s = (samples.len() - 1) as f32;
        for k in 0..samples.len() - 1 {
            let t0 = k as f32 / n_s;
            let t1 = (k + 1) as f32 / n_s;
            let ax = inner_x + inner_w * t0;
            let bx = inner_x + inner_w * t1;
            let ay = mid - samples[k].clamp(-1.0, 1.0) * half;
            let by = mid - samples[k + 1].clamp(-1.0, 1.0) * half;
            out.push(segment_quad(ax, ay, bx, by, th, c));
        }
    }
    out
}

fn segment_quad(x0: f32, y0: f32, x1: f32, y1: f32, th: f32, color: [f32; 4]) -> Quad {
    let x = x0.min(x1);
    let y = y0.min(y1);
    Quad {
        x,
        y,
        w: (x0 - x1).abs().max(th),
        h: (y0 - y1).abs().max(th),
        color,
        round: 0.0,
        glow: 0.0,
    }
}

pub fn downsample(samples: &[f32], max: usize) -> Vec<f32> {
    if samples.is_empty() || max == 0 {
        return Vec::new();
    }
    if samples.len() <= max {
        return samples.to_vec();
    }
    let last = (samples.len() - 1) as f32;
    let den = (max - 1).max(1) as f32;
    (0..max)
        .map(|i| {
            let t = i as f32 * last / den;
            let a = t.floor() as usize;
            let b = (a + 1).min(samples.len() - 1);
            let f = t - a as f32;
            samples[a] * (1.0 - f) + samples[b] * f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viz::Note;

    fn note(pitch: u8, start: f64, dur: f64) -> Note {
        Note {
            pitch,
            start,
            dur,
            loop_len: 16.0,
            color: [0.2, 0.8, 0.3, 1.0],
            when: Vec::new(),
            lane: 0,
            wave: String::new(),
        }
    }

    #[test]
    fn gonio_fit_scale_fills_width_even_if_thin() {
        let thin = vec![[0.4, 0.42], [0.3, 0.28], [-0.2, -0.18]];
        let s = gonio_fit_scale(&thin);
        let mut side_p = 0.0f32;
        let mut mid_p = 0.0f32;
        for &[l, r] in &thin {
            side_p = side_p.max((r - l).abs());
            mid_p = mid_p.max((l + r).abs());
        }
        assert!((side_p * s[0] - 0.80).abs() < 1e-4, "width scale {}", s[0]);
        assert!((mid_p * s[1] - 0.80).abs() < 1e-4, "height scale {}", s[1]);
        assert!(s[0] > s[1], "thin figure needs more X stretch");
        assert_eq!(gonio_fit_scale(&[[0.0, 0.0]]), [0.0, 0.0]);
    }

    #[test]
    fn future_note_sits_to_the_right_of_hit() {
        let notes = [note(60, 4.0, 1.0)];
        let qs = note_quads(&notes, 0.0, 8.0, 1.0, NOTE_TOP);
        let n = qs.iter().find(|q| q.round > 0.5).expect("note quad");
        assert!(n.x > HIT_X + 0.2, "future should be right of hit, x={}", n.x);
    }

    #[test]
    fn now_note_crosses_hit_line() {
        let notes = [note(60, 0.0, 1.0)];
        let qs = note_quads(&notes, 0.2, 8.0, 1.0, NOTE_TOP);
        let n = qs.iter().find(|q| q.round > 0.5).expect("note quad");
        assert!(n.x <= HIT_X + 0.02);
        assert!(n.x + n.w > HIT_X);
    }

    #[test]
    fn past_note_keeps_full_width() {
        let notes = [note(60, 0.0, 1.0)];
        let window = 8.0;
        let qs = note_quads(&notes, 2.0, window, 1.0, NOTE_TOP);
        let n = qs.iter().find(|q| q.round > 0.5).expect("note quad");
        let usable = 1.0 - HIT_X;
        let want = (1.0 / window) as f32 * usable;
        assert!((n.w - want).abs() < 0.002, "w={} want={}", n.w, want);
        assert!(n.x < HIT_X, "start should be left of hit, x={}", n.x);
    }

    #[test]
    fn press_moves_note_down() {
        let notes = [note(60, 0.0, 2.0)];
        let rest = note_quads(&notes, -0.5, 8.0, 1.0, NOTE_TOP);
        let down = note_quads(&notes, 0.5, 8.0, 1.0, NOTE_TOP);
        let y0 = rest.iter().find(|q| q.round > 0.5).unwrap().y;
        let y1 = down.iter().find(|q| q.round > 0.5).unwrap().y;
        assert!(y1 > y0 + 0.001, "pressed y={y1} rest y={y0}");
    }

    #[test]
    fn high_pitch_is_above_low_pitch() {
        let mut lo = note(48, 0.0, 1.0);
        lo.color = [1.0, 0.0, 0.0, 1.0];
        let mut hi = note(72, 0.0, 1.0);
        hi.color = [0.0, 1.0, 0.0, 1.0];
        let qs = note_quads(&[lo, hi], 0.0, 8.0, 1.0, NOTE_TOP);
        let loq = qs.iter().find(|q| q.color[0] > 0.5).unwrap();
        let hiq = qs.iter().find(|q| q.color[1] > 0.5).unwrap();
        assert!(hiq.y < loq.y, "high y={} low y={}", hiq.y, loq.y);
    }

    #[test]
    fn lanes_do_not_overlap() {
        let mut a = note(60, 0.0, 1.0);
        a.lane = 0;
        a.color = [1.0, 0.0, 0.0, 1.0];
        let mut b = note(60, 0.0, 1.0);
        b.lane = 1;
        b.color = [0.0, 1.0, 0.0, 1.0];
        let qs = note_quads(&[a, b], 0.0, 8.0, 1.0, NOTE_TOP);
        let qa = qs.iter().find(|q| q.color[0] > 0.5).unwrap();
        let qb = qs.iter().find(|q| q.color[1] > 0.5).unwrap();
        assert!(qa.y + qa.h <= qb.y + 0.002 || qb.y + qb.h <= qa.y + 0.002);
    }

    #[test]
    fn high_octave_lane_sits_above_low_octave_lane() {
        let mut low = note(48, 0.0, 1.0);
        low.lane = 0;
        low.color = [1.0, 0.0, 0.0, 1.0];
        let mut high = note(84, 0.0, 1.0);
        high.lane = 1;
        high.color = [0.0, 1.0, 0.0, 1.0];
        let qs = note_quads(&[low, high], 0.0, 8.0, 1.0, NOTE_TOP);
        let loq = qs.iter().find(|q| q.color[0] > 0.5).unwrap();
        let hiq = qs.iter().find(|q| q.color[1] > 0.5).unwrap();
        assert!(hiq.y + hiq.h <= loq.y + 0.002, "high lane y={} low lane y={}", hiq.y, loq.y);
    }

    #[test]
    fn played_note_fades_out() {
        let notes = [note(60, 0.0, 1.0)];
        let live = note_quads(&notes, 0.5, 8.0, 1.0, NOTE_TOP);
        let gone = note_quads(&notes, 2.2, 8.0, 1.0, NOTE_TOP);
        let a0 = live
            .iter()
            .filter(|q| q.round > 0.5)
            .map(|q| q.color[3])
            .fold(0.0f32, f32::max);
        let a1 = gone
            .iter()
            .filter(|q| q.round > 0.5)
            .map(|q| q.color[3])
            .fold(0.0f32, f32::max);
        assert!(a0 > 0.5, "live a={a0}");
        assert!(a1 < a0 * 0.6, "live a={a0} faded a={a1}");
    }

    #[test]
    fn incoming_note_fades_in_from_right() {
        let notes = [note(60, 7.9, 0.4)];
        let edge = note_quads(&notes, 0.0, 8.0, 1.0, NOTE_TOP);
        let n = edge.iter().find(|q| q.round > 0.5).expect("incoming note");
        assert!(n.x > 0.9, "incoming should sit near the right edge, x={}", n.x);
        assert!(n.color[3] < 0.75, "incoming should be faded, a={}", n.color[3]);
        let notes2 = [note(60, 4.0, 0.4)];
        let solid = note_quads(&notes2, 0.0, 8.0, 1.0, NOTE_TOP);
        let n2 = solid.iter().find(|q| q.round > 0.5).unwrap();
        assert!(n2.color[3] > n.color[3], "mid a={} edge a={}", n2.color[3], n.color[3]);
    }

    #[test]
    fn notes_leave_top_margin() {
        let notes = [note(72, 0.0, 1.0)];
        let qs = note_quads(&notes, 0.0, 8.0, 1.0, NOTE_TOP);
        let n = qs.iter().find(|q| q.round > 0.5).expect("note");
        assert!(n.y >= NOTE_TOP - 0.001, "top note y={} pad={}", n.y, NOTE_TOP);
        let hit = qs.iter().find(|q| q.round < 0.5).expect("hit");
        assert!(hit.y >= NOTE_TOP - 0.001);
        let top = crate::viz::text::overlay_reserve("Title", "Author: Ada");
        let qs = note_quads(&notes, 0.0, 8.0, 1.0, top);
        let n = qs.iter().find(|q| q.round > 0.5).expect("note");
        assert!(n.y >= top - 0.001, "overlay note y={} pad={}", n.y, top);
        assert!(top > NOTE_TOP);
    }

    #[test]
    fn waves_sit_below_notes_in_a_row() {
        let waves = [
            Wave {
                color: [0.2, 0.6, 0.9, 1.0],
                samples: vec![0.0, 0.5, -0.5, 0.2],
            },
            Wave {
                color: [0.9, 0.4, 0.2, 1.0],
                samples: vec![0.1, -0.2, 0.3, 0.0],
            },
        ];
        let qs = wave_quads(&waves, 0.76);
        let frames: Vec<&Quad> = qs.iter().filter(|q| q.round > 0.5 && q.glow < 0.5).collect();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|q| q.y >= 0.75));
        assert!(frames[1].x > frames[0].x + frames[0].w * 0.4, "side by side");
        assert!((frames[0].w - frames[1].w).abs() < 0.002, "equal width");
        assert!((frames[0].h - frames[1].h).abs() < 0.002, "equal height");
        assert!(frames[0].h < frames[0].w * 0.7, "shorter than wide");
    }
}
