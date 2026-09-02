use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use glam::Vec2;
use mega_ui::{NodeLink, NodeSpace};

use super::node::{output_port_type, port, tick_to_beats, GraphNode, NodeKind, SeqNote};

pub struct GraphDoc {
    pub nodes: Vec<GraphNode>,
    pub space: NodeSpace,
    pub next_serial: u64,
    pub output_id: String,
    pub bpm: f32,
    pub play_from: i32,
    pub seek_gen: u64,
    pub seek_beats: f64,
}

impl GraphDoc {
    pub fn blank() -> Self {
        let mut space = NodeSpace::new();
        space.pan = Vec2::new(24.0, 24.0);
        space.register_type(port::AUDIO, "Audio", [0.35, 0.75, 0.95, 1.0]);
        space.register_type(port::CLOCK, "Clock", [0.95, 0.82, 0.28, 1.0]);
        space.register_type(port::NOTES, "Notes", [0.95, 0.55, 0.22, 1.0]);
        Self {
            nodes: Vec::new(),
            space,
            next_serial: 1,
            output_id: String::new(),
            bpm: 120.0,
            play_from: 1,
            seek_gen: 0,
            seek_beats: 0.0,
        }
    }

    pub fn new_default() -> Self {
        let mut doc = Self::blank();

        let clock = doc.spawn_node(NodeKind::Clock, Vec2::new(40.0, 40.0));
        let seq_a = doc.spawn_node(NodeKind::Sequencer, Vec2::new(40.0, 180.0));
        if let Some(n) = doc.nodes.iter_mut().find(|n| n.id == seq_a) {
            n.notes = vec![
                SeqNote { step: 0, pitch: 60, len: 1 },
                SeqNote { step: 4, pitch: 64, len: 1 },
                SeqNote { step: 8, pitch: 67, len: 1 },
                SeqNote { step: 12, pitch: 64, len: 1 },
            ];
        }
        let seq_b = doc.spawn_node(NodeKind::Sequencer, Vec2::new(40.0, 520.0));
        if let Some(n) = doc.nodes.iter_mut().find(|n| n.id == seq_b) {
            n.notes = vec![
                SeqNote { step: 2, pitch: 62, len: 1 },
                SeqNote { step: 6, pitch: 65, len: 1 },
                SeqNote { step: 10, pitch: 69, len: 1 },
                SeqNote { step: 14, pitch: 71, len: 1 },
            ];
        }
        let join = doc.spawn_node(NodeKind::NoteJoin, Vec2::new(320.0, 360.0));
        let voice = doc.spawn_node(NodeKind::Voice, Vec2::new(500.0, 360.0));
        let delay = doc.spawn_node(NodeKind::Delay, Vec2::new(700.0, 360.0));
        let gain = doc.spawn_node(NodeKind::Gain, Vec2::new(900.0, 360.0));
        let scope = doc.spawn_node(NodeKind::Scope, Vec2::new(1080.0, 360.0));
        let out = doc.spawn_node(NodeKind::Output, Vec2::new(1280.0, 360.0));
        doc.output_id = out.clone();

        let _ = doc.connect(&clock, "clock", &seq_a, "clock");
        let _ = doc.connect(&clock, "clock", &seq_b, "clock");
        let _ = doc.connect(&seq_a, "notes", &join, "1");
        let _ = doc.connect(&seq_b, "notes", &join, "2");
        let _ = doc.connect(&join, "out", &voice, "notes");
        let _ = doc.connect(&voice, "out", &delay, "in");
        let _ = doc.connect(&delay, "out", &gain, "in");
        let _ = doc.connect(&gain, "out", &scope, "in");
        let _ = doc.connect(&scope, "out", &out, "in");
        doc
    }

    pub fn spawn_node(&mut self, kind: NodeKind, pos: Vec2) -> String {
        if kind == NodeKind::Output && self.nodes.iter().any(|n| n.kind == NodeKind::Output) {
            return self.output_id.clone();
        }
        let id = format!("n{}", self.next_serial);
        self.next_serial += 1;
        self.nodes.push(GraphNode::new(id.clone(), kind, pos));
        id
    }

