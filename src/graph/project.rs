use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::doc::GraphDoc;
use super::node::{GraphNode, NodeKind, SeqNote};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorView {
    Graph,
    Sequence(String),
    Instrument(String),
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

pub struct Project {
    pub main: GraphDoc,
    pub sequences: Vec<Sequence>,
    pub instruments: Vec<Instrument>,
    pub view: EditorView,
    pub tree_sel: Option<String>,
    pub next_seq: u64,
    pub next_inst: u64,
    pub pending_delete_seq: Option<String>,
    /// Instrument editor: run transport so Trance Gate / clocked nodes move.
    pub preview_clock: bool,
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
            view: EditorView::Graph,
            tree_sel: Some("graph".into()),
            next_seq: 3,
            next_inst: 2,
            pending_delete_seq: None,
            preview_clock: false,
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
            self.view = EditorView::Graph;
            self.tree_sel = Some("graph".into());
        }
        self.pending_delete_seq = None;
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

    pub fn select_sequence(&mut self, id: &str) {
        self.view = EditorView::Sequence(id.to_string());
        self.tree_sel = Some(format!("seq:{id}"));
    }

    pub fn select_instrument(&mut self, id: &str) {
        self.view = EditorView::Instrument(id.to_string());
        self.tree_sel = Some(format!("inst:{id}"));
    }

    pub fn apply_tree_sel(&mut self) {
        match self.tree_sel.clone() {
            Some(s) if s == "graph" => self.view = EditorView::Graph,
            Some(s) if s.starts_with("seq:") => {
                let id = &s[4..];
                if self.sequences.iter().any(|seq| seq.id == id) {
                    self.view = EditorView::Sequence(id.to_string());
                }
            }
            Some(s) if s.starts_with("inst:") => {
                let id = &s[5..];
                if self.instruments.iter().any(|inst| inst.id == id) {
                    self.view = EditorView::Instrument(id.to_string());
                }
            }
            _ => {}
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
            EditorView::Sequence(_) => None,
        }
    }

    pub fn sequence_mut(&mut self, id: &str) -> Option<&mut Sequence> {
        self.sequences.iter_mut().find(|s| s.id == id)
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
}
