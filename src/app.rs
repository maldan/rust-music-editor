use std::path::PathBuf;
use std::sync::Arc;

use mega_audio::events::{event_channel, EventSender};
use mega_audio::note::NoteEvent;
use mega_audio::{AudioEngine, GraphSetup};
use mega_ui::DockState;

use crate::compile::{Live, Patch};
use crate::graph::{with_graph_ext, FILE_EXT, Project};
use crate::monitor::Monitor;
use crate::ui::default_dock;

pub struct App {
    pub project: Project,
    pub dock: DockState,
    pub playing: bool,
    pub monitor: Arc<Monitor>,
    pub status: String,
    pub preview_tx: EventSender<NoteEvent>,
    current_path: Option<PathBuf>,
    last_fp: u64,
    last_playing: bool,
    last_seek_gen: u64,
    tx: EventSender<Patch>,
    _engine: AudioEngine,
}

impl App {
    pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let project = Project::new_default();
        let (tx, mut rx) = event_channel::<Patch>(16);
        let (preview_tx, mut preview_rx) = event_channel::<NoteEvent>(64);
        let first = Patch::from_project(&project, false);
        let monitor = Arc::new(Monitor::default());
        let mon_audio = monitor.clone();

        let (engine, _notes) = AudioEngine::start(move |sample_rate| {
            let (mut live, g) = Live::new(&first, sample_rate, mon_audio);
            GraphSetup::new(g).with_on_sample(move |graph| {
                let mut last = None;
                while let Some(p) = rx.try_recv() {
                    last = Some(p);
                }
                if let Some(p) = last {
                    live.apply(graph, p);
                }
                while let Some(ev) = preview_rx.try_recv() {
                    live.preview_event(graph, ev);
                }
                live.tick(graph);
            })
        })?;

        Ok(Self {
            last_fp: project.fingerprint(),
            last_playing: false,
            last_seek_gen: project.main.seek_gen,
            project,
            dock: default_dock(),
            playing: false,
            monitor,
            status: String::new(),
            preview_tx,
            current_path: None,
            tx,
            _engine: engine,
        })
    }

    pub fn sync_audio(&mut self) {
        let fp = self.project.fingerprint();
        if fp == self.last_fp
            && self.playing == self.last_playing
            && self.project.main.seek_gen == self.last_seek_gen
        {
            return;
        }
        self.last_fp = fp;
        self.last_playing = self.playing;
        self.last_seek_gen = self.project.main.seek_gen;
        let _ = self.tx.send(Patch::from_project(&self.project, self.playing));
    }

    pub fn save(&mut self) {
        if let Some(path) = self.current_path.clone() {
            self.write_to(path);
        } else {
            self.save_dialog();
        }
    }

    pub fn save_dialog(&mut self) {
        let fallback = format!("graph.{FILE_EXT}");
        let name = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(&fallback);
        let path = rfd::FileDialog::new()
            .add_filter("Music graph", &[FILE_EXT, "json"])
            .set_file_name(name)
            .save_file();
        let Some(path) = path else {
            return;
        };
        self.write_to(with_graph_ext(path));
    }

    fn write_to(&mut self, path: PathBuf) {
        match self.project.save_to_path(&path) {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                self.current_path = Some(path);
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    pub fn open_dialog(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("Music graph", &[FILE_EXT, "json"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        match Project::load_from_path(&path) {
            Ok(project) => {
                self.project = project;
                self.playing = false;
                self.current_path = Some(path.clone());
                self.status = format!("Opened {}", path.display());
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }
}
