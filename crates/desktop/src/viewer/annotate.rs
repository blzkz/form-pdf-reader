//! Anotar el documento: resaltar, subrayar y tachar texto (se marca
//! arrastrando sobre él), notas y texto libre (se pulsa donde ponerlos y la
//! aplicación pide el texto), dibujo a mano (los trazos se juntan en una
//! anotación al terminar: Intro, otra herramienta o salir) y seleccionar una
//! anotación para borrarla (Supr) o editar su texto (doble clic).
//!
//! Mientras se anota, el ratón no va al formulario.

use super::{DocView, PT_TO_PX, Selection, ViewEvents};
use eframe::egui::{self, Color32, CursorIcon, Event, Key, PointerButton, Pos2, Rect, Stroke, StrokeKind};
use form_pdf_reader::pdfium::{AnnotKind, Annotation};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnnotTool {
    /// Seleccionar una anotación (para borrarla o editarla).
    Select,
    Highlight,
    Underline,
    StrikeOut,
    Note,
    FreeText,
    Ink,
}

impl AnnotTool {
    fn markup(self) -> Option<AnnotKind> {
        match self {
            AnnotTool::Highlight => Some(AnnotKind::Highlight),
            AnnotTool::Underline => Some(AnnotKind::Underline),
            AnnotTool::StrikeOut => Some(AnnotKind::StrikeOut),
            _ => None,
        }
    }

    /// Color por omisión de cada herramienta.
    pub fn default_color(self) -> [u8; 3] {
        match self {
            AnnotTool::Highlight | AnnotTool::Note | AnnotTool::Select => [255, 214, 0],
            AnnotTool::Underline => [33, 150, 243],
            AnnotTool::StrikeOut => [229, 57, 53],
            AnnotTool::FreeText => [20, 20, 20],
            AnnotTool::Ink => [21, 101, 192],
        }
    }
}

/// Lo que la aplicación tiene que preguntar (el texto de una nota o de un
/// texto libre).
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotRequest {
    /// Nota nueva con su esquina superior izquierda en (x, y), en puntos de
    /// página.
    Note { page: usize, x: f32, y: f32 },
    FreeText { page: usize, x: f32, y: f32 },
    /// Cambiar el texto de una nota o de un texto libre.
    Edit(Annotation),
}

pub struct AnnotState {
    pub tool: AnnotTool,
    /// Color de cada herramienta (en el orden de `AnnotTool`).
    colors: [[u8; 3]; 7],
    pub ink_width: f32,
    pub font_size: f32,
    /// Trazos del dibujo en curso (puntos de página) y su página.
    strokes: Vec<Vec<[f32; 2]>>,
    stroke_page: Option<usize>,
    drawing: bool,
    pub selected: Option<Annotation>,
    request: Option<AnnotRequest>,
    last_click: Option<(Instant, Pos2)>,
}

impl AnnotState {
    fn new() -> AnnotState {
        let tools = [AnnotTool::Select, AnnotTool::Highlight, AnnotTool::Underline, AnnotTool::StrikeOut, AnnotTool::Note, AnnotTool::FreeText, AnnotTool::Ink];
        AnnotState {
            tool: AnnotTool::Highlight,
            colors: tools.map(AnnotTool::default_color),
            ink_width: 2.0,
            font_size: 12.0,
            strokes: Vec::new(),
            stroke_page: None,
            drawing: false,
            selected: None,
            request: None,
            last_click: None,
        }
    }

    pub fn color(&self) -> [u8; 3] {
        self.colors[self.tool as usize]
    }

    pub fn set_color(&mut self, c: [u8; 3]) {
        self.colors[self.tool as usize] = c;
    }

    /// Hay trazos sin terminar.
    pub fn has_strokes(&self) -> bool {
        !self.strokes.is_empty()
    }
}

