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
        app.status = match result {
            Ok(()) => format!("Exported {path}"),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    if !app.export_open && app.export_job.is_none() {
        return;
    }

    let busy = app.export_job.is_some();
    let mut open = true;
    let mut start = false;
    let mut cancel = false;
    ui.modal(
        {
            let w = Window::new("Export MP3").size(Vec2::new(380.0, 200.0));
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
                ui.text_input("export_path", &mut app.export_path);
                if ui.button("Browse").clicked {
                    pick_path(app);
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
                ui.add_enabled(!app.export_path.trim().is_empty(), |ui| {
                    if ui.button("Export").clicked {
                        start = true;
                    }
                });
            });
        },
    );
    if cancel || !open {
        app.export_open = false;
        return;
    }
    if start {
        begin_export(app);
    }
}

fn pick_path(app: &mut App) {
    let name = if app.export_path.trim().is_empty() {
        "export.mp3".into()
    } else {
        app.export_path.clone()
    };
    let path = rfd::FileDialog::new()
        .add_filter("MP3", &["mp3"])
        .set_file_name(&name)
        .save_file();
    if let Some(path) = path {
        app.export_path = path.display().to_string();
    }
}

fn begin_export(app: &mut App) {
    let path = app.export_path.trim().to_string();
    if path.is_empty() {
        return;
    }
    let bars = app.export_bars.clamp(1, MAX_BARS) as u32;
    let mut patch = Patch::from_main(&app.project, true);
    patch.playing = true;
    let progress = Arc::new(AtomicU32::new(0));
    let done = Arc::new(Mutex::new(None));
    let progress_t = progress.clone();
    let done_t = done.clone();
    let path_buf = std::path::PathBuf::from(&path);
    let path_job = path_buf.clone();
    thread::spawn(move || {
        let result = export::write_mp3(&patch, bars, &path_buf, &progress_t);
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
