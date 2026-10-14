//! Barra de título propia, como Edge o las apps de Windows 11: las pestañas
//! de los documentos, un botón para abrir otro y los botones de la ventana.
//! La ventana no tiene la decoración del sistema (ver main.rs), así que aquí
//! también se mueve, se maximiza con doble clic y se redimensiona por los
//! bordes.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
    ViewportCommand, viewport::ResizeDirection,
};

use crate::theme::{self, icon};

pub const BAR_HEIGHT: f32 = 40.0;
const BUTTON_W: f32 = 46.0;

pub struct TabInfo {
    pub name: String,
    pub tooltip: String,
    pub dirty: bool,
}

pub enum TabAction {
    Select(usize),
    Close(usize),
    Open,
}

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false))
}

/// La barra entera. Devuelve lo que se ha pulsado en las pestañas.
///
/// El panel solo le reserva el sitio: la barra va en una capa propia por
/// encima de los diálogos, para que con uno abierto la ventana se pueda
/// seguir moviendo, minimizar o cerrar, como con las ventanas del sistema.
pub fn title_bar(ui: &mut Ui, tabs: &[TabInfo], active: usize) -> Option<TabAction> {
    let bar = egui::Panel::top("barra_titulo")
        .exact_size(BAR_HEIGHT)
        .show_separator_line(false)
        .frame(egui::Frame::NONE)
        .show(ui, |ui| ui.max_rect())
        .inner;
    let ctx = ui.ctx().clone();
    let mut action = None;
    let layer = egui::Area::new(Id::new("capa_barra_titulo"))
        .order(Order::Foreground)
        .fixed_pos(bar.min)
        .constrain(false)
        .show(&ctx, |ui| {
            ui.set_clip_rect(bar);
            ui.allocate_rect(bar, Sense::hover());
            action = bar_contents(ui, bar, tabs, active);
        })
        .response
        .layer_id;
    ctx.move_to_top(layer);
    action
}

fn bar_contents(ui: &mut Ui, bar: Rect, tabs: &[TabInfo], active: usize) -> Option<TabAction> {
    let p = theme::palette(ui);
    let mut action = None;
    let ctx = ui.ctx().clone();
    let painter = ui.painter().clone();
    painter.rect_filled(bar, 0.0, p.title_bar);
    let controls_left = bar.right() - 3.0 * BUTTON_W;

    // Icono de la aplicación.
    let mut x = bar.left() + 12.0;
    painter.text(Pos2::new(x, bar.center().y), Align2::LEFT_CENTER, icon::FILE_PDF, FontId::proportional(18.0), Color32::from_rgb(0xD9, 0x34, 0x2B));
    x += 28.0;

    // Pestañas: se encogen si no caben.
    let plus_w = 34.0;
    let room = (controls_left - x - plus_w - 48.0).max(0.0);
    let tab_w = if tabs.is_empty() { 0.0 } else { (room / tabs.len() as f32).clamp(72.0, 230.0) };
    let top = bar.top() + 6.0;
    for (i, t) in tabs.iter().enumerate() {
        let r = Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + tab_w, bar.bottom()));
        if let Some(a) = tab(ui, i, t, i == active, r) {
            action = Some(a);
        }
        x += tab_w + 2.0;
    }
    if tabs.is_empty() {
        painter.text(Pos2::new(x, bar.center().y), Align2::LEFT_CENTER, "Form PDF Reader", FontId::proportional(13.0), p.text_weak);
        x += 130.0;
    }

    // Abrir otro documento.
    let plus = Rect::from_center_size(Pos2::new(x + plus_w / 2.0, top + (bar.bottom() - top) / 2.0 - 2.0), Vec2::splat(28.0));
    let r = ui.interact(plus, Id::new("pestaña_nueva"), Sense::click()).on_hover_text(form_pdf_reader::t!("toolbar.open"));
    if r.hovered() {
        painter.rect_filled(plus, 6.0, p.hover);
    }
    painter.text(plus.center(), Align2::CENTER_CENTER, icon::PLUS, FontId::proportional(16.0), p.text);
    if r.clicked() {
        action = Some(TabAction::Open);
    }
    x = plus.right() + 4.0;

    // El resto de la barra mueve la ventana; doble clic, maximizar.
    let drag = Rect::from_min_max(Pos2::new(x, bar.top()), Pos2::new(controls_left, bar.bottom()));
    if drag.width() > 0.0 {
        let r = ui.interact(drag, Id::new("mover_ventana"), Sense::click_and_drag());
        if r.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(&ctx)));
        } else if r.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
    }

    window_buttons(ui, Rect::from_min_max(Pos2::new(controls_left, bar.top()), bar.max));
    action
}