fn color32([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

impl DocView {
    /// Se puede anotar este documento (no XFA, no bloqueado).
    pub fn can_annotate(&self) -> bool {
        self.doc.can_annotate() && !self.read_only
    }

    pub fn is_annotating(&self) -> bool {
        self.annot.is_some()
    }

    pub fn start_annotating(&mut self) {
        if self.can_annotate() && self.annot.is_none() {
            if self.tool == super::Tool::Form {
                self.commit_focus();
            }
            self.selection = None;
            self.selection_rects.clear();
            self.annot = Some(AnnotState::new());
        }
    }

    /// Termina de anotar (escribe el dibujo en curso). true si cambió el
    /// documento.
    pub fn stop_annotating(&mut self) -> Result<bool, String> {
        let r = self.commit_ink();
        self.annot = None;
        self.selection = None;
        self.selection_rects.clear();
        r
    }

    pub fn annot_state(&mut self) -> Option<&mut AnnotState> {
        self.annot.as_mut()
    }

    pub fn set_annot_tool(&mut self, t: AnnotTool) -> Result<bool, String> {
        let Some(a) = self.annot.as_ref() else { return Ok(false) };
        if a.tool == t {
            return Ok(false);
        }
        let r = self.commit_ink();
        if let Some(a) = self.annot.as_mut() {
            a.tool = t;
            a.selected = None;
        }
        self.selection = None;
        self.selection_rects.clear();
        r
    }

    /// Lo que hay que preguntar al usuario (ver `AnnotRequest`).
    pub fn take_annot_request(&mut self) -> Option<AnnotRequest> {
        self.annot.as_mut().and_then(|a| a.request.take())
    }

    /// Escribe los trazos en curso como una anotación. true si había.
    pub fn commit_ink(&mut self) -> Result<bool, String> {
        let Some(a) = self.annot.as_mut() else { return Ok(false) };
        let strokes = std::mem::take(&mut a.strokes);
        a.drawing = false;
        let (Some(page), false) = (a.stroke_page.take(), strokes.is_empty()) else { return Ok(false) };
        let (width, color) = (a.ink_width, a.colors[AnnotTool::Ink as usize]);
        self.doc.add_ink(page, &strokes, width, color).map_err(|e| format!("{e:#}"))?;
        self.invalidate_page(page);
        Ok(true)
    }

    /// Descarta el dibujo en curso.
    pub fn discard_ink(&mut self) {
        if let Some(a) = self.annot.as_mut() {
            a.strokes.clear();
            a.stroke_page = None;
            a.drawing = false;
        }
    }

    pub fn add_note(&mut self, page: usize, x: f32, y: f32, text: &str) -> Result<(), String> {
        let color = self.annot.as_ref().map_or(AnnotTool::Note.default_color(), |a| a.colors[AnnotTool::Note as usize]);
        self.doc.add_note(page, x, y, text, color).map_err(|e| format!("{e:#}"))?;
        self.invalidate_page(page);
        Ok(())
    }

    pub fn add_free_text(&mut self, page: usize, x: f32, y: f32, text: &str) -> Result<(), String> {
        let (size, color) = self.annot.as_ref().map_or((12.0, AnnotTool::FreeText.default_color()), |a| (a.font_size, a.colors[AnnotTool::FreeText as usize]));
        self.doc.add_free_text(page, x, y, text, size, color).map_err(|e| format!("{e:#}"))?;
        self.invalidate_page(page);
        Ok(())
    }

    pub fn set_annotation_text(&mut self, a: &Annotation, text: &str) -> Result<(), String> {
        self.doc.set_annotation_text(a.page, a.index, text).map_err(|e| format!("{e:#}"))?;
        if let Some(s) = self.annot.as_mut() {
            s.selected = None;
        }
        self.invalidate_page(a.page);
        Ok(())
    }

    /// Borra la anotación seleccionada. true si había una.
    pub fn delete_selected_annotation(&mut self) -> Result<bool, String> {
        let Some(sel) = self.annot.as_mut().and_then(|a| a.selected.take()) else { return Ok(false) };
        self.doc.remove_annotation(sel.page, sel.index).map_err(|e| format!("{e:#}"))?;
        self.invalidate_page(sel.page);
        Ok(true)
    }

    /// Ratón y teclado al anotar. true si se los queda (no van al formulario
    /// ni a la herramienta).
    pub(super) fn handle_annotations(&mut self, ctx: &egui::Context, events: &[Event], ev: &mut ViewEvents, area: Rect) -> bool {
        let Some(tool) = self.annot.as_ref().map(|a| a.tool) else { return false };
        let hover = ctx.input(|i| i.pointer.hover_pos()).filter(|p| area.contains(*p));
        if let Some(p) = hover.filter(|_| self.hit(hover.unwrap_or_default()).is_some()) {
            ctx.set_cursor_icon(match tool {
                AnnotTool::Highlight | AnnotTool::Underline | AnnotTool::StrikeOut => CursorIcon::Text,
                AnnotTool::Note | AnnotTool::FreeText | AnnotTool::Ink => CursorIcon::Crosshair,
                AnnotTool::Select => {
                    let over = self.hit(p).and_then(|(pg, x, y)| self.doc.annotation_at(pg, x as f32, y as f32));
                    if over.is_some() { CursorIcon::PointingHand } else { CursorIcon::Default }
                }
            });
        }
        let mut took = false;
        for e in events {
            match e {
                Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers } if area.contains(*pos) => {
                    took = true;
                    let Some((page, x, y)) = self.hit(*pos) else {
                        if let Some(a) = self.annot.as_mut() {
                            a.selected = None;
                        }
                        continue;
                    };
                    let (x, y) = (x as f32, y as f32);
                    match tool {
                        AnnotTool::Highlight | AnnotTool::Underline | AnnotTool::StrikeOut => {
                            self.selecting = true;
                            let hit = self.doc.text_char_at(page, x as f64, y as f64).map(|c| (page, c));
                            self.selection = match (hit, self.selection) {
                                (Some(h), Some(sel)) if modifiers.shift => Some(Selection { a: sel.a, b: h }),
                                (Some(h), _) => Some(Selection { a: h, b: h }),
                                (None, _) => None,
                            };
                            self.update_selection_rects();
                        }
                        AnnotTool::Note => {
                            if let Some(a) = self.annot.as_mut() {
                                a.request = Some(AnnotRequest::Note { page, x, y });
                            }
                        }
                        AnnotTool::FreeText => {
                            if let Some(a) = self.annot.as_mut() {
                                a.request = Some(AnnotRequest::FreeText { page, x, y });
                            }
                        }
                        AnnotTool::Ink => {
                            if self.annot.as_ref().is_some_and(|a| a.stroke_page.is_some_and(|p| p != page)) {
                                match self.commit_ink() {
                                    Ok(c) => ev.modified |= c,
                                    Err(e) => ev.annot_error = Some(e),
                                }
                            }
                            if let Some(a) = self.annot.as_mut() {
                                a.stroke_page = Some(page);
                                a.strokes.push(vec![[x, y]]);
                                a.drawing = true;
                            }
                        }
                        AnnotTool::Select => {
                            let found = self.doc.annotation_at(page, x, y);
                            let now = Instant::now();
                            let Some(a) = self.annot.as_mut() else { continue };
                            let double = a.last_click.is_some_and(|(t, q)| now.duration_since(t) < Duration::from_millis(400) && q.distance(*pos) < 5.0);
                            a.last_click = Some((now, *pos));
                            if double
                                && let Some(f) = found.as_ref().filter(|f| matches!(f.kind, AnnotKind::Note | AnnotKind::FreeText))
                            {
                                a.request = Some(AnnotRequest::Edit(f.clone()));
                            }
                            a.selected = found;
                        }
                    }
                }
                Event::PointerMoved(pos) => {
                    let selecting = self.selecting;
                    match tool {
                        AnnotTool::Highlight | AnnotTool::Underline | AnnotTool::StrikeOut if selecting => {
                            if let Some((p, x, y)) = self.hit(*pos)
                                && let Some(c) = self.doc.text_char_at(p, x, y)
                            {
                                let sel = self.selection.map(|s| Selection { a: s.a, b: (p, c) }).unwrap_or(Selection { a: (p, c), b: (p, c) });
                                if Some(sel) != self.selection {
                                    self.selection = Some(sel);
                                    self.update_selection_rects();
                                }
                            }
                        }
                        AnnotTool::Ink => {
                            let page = self.annot.as_ref().filter(|a| a.drawing).and_then(|a| a.stroke_page);
                            if let Some(page) = page
                                && let Some((x, y)) = self.page_rects.iter().find(|(i, _)| *i == page).and_then(|(_, r)| self.to_page_coords(page, *r, *pos))
                                && let Some(a) = self.annot.as_mut()
                                && let Some(s) = a.strokes.last_mut()
                            {
                                let (x, y) = (x as f32, y as f32);
                                // Sin puntos casi repetidos (medio punto de página).
                                if s.last().is_none_or(|l| (l[0] - x).hypot(l[1] - y) > 0.5) {
                                    s.push([x, y]);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Event::PointerButton { button: PointerButton::Primary, pressed: false, .. } => {
                    if let Some(a) = self.annot.as_mut() {
                        a.drawing = false;
                    }
                    if self.selecting {
                        self.selecting = false;
                        if let Some(kind) = tool.markup() {
                            let color = self.annot.as_ref().map_or(tool.default_color(), |a| a.color());
                            for (p, start, count) in self.selection_ranges() {
                                match self.doc.add_markup_range(p, kind, start, count, color) {
                                    Ok(()) => {
                                        ev.modified = true;
                                        self.invalidate_page(p);
                                    }
                                    Err(e) => ev.annot_error = Some(format!("{e:#}")),
                                }
                            }
                            self.selection = None;
                            self.selection_rects.clear();
                        }
                    }
                }
                Event::Key { key: Key::Delete | Key::Backspace, pressed: true, .. } if self.has_focus => match self.delete_selected_annotation() {
                    Ok(c) => ev.modified |= c,
                    Err(e) => ev.annot_error = Some(e),
                },
                Event::Key { key: Key::Enter, pressed: true, .. } if self.has_focus && tool == AnnotTool::Ink => match self.commit_ink() {
                    Ok(c) => ev.modified |= c,
                    Err(e) => ev.annot_error = Some(e),
                },
                Event::Key { key: Key::Escape, pressed: true, .. } if self.has_focus => {
                    self.discard_ink();
                    if let Some(a) = self.annot.as_mut() {
                        a.selected = None;
                    }
                }
                _ => {}
            }
        }
        // El comentario de una nota al pasar por encima.
        if !took
            && let Some(p) = hover
            && let Some((pg, x, y)) = self.hit(p)
            && let Some(n) = self.doc.annotation_at(pg, x as f32, y as f32).filter(|a| a.kind == AnnotKind::Note && !a.contents.is_empty())
        {
            egui::Tooltip::always_open(ctx.clone(), egui::LayerId::background(), egui::Id::new("nota"), egui::PopupAnchor::Pointer).show(|ui| {
                ui.set_max_width(320.0);
                ui.label(n.contents);
            });
        }
        took
    }

    /// El dibujo en curso y el recuadro de la anotación seleccionada.
    pub(super) fn draw_annotations(&self, painter: &egui::Painter, accent: Color32) {
        let Some(a) = self.annot.as_ref() else { return };
        if let Some(page) = a.stroke_page
            && let Some((_, pr)) = self.page_rects.iter().find(|(i, _)| *i == page).copied()
        {
            let w = (a.ink_width * self.zoom * PT_TO_PX).max(1.0);
            let color = color32(a.colors[AnnotTool::Ink as usize]);
            for s in &a.strokes {
                let pts: Vec<Pos2> = s.iter().map(|p| self.page_to_screen(page, pr, p[0] as f64, p[1] as f64)).collect();
                if pts.len() == 1 {
                    painter.circle_filled(pts[0], w / 2.0, color);
                } else {
                    painter.add(egui::Shape::line(pts, Stroke::new(w, color)));
                }
            }
        }
        if let Some(sel) = &a.selected
            && let Some((_, pr)) = self.page_rects.iter().find(|(i, _)| *i == sel.page).copied()
        {
            let [l, b, r, t] = sel.rect;
            let p0 = self.page_to_screen(sel.page, pr, l as f64, t as f64);
            let p1 = self.page_to_screen(sel.page, pr, r as f64, b as f64);
            painter.rect_stroke(Rect::from_two_pos(p0, p1).expand(3.0), 3.0, Stroke::new(2.0, accent), StrokeKind::Outside);
        }
    }
}
