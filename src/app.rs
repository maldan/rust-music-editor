use std::sync::Arc;

use mega_audio::events::{event_channel, EventSender};
use mega_audio::{AudioEngine, GraphSetup};
use mega_ui::DockState;

use crate::compile::{Live, Patch};
use crate::graph::GraphDoc;
use crate::monitor::Monitor;
use crate::ui::default_dock;

pub struct App {
    pub graph: GraphDoc,
    pub dock: DockState,
    pub playing: bool,
    pub monitor: Arc<Monitor>,
    last_fp: u64,
    last_playing: bool,
    tx: EventSender<Patch>,
    _engine: AudioEngine,
}

impl App {
    pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let graph = GraphDoc::new_default();
        let (tx, mut rx) = event_channel::<Patch>(16);
        let first = Patch::from_doc(&graph, false);
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
                live.tick(graph);
            })
        })?;

        Ok(Self {
            last_fp: graph.fingerprint(),
            last_playing: false,
            graph,
            dock: default_dock(),
            playing: false,
            monitor,
            tx,
            _engine: engine,
        })
    }

    pub fn sync_audio(&mut self) {
        let fp = self.graph.fingerprint();
        if fp == self.last_fp && self.playing == self.last_playing {
            return;
        }
        self.last_fp = fp;
        self.last_playing = self.playing;
        let _ = self.tx.send(Patch::from_doc(&self.graph, self.playing));
    }
}
