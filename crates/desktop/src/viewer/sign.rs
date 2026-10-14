//! Firmas manuscritas en el visor: la firma elegida sigue al ratón hasta
//! que se pulsa en la página; puesta, se dibuja aquí encima de la página y
//! se puede seleccionar, mover, cambiar de tamaño (esquina de abajo a la
//! derecha) y quitar (× o Supr) hasta que se guarda, se imprime o se
//! exporta: entonces pasa al PDF (`Document::commit_signatures`).

use super::{DocView, PT_TO_PX, ViewEvents};
use eframe::egui::{self, Color32, CursorIcon, Event, Key, PointerButton, Pos2, Rect, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use form_pdf_reader::pdfium::{render_signature, signature_size};
use form_pdf_reader::sign::SavedSignature;

/// Firma a punto de ponerse.
pub struct Placing {
    sig: SavedSignature,
    /// Tamaño en puntos de página.
    size: (f32, f32),
    tex: TextureHandle,
}

/// Arrastre de una firma puesta.
#[derive(Clone, Copy)]
pub struct SigDrag {
    resize: bool,
    /// Punto de página donde empezó.
    start: (f64, f64),
    rect0: [f32; 4],
    cur: [f32; 4],
}

/// Imagen de una firma para la interfaz.
pub fn signature_texture(ctx: &egui::Context, sig: &SavedSignature, height: i32) -> Option<TextureHandle> {
    let (w, h, px) = render_signature(sig, height)?;
    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
    Some(ctx.load_texture(format!("firma-{}-{height}", sig.id), img, TextureOptions::LINEAR))
}

const HANDLE: f32 = 12.0;
const MIN_W: f32 = 20.0;

impl DocView {
    /// La firma sigue al ratón hasta pulsar en una página (Esc cancela).
    pub fn start_placing(&mut self, ctx: &egui::Context, sig: SavedSignature) {
        let Some(tex) = signature_texture(ctx, &sig, 160) else { return };
        let size = signature_size(&sig);
        self.sig_selected = None;
        self.placing = Some(Placing { sig, size, tex });
    }

    pub fn is_placing(&self) -> bool {
        self.placing.is_some()
    }

    /// Rectángulo en pantalla de un rectángulo de página.
    fn sig_screen_rect(&self, page: usize, r: [f32; 4]) -> Option<Rect> {
        let (_, pr) = self.page_rects.iter().find(|(i, _)| *i == page)?;
        let a = self.page_to_screen(page, *pr, r[0] as f64, r[3] as f64);
        let b = self.page_to_screen(page, *pr, r[2] as f64, r[1] as f64);
        Some(Rect::from_two_pos(a, b))
    }

    fn page_point(&self, page: usize, pos: Pos2) -> Option<(f64, f64)> {
        let (_, pr) = self.page_rects.iter().find(|(i, _)| *i == page)?;
        self.to_page_coords(page, *pr, pos)
    }

    /// Mantiene `r` dentro de la página.
    fn clamp_to_page(&self, page: usize, mut r: [f32; 4]) -> [f32; 4] {
        let (pw, ph) = self.doc.page_size(page);
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        let dx = (-r[0]).max(0.0) - (r[2] - pw).max(0.0);
        let dy = (-r[1]).max(0.0) - (r[3] - ph).max(0.0);
        r = [r[0] + dx, r[1] + dy, r[0] + dx + w, r[1] + dy + h];
        r
    }

    /// Ratón y teclado para las firmas. true si se los queda (no van a
    /// PDFium ni a la herramienta).
    pub(super) fn handle_signatures(&mut self, ctx: &egui::Context, events: &[Event], ev: &mut ViewEvents, area: Rect) -> bool {
        // Ya se escribieron en el PDF (guardar, imprimir, exportar): ahora
        // las dibuja PDFium en la página.
        let n = self.doc.placed_signatures().len();
        if self.sig_tex.len() != n {
            self.sig_tex = vec![None; n];
            self.sig_selected = None;
            self.sig_drag = None;
            self.invalidate_all();
        }
        // Esc cancela la colocación; Supr quita la seleccionada.
        for e in events {
            if let Event::Key { key, pressed: true, .. } = e {
                match key {
                    Key::Escape if self.placing.is_some() => {
                        self.placing = None;
                        return true;
                    }
                    Key::Delete | Key::Backspace if self.sig_selected.is_some() && !self.doc.editing_text() => {
                        self.remove_selected_signature(ev);
                        return true;
                    }
                    _ => {}
                }
            }
        }

        if let Some(pl) = &self.placing {
            let size = pl.size;
            for e in events {
                if let Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } = e
                    && area.contains(*pos)
                    && let Some((page, x, y)) = self.hit(*pos)
                {
                    let (x, y) = (x as f32, y as f32);
                    let r = self.clamp_to_page(page, [x - size.0 / 2.0, y - size.1 / 2.0, x + size.0 / 2.0, y + size.1 / 2.0]);
                    let pl = self.placing.take().expect("hay firma");
                    match self.doc.add_signature(page, r, &pl.sig) {
                        Ok(i) => {
                            self.sig_tex.insert(i, None);
                            self.sig_selected = Some(i);
                            ev.modified = true;
                        }
                        Err(err) => ev.sign_error = Some(format!("{err:#}")),
                    }
                    ctx.request_repaint();
                    return true;
                }
            }
            if ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| area.contains(p)) {
                ctx.set_cursor_icon(CursorIcon::Crosshair);
                ctx.request_repaint();
            }
            return true;
        }

        let Some(i) = self.sig_selected else { return false };
        let Some(placed) = self.doc.placed_signatures().get(i).cloned() else {
            self.sig_selected = None;
            return false;
        };
        let Some(sr) = self.sig_screen_rect(placed.page, placed.rect) else { return false };
        let handle = Rect::from_center_size(sr.right_bottom(), Vec2::splat(HANDLE + 4.0));
        let close = Rect::from_center_size(sr.right_top() + Vec2::new(10.0, -10.0), Vec2::splat(20.0));
        let mut consumed = false;
        for e in events {
            match e {
                Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } => {
                    if close.contains(*pos) {
                        self.remove_selected_signature(ev);
                        return true;
                    }
                    let resize = handle.contains(*pos);
                    if (resize || sr.contains(*pos))
                        && let Some(start) = self.page_point(placed.page, *pos)
                    {
                        self.sig_drag = Some(SigDrag { resize, start, rect0: placed.rect, cur: placed.rect });
                        consumed = true;
                    } else {
                        // Pulsar fuera deja de seleccionarla.
                        self.sig_selected = None;
                        self.sig_drag = None;
                        return false;
                    }
                }
                Event::PointerMoved(pos) if self.sig_drag.is_some() => {
                    let d = self.sig_drag.expect("arrastre");
                    if let Some((x, y)) = self.page_point(placed.page, *pos) {
                        let r = d.rect0;
                        let cur = if d.resize {
                            // La esquina de arriba a la izquierda queda fija.
                            let aspect = (r[3] - r[1]) / (r[2] - r[0]);
                            let w = (x as f32 - r[0]).max(MIN_W);
                            [r[0], r[3] - w * aspect, r[0] + w, r[3]]
                        } else {
                            let (dx, dy) = ((x - d.start.0) as f32, (y - d.start.1) as f32);
                            [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]
                        };
                        self.sig_drag = Some(SigDrag { cur: self.clamp_to_page(placed.page, cur), ..d });
                    }
                    consumed = true;
                }
                Event::PointerButton { button: PointerButton::Primary, pressed: false, .. } if self.sig_drag.is_some() => {
                    let d = self.sig_drag.take().expect("arrastre");
                    if d.cur != d.rect0 {
                        match self.doc.move_signature(i, d.cur) {
                            Ok(()) => ev.modified = true,
                            Err(err) => ev.sign_error = Some(format!("{err:#}")),
                        }
                    }
                    consumed = true;
                }
                _ => {}
            }
        }
        // Cursor sobre la firma seleccionada.
        if let Some(p) = ctx.input(|i| i.pointer.hover_pos()) {
            if self.sig_drag.is_some_and(|d| d.resize) || handle.contains(p) {
                ctx.set_cursor_icon(CursorIcon::ResizeNwSe);
            } else if self.sig_drag.is_some() || sr.contains(p) {
                ctx.set_cursor_icon(CursorIcon::Move);
            } else if close.contains(p) {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
        }
        consumed || self.sig_drag.is_some()
    }

    fn remove_selected_signature(&mut self, ev: &mut ViewEvents) {
        if let Some(i) = self.sig_selected.take() {
            self.doc.remove_signature(i);
            if i < self.sig_tex.len() {
                self.sig_tex.remove(i);
            }
            self.sig_drag = None;
            ev.modified = true;
        }
    }

    /// Las firmas puestas, la que sigue al ratón y el marco de la seleccionada.
    pub(super) fn draw_signatures(&mut self, ctx: &egui::Context, painter: &egui::Painter) {
        let accent = crate::theme::palette_of(ctx.global_style().visuals.dark_mode).accent;
        let image = |tex: &TextureHandle, r: Rect, alpha: f32| {
            painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE.gamma_multiply(alpha));
        };
        // Las puestas, con una imagen del tamaño con que se ven.
        let ppp = ctx.pixels_per_point();
        for i in 0..self.doc.placed_signatures().len().min(self.sig_tex.len()) {
            let placed = self.doc.placed_signatures()[i].clone();
            let dragging = self.sig_selected == Some(i) && self.sig_drag.is_some();
            let rect = if dragging { self.sig_drag.map_or(placed.rect, |d| d.cur) } else { placed.rect };
            let Some(sr) = self.sig_screen_rect(placed.page, rect) else { continue };
            let want = (sr.height() * ppp).round().clamp(16.0, 900.0) as i32;
            let stale = self.sig_tex[i].as_ref().is_none_or(|(_, h)| (*h - want).abs() * 4 > want);
            if stale && let Some(tex) = signature_texture(ctx, &placed.signature, want) {
                self.sig_tex[i] = Some((tex, want));
            }
            if let Some((tex, _)) = &self.sig_tex[i] {
                image(tex, sr, if dragging { 0.7 } else { 1.0 });
            }
        }
        if let Some(pl) = &self.placing {
            if let Some(p) = ctx.input(|i| i.pointer.hover_pos())
                && self.hit(p).is_some()
            {
                let s = self.zoom * PT_TO_PX;
                let r = Rect::from_center_size(p, Vec2::new(pl.size.0 * s, pl.size.1 * s));
                image(&pl.tex, r, 0.75);
                painter.rect_stroke(r, 3.0, Stroke::new(1.0, accent.gamma_multiply(0.6)), StrokeKind::Outside);
            }
            return;
        }
        let Some(i) = self.sig_selected else { return };
        let Some(placed) = self.doc.placed_signatures().get(i) else { return };
        let rect = self.sig_drag.map_or(placed.rect, |d| d.cur);
        let Some(sr) = self.sig_screen_rect(placed.page, rect) else { return };
        painter.rect_stroke(sr.expand(3.0), 3.0, Stroke::new(1.5, accent), StrokeKind::Outside);
        let handle = Rect::from_center_size(sr.right_bottom() + Vec2::splat(3.0), Vec2::splat(HANDLE));
        painter.rect(handle, 2.0, Color32::WHITE, Stroke::new(1.5, accent), StrokeKind::Inside);
        if self.sig_drag.is_none() {
            let c = sr.right_top() + Vec2::new(10.0, -10.0);
            painter.circle(c, 10.0, accent, Stroke::new(1.5, Color32::WHITE));
            painter.text(c, egui::Align2::CENTER_CENTER, crate::theme::icon::X, egui::FontId::proportional(12.0), Color32::WHITE);
        }
    }
}
