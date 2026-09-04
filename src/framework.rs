//! Shared winit + wgpu host for mega-ui examples.
//!
//! Rendering goes through [`mega_ui::wgpu::UiRenderer`]. This file only owns the
//! window, surface, input mapping, and event loop.

use std::sync::Arc;
use std::time::Instant;

use glam::Vec2;
use mega_ui::wgpu::UiRenderer;
use mega_ui::{CursorIcon, Ui, UiInput};

pub use mega_ui::wgpu::DrawStats;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{CursorGrabMode, Window as WinitWindow, WindowId};

/// "Сырые" физические клавиши за этот кадр — независимо от того, съел ли их
/// UI (текстовые поля и т.п.). Нужны демкам вроде синтезатора, где клавиатура
/// компьютера играет роль миди-клавиатуры.
#[derive(Default, Clone)]
pub struct KeyEvents {
    pub pressed: Vec<KeyCode>,
    pub released: Vec<KeyCode>,
    /// Зажата ли левая кнопка мыши прямо сейчас (для press-and-hold виджетов
    /// вроде клавиш пианино, где `Response::clicked` из `ui.button` не подходит —
    /// он срабатывает только один раз на отпускание).
    pub mouse_down: bool,
    pub save: bool,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: UiRenderer,
}

#[derive(Default)]
pub struct FrameInput {
    pub mouse_pos: Vec2,
    pub mouse_down: bool,
    mouse_pressed: bool,
    mouse_released: bool,
    mouse_right_down: bool,
    mouse_right_pressed: bool,
    mouse_right_released: bool,
    mouse_middle_down: bool,
    mouse_middle_pressed: bool,
    mouse_middle_released: bool,
    scroll_delta: Vec2,
    mouse_delta: Vec2,
    text: String,
    key_backspace: bool,
    key_delete: bool,
    key_enter: bool,
    key_left: bool,
    key_right: bool,
    key_up: bool,
    key_down: bool,
    key_home: bool,
    key_end: bool,
    key_shift: bool,
    key_ctrl: bool,
    key_copy: bool,
    key_paste: bool,
    key_cut: bool,
    key_select_all: bool,
    key_duplicate: bool,
    modifiers: winit::keyboard::ModifiersState,
    /// System/internal clipboard text for this frame's paste.
    clipboard_paste: String,
}

impl FrameInput {
    fn clear_edges(&mut self) {
        self.mouse_pressed = false;
        self.mouse_released = false;
        self.mouse_right_pressed = false;
        self.mouse_right_released = false;
        self.mouse_middle_pressed = false;
        self.mouse_middle_released = false;
        self.scroll_delta = Vec2::ZERO;
        self.mouse_delta = Vec2::ZERO;
        self.text.clear();
        self.key_backspace = false;
        self.key_delete = false;
        self.key_enter = false;
        self.key_left = false;
        self.key_right = false;
        self.key_up = false;
        self.key_down = false;
        self.key_home = false;
        self.key_end = false;
        self.key_copy = false;
        self.key_paste = false;
        self.key_cut = false;
        self.key_select_all = false;
        self.key_duplicate = false;
        self.clipboard_paste.clear();
    }

    fn shortcut_mod(&self) -> bool {
        self.modifiers.control_key() || self.modifiers.super_key()
    }

    pub fn to_ui(&self, viewport: Vec2, dt: f32) -> UiInput {
        UiInput {
            mouse_pos: self.mouse_pos,
            mouse_down: self.mouse_down,
            mouse_pressed: self.mouse_pressed,
            mouse_released: self.mouse_released,
            mouse_right_down: self.mouse_right_down,
            mouse_right_pressed: self.mouse_right_pressed,
            mouse_right_released: self.mouse_right_released,
            mouse_middle_down: self.mouse_middle_down,
            mouse_middle_pressed: self.mouse_middle_pressed,
            mouse_middle_released: self.mouse_middle_released,
            viewport,
            scroll_delta: self.scroll_delta,
            mouse_delta: self.mouse_delta,
            dt,
            text: self.text.clone(),
            key_backspace: self.key_backspace,
            key_delete: self.key_delete,
            key_enter: self.key_enter,
            key_left: self.key_left,
            key_right: self.key_right,
            key_up: self.key_up,
            key_down: self.key_down,
            key_home: self.key_home,
            key_end: self.key_end,
            key_shift: self.key_shift,
            // Treat Cmd (macOS) like Ctrl so widgets suppress character insert on shortcuts.
            key_ctrl: self.key_ctrl || self.modifiers.super_key(),
            key_copy: self.key_copy,
            key_paste: self.key_paste,
            key_cut: self.key_cut,
            key_select_all: self.key_select_all,
            key_duplicate: self.key_duplicate,
            clipboard: self.clipboard_paste.clone(),
        }
    }
}

