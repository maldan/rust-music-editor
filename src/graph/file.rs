use std::fs;
use std::path::Path;

use glam::Vec2;
use serde::{Deserialize, Deserializer, Serialize};

use super::doc::GraphDoc;
use super::node::{parse_tick, GraphNode, NodeKind};
use super::project::{Instrument, Project, Sequence};

/// On-disk extension; payload is still JSON.
pub const FILE_EXT: &str = "megp";

#[derive(Serialize, Deserialize)]
struct GraphFile {
    version: u32,
    next_serial: u64,
    output_id: String,
    #[serde(default)]
    input_id: String,
    pan: [f32; 2],
    zoom: f32,
    #[serde(default)]
    bpm: Option<f32>,
    #[serde(default = "default_play_from", deserialize_with = "de_play_from")]
    play_from: i32,
    nodes: Vec<GraphNode>,
    links: Vec<FileLink>,
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    graph: GraphFile,
    #[serde(default)]
    sequences: Vec<Sequence>,
    #[serde(default)]
    instruments: Vec<InstrumentFile>,
}

#[derive(Serialize, Deserialize)]
struct InstrumentFile {
    id: String,
    name: String,
    graph: GraphFile,
}

fn default_play_from() -> i32 {
    1
}

fn de_play_from<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Tick {
        N(i32),
        S(String),
    }
    Ok(match Tick::deserialize(d)? {
        Tick::N(n) => n.max(1),
        Tick::S(s) => parse_tick(&s),
    })
}

#[derive(Serialize, Deserialize)]
struct FileLink {
    from_node: String,
    from_port: String,
    to_node: String,
    to_port: String,
}

impl GraphDoc {
    fn to_file(&self) -> GraphFile {
        GraphFile {
            version: 1,
            next_serial: self.next_serial,
            output_id: self.output_id.clone(),
            input_id: self.input_id.clone(),
            pan: [self.space.pan.x, self.space.pan.y],
            zoom: self.space.zoom,
            bpm: Some(self.bpm),
            play_from: self.play_from.max(1),
            nodes: self.nodes.clone(),
            links: self
                .space
                .links
                .iter()
                .map(|l| FileLink {
                    from_node: l.from_node.clone(),
                    from_port: l.from_port.clone(),
                    to_node: l.to_node.clone(),
                    to_port: l.to_port.clone(),
                })
                .collect(),
        }
    }

    fn from_file(file: GraphFile) -> Result<Self, String> {
        if file.nodes.is_empty() {
            return Err("graph has no nodes".into());
        }
        let mut doc = Self::blank();
        doc.nodes = file.nodes;
        for n in &mut doc.nodes {
            n.migrate_seq_when();
            n.migrate_mixer_kind();
        }
        doc.next_serial = file.next_serial.max(1);
        doc.output_id = file.output_id;
        doc.input_id = file.input_id;
        doc.space.pan = Vec2::new(file.pan[0], file.pan[1]);
        if file.zoom > 0.05 {
            doc.space.zoom = file.zoom;
        }
        doc.bpm = file
            .bpm
            .filter(|b| *b >= 1.0)
            .or_else(|| {
                doc.nodes
                    .iter()
                    .find(|n| n.kind == NodeKind::Clock)
                    .map(|n| n.bpm.max(1.0))
            })
            .unwrap_or(120.0);
        doc.play_from = file.play_from.max(1);
        for l in &file.links {
            let to_port = remap_mix_port(&doc.nodes, &l.to_node, &l.to_port);
            let _ = doc.connect(&l.from_node, &l.from_port, &l.to_node, &to_port);
        }
        if !doc
            .nodes
            .iter()
            .any(|n| n.id == doc.output_id && n.kind == NodeKind::Output)
        {
            if let Some(id) = doc
                .nodes
                .iter()
                .find(|n| n.kind == NodeKind::Output)
                .map(|n| n.id.clone())
            {
                doc.output_id = id;
            } else {
                doc.output_id = doc.spawn_node(NodeKind::Output, Vec2::new(980.0, 220.0));
            }
        }
        if !doc.input_id.is_empty()
            && !doc
                .nodes
                .iter()
                .any(|n| n.id == doc.input_id && n.kind == NodeKind::Input)
        {
            if let Some(id) = doc
                .nodes
                .iter()
                .find(|n| n.kind == NodeKind::Input)
                .map(|n| n.id.clone())
            {
                doc.input_id = id;
            }
        }
        let max_n = doc
            .nodes
            .iter()
            .filter_map(|n| n.id.strip_prefix('n')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        doc.next_serial = doc.next_serial.max(max_n + 1);
        Ok(doc)
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.to_file()).map_err(|e| e.to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let file: GraphFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Self::from_file(file)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        fs::write(path, self.to_json()?).map_err(|e| e.to_string())
    }

    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::from_json(&text)
    }
}

