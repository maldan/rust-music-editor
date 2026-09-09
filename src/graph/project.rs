use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use mega_audio::sample::{peaks, AudioClip, load_audio_file};

use super::doc::GraphDoc;
use super::node::{GraphNode, NodeKind, SeqNote, SEQ_STEPS};

const SAMPLE_PEAKS: usize = 2048;

pub const DEFAULT_GROUP_ID: u32 = 0;
pub const DEFAULT_GROUP_COLOR: [f32; 4] = [0.32, 0.72, 0.40, 1.0];

const GROUP_PALETTE: [[f32; 4]; 7] = [
    [0.28, 0.62, 0.92, 1.0],
    [0.92, 0.62, 0.22, 1.0],
    [0.72, 0.38, 0.92, 1.0],
    [0.92, 0.82, 0.22, 1.0],
    [0.22, 0.78, 0.72, 1.0],
    [0.92, 0.38, 0.55, 1.0],
    [0.55, 0.72, 0.28, 1.0],
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorView {
    Graph,
    Sequence(String),
    Instrument(String),
    Sample(String),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NoteGroup {
    pub id: u32,
    pub name: String,
    #[serde(default = "default_group_color")]
    pub color: [f32; 4],
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub play_inst: String,
}

fn default_group_color() -> [f32; 4] {
    DEFAULT_GROUP_COLOR
}

fn default_true() -> bool {
    true
}

pub fn default_note_group(play_inst: String) -> NoteGroup {
    NoteGroup {
        id: DEFAULT_GROUP_ID,
        name: "Default".into(),
        color: DEFAULT_GROUP_COLOR,
        visible: true,
        play_inst,
    }
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
    /// Kept in sync with the default group's `play_inst`.
    #[serde(default)]
    pub play_inst: String,
    #[serde(default)]
    pub groups: Vec<NoteGroup>,
    #[serde(default = "default_next_group")]
    pub next_group: u32,
}

fn default_next_group() -> u32 {
    1
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

    pub fn ensure_groups(&mut self) {
        if self.groups.is_empty() {
            self.groups.push(default_note_group(self.play_inst.clone()));
        } else if !self.groups.iter().any(|g| g.id == DEFAULT_GROUP_ID) {
            self.groups.insert(0, default_note_group(self.play_inst.clone()));
        }
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == DEFAULT_GROUP_ID) {
            if g.play_inst.is_empty() && !self.play_inst.is_empty() {
                g.play_inst = self.play_inst.clone();
            }
            self.play_inst = g.play_inst.clone();
        }
        let max_id = self.groups.iter().map(|g| g.id).max().unwrap_or(0);
        self.next_group = self.next_group.max(max_id.saturating_add(1)).max(1);
    }

    pub fn group(&self, id: u32) -> Option<&NoteGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_visible(&self, id: u32) -> bool {
        self.groups
            .iter()
            .find(|g| g.id == id)
            .map(|g| g.visible)
            .unwrap_or(true)
    }

    pub fn visible_notes(&self) -> Vec<SeqNote> {
        self.notes
            .iter()
            .copied()
            .filter(|n| self.group_visible(n.group))
            .collect()
    }

    /// `None` = all visible groups. `Some(id)` = that group even if hidden.
    pub fn notes_for_play(&self, group: Option<u32>) -> Vec<SeqNote> {
        match group {
            None => self.visible_notes(),
            Some(gid) => {
                if self.group(gid).is_none() {
                    return self.visible_notes();
                }
                self.notes
                    .iter()
                    .copied()
                    .filter(|n| n.group == gid)
                    .collect()
            }
        }
    }

    pub fn add_group(&mut self) -> u32 {
        self.ensure_groups();
        let id = self.next_group.max(1);
        self.next_group = id.saturating_add(1);
        let color = GROUP_PALETTE[(id as usize).saturating_sub(1) % GROUP_PALETTE.len()];
        self.groups.push(NoteGroup {
            id,
            name: format!("Group {id}"),
            color,
            visible: true,
            play_inst: String::new(),
        });
        id
    }

    pub fn remove_group(&mut self, id: u32) {
        if id == DEFAULT_GROUP_ID {
            return;
        }
        self.groups.retain(|g| g.id != id);
        for n in &mut self.notes {
            if n.group == id {
                n.group = DEFAULT_GROUP_ID;
            }
        }
    }

    pub fn set_group_play_inst(&mut self, id: u32, inst: String) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
            g.play_inst = inst.clone();
        }
        if id == DEFAULT_GROUP_ID {
            self.play_inst = inst;
        }
    }
}

