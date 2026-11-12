// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Lightweight CPU window path, used when the computer has no real GPU (VMs, remote desktop,
//! servers). There, Windows' software Direct3D driver needs ~200 ms per frame for this window;
//! egui's own tile-cached CPU rasterizer draws it in a few milliseconds, and the frame is shown
//! with a plain bitmap copy (softbuffer). Window moves don't cause redraws at all here: the
//! system moves the existing pixels, exactly like an ordinary Win32 window.

use crate::app::HexApp;
use eframe::egui::{self, ViewportId};
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

#[derive(Debug)]
enum UserEvent {
    Repaint(Duration),
}

struct Gfx {
    window: Arc<Window>,
    state: egui_winit::State,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    _context: softbuffer::Context<Arc<Window>>,
}

struct Soft {
    ctx: egui::Context,
    app: HexApp,
    builder: egui::ViewportBuilder,
    gfx: Option<Gfx>,
    painter: EguiSoftwareRender,
    info: egui::ViewportInfo,
    next_repaint: Option<Instant>,
    last_cpu: Option<f32>,
    exit: bool,
}

/// Background colour behind egui (BGRA in a u32: 0x00RRGGBB).
const CLEAR: u32 = 0x0004_0709;

impl Soft {
    fn paint(&mut self, event_loop: &ActiveEventLoop) {
        let Some(g) = self.gfx.as_mut() else { return };
        let t0 = Instant::now();
        let size = g.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return; // minimised
        }
        let raw = g.state.take_egui_input(&g.window);
        let app = &mut self.app;
        let cpu = self.last_cpu;
        let out = self.ctx.run_ui(raw, |ui| app.draw(ui, cpu));
        g.state.handle_platform_output(&g.window, out.platform_output);
        let ppp = out.pixels_per_point;
        let prims = self.ctx.tessellate(out.shapes, ppp);
        let mut textures = out.textures_delta;

        let (w, h) = (size.width, size.height);
        if g.surface.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap()).is_err() {
            textures.clear();
            return;
        }
        let Ok(mut buf) = g.surface.buffer_mut() else {
            textures.clear();
            return;
        };
        buf.fill(CLEAR);
        {
            // softbuffer pixels are 0x00RRGGBB, i.e. bytes B,G,R,0 in memory
            let px: &mut [[u8; 4]] = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut [u8; 4], buf.len()) };
            let mut target = BufferMutRef::new(px, w as usize, h as usize);
            self.painter.render(&mut target, &prims, &textures, ppp);
        }
        textures.clear(); // applied by the painter above

        // window commands (title, close, screenshot...)
        let mut actions = Vec::new();
        if let Some(vo) = out.viewport_output.get(&ViewportId::ROOT) {
            egui_winit::process_viewport_commands(&self.ctx, &mut self.info, vo.commands.clone(), &g.window, &mut actions);
            if vo.repaint_delay.is_zero() {
                g.window.request_redraw();
            } else if vo.repaint_delay < Duration::from_secs(3600) {
                let at = Instant::now() + vo.repaint_delay;
                self.next_repaint = Some(self.next_repaint.map_or(at, |t| t.min(at)));
            }
        }
        for a in actions {
            if let egui_winit::ActionRequested::Screenshot(cb) = a {
                let mut rgba = Vec::with_capacity(buf.len() * 4);
                for p in buf.iter() {
                    rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
                }
                cb.complete(Arc::new(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba)));
            }
        }
        let _ = buf.present();
        if self.info.events.iter().any(|e| matches!(e, egui::ViewportEvent::Close)) {
            self.exit = true;
            event_loop.exit();
        }
        self.info.events.clear();
        self.last_cpu = Some(t0.elapsed().as_secs_f32());
    }
}

impl ApplicationHandler<UserEvent> for Soft {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let window = match egui_winit::create_window(&self.ctx, event_loop, &self.builder) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("hexiso: cannot create the window: {}", e);
                event_loop.exit();
                return;
            }
        };
        let state = egui_winit::State::new(self.ctx.clone(), ViewportId::ROOT, &window, Some(window.scale_factor() as f32), window.theme(), None);
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("hexiso: cannot draw: {}", e);
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("hexiso: cannot draw: {}", e);
                event_loop.exit();
                return;
            }
        };
        window.request_redraw();
        self.gfx = Some(Gfx { window, state, surface, _context: context });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match &event {
            // nothing on screen changes when the window moves: let the system move the pixels
            WindowEvent::Moved(_) => return,
            WindowEvent::CloseRequested => {
                self.exit = true;
                event_loop.exit();
                return;
            }
            WindowEvent::RedrawRequested => {
                self.paint(event_loop);
                return;
            }
            _ => {}
        }
        if let Some(g) = self.gfx.as_mut() {
            let r = g.state.on_window_event(&g.window, &event);
            if r.repaint || matches!(event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
                g.window.request_redraw();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Repaint(delay) = event;
        if delay.is_zero() {
            if let Some(g) = &self.gfx {
                g.window.request_redraw();
            }
        } else if delay < Duration::from_secs(3600) {
            let at = Instant::now() + delay;
            self.next_repaint = Some(self.next_repaint.map_or(at, |t| t.min(at)));
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.exit {
            return;
        }
        match self.next_repaint {
            Some(t) if Instant::now() >= t => {
                self.next_repaint = None;
                if let Some(g) = &self.gfx {
                    g.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// Run the window with the CPU renderer. Returns when the window closes.
pub fn run(builder: egui::ViewportBuilder, make_app: impl FnOnce(&egui::Context) -> HexApp) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| e.to_string())?;
    let ctx = egui::Context::default();
    let proxy = event_loop.create_proxy();
    ctx.set_request_repaint_callback(move |info| {
        let _ = proxy.send_event(UserEvent::Repaint(info.delay));
    });
    crate::theme::apply(&ctx);
    let mut app = make_app(&ctx);
    app.set_renderer("CPU renderer (built-in, no GPU needed)".into());
    let mut soft = Soft {
        ctx,
        app,
        builder,
        gfx: None,
        painter: EguiSoftwareRender::new(ColorFieldOrder::Bgra),
        info: egui::ViewportInfo::default(),
        next_repaint: None,
        last_cpu: None,
        exit: false,
    };
    event_loop.run_app(&mut soft).map_err(|e| e.to_string())
}

/// True when the computer has a real (hardware) GPU that wgpu can use.
pub fn has_hardware_gpu() -> bool {
    use eframe::wgpu;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::PRIMARY));
    adapters.iter().any(|a| {
        let i = a.get_info();
        let n = i.name.to_lowercase();
        !matches!(i.device_type, wgpu::DeviceType::Cpu) && !n.contains("basic render") && !n.contains("llvmpipe") && !n.contains("swiftshader") && !n.contains("warp")
    })
}
