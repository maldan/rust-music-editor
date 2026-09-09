use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

use mega_audio::events::{event_channel, EventSender};
use mega_audio::note::NoteEvent;
use mega_audio::{AudioEngine, CaptureTap, GraphSetup, InputCapture};
use mega_ui::DockState;

use crate::compile::{Live, Patch};
use crate::graph::{with_graph_ext, FILE_EXT, NodeKind, Project};
use crate::monitor::Monitor;
use crate::ui::default_dock;
use crate::ui::DeviceLists;

pub struct ExportJob {
    pub path: PathBuf,
    pub progress: Arc<AtomicU32>,
    pub done: Arc<Mutex<Option<Result<(), String>>>>,
}

struct CaptureSlot {
    tap: Arc<CaptureTap>,
    _stream: Option<InputCapture>,
}

struct CaptureBank {
    slots: HashMap<String, CaptureSlot>,
    prev: HashSet<String>,
}

impl CaptureBank {
    fn new() -> Self {
        Self {
            slots: HashMap::new(),
            prev: HashSet::new(),
        }
    }

    fn bind(
        &mut self,
        nodes: &[crate::graph::GraphNode],
        status: &mut String,
    ) -> HashMap<String, Arc<CaptureTap>> {
        let mut wanted = HashSet::new();
        let mut out = HashMap::new();
        for n in nodes {
            if n.kind != NodeKind::AudioIn {
                continue;
            }
            let key = n.audio_device.clone();
            wanted.insert(key.clone());
            if !self.slots.contains_key(&key) {
                let tap = CaptureTap::new();
                let name = if key.is_empty() { None } else { Some(key.as_str()) };
                let stream = match InputCapture::start(name, tap.clone()) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        *status = format!("Audio in: {e}");
                        None
                    }
                };
                self.slots.insert(
                    key.clone(),
                    CaptureSlot {
                        tap: tap.clone(),
                        _stream: stream,
                    },
                );
            }
            if let Some(slot) = self.slots.get(&key) {
                out.insert(n.id.clone(), slot.tap.clone());
            }
        }
        self.slots
            .retain(|k, _| wanted.contains(k) || self.prev.contains(k));
        self.prev = wanted;
        out
    }
}

pub struct App {
    pub project: Project,
    pub dock: DockState,
    pub playing: bool,
    pub monitor: Arc<Monitor>,
    pub status: String,
    pub preview_tx: EventSender<NoteEvent>,
    pub export_open: bool,
    pub export_path: String,
    pub export_bars: i32,
    pub export_job: Option<ExportJob>,
    pub devices: DeviceLists,
    current_path: Option<PathBuf>,
    last_fp: u64,
    last_playing: bool,
    last_seek_gen: u64,
    engine_out: String,
    captures: CaptureBank,
    tx: EventSender<Patch>,
    _engine: AudioEngine,
}

fn output_device(project: &Project) -> String {
    project
        .main
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Output)
        .map(|n| n.audio_device.clone())
        .unwrap_or_default()
}

fn boot(
    project: &Project,
    playing: bool,
    monitor: Arc<Monitor>,
    output: Option<&str>,
) -> Result<(AudioEngine, EventSender<Patch>, EventSender<NoteEvent>), Box<dyn std::error::Error>> {
    let (tx, mut rx) = event_channel::<Patch>(16);
    let (preview_tx, mut preview_rx) = event_channel::<NoteEvent>(64);
    let first = Patch::from_project(project, playing);
    let mon_audio = monitor;
    let (engine, _notes) = AudioEngine::start_on(output, move |sample_rate| {
        let (mut live, g) = Live::new(&first, sample_rate, mon_audio);
        GraphSetup::new(g).with_on_sample(move |graph, n| {
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
            live.tick_block(graph, n);
        })
    })?;
    Ok((engine, tx, preview_tx))
}

impl App {
    pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let project = Project::new_default();
        let monitor = Arc::new(Monitor::default());
        let (engine, tx, preview_tx) = boot(&project, false, monitor.clone(), None)?;

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
            export_open: false,
            export_path: String::new(),
            export_bars: 8,
            export_job: None,
            devices: DeviceLists::fetch(),
            current_path: None,
            engine_out: String::new(),
            captures: CaptureBank::new(),
            tx,
            _engine: engine,
        })
    }

    fn restart_engine(&mut self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let out = if name.is_empty() { None } else { Some(name) };
        let (engine, tx, preview_tx) =
            boot(&self.project, self.playing, self.monitor.clone(), out)?;
        self._engine = engine;
        self.tx = tx;
        self.preview_tx = preview_tx;
        self.engine_out = name.to_string();
        self.last_fp = 0;
        Ok(())
    }

    pub fn sync_audio(&mut self) {
        let want_out = output_device(&self.project);
        if want_out != self.engine_out {
            if let Err(e) = self.restart_engine(&want_out) {
                self.status = format!("Output device: {e}");
                self.engine_out = want_out;
            }
        }
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
        let mut patch = Patch::from_project(&self.project, self.playing);
        patch.captures = self.captures.bind(&patch.nodes, &mut self.status);
        let _ = self.tx.send(patch);
    }

    pub fn new_project(&mut self) {
        self.replace_project(Project::new_default(), None, "New project".into());
        self.project.main.space.fit_view = true;
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
                let status = format!("Opened {}", path.display());
                self.replace_project(project, Some(path), status);
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    fn replace_project(&mut self, project: Project, path: Option<PathBuf>, status: String) {
        self.project = project;
        self.playing = false;
        self.current_path = path;
        self.last_fp = 0;
        self.last_playing = false;
        self.last_seek_gen = 0;
        self.export_open = false;
        self.export_job = None;
        self.captures = CaptureBank::new();
        self.status = status;
    }

    pub fn import_midi_dialog(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("MIDI", &["mid", "midi"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        match std::fs::read(&path) {
            Ok(bytes) => match self.project.import_midi(&bytes) {
                Ok(n) => {
                    self.status = format!(
                        "Imported {n} sequence{} from {}",
                        if n == 1 { "" } else { "s" },
                        path.display()
                    );
                }
                Err(e) => self.status = format!("MIDI import failed: {e}"),
            },
            Err(e) => self.status = format!("MIDI import failed: {e}"),
        }
    }

    pub fn import_sample_dialog(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("Audio", &["wav", "mp3", "ogg", "flac", "aiff", "aif"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        match self.project.import_sample(&path) {
            Ok(_) => {
                self.status = format!("Imported sample {}", path.display());
            }
            Err(e) => self.status = format!("Sample import failed: {e}"),
        }
    }
}