/// Per-example UI scene.
pub trait Scene {
    fn title() -> &'static str;
    fn window_size() -> (f64, f64) {
        (960.0, 640.0)
    }
    /// Called once after `Ui::new`.
    fn init(_ui: &mut Ui) {}
    /// Build widgets for this frame. Return `true` to keep redrawing.
    /// `stats` is from the previous frame's draw list (commands / GPU batches / quads).
    /// `keys` — физические клавиши, нажатые/отпущенные за этот кадр (см. [`KeyEvents`]).
    fn build(
        ui: &mut Ui,
        state: &mut Self,
        viewport: Vec2,
        dt: f32,
        stats: DrawStats,
        keys: &KeyEvents,
    ) -> bool;
}

pub struct Host<S: Scene> {
    window: Option<Arc<WinitWindow>>,
    gpu: Option<Gpu>,
    ui: Ui,
    state: S,
    input: FrameInput,
    last_frame: Instant,
    cursor: CursorIcon,
    cursor_visible: bool,
    cursor_grabbed: bool,
    cursor_lock_pos: Option<Vec2>,
    draw_stats: DrawStats,
    clipboard: Option<arboard::Clipboard>,
    key_events: KeyEvents,
}

impl<S: Scene> Host<S> {
    pub fn new(state: S) -> Self {
        let mut ui = Ui::new();
        S::init(&mut ui);
        Self {
            window: None,
            gpu: None,
            ui,
            state,
            input: FrameInput::default(),
            last_frame: Instant::now(),
            cursor: CursorIcon::Default,
            cursor_visible: true,
            cursor_grabbed: false,
            cursor_lock_pos: None,
            draw_stats: DrawStats::default(),
            clipboard: arboard::Clipboard::new().ok(),
            key_events: KeyEvents::default(),
        }
    }

    pub fn run(state: S) {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
        let event_loop = EventLoop::new().expect("event loop");
        event_loop.set_control_flow(ControlFlow::Wait);
        let mut host = Self::new(state);
        event_loop.run_app(&mut host).expect("run app");
    }

    fn init_gpu(&mut self, window: Arc<WinitWindow>) {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("no suitable GPU adapter");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mega-ui demo"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::default(),
        }))
        .expect("request_device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let renderer = UiRenderer::new(&device, &queue, format, &self.ui);
        renderer.set_viewport(&queue, width as f32, height as f32);

        self.gpu = Some(Gpu {
            device,
            queue,
            surface,
            config,
            renderer,
        });
        self.window = Some(window);
    }

    fn resize(&mut self, width: u32, height: u32) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        gpu.config.width = width;
        gpu.config.height = height;
        gpu.surface.configure(&gpu.device, &gpu.config);
        gpu.renderer
            .set_viewport(&gpu.queue, width as f32, height as f32);
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }

        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        let viewport = Vec2::new(size.width as f32, size.height as f32);
        let input = self.input.to_ui(viewport, dt);
        self.key_events.mouse_down = self.input.mouse_down;
        self.ui.begin_frame(input);
        let keep = S::build(
            &mut self.ui,
            &mut self.state,
            viewport,
            dt,
            self.draw_stats,
            &self.key_events,
        );
        self.key_events.pressed.clear();
        self.key_events.released.clear();
        self.key_events.save = false;
        let out = self.ui.end_frame();
        let needs_repaint = out.needs_repaint || keep;

        if let Some(text) = out.clipboard {
            if let Some(cb) = self.clipboard.as_mut() {
                let _ = cb.set_text(text);
            }
        }

        self.apply_cursor(&window, out.cursor, !out.hide_cursor, out.cursor_anchor);

        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        gpu.renderer
            .sync_atlases(&gpu.device, &gpu.queue, &mut self.ui);

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => {
                window.request_redraw();
                return;
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ui frame"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04,
                            g: 0.04,
                            b: 0.04,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.draw_stats = gpu.renderer.draw(&gpu.queue, &mut pass, &out.draw_list);
        }

        gpu.queue.submit(Some(encoder.finish()));
        gpu.queue.present(frame);
        self.input.clear_edges();

        if needs_repaint {
            window.request_redraw();
        }
    }

    fn begin_paste(&mut self) {
        self.input.key_paste = true;
        if !self.input.clipboard_paste.is_empty() {
            return;
        }
        if let Some(cb) = self.clipboard.as_mut() {
            if let Ok(text) = cb.get_text() {
                self.input.clipboard_paste = text;
            }
        }
    }

    fn apply_cursor(
        &mut self,
        window: &WinitWindow,
        cursor: CursorIcon,
        visible: bool,
        anchor: Option<Vec2>,
    ) {
        if !visible {
            if let Some(p) = anchor {
                self.cursor_lock_pos = Some(p);
            }
            if !self.cursor_grabbed {
                let locked = window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
                self.cursor_grabbed = locked.is_ok();
            }
            if self.cursor_visible {
                window.set_cursor_visible(false);
                self.cursor_visible = false;
            }
            return;
        }

        if self.cursor_grabbed || !self.cursor_visible {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            self.cursor_grabbed = false;
            if let Some(p) = self.cursor_lock_pos.take().or(anchor) {
                let _ = window.set_cursor_position(PhysicalPosition::new(p.x as f64, p.y as f64));
                self.input.mouse_pos = p;
            }
            window.set_cursor_visible(true);
            self.cursor_visible = true;
            self.cursor = cursor;
            window.set_cursor(map_cursor(cursor));
            return;
        }

        if cursor == self.cursor {
            return;
        }
        self.cursor = cursor;
        window.set_cursor(map_cursor(cursor));
    }
}

