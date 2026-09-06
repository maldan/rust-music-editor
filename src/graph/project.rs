use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use mega_audio::sample::{peaks, AudioClip, load_audio_file};

use super::doc::GraphDoc;
use super::node::{GraphNode, NodeKind, SeqNote};

const SAMPLE_PEAKS: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorView {
    Graph,
    Sequence(String),
    Instrument(String),
    Sample(String),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Sequence {
    pub id: String,
    pub name: String,
    #[serde(default = "default_seq_loop_bars")]
    pub seq_loop_bars: u32,
    #[serde(default = "default_seq_octave")]
    pub seq_octave: i32,
    #[serde(default)]
    pub notes: Vec<SeqNote>,
    /// Empty = built-in sine. Otherwise an instrument entity id.
    #[serde(default)]
    pub play_inst: String,
}

fn default_seq_loop_bars() -> u32 {
    1
}
fn default_seq_octave() -> i32 {
    4
}

impl Sequence {
    pub fn loop_bars(&self) -> u32 {
        self.seq_loop_bars.max(1)
    }
}

pub struct Instrument {
    pub id: String,
    pub name: String,
    pub graph: GraphDoc,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Sample {
    pub id: String,
    pub name: String,
    pub path: String,
    pub sample_rate: u32,
    pub frames: u32,
    #[serde(default)]
    pub peaks: Vec<f32>,
    #[serde(skip)]
    pub clip: Option<Arc<AudioClip>>,
}

impl Sample {
    pub fn duration(&self) -> f32 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames as f32 / self.sample_rate as f32
        }
    }

    pub fn from_file(id: String, path: &Path) -> Result<Self, String> {
        let clip = load_audio_file(path).map_err(|e| e.to_string())?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Sample")
            .to_string();
        let frames = clip.data.len() as u32;
        let sample_rate = clip.sample_rate;
        let wave = peaks(&clip.data, SAMPLE_PEAKS);
        Ok(Self {
            id,
            name,
            path: path.display().to_string(),
            sample_rate,
            frames,
            peaks: wave,
            clip: Some(Arc::new(clip)),
        })
    }

    pub fn reload(&mut self) {
        let Ok(clip) = load_audio_file(Path::new(&self.path)) else {
            return;
        };
        self.frames = clip.data.len() as u32;
        self.sample_rate = clip.sample_rate;
        if self.peaks.is_empty() {
            self.peaks = peaks(&clip.data, SAMPLE_PEAKS);
        }
        self.clip = Some(Arc::new(clip));
    }
}

pub struct Project {
    pub main: GraphDoc,
    pub sequences: Vec<Sequence>,
    pub instruments: Vec<Instrument>,
    pub samples: Vec<Sample>,
    pub view: EditorView,
    pub tree_sel: Option<String>,
    pub next_seq: u64,
    pub next_inst: u64,
    pub next_sample: u64,
    pub pending_delete_seq: Option<String>,
    pub pending_delete_inst: Option<String>,
    pub pending_delete_sample: Option<String>,
    pub sample_seek: f64,
}

impl Project {
    pub fn new_default() -> Self {
        let mut p = Self {
            main: GraphDoc::new_default(),
            sequences: vec![
                Sequence {
                    id: "s1".into(),
                    name: "Sequence 1".into(),
                    seq_loop_bars: 1,
                    seq_octave: 4,
                    notes: vec![
                        SeqNote { step: 0, pitch: 60, len: 1 },
                        SeqNote { step: 4, pitch: 64, len: 1 },
                        SeqNote { step: 8, pitch: 67, len: 1 },
                        SeqNote { step: 12, pitch: 64, len: 1 },
                    ],
                    play_inst: String::new(),
                },
                Sequence {
                    id: "s2".into(),
                    name: "Sequence 2".into(),
                    seq_loop_bars: 1,
                    seq_octave: 4,
                    notes: vec![
                        SeqNote { step: 2, pitch: 62, len: 1 },
                        SeqNote { step: 6, pitch: 65, len: 1 },
                        SeqNote { step: 10, pitch: 69, len: 1 },
                        SeqNote { step: 14, pitch: 71, len: 1 },
                    ],
                    play_inst: String::new(),
                },
            ],
            instruments: vec![Instrument {
                id: "i1".into(),
                name: "Sine".into(),
                graph: GraphDoc::new_instrument(),
            }],
            samples: Vec::new(),
            view: EditorView::Graph,
            tree_sel: Some("graph".into()),
            next_seq: 3,
            next_inst: 2,
            next_sample: 1,
            pending_delete_seq: None,
            pending_delete_inst: None,
            pending_delete_sample: None,
            sample_seek: 0.0,
        };
        p.sync_serials();
        p
    }

    pub fn add_sequence(&mut self) -> String {
        let id = format!("s{}", self.next_seq);
        self.next_seq += 1;
        let name = format!("Sequence {}", self.sequences.len() + 1);
        self.sequences.push(Sequence {
            id: id.clone(),
            name,
            seq_loop_bars: 1,
            seq_octave: 4,
            notes: Vec::new(),
            play_inst: String::new(),
        });
        self.select_sequence(&id);
        id
    }

