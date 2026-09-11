use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::frame::{Frame, NOTES_H, NOTE_TOP};
use super::layout::{note_quads, wave_quads};
use super::text::{overlay_quads, overlay_reserve};

const MAX_NOTES: usize = 8192;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    size: [f32; 2],
    gonio_scale: [f32; 2],
    decay: f32,
    _pad: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct NoteInst {
    rect: [f32; 4],
    color: [f32; 4],
    extra: [f32; 4],
}

struct ColorOut {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
}

/// Renders a [`Frame`] into an offscreen color target (dock tab or later MP4).
pub struct Renderer {
    format: wgpu::TextureFormat,
    note_pipe: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    note_buf: wgpu::Buffer,
    color: Option<ColorOut>,
    color_w: u32,
    color_h: u32,
    #[allow(dead_code)]
    dummy: wgpu::Texture,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viz"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viz bind"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viz layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let note_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viz notes"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_note"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<NoteInst>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_note"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viz uniforms"),
            contents: bytemuck::bytes_of(&Uniforms {
                size: [1.0, 1.0],
                gonio_scale: [1.0, 1.0],
                decay: 0.0,
                _pad: [0.0; 3],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viz dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let dummy_view = dummy.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viz bind"),
            layout: &bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&dummy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let note_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viz notes"),
            size: (MAX_NOTES * std::mem::size_of::<NoteInst>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            format,
            note_pipe,
            bind,
            uniform_buf,
            note_buf,
            color: None,
            color_w: 0,
            color_h: 0,
            dummy,
        }
    }

    fn ensure_color(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        if self.color.is_some() && self.color_w == w && self.color_h == h {
            return;
        }
        self.color_w = w.max(1);
        self.color_h = h.max(1);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viz color"),
            size: wgpu::Extent3d {
                width: self.color_w,
                height: self.color_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.color = Some(ColorOut { tex, view });
    }

    pub fn color_view(&self) -> Option<wgpu::TextureView> {
        self.color.as_ref().map(|c| c.view.clone())
    }

    /// Draw `frame` into the offscreen color target (`color_view`).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
        frame: &Frame,
    ) {
        let w = width.max(1);
        let h = height.max(1);
        self.ensure_color(device, w, h);
        let notes_h = if frame.waves.is_empty() { 1.0 } else { NOTES_H };
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&Uniforms {
                size: [w as f32, h as f32],
                gonio_scale: [1.0, 1.0],
                decay: 0.0,
                _pad: [0.0; 3],
            }),
        );

        let top = overlay_reserve(&frame.title, &frame.credit).max(NOTE_TOP);
        let mut quads = note_quads(
            &frame.notes,
            frame.now_beats,
            frame.window_beats,
            notes_h,
            top,
        );
        quads.extend(wave_quads(&frame.waves, notes_h));
        let aspect = w as f32 / h as f32;
        let overlay = overlay_quads(&frame.title, &frame.credit, aspect);
        let note_cap = MAX_NOTES.saturating_sub(overlay.len());
        if quads.len() > note_cap {
            quads.truncate(note_cap);
        }
        quads.extend(overlay);
        let notes: Vec<NoteInst> = quads
            .iter()
            .take(MAX_NOTES)
            .map(|q| NoteInst {
                rect: [q.x, q.y, q.w, q.h],
                color: q.color,
                extra: [q.round, q.glow, q.stroke, 0.0],
            })
            .collect();
        let note_n = notes.len();
        if note_n > 0 {
            queue.write_buffer(&self.note_buf, 0, bytemuck::cast_slice(&notes));
        }

        let Some(color) = self.color.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viz out"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if note_n > 0 {
            pass.set_pipeline(&self.note_pipe);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.note_buf.slice(..));
            pass.draw(0..6, 0..note_n as u32);
        }
    }

    /// Render and copy RGBA8 (unpadded rows) for ffmpeg / files.
    pub fn render_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        frame: &Frame,
    ) -> Result<Vec<u8>, String> {
        let w = width.max(1);
        let h = height.max(1);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viz rgba"),
        });
        self.render(device, queue, &mut encoder, w, h, frame);
        let Some(color) = self.color.as_ref() else {
            return Err("viz color target missing".into());
        };
        let padded = padded_bytes_per_row(w);
        let buf_size = padded as u64 * h as u64;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viz readback"),
            size: buf_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &color.tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| ());
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let data = slice
            .get_mapped_range()
            .map_err(|e| format!("map viz: {e:?}"))?;
        let src_stride = padded as usize;
        let dst_stride = (w * 4) as usize;
        let mut out = vec![0u8; dst_stride * h as usize];
        for y in 0..h as usize {
            let s = y * src_stride;
            let d = y * dst_stride;
            out[d..d + dst_stride].copy_from_slice(&data[s..s + dst_stride]);
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }
}

fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width.saturating_mul(4);
    unpadded.div_ceil(256) * 256
}