pub struct Instrument {
    pub id: String,
    pub name: String,
    pub graph: GraphDoc,
    /// Empty = built-in C3/C4/C5 demo. Otherwise a sequence id to preview through this instrument.
    pub play_seq: String,
    /// `None` = every visible group in `play_seq`. `Some` = one group id.
    pub play_group: Option<u32>,
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
                        SeqNote { step: 0, pitch: 60, len: 1, group: 0 },
                        SeqNote { step: 4, pitch: 64, len: 1, group: 0 },
                        SeqNote { step: 8, pitch: 67, len: 1, group: 0 },
                        SeqNote { step: 12, pitch: 64, len: 1, group: 0 },
                    ],
                    play_inst: String::new(),
                    groups: vec![default_note_group(String::new())],
                    next_group: 1,
                },
                Sequence {
                    id: "s2".into(),
                    name: "Sequence 2".into(),
                    seq_loop_bars: 1,
                    seq_octave: 4,
                    notes: vec![
                        SeqNote { step: 2, pitch: 62, len: 1, group: 0 },
                        SeqNote { step: 6, pitch: 65, len: 1, group: 0 },
                        SeqNote { step: 10, pitch: 69, len: 1, group: 0 },
                        SeqNote { step: 14, pitch: 71, len: 1, group: 0 },
                    ],
                    play_inst: String::new(),
                    groups: vec![default_note_group(String::new())],
                    next_group: 1,
                },
            ],
            instruments: vec![Instrument {
                id: "i1".into(),
                name: "Sine".into(),
                graph: GraphDoc::new_instrument(),
                play_seq: String::new(),
                play_group: None,
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
            groups: vec![default_note_group(String::new())],
            next_group: 1,
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
                groups: vec![default_note_group(String::new())],
                next_group: 1,
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
        for i in &mut self.instruments {
            if i.play_seq == id {
                i.play_seq.clear();
                i.play_group = None;
            }
        }
        if matches!(&self.view, EditorView::Sequence(cur) if cur == id) {
            self.select_graph();
        }
        self.pending_delete_seq = None;
    }

    pub fn merge_sequence(&mut self, into_id: &str, from_id: &str) -> bool {
        if into_id == from_id {
            return false;
        }
        let Some(from_i) = self.sequences.iter().position(|s| s.id == from_id) else {
            return false;
        };
        if self.sequences.iter().all(|s| s.id != into_id) {
            return false;
        }
        let src = self.sequences[from_i].clone();
        let Some(dst) = self.sequence_mut(into_id) else {
            return false;
        };
        dst.ensure_groups();
        let gid = dst.add_group();
        let play = src
            .groups
            .iter()
            .find(|g| g.id == DEFAULT_GROUP_ID)
            .map(|g| g.play_inst.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| src.play_inst.clone());
        if let Some(g) = dst.groups.iter_mut().find(|g| g.id == gid) {
            g.name = src.name.clone();
            g.play_inst = play;
        }
        let mut notes = src.notes;
        for n in &mut notes {
            n.group = gid;
        }
        let end = notes
            .iter()
            .map(|n| n.step + n.len.max(1))
            .max()
            .unwrap_or(0);
        let bars = end.div_ceil(SEQ_STEPS).max(1);
        dst.seq_loop_bars = dst.seq_loop_bars.max(src.seq_loop_bars).max(bars);
        dst.notes.extend(notes);
        for n in &mut self.main.nodes {
            if n.kind == NodeKind::Sequencer && n.seq_id == from_id {
                n.seq_id = into_id.to_string();
                if n.seq_group.is_some() {
                    n.seq_group = Some(gid);
                }
            }
        }
        for i in &mut self.instruments {
            if i.play_seq == from_id {
                i.play_seq = into_id.to_string();
                if i.play_group.is_some() {
                    i.play_group = Some(gid);
                }
            }
        }
        self.remove_sequence(from_id);
        true
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
            play_seq: String::new(),
            play_group: None,
        });
        self.select_instrument(&id);
        id
    }

    pub fn forget_play_group(&mut self, seq_id: &str, group: u32) {
        for i in &mut self.instruments {
            if i.play_seq == seq_id && i.play_group == Some(group) {
                i.play_group = None;
            }
            for n in &mut i.graph.nodes {
                if n.kind == NodeKind::Sequencer && n.seq_id == seq_id && n.seq_group == Some(group) {
                    n.seq_group = None;
                }
            }
        }
        for n in &mut self.main.nodes {
            if n.kind == NodeKind::Sequencer && n.seq_id == seq_id && n.seq_group == Some(group) {
                n.seq_group = None;
            }
        }
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
            for g in &mut s.groups {
                if g.play_inst == id {
                    g.play_inst.clear();
                }
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
                n.notes = seq.notes_for_play(n.seq_group);
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
            s.groups.len().hash(&mut h);
            for g in &s.groups {
                g.id.hash(&mut h);
                g.name.hash(&mut h);
                g.visible.hash(&mut h);
                g.play_inst.hash(&mut h);
                for c in g.color {
                    c.to_bits().hash(&mut h);
                }
            }
            s.notes.len().hash(&mut h);
            for n in &s.notes {
                n.step.hash(&mut h);
                n.pitch.hash(&mut h);
                n.len.hash(&mut h);
                n.group.hash(&mut h);
            }
        }
        for i in &self.instruments {
            i.id.hash(&mut h);
            i.name.hash(&mut h);
            i.play_seq.hash(&mut h);
            i.play_group.hash(&mut h);
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
        assert_eq!(p.instruments[0].play_seq, "");
    }

    #[test]
    fn remove_sequence_clears_instrument_play_seq() {
        let mut p = Project::new_default();
        p.instruments[0].play_seq = "s1".into();
        p.remove_sequence("s1");
        assert_eq!(p.instruments[0].play_seq, "");
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

    #[test]
    fn hidden_group_notes_leave_graph_sequencer() {
        let mut p = Project::new_default();
        p.sequences[0].add_group();
        let gid = p.sequences[0].groups[1].id;
        p.sequences[0].notes[0].group = gid;
        p.sequences[0].groups[1].visible = false;
        let seq_id = p.sequences[0].id.clone();
        let node = p
            .main
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Sequencer)
            .map(|n| n.id.clone())
            .expect("seq node");
        if let Some(n) = p.main.nodes.iter_mut().find(|n| n.id == node) {
            n.seq_id = seq_id;
        }
        Project::apply_seq_notes(&mut p.main.nodes, &p.sequences);
        let n = p.main.nodes.iter().find(|n| n.id == node).unwrap();
        assert!(n.notes.iter().all(|note| note.group != gid));
        assert!(!n.notes.is_empty());
    }

    #[test]
    fn sequencer_node_can_play_one_hidden_group() {
        let mut p = Project::new_default();
        p.sequences[0].add_group();
        let gid = p.sequences[0].groups[1].id;
        p.sequences[0].notes[0].group = gid;
        p.sequences[0].groups[1].visible = false;
        let seq_id = p.sequences[0].id.clone();
        let node = p
            .main
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Sequencer)
            .map(|n| n.id.clone())
            .expect("seq node");
        if let Some(n) = p.main.nodes.iter_mut().find(|n| n.id == node) {
            n.seq_id = seq_id;
            n.seq_group = Some(gid);
        }
        Project::apply_seq_notes(&mut p.main.nodes, &p.sequences);
        let n = p.main.nodes.iter().find(|n| n.id == node).unwrap();
        assert_eq!(n.notes.len(), 1);
        assert_eq!(n.notes[0].group, gid);
    }

    #[test]
    fn remove_group_moves_notes_to_default() {
        let mut p = Project::new_default();
        let gid = p.sequences[0].add_group();
        p.sequences[0].notes[0].group = gid;
        p.sequences[0].remove_group(gid);
        assert_eq!(p.sequences[0].notes[0].group, DEFAULT_GROUP_ID);
        assert!(p.sequences[0].groups.iter().all(|g| g.id != gid));
        p.sequences[0].remove_group(DEFAULT_GROUP_ID);
        assert!(p.sequences[0].groups.iter().any(|g| g.id == DEFAULT_GROUP_ID));
    }

    #[test]
    fn notes_for_play_filters_group() {
        let mut p = Project::new_default();
        let gid = p.sequences[0].add_group();
        p.sequences[0].notes[0].group = gid;
        let all = p.sequences[0].notes_for_play(None);
        let one = p.sequences[0].notes_for_play(Some(gid));
        assert_eq!(all.len(), p.sequences[0].notes.len());
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].group, gid);
    }

    #[test]
    fn remove_group_clears_instrument_play_group() {
        let mut p = Project::new_default();
        let gid = p.sequences[0].add_group();
        p.instruments[0].play_seq = p.sequences[0].id.clone();
        p.instruments[0].play_group = Some(gid);
        p.forget_play_group(&p.sequences[0].id.clone(), gid);
        p.sequences[0].remove_group(gid);
        assert_eq!(p.instruments[0].play_group, None);
    }

    #[test]
    fn merge_sequence_becomes_group() {
        let mut p = Project::new_default();
        let n0 = p.sequences[0].notes.len();
        let n1 = p.sequences[1].notes.len();
        let name = p.sequences[1].name.clone();
        assert!(p.merge_sequence("s1", "s2"));
        assert_eq!(p.sequences.len(), 1);
        assert_eq!(p.sequences[0].id, "s1");
        assert_eq!(p.sequences[0].notes.len(), n0 + n1);
        let g = p.sequences[0]
            .groups
            .iter()
            .find(|g| g.name == name)
            .expect("merged group");
        assert_ne!(g.id, DEFAULT_GROUP_ID);
        assert_eq!(
            p.sequences[0]
                .notes
                .iter()
                .filter(|n| n.group == g.id)
                .count(),
            n1
        );
        assert!(!p.merge_sequence("s1", "s1"));
    }
}
