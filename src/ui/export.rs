use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

use glam::Vec2;
use mega_ui::{Ui, Window};

use crate::app::App;
use crate::compile::Patch;
use crate::export::{self, MAX_BARS};

pub fn draw(ui: &mut Ui, app: &mut App) {
    let finished = app.export_job.as_ref().and_then(|job| {
        job.done.lock().ok().and_then(|mut g| g.take()).map(|result| {
            let path = job.path.display().to_string();
            (result, path)
        })
    });
    if let Some((result, path)) = finished {
        app.export_job = None;
        app.export_open = false;
        app.export_video_open = false;
        app.status = match result {
            Ok(()) => format!("Exported {path}"),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    draw_dialog(ui, app, false);
    draw_dialog(ui, app, true);
}

fn draw_dialog(ui: &mut Ui, app: &mut App, video: bool) {
    let open_flag = if video {
        app.export_video_open
    } else {
        app.export_open
    };
    let busy = app.export_job.is_some() && open_flag;
    if !open_flag && !busy {
        return;
    }
    let mut open = true;
    let mut start = false;
    let mut cancel = false;
    let title = if video { "Export MP4" } else { "Export MP3" };
    let path_id = if video { "export_video_path" } else { "export_path" };
    ui.modal(
        {
            let w = Window::new(title).size(Vec2::new(380.0, 200.0));
            if busy {
                w
            } else {
                w.open(&mut open)
            }
        },
        |ui| {
            if busy {
                let t = app
                    .export_job
                    .as_ref()
                    .map(|j| j.progress.load(Ordering::Relaxed) as f32 / 1000.0)
                    .unwrap_or(0.0);
                ui.label("Rendering…");
                ui.progress_bar(t);
                ui.request_repaint();
                return;
            }
            ui.label("Save to");
            ui.horizontal(|ui| {
                let path = if video {
                    &mut app.export_video_path
                } else {
                    &mut app.export_path
                };
                ui.text_input(path_id, path);
                if ui.button("Browse").clicked {
                    pick_path(path, video);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Bars");
                ui.drag_int("export_bars", &mut app.export_bars, 1);
                app.export_bars = app.export_bars.clamp(1, MAX_BARS);
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked {
                    cancel = true;
                    ui.close_modal();
                }
                let empty = if video {
                    app.export_video_path.trim().is_empty()
                } else {
                    app.export_path.trim().is_empty()
                };
                ui.add_enabled(!empty, |ui| {
                    if ui.button("Export").clicked {
                        start = true;
                    }
                });
            });
        },
    );
    if cancel || !open {
        if !busy {
            if video {
                app.export_video_open = false;
            } else {
                app.export_open = false;
            }
        }
        return;
    }
    if start {
        begin_export(app, video);
    }
}

fn pick_path(path: &mut String, video: bool) {
    let name = if path.trim().is_empty() {
        if video {
            "export.mp4".into()
        } else {
            "export.mp3".into()
        }
    } else {
        path.clone()
    };
    let mut dlg = rfd::FileDialog::new().set_file_name(&name);
    dlg = if video {
        dlg.add_filter("MP4", &["mp4"])
    } else {
        dlg.add_filter("MP3", &["mp3"])
    };
    if let Some(p) = dlg.save_file() {
        *path = p.display().to_string();
    }
}

fn begin_export(app: &mut App, video: bool) {
    let path = if video {
        app.export_video_path.trim().to_string()
    } else {
        app.export_path.trim().to_string()
    };
    if path.is_empty() {
        return;
    }
    let bars = app.export_bars.clamp(1, MAX_BARS) as u32;
    let mut patch = Patch::from_main(&app.project, true);
    patch.playing = true;
    let sequences = app.project.sequences.clone();
    let title = app.project.title.clone();
    let credit = app.project.viz_credit();
    let progress = Arc::new(AtomicU32::new(0));
    let done = Arc::new(Mutex::new(None));
    let progress_t = progress.clone();
    let done_t = done.clone();
    let path_buf = std::path::PathBuf::from(&path);
    let path_job = path_buf.clone();
    thread::spawn(move || {
        let result = if video {
            export::write_mp4(&patch, &sequences, &title, &credit, bars, &path_buf, &progress_t)
        } else {
            export::write_mp3(&patch, bars, &path_buf, &progress_t)
        };
        if let Ok(mut g) = done_t.lock() {
            *g = Some(result);
        }
    });
    app.export_job = Some(crate::app::ExportJob {
        path: path_job,
        progress,
        done,
    });
}
