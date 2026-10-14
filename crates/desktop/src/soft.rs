//! Ventana sin tarjeta gráfica (máquinas virtuales, escritorios remotos).
//!
//! egui se dibuja con la CPU (egui_software_backend) y la imagen se copia a
//! la ventana con softbuffer, que en Windows usa GDI, como Adobe Reader. Con
//! Direct3D por software (WARP) cada fotograma de una ventana grande tardaba
//! unos 400 ms; así, unos 10 ms.
//!
//! Hace lo que hacía eframe para esta app: una sola ventana, accesibilidad
//! (AccessKit), portapapeles, IME, ficheros arrastrados y cerrar con cambios
//! sin guardar (ViewportCommand::CancelClose).

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, ViewportBuilder, ViewportCommand, ViewportId, ViewportInfo};
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use egui_winit::accesskit_winit;
use egui_winit::winit::application::ApplicationHandler;
use egui_winit::winit::event::WindowEvent;
use egui_winit::winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use egui_winit::winit::window::{Window, WindowId};

use crate::app::PdfApp;

/// Tiempo mínimo entre fotogramas que pide egui (no los de la entrada).
const FRAME_MIN: std::time::Duration = std::time::Duration::from_millis(16);

type MakeApp = Box<dyn FnOnce(&egui::Context) -> PdfApp>;