    pub fn import_midi(&mut self, bytes: &[u8]) -> Result<usize, String> {
        let tracks = super::midi::parse_midi(bytes)?;
        let n = tracks.len();
        for t in tracks {
            let id = format!("s{}", self.next_seq);
            self.next_seq += 1;
            self.sequences.push(Sequence {
                id,
                name: t.name,
                seq_loop_bars: t.bars,
                seq_octave: t.octave,
                notes: t.notes,
                play_inst: String::new(),
            });
        }
        Ok(n)
    }

    pub fn remove_sequence(&mut self, id: &str) {
        self.sequences.retain(|s| s.id != id);
        for n in &mut self.main.nodes {
            if n.kind == NodeKind::Sequencer && n.seq_id == id {
                n.seq_id.clear();
                n.notes.clear();
            }
        }
        if matches!(&self.view, EditorView::Sequence(cur) if cur == id) {
            self.select_graph();
        }
        self.pending_delete_seq = None;
    }

    pub fn import_sample(&mut self, path: &Path) -> Result<String, String> {
        let id = format!("a{}", self.next_sample);
        self.next_sample += 1;
        let sample = Sample::from_file(id.clone(), path)?;
        self.samples.push(sample);
        self.select_sample(&id);
        Ok(id)
    }

    pub fn remove_sample(&mut self, id: &str) {
        self.samples.retain(|s| s.id != id);
        for n in &mut self.main.nodes {
            if n.kind == NodeKind::Sample && n.sample_id == id {
                n.sample_id.clear();
            }
        }
        if matches!(&self.view, EditorView::Sample(cur) if cur == id) {
            self.select_graph();
        }
        self.pending_delete_sample = None;
    }

    pub fn add_instrument(&mut self) -> String {
        let id = format!("i{}", self.next_inst);
        self.next_inst += 1;
        let name = format!("Instrument {}", self.instruments.len() + 1);
        self.instruments.push(Instrument {
            id: id.clone(),
            name,
            graph: GraphDoc::new_instrument(),
        });
        self.select_instrument(&id);
        id
    }

    pub fn remove_instrument(&mut self, id: &str) {
        self.instruments.retain(|i| i.id != id);
        for n in &mut self.main.nodes {
            if n.kind == NodeKind::Instrument && n.inst_id == id {
                n.inst_id.clear();
            }
        }
        for s in &mut self.sequences {
            if s.play_inst == id {
                s.play_inst.clear();
            }
        }
        if matches!(&self.view, EditorView::Instrument(cur) if cur == id) {
            self.select_graph();
        }
        self.pending_delete_inst = None;
    }

    pub fn select_graph(&mut self) {
        self.view = EditorView::Graph;
        self.tree_sel = Some("graph".into());
        self.bump_seek();
    }

    pub fn select_sequence(&mut self, id: &str) {
        self.view = EditorView::Sequence(id.to_string());
        self.tree_sel = Some(format!("seq:{id}"));
        self.bump_seek();
    }

    pub fn select_instrument(&mut self, id: &str) {
        self.view = EditorView::Instrument(id.to_string());
        self.tree_sel = Some(format!("inst:{id}"));
        self.bump_seek();
    }

    pub fn select_sample(&mut self, id: &str) {
        self.view = EditorView::Sample(id.to_string());
        self.tree_sel = Some(format!("smp:{id}"));
        self.bump_seek();
    }

    fn bump_seek(&mut self) {
        self.main.seek_gen = self.main.seek_gen.wrapping_add(1);
    }

    pub fn seek_sample(&mut self, seconds: f64) {
        self.sample_seek = seconds.max(0.0);
        self.bump_seek();
    }

    pub fn apply_tree_sel(&mut self) {
        let next = match self.tree_sel.clone() {
            Some(s) if s == "graph" => Some(EditorView::Graph),
            Some(s) if s.starts_with("seq:") => {
                let id = s[4..].to_string();
                self.sequences.iter().any(|seq| seq.id == id).then_some(EditorView::Sequence(id))
            }
            Some(s) if s.starts_with("inst:") => {
                let id = s[5..].to_string();
                self.instruments
                    .iter()
                    .any(|inst| inst.id == id)
                    .then_some(EditorView::Instrument(id))
            }
            Some(s) if s.starts_with("smp:") => {
                let id = s[4..].to_string();
                self.samples.iter().any(|smp| smp.id == id).then_some(EditorView::Sample(id))
            }
            _ => None,
        };
        let Some(next) = next else {
            return;
        };
        if next == self.view {
            return;
        }
        match next {
            EditorView::Graph => self.select_graph(),
            EditorView::Sequence(id) => self.select_sequence(&id),
            EditorView::Instrument(id) => self.select_instrument(&id),
            EditorView::Sample(id) => self.select_sample(&id),
        }
    }