    pub fn connect(
        &mut self,
        from: &str,
        from_port: &str,
        to: &str,
        to_port: &str,
    ) -> Result<(), String> {
        let from_kind = self
            .nodes
            .iter()
            .find(|n| n.id == from)
            .map(|n| n.kind)
            .ok_or_else(|| format!("unknown node '{from}'"))?;
        if !self.nodes.iter().any(|n| n.id == to) {
            return Err(format!("unknown node '{to}'"));
        }
        self.space
            .links
            .retain(|l| !(l.to_node == to && l.to_port == to_port));
        let id = self.space.next_link_id;
        self.space.next_link_id += 1;
        self.space.links.push(NodeLink {
            id,
            from_node: from.to_string(),
            from_port: from_port.to_string(),
            to_node: to.to_string(),
            to_port: to_port.to_string(),
            ty: output_port_type(from_kind, from_port),
        });
        Ok(())
    }

    pub fn apply_deletes(&mut self) {
        let mut ids = self.space.take_delete_nodes();
        ids.retain(|id| {
            self.nodes
                .iter()
                .find(|n| n.id == *id)
                .is_some_and(|n| n.kind.can_delete())
        });
        for id in &ids {
            self.space.detach_node(id);
            self.nodes.retain(|n| n.id != *id);
        }
    }

    pub fn apply_clones(&mut self) {
        let ids = self.space.take_clone_nodes();
        if ids.is_empty() {
            return;
        }
        let offset = self.space.clone_offset();
        let mut id_map = std::collections::HashMap::new();
        let mut new_sel = Vec::new();
        for old_id in &ids {
            let Some(src) = self.nodes.iter().find(|n| n.id == *old_id).cloned() else {
                continue;
            };
            if !src.kind.can_delete() {
                continue;
            }
            let new_id = format!("n{}", self.next_serial);
            self.next_serial += 1;
            let mut clone = src;
            clone.id = new_id.clone();
            clone.pos += offset;
            id_map.insert(old_id.clone(), new_id.clone());
            new_sel.push(new_id);
            self.nodes.push(clone);
        }
        self.space.duplicate_links(&id_map);
        self.space.selected_nodes = new_sel;
        self.space.selected_link = None;
    }

    pub fn cue_play(&mut self) {
        self.seek_beats = tick_to_beats(self.play_from);
        self.seek_gen = self.seek_gen.wrapping_add(1);
    }

    pub fn reset_tick(&mut self) {
        self.seek_beats = 0.0;
        self.seek_gen = self.seek_gen.wrapping_add(1);
    }

    pub fn fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.space.links.len().hash(&mut h);
        for l in &self.space.links {
            l.from_node.hash(&mut h);
            l.from_port.hash(&mut h);
            l.to_node.hash(&mut h);
            l.to_port.hash(&mut h);
        }
        for n in &self.nodes {
            n.id.hash(&mut h);
            n.kind.hash(&mut h);
            n.bypass.hash(&mut h);
            n.waveform.hash(&mut h);
            n.freq.to_bits().hash(&mut h);
            n.lfo_rate.to_bits().hash(&mut h);
            n.lfo_depth.to_bits().hash(&mut h);
            n.cutoff.to_bits().hash(&mut h);
            n.q.to_bits().hash(&mut h);
            n.gain.to_bits().hash(&mut h);
            n.drive.to_bits().hash(&mut h);
            n.pulse_width.to_bits().hash(&mut h);
            n.clamp_min.to_bits().hash(&mut h);
            n.clamp_max.to_bits().hash(&mut h);
            n.map_in_min.to_bits().hash(&mut h);
            n.map_in_max.to_bits().hash(&mut h);
            n.map_out_min.to_bits().hash(&mut h);
            n.map_out_max.to_bits().hash(&mut h);
            n.delay_time.to_bits().hash(&mut h);
            n.delay_feedback.to_bits().hash(&mut h);
            n.delay_mix.to_bits().hash(&mut h);
            n.chorus_rate.to_bits().hash(&mut h);
            n.chorus_depth.to_bits().hash(&mut h);
            n.chorus_mix.to_bits().hash(&mut h);
            n.mix_a.to_bits().hash(&mut h);
            n.mix_b.to_bits().hash(&mut h);
            n.seq_when.hash(&mut h);
            n.seq_loop_bars.hash(&mut h);
            n.notes.len().hash(&mut h);
            for note in &n.notes {
                note.step.hash(&mut h);
                note.pitch.hash(&mut h);
                note.len.hash(&mut h);
            }
            n.transpose_notes.hash(&mut h);
            n.transpose_octaves.hash(&mut h);
            n.transpose_steps.to_bits().hash(&mut h);
            n.adsr_attack.to_bits().hash(&mut h);
            n.adsr_decay.to_bits().hash(&mut h);
            n.adsr_sustain.to_bits().hash(&mut h);
            n.adsr_release.to_bits().hash(&mut h);
        }
        self.bpm.to_bits().hash(&mut h);
        h.finish()
    }
}