impl<S: Scene> ApplicationHandler for Host<S> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (w, h) = S::window_size();
        let attrs = WinitWindow::default_attributes()
            .with_title(S::title())
            .with_inner_size(LogicalSize::new(w, h));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        self.init_gpu(window.clone());
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.resize(size.width, size.height);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = size {
                    self.resize(size.width, size.height);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.input.mouse_pos = Vec2::new(position.x as f32, position.y as f32);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                match button {
                    MouseButton::Left => {
                        if down && !self.input.mouse_down {
                            self.input.mouse_pressed = true;
                        }
                        if !down && self.input.mouse_down {
                            self.input.mouse_released = true;
                        }
                        self.input.mouse_down = down;
                    }
                    MouseButton::Right => {
                        if down && !self.input.mouse_right_down {
                            self.input.mouse_right_pressed = true;
                        }
                        if !down && self.input.mouse_right_down {
                            self.input.mouse_right_released = true;
                        }
                        self.input.mouse_right_down = down;
                    }
                    MouseButton::Middle => {
                        if down && !self.input.mouse_middle_down {
                            self.input.mouse_middle_pressed = true;
                        }
                        if !down && self.input.mouse_middle_down {
                            self.input.mouse_middle_released = true;
                        }
                        self.input.mouse_middle_down = down;
                    }
                    _ => {}
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.scroll_delta = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(x * 40.0, y * 40.0),
                    MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x as f32, p.y as f32),
                };
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.input.modifiers = mods.state();
                self.input.key_shift = mods.state().shift_key();
                self.input.key_ctrl = mods.state().control_key();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed if !event.repeat => self.key_events.pressed.push(code),
                        ElementState::Released => self.key_events.released.push(code),
                        _ => {}
                    }
                }
                if event.state != ElementState::Pressed {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                let shortcut = self.input.shortcut_mod();
                match &event.logical_key {
                    Key::Named(NamedKey::Backspace) => self.input.key_backspace = true,
                    Key::Named(NamedKey::Delete) => self.input.key_delete = true,
                    Key::Named(NamedKey::Enter) => self.input.key_enter = true,
                    Key::Named(NamedKey::ArrowLeft) => self.input.key_left = true,
                    Key::Named(NamedKey::ArrowRight) => self.input.key_right = true,
                    Key::Named(NamedKey::ArrowUp) => self.input.key_up = true,
                    Key::Named(NamedKey::ArrowDown) => self.input.key_down = true,
                    Key::Named(NamedKey::Home) => self.input.key_home = true,
                    Key::Named(NamedKey::End) => self.input.key_end = true,
                    Key::Character(c) if shortcut => match c.to_lowercase().as_str() {
                        "c" => self.input.key_copy = true,
                        "v" => self.begin_paste(),
                        "x" => self.input.key_cut = true,
                        "a" => self.input.key_select_all = true,
                        "d" => self.input.key_duplicate = true,
                        "s" => self.key_events.save = true,
                        _ => {}
                    },
                    _ => {}
                }
                // Physical keys: more reliable with Ctrl/Cmd across platforms.
                if shortcut {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        match code {
                            KeyCode::KeyC => self.input.key_copy = true,
                            KeyCode::KeyV => self.begin_paste(),
                            KeyCode::KeyX => self.input.key_cut = true,
                            KeyCode::KeyA => self.input.key_select_all = true,
                            KeyCode::KeyD => self.input.key_duplicate = true,
                            KeyCode::KeyS => self.key_events.save = true,
                            _ => {}
                        }
                    }
                }
                if !shortcut {
                    if let Some(text) = event.text.as_ref() {
                        for ch in text.chars() {
                            if !ch.is_control() {
                                self.input.text.push(ch);
                            }
                        }
                    }
                }
                if let PhysicalKey::Code(KeyCode::Escape) = event.physical_key {
                    event_loop.exit();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                self.input.text.push_str(&text);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.mouse_delta.x += delta.0 as f32;
            self.input.mouse_delta.y += delta.1 as f32;
            if self.cursor_grabbed {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }
}

fn map_cursor(icon: CursorIcon) -> winit::window::CursorIcon {
    match icon {
        CursorIcon::Default => winit::window::CursorIcon::Default,
        CursorIcon::Pointer => winit::window::CursorIcon::Pointer,
        CursorIcon::Move => winit::window::CursorIcon::Move,
        CursorIcon::ResizeNwse => winit::window::CursorIcon::NwseResize,
        CursorIcon::ResizeEw => winit::window::CursorIcon::EwResize,
        CursorIcon::ResizeNs => winit::window::CursorIcon::NsResize,
        CursorIcon::Text => winit::window::CursorIcon::Text,
    }
}