    pub fn active_graph(&mut self) -> Option<&mut GraphDoc> {
        match &self.view {
            EditorView::Graph => Some(&mut self.main),
            EditorView::Instrument(id) => self
                .instruments
                .iter_mut()
                .find(|i| i.id == *id)
                .map(|i| &mut i.graph),
            EditorView::Sequence(_) | EditorView::Sample(_) => None,
        }
    }

    pub fn sequence_mut(&mut self, id: &str) -> Option<&mut Sequence> {
        self.sequences.iter_mut().find(|s| s.id == id)
    }

    pub fn instrument_mut(&mut self, id: &str) -> Option<&mut Instrument> {
        self.instruments.iter_mut().find(|i| i.id == id)
    }

    pub fn sample_mut(&mut self, id: &str) -> Option<&mut Sample> {
        self.samples.iter_mut().find(|s| s.id == id)
    }

    pub fn apply_seq_notes(nodes: &mut [GraphNode], sequences: &[Sequence]) {
        for n in nodes {
            if n.kind != NodeKind::Sequencer || n.seq_id.is_empty() {
                continue;
            }
            if let Some(seq) = sequences.iter().find(|s| s.id == n.seq_id) {
                n.notes = seq.notes.clone();
                n.seq_loop_bars = seq.seq_loop_bars;
                n.seq_octave = seq.seq_octave;
            }
        }
    }

    pub fn fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        match &self.view {
            EditorView::Graph => 0u8.hash(&mut h),
            EditorView::Sequence(id) => {
                1u8.hash(&mut h);
                id.hash(&mut h);
            }
            EditorView::Instrument(id) => {
                2u8.hash(&mut h);
                id.hash(&mut h);
            }
            EditorView::Sample(id) => {
                3u8.hash(&mut h);
                id.hash(&mut h);
            }
        }
        self.main.fingerprint().hash(&mut h);
        for s in &self.sequences {
            s.id.hash(&mut h);
            s.name.hash(&mut h);
            s.seq_loop_bars.hash(&mut h);
            s.play_inst.hash(&mut h);
            s.notes.len().hash(&mut h);
            for n in &s.notes {
                n.step.hash(&mut h);
                n.pitch.hash(&mut h);
                n.len.hash(&mut h);
            }
        }
        for i in &self.instruments {
            i.id.hash(&mut h);
            i.name.hash(&mut h);
            i.graph.fingerprint().hash(&mut h);
        }
        for s in &self.samples {
            s.id.hash(&mut h);
            s.name.hash(&mut h);
            s.path.hash(&mut h);
            s.frames.hash(&mut h);
            s.sample_rate.hash(&mut h);
        }
        h.finish()
    }

    pub(crate) fn sync_serials(&mut self) {
        let max_s = self
            .sequences
            .iter()
            .filter_map(|s| s.id.strip_prefix('s')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        self.next_seq = self.next_seq.max(max_s + 1);
        let max_i = self
            .instruments
            .iter()
            .filter_map(|i| i.id.strip_prefix('i')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        self.next_inst = self.next_inst.max(max_i + 1);
        let max_a = self
            .samples
            .iter()
            .filter_map(|s| s.id.strip_prefix('a')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        self.next_sample = self.next_sample.max(max_a + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_sequence_clears_graph_refs() {
        let mut p = Project::new_default();
        p.remove_sequence("s1");
        assert!(p.sequences.iter().all(|s| s.id != "s1"));
        assert!(
            p.main
                .nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Sequencer)
                .all(|n| n.seq_id != "s1")
        );
        assert_eq!(p.view, EditorView::Graph);
    }

    #[test]
    fn remove_sample_clears_graph_refs() {
        let mut p = Project::new_default();
        p.samples.push(Sample {
            id: "a1".into(),
            name: "Kick".into(),
            path: String::new(),
            sample_rate: 44100,
            frames: 0,
            peaks: Vec::new(),
            clip: None,
        });
        let id = p.main.spawn_node(NodeKind::Sample, glam::Vec2::ZERO);
        if let Some(n) = p.main.nodes.iter_mut().find(|n| n.id == id) {
            n.sample_id = "a1".into();
        }
        p.remove_sample("a1");
        assert!(p.samples.iter().all(|s| s.id != "a1"));
        assert!(
            p.main
                .nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Sample)
                .all(|n| n.sample_id != "a1")
        );
    }

    #[test]
    fn remove_instrument_clears_refs() {
        let mut p = Project::new_default();
        p.sequences[0].play_inst = "i1".into();
        let id = p.main.spawn_node(NodeKind::Instrument, glam::Vec2::ZERO);
        if let Some(n) = p.main.nodes.iter_mut().find(|n| n.id == id) {
            n.inst_id = "i1".into();
        }
        p.select_instrument("i1");
        p.remove_instrument("i1");
        assert!(p.instruments.iter().all(|i| i.id != "i1"));
        assert_eq!(p.sequences[0].play_inst, "");
        assert!(
            p.main
                .nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Instrument)
                .all(|n| n.inst_id != "i1")
        );
        assert_eq!(p.view, EditorView::Graph);
    }
}
