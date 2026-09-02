use std::fs;
use std::path::Path;

use glam::Vec2;
use serde::{Deserialize, Serialize};

use super::doc::GraphDoc;
use super::node::{GraphNode, NodeKind};

#[derive(Serialize, Deserialize)]
struct GraphFile {
    version: u32,
    next_serial: u64,
    output_id: String,
    pan: [f32; 2],
    zoom: f32,
    nodes: Vec<GraphNode>,
    links: Vec<FileLink>,
}

#[derive(Serialize, Deserialize)]
struct FileLink {
    from_node: String,
    from_port: String,
    to_node: String,
    to_port: String,
}

impl GraphDoc {
    pub fn to_json(&self) -> Result<String, String> {
        let file = GraphFile {
            version: 1,
            next_serial: self.next_serial,
            output_id: self.output_id.clone(),
            pan: [self.space.pan.x, self.space.pan.y],
            zoom: self.space.zoom,
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
        };
        serde_json::to_string_pretty(&file).map_err(|e| e.to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let file: GraphFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if file.nodes.is_empty() {
            return Err("graph has no nodes".into());
        }
        let mut doc = Self::blank();
        doc.nodes = file.nodes;
        doc.next_serial = file.next_serial.max(1);
        doc.output_id = file.output_id;
        doc.space.pan = Vec2::new(file.pan[0], file.pan[1]);
        if file.zoom > 0.05 {
            doc.space.zoom = file.zoom;
        }
        for l in &file.links {
            let _ = doc.connect(&l.from_node, &l.from_port, &l.to_node, &l.to_port);
        }
        if !doc.nodes.iter().any(|n| n.id == doc.output_id && n.kind == NodeKind::Output)
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
        let max_n = doc
            .nodes
            .iter()
            .filter_map(|n| n.id.strip_prefix('n')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        doc.next_serial = doc.next_serial.max(max_n + 1);
        Ok(doc)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        fs::write(path, self.to_json()?).map_err(|e| e.to_string())
    }

    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::from_json(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_graph_roundtrip() {
        let a = GraphDoc::new_default();
        let json = a.to_json().unwrap();
        let b = GraphDoc::from_json(&json).unwrap();
        assert_eq!(a.nodes.len(), b.nodes.len());
        assert_eq!(a.space.links.len(), b.space.links.len());
        assert_eq!(a.output_id, b.output_id);
        assert_eq!(a.fingerprint(), b.fingerprint());
    }
}
