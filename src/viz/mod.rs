//! Fullscreen note visualizer (wgpu). CPU snapshot in [`frame`], GPU in [`gpu`].
//!
//! A frame is just beats + notes + gonio samples, so the same path can later
//! render into a texture for MP4.

mod frame;
mod gpu;
mod layout;

pub use frame::{Frame, Note, Wave, MAX_WAVES, WAVE_BINS, WINDOW_BEATS};
pub use gpu::Renderer;
pub use layout::downsample;

/// Host texture slot for the Visual dock tab (`Ui::texture`).
pub const TEX_SLOT: u32 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounding_uses_loop_and_when() {
        let n = Note {
            pitch: 60,
            start: 0.0,
            dur: 1.0,
            loop_len: 4.0,
            color: [1.0; 4],
            when: vec![(0.0, 8.0)],
            lane: 0,
            wave: String::new(),
        };
        assert!(n.sounding_at(0.5));
        assert!(!n.sounding_at(1.5));
        assert!(n.sounding_at(4.2));
        assert!(!n.sounding_at(10.0));
    }

    #[test]
    fn shaders_parse() {
        naga::front::wgsl::parse_str(include_str!("shaders.wgsl")).expect("viz wgsl");
    }
}