fn tab(ui: &mut Ui, i: usize, t: &TabInfo, active: bool, r: Rect) -> Option<TabAction> {
    let p = theme::palette(ui);
    let painter = ui.painter().clone();
    let id = Id::new(("pestaña", i));
    let resp = ui.interact(r, id, Sense::click()).on_hover_text(&t.tooltip);
    let hovered = resp.hovered();
    let close = Rect::from_center_size(Pos2::new(r.right() - 18.0, r.center().y - 2.0), Vec2::splat(22.0));
    let close_resp = ui.interact(close, id.with("cerrar"), Sense::click()).on_hover_text(form_pdf_reader::t!("tab.close"));

    if active {
        let cr = CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 };
        painter.rect(r, cr, p.surface, Stroke::new(1.0, p.border), StrokeKind::Inside);
        // Se une con la barra de herramientas de abajo.
        painter.rect_filled(Rect::from_min_max(Pos2::new(r.left() + 1.0, r.bottom() - 2.0), Pos2::new(r.right() - 1.0, r.bottom() + 1.0)), 0.0, p.surface);
    } else if hovered || close_resp.hovered() {
        painter.rect_filled(r.shrink2(Vec2::new(0.0, 3.0)).translate(Vec2::new(0.0, -2.0)), 7.0, p.hover);
    }

    let cy = r.center().y - 2.0;
    painter.text(Pos2::new(r.left() + 12.0, cy), Align2::LEFT_CENTER, icon::FILE_PDF, FontId::proportional(15.0), Color32::from_rgb(0xD9, 0x34, 0x2B));
    let mut text_x = r.left() + 34.0;
    if t.dirty {
        painter.circle_filled(Pos2::new(text_x + 3.0, cy), 3.5, p.warn);
        text_x += 12.0;
    }
    let show_close = active || hovered || close_resp.hovered();
    let max_w = (if show_close { close.left() - 4.0 } else { r.right() - 10.0 }) - text_x;
    if max_w > 8.0 {
        let mut job = egui::text::LayoutJob::simple_singleline(t.name.clone(), FontId::proportional(13.0), if active { p.text } else { p.text_weak });
        job.wrap.max_width = max_w;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(Pos2::new(text_x, cy - galley.size().y / 2.0), galley, p.text);
    }
    if show_close {
        if close_resp.hovered() {
            painter.rect_filled(close, 5.0, p.pressed);
        }
        painter.text(close.center(), Align2::CENTER_CENTER, icon::X, FontId::proportional(13.0), p.text_weak);
    }

    if close_resp.clicked() || resp.middle_clicked() {
        Some(TabAction::Close(i))
    } else if resp.clicked() {
        Some(TabAction::Select(i))
    } else {
        None
    }
}

fn window_buttons(ui: &mut Ui, area: Rect) {
    let p = theme::palette(ui);
    let ctx = ui.ctx().clone();
    let painter = ui.painter().clone();
    let max = maximized(&ctx);
    let buttons = [
        (icon::MINUS, "window.minimize", ViewportCommand::Minimized(true)),
        (if max { icon::COPY } else { icon::SQUARE }, if max { "window.restore" } else { "window.maximize" }, ViewportCommand::Maximized(!max)),
        (icon::X, "window.close", ViewportCommand::Close),
    ];
    for (k, (glyph, tip, cmd)) in buttons.into_iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(area.left() + k as f32 * BUTTON_W, area.top()), Vec2::new(BUTTON_W, area.height()));
        let resp = ui.interact(r, Id::new(("ventana", k)), Sense::click()).on_hover_text(form_pdf_reader::t!(tip));
        let is_close = k == 2;
        let mut ink = p.text;
        // Windows 11: el botón de maximizar lo atiende el sistema (snap.rs).
        #[cfg(windows)]
        let hovered = if k == 1 {
            crate::snap::set_max_button(r, ctx.pixels_per_point());
            resp.hovered() || crate::snap::max_hovered()
        } else {
            resp.hovered()
        };
        #[cfg(not(windows))]
        let hovered = resp.hovered();
        if hovered {
            painter.rect_filled(r, 0.0, if is_close { p.close_hover } else { p.hover });
            if is_close {
                ink = Color32::WHITE;
            }
        }
        painter.text(r.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(15.0), ink);
        if resp.clicked() {
            ctx.send_viewport_cmd(cmd);
        }
    }
}

/// Redimensionar desde los bordes de la ventana (sin decoración del
/// sistema). Va al final del fotograma, para que su cursor gane.
pub fn resize_edges(ctx: &egui::Context) {
    if maximized(ctx) {
        return;
    }
    let screen = ctx.content_rect();
    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    const M: f32 = 6.0;
    // En Windows los demás bordes son del sistema (snap.rs): solo el de arriba.
    let own = !cfg!(windows);
    let (l, r) = (own && pos.x - screen.left() < M, own && screen.right() - pos.x < M);
    let (t, b) = (pos.y - screen.top() < M, own && screen.bottom() - pos.y < M);
    let dir = match (l, r, t, b) {
        (true, _, true, _) => (ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest),
        (_, true, true, _) => (ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast),
        (true, _, _, true) => (ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest),
        (_, true, _, true) => (ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast),
        (true, ..) => (ResizeDirection::West, CursorIcon::ResizeWest),
        (_, true, ..) => (ResizeDirection::East, CursorIcon::ResizeEast),
        (_, _, true, _) => (ResizeDirection::North, CursorIcon::ResizeNorth),
        (_, _, _, true) => (ResizeDirection::South, CursorIcon::ResizeSouth),
        _ => return,
    };
    ctx.set_cursor_icon(dir.1);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir.0));
    }
}

/// Borde fino alrededor de la ventana, que sin la decoración del sistema
/// se confundiría con lo que hay detrás.
pub fn window_border(ctx: &egui::Context) {
    // En Windows el borde y la sombra los pone el sistema (snap.rs).
    if cfg!(windows) || maximized(ctx) {
        return;
    }
    let p = theme::palette_of(ctx.global_style().visuals.dark_mode);
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("borde_ventana")));
    painter.rect_stroke(ctx.content_rect(), 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
}