impl Project {
    pub fn to_json(&self) -> Result<String, String> {
        let file = ProjectFile {
            version: 2,
            graph: self.main.to_file(),
            sequences: self.sequences.clone(),
            instruments: self
                .instruments
                .iter()
                .map(|i| InstrumentFile {
                    id: i.id.clone(),
                    name: i.name.clone(),
                    graph: i.graph.to_file(),
                })
                .collect(),
        };
        serde_json::to_string_pretty(&file).map_err(|e| e.to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        if let Ok(file) = serde_json::from_str::<ProjectFile>(text) {
            if file.version >= 2 {
                return Self::from_v2(file);
            }
        }
        let graph = GraphDoc::from_json(text)?;
        Ok(migrate_v1(graph))
    }

    fn from_v2(file: ProjectFile) -> Result<Self, String> {
        let mut p = Self {
            main: GraphDoc::from_file(file.graph)?,
            sequences: file.sequences,
            instruments: Vec::new(),
            view: super::project::EditorView::Graph,
            tree_sel: Some("graph".into()),
            next_seq: 1,
            next_inst: 1,
        };
        for inst in file.instruments {
            p.instruments.push(Instrument {
                id: inst.id,
                name: inst.name,
                graph: GraphDoc::from_file(inst.graph)?,
            });
        }
        if p.sequences.is_empty() {
            extract_sequences(&mut p.main, &mut p.sequences);
        }
        p.sync_serials();
        Ok(p)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        fs::write(path, self.to_json()?).map_err(|e| e.to_string())
    }

    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::from_json(&text)
    }
}

fn migrate_v1(mut graph: GraphDoc) -> Project {
    let mut sequences = Vec::new();
    extract_sequences(&mut graph, &mut sequences);
    let mut p = Project {
        main: graph,
        sequences,
        instruments: Vec::new(),
        view: super::project::EditorView::Graph,
        tree_sel: Some("graph".into()),
        next_seq: 1,
        next_inst: 1,
    };
        p.sync_serials();
        p
    }

fn extract_sequences(graph: &mut GraphDoc, sequences: &mut Vec<Sequence>) {
    let mut n = sequences.len() as u64 + 1;
    for node in &mut graph.nodes {
        if node.kind != NodeKind::Sequencer || !node.seq_id.is_empty() {
            continue;
        }
        let id = format!("s{n}");
        n += 1;
        sequences.push(Sequence {
            id: id.clone(),
            name: format!("Sequence {}", sequences.len() + 1),
            seq_loop_bars: node.seq_loop_bars,
            seq_octave: node.seq_octave,
            notes: node.notes.clone(),
            play_inst: String::new(),
        });
        node.seq_id = id;
    }
}

pub fn with_graph_ext(mut path: std::path::PathBuf) -> std::path::PathBuf {
    if path.extension().is_none() {
        path.set_extension(FILE_EXT);
    }
    path
}

fn remap_mix_port(nodes: &[GraphNode], to_node: &str, to_port: &str) -> String {
    let mix = nodes
        .iter()
        .any(|n| n.id == to_node && n.kind == NodeKind::Mixer);
    if !mix {
        return to_port.to_string();
    }
    match to_port {
        "a" => "1".into(),
        "b" => "2".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Project;

    #[test]
    fn default_project_roundtrip() {
        let a = Project::new_default();
        let json = a.to_json().unwrap();
        let b = Project::from_json(&json).unwrap();
        assert_eq!(a.main.nodes.len(), b.main.nodes.len());
        assert_eq!(a.sequences.len(), b.sequences.len());
        assert_eq!(a.instruments.len(), b.instruments.len());
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn v1_sequencer_becomes_entity() {
        let mut doc = GraphDoc::blank();
        let seq = doc.spawn_node(NodeKind::Sequencer, Vec2::ZERO);
        let out = doc.spawn_node(NodeKind::Output, Vec2::ZERO);
        if let Some(n) = doc.nodes.iter_mut().find(|n| n.id == seq) {
            n.notes = vec![crate::graph::SeqNote {
                step: 0,
                pitch: 60,
                len: 2,
            }];
        }
        doc.output_id = out;
        let p = Project::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(p.sequences.len(), 1);
        assert_eq!(p.sequences[0].notes[0].pitch, 60);
        assert_eq!(p.main.nodes.iter().find(|n| n.id == seq).unwrap().seq_id, "s1");
    }

    #[test]
    fn mix_legacy_ab_ports_remap() {
        let mut doc = GraphDoc::blank();
        let osc = doc.spawn_node(NodeKind::Osc, Vec2::ZERO);
        let mix = doc.spawn_node(NodeKind::Mixer, Vec2::ZERO);
        let out = doc.spawn_node(NodeKind::Output, Vec2::ZERO);
        doc.output_id = out;
        doc.connect(&osc, "out", &mix, "1").unwrap();
        let json = doc
            .to_json()
            .unwrap()
            .replace("\"to_port\": \"1\"", "\"to_port\": \"a\"");
        let loaded = GraphDoc::from_json(&json).unwrap();
        assert!(
            loaded
                .space
                .links
                .iter()
                .any(|l| l.to_node == mix && l.to_port == "1"),
            "legacy mix port a should become 1"
        );
    }
}