enum UserEvent {
    /// egui pide otro fotograma (desde cualquier hilo).
    Repaint(Instant),
    AccessKit(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(e: accesskit_winit::Event) -> Self {
        UserEvent::AccessKit(e)
    }
}

struct Win {
    window: Arc<Window>,
    state: egui_winit::State,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    renderer: EguiSoftwareRender,
    info: ViewportInfo,
    shown: bool,
}

struct Runner {
    ctx: egui::Context,
    proxy: EventLoopProxy<UserEvent>,
    viewport: ViewportBuilder,
    make_app: Option<MakeApp>,
    app: Option<PdfApp>,
    win: Option<Win>,
    repaint_at: Option<Instant>,
    last_paint: Instant,
    error: Option<anyhow::Error>,
}

pub fn run(viewport: ViewportBuilder, make_app: impl FnOnce(&egui::Context) -> PdfApp + 'static) -> anyhow::Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let ctx = egui::Context::default();
    {
        let proxy = proxy.clone();
        ctx.set_request_repaint_callback(move |info| {
            let when = Instant::now().checked_add(info.delay).unwrap_or_else(|| Instant::now() + std::time::Duration::from_secs(3600));
            let _ = proxy.send_event(UserEvent::Repaint(when));
        });
    }
    let mut runner = Runner { ctx, proxy, viewport, make_app: Some(Box::new(make_app)), app: None, win: None, repaint_at: None, last_paint: Instant::now(), error: None };
    event_loop.run_app(&mut runner)?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl Runner {
    fn create_window(&mut self, el: &ActiveEventLoop) -> anyhow::Result<()> {
        // Oculta hasta el primer fotograma, como eframe.
        let builder = self.viewport.clone().with_visible(false);
        let window = Arc::new(egui_winit::create_window(&self.ctx, el, &builder)?);
        let mut state = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            el,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(16384),
        );
        state.init_accesskit(el, &window, self.proxy.clone());
        #[cfg(windows)]
        crate::snap::install(&*window, &self.ctx);
        let context = softbuffer::Context::new(window.clone()).map_err(|e| anyhow::anyhow!("softbuffer: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone()).map_err(|e| anyhow::anyhow!("softbuffer: {e}"))?;
        let mut info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &self.ctx, &window, true);
        if self.app.is_none()
            && let Some(make) = self.make_app.take()
        {
            self.app = Some(make(&self.ctx));
        }
        self.win = Some(Win { window, state, surface, renderer: EguiSoftwareRender::new(ColorFieldOrder::Bgra), info, shown: false });
        Ok(())
    }

    /// Un fotograma: la interfaz de la app y su dibujo en la ventana.
    fn paint(&mut self, el: &ActiveEventLoop) -> anyhow::Result<()> {
        let (Some(win), Some(app)) = (self.win.as_mut(), self.app.as_mut()) else { return Ok(()) };
        self.last_paint = Instant::now();
        egui_winit::update_viewport_info(&mut win.info, &self.ctx, &win.window, false);
        let mut raw = win.state.take_egui_input(&win.window);
        raw.viewports = std::iter::once((ViewportId::ROOT, win.info.clone())).collect();
        app.before_frame(&self.ctx, &mut raw);
        let close_requested = raw.viewport().close_requested();
        let out = self.ctx.run_ui(raw, |ui| app.frame(ui));
        win.info.events.clear();
        win.state.handle_platform_output(&win.window, out.platform_output);

        let commands = out.viewport_output.get(&ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default();
        if close_requested && !commands.contains(&ViewportCommand::CancelClose) {
            el.exit();
            return Ok(());
        }
        let mut actions = Vec::new();
        let mut screenshot = None;
        egui_winit::process_viewport_commands(&self.ctx, &mut win.info, commands, &win.window, &mut actions);
        for a in actions {
            let ev = match a {
                egui_winit::ActionRequested::Cut => egui::Event::Cut,
                egui_winit::ActionRequested::Copy => egui::Event::Copy,
                egui_winit::ActionRequested::Paste => match win.state.clipboard_text() {
                    Some(t) => egui::Event::Paste(t),
                    None => continue,
                },
                egui_winit::ActionRequested::Screenshot(data) => {
                    screenshot = Some(data);
                    continue;
                }
            };
            win.state.egui_input_mut().events.push(ev);
        }
        // Una orden (Close) puede haber dejado eventos para el siguiente.
        if !win.info.events.is_empty() {
            win.window.request_redraw();
        }

        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let size = win.window.inner_size();
        match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            (Some(w), Some(h)) => {
                win.surface.resize(w, h).map_err(|e| anyhow::anyhow!("softbuffer: {e}"))?;
                let mut buffer = win.surface.buffer_mut().map_err(|e| anyhow::anyhow!("softbuffer: {e}"))?;
                buffer.fill(0);
                let pixels: &mut [[u8; 4]] = bytemuck::cast_slice_mut(&mut buffer);
                let mut target = BufferMutRef::new(pixels, w.get() as usize, h.get() as usize);
                win.renderer.render(&mut target, &prims, &out.textures_delta, out.pixels_per_point);
                if let Some(user_data) = screenshot {
                    let px = bytemuck::cast_slice::<u32, [u8; 4]>(&buffer).iter().map(|p| egui::Color32::from_rgb(p[2], p[1], p[0])).collect();
                    let image = Arc::new(egui::ColorImage::new([w.get() as usize, h.get() as usize], px));
                    win.state.egui_input_mut().events.push(egui::Event::Screenshot { viewport_id: ViewportId::ROOT, user_data, image });
                    win.window.request_redraw();
                }
                buffer.present().map_err(|e| anyhow::anyhow!("softbuffer: {e}"))?;
            }
            // Minimizada: las texturas se actualizan igualmente.
            _ => {
                let mut px = [[0u8; 4]; 1];
                let mut target = BufferMutRef::new(&mut px, 1, 1);
                win.renderer.render(&mut target, &prims, &out.textures_delta, out.pixels_per_point);
            }
        }
        if !win.shown {
            win.shown = true;
            win.window.set_visible(true);
        }
        Ok(())
    }

    fn fail(&mut self, el: &ActiveEventLoop, e: anyhow::Error) {
        log::error!("{e:#}");
        self.error = Some(e);
        el.exit();
    }
}

impl ApplicationHandler<UserEvent> for Runner {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.win.is_some() {
            return;
        }
        // Primer fotograma ya: una ventana oculta no recibe RedrawRequested
        // en Windows, y se muestra al acabar de dibujarlo.
        if let Err(e) = self.create_window(el).and_then(|()| self.paint(el)) {
            self.fail(el, e);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let WindowEvent::RedrawRequested = event {
            if let Err(e) = self.paint(el) {
                self.fail(el, e);
            }
            return;
        }
        let Some(win) = self.win.as_mut() else { return };
        let response = win.state.on_window_event(&win.window, &event);
        match event {
            // La app decide (cambios sin guardar) en el siguiente fotograma.
            WindowEvent::CloseRequested => {
                win.info.events.push(egui::ViewportEvent::Close);
                win.window.request_redraw();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => win.window.request_redraw(),
            _ => {}
        }
        if response.repaint {
            win.window.request_redraw();
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Repaint(when) => {
                // Las animaciones de egui piden fotogramas seguidos: como mucho
                // unos 60 por segundo (la entrada del usuario no espera).
                let when = when.max(self.last_paint + FRAME_MIN);
                self.repaint_at = Some(self.repaint_at.map_or(when, |t| t.min(when)));
            }
            UserEvent::AccessKit(e) => {
                let Some(win) = self.win.as_mut() else { return };
                match e.window_event {
                    accesskit_winit::WindowEvent::InitialTreeRequested => {
                        self.ctx.enable_accesskit();
                        win.window.request_redraw();
                    }
                    accesskit_winit::WindowEvent::ActionRequested(request) => {
                        win.state.on_accesskit_action_request(request);
                        win.window.request_redraw();
                    }
                    accesskit_winit::WindowEvent::AccessibilityDeactivated => self.ctx.disable_accesskit(),
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match self.repaint_at {
            Some(t) if t <= Instant::now() => {
                self.repaint_at = None;
                if let Some(win) = &self.win {
                    win.window.request_redraw();
                }
                el.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }
}
