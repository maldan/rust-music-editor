use glam::Vec2;
use mega_ui::{LayoutOpts, Ui};

use crate::graph::{GraphNode, BEATS_PER_STEP, SEQ_BASE_PITCH, SEQ_PITCHES, SEQ_STEPS};

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

const LABEL_W: f32 = 28.0;
const CELL_W: f32 = 12.0;
const CELL_H: f32 = 11.0;
const HEAD_W: f32 = 2.0;

fn pitch_name(pitch: u8) -> String {
    format!("{}{}", NAMES[(pitch % 12) as usize], pitch / 12 - 1)
}

pub fn draw_in_node(ui: &mut Ui, node_id: &str, node: &mut GraphNode, playhead: Option<f32>) {
    let s = ui.scale();
    let cell = Vec2::new(CELL_W * s, CELL_H * s);
    let loop_len = SEQ_STEPS as f32 * BEATS_PER_STEP;
    let play_step = playhead.map(|p| {
        let t = (p / loop_len).clamp(0.0, 0.999);
        (t * SEQ_STEPS as f32) as u32
    });

    ui.row_with(
        LayoutOpts {
            spacing: Some(1.0),
            ..Default::default()
        },
        |ui| {
            ui.label_fixed(LABEL_W, "");
            for step in 0..SEQ_STEPS {
                let on = play_step == Some(step);
                let color = if on {
                    [1.0, 0.38, 0.16, 1.0]
                } else if step % 4 == 0 {
                    [0.22, 0.22, 0.26, 1.0]
                } else {
                    [0.14, 0.14, 0.16, 1.0]
                };
                ui.surface(Vec2::new(CELL_W * s, 3.0 * s), color);
            }
        },
    );

    for row in 0..SEQ_PITCHES {
        let pitch = SEQ_BASE_PITCH + (SEQ_PITCHES - 1 - row) as u8;
        let sharp = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
        ui.row_with(
            LayoutOpts {
                spacing: Some(1.0),
                ..Default::default()
            },
            |ui| {
                ui.label_fixed(LABEL_W, &pitch_name(pitch));
                for step in 0..SEQ_STEPS {
                    let on = node
                        .notes
                        .iter()
                        .any(|n| n.step == step as u8 && n.pitch == pitch);
                    let mut color = if on {
                        [0.95, 0.72, 0.22, 1.0]
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
                    if play_step == Some(step) && !on {
                        color = [0.42, 0.22, 0.14, 1.0];
                    } else if play_step == Some(step) && on {
                        color = [1.0, 0.82, 0.35, 1.0];
                    }
                    let id = format!("{node_id}_{step}_{pitch}");
                    if play_step == Some(step) {
                        ui.surface(
                            Vec2::new(HEAD_W * s, CELL_H * s),
                            [1.0, 0.42, 0.18, 1.0],
                        );
                        let rest = Vec2::new((CELL_W - HEAD_W).max(6.0) * s, CELL_H * s);
                        if ui.click_rect(&id, rest, color).clicked {
                            node.toggle_note(step as u8, pitch);
                        }
                    } else if ui.click_rect(&id, cell, color).clicked {
                        node.toggle_note(step as u8, pitch);
                    }
                }
            },
        );
    }
}
