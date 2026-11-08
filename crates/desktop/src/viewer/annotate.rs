//! Anotar el documento: resaltar, subrayar y tachar texto (se marca
//! arrastrando sobre él), notas y texto libre (se pulsa donde ponerlos, o se
//! arrastra para marcar la zona de un texto, y se escribe ahí mismo), dibujo a mano (los trazos se juntan en una
//! anotación al terminar: Intro, otra herramienta o salir) y seleccionar una
//! anotación para borrarla (Supr), editar su texto (doble clic), cambiar su
//! color, arrastrarla a otro sitio o cambiar su tamaño con el tirador de la
//! esquina (texto y dibujo). Las herramientas de texto y de nota también
//! eligen los textos y las notas ya puestos. La goma quita la marca de los
//! caracteres que se arrastran.
//!
//! El clic derecho en una marca de texto (anotando o no) abre su menú:
//! cambiar el color, quitarla o copiar el texto marcado.
//!
//! Mientras se anota, el ratón no va al formulario.

use super::{DocView, PT_TO_PX, Selection, ViewEvents};
use crate::theme::icon;
use eframe::egui::{self, Color32, CursorIcon, Event, Key, PointerButton, Pos2, Rect, Stroke, StrokeKind};
use form_pdf_reader::pdfium::{AnnotKind, Annotation};
use form_pdf_reader::t;
use std::time::{Duration, Instant};

/// Los colores de las anotaciones.
pub const ANNOT_COLORS: [[u8; 3]; 6] = [[255, 214, 0], [76, 175, 80], [33, 150, 243], [229, 57, 53], [156, 39, 176], [20, 20, 20]];

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

    /// Elige las anotaciones de ese tipo al pulsar sobre ellas (en vez de
    /// poner una nueva).
    fn picks(self, kind: AnnotKind) -> bool {
        match self {
            AnnotTool::Select => true,
            AnnotTool::Note => kind == AnnotKind::Note,
            AnnotTool::FreeText => kind == AnnotKind::FreeText,
            _ => false,
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

/// Lado del tirador de tamaño de la anotación seleccionada, en la pantalla.
const HANDLE: f32 = 12.0;
/// Ancho mínimo (en puntos) al cambiar el tamaño de una anotación.
const MIN_W: f32 = 20.0;
/// Lo mínimo (en puntos) que hay que arrastrar para marcar una zona; menos es
/// un clic.
const MIN_ZONE_W: f32 = 30.0;
const MIN_ZONE_H: f32 = 8.0;
/// Margen interior de un texto libre y lado del icono de una nota, en puntos
/// (los de `Document`).
const FREE_TEXT_PAD: f32 = 3.0;
const NOTE_ICON: f32 = 20.0;

/// Una anotación que se está moviendo o cambiando de tamaño. Se escribe al
/// soltar; hasta entonces se ve su recuadro.
#[derive(Clone, Copy)]
struct AnnotDrag {
    resize: bool,
    /// Punto de página donde empezó.
    start: (f64, f64),
    rect0: [f32; 4],
    cur: [f32; 4],
}

/// Una zona que se está marcando para escribir en ella (puntos de página).
#[derive(Clone, Copy)]
struct ZoneDrag {
    page: usize,
    start: [f32; 2],
    cur: [f32; 2],
}

/// Qué se está escribiendo en la página.
enum InlineKind {
    /// Texto nuevo; en una zona (ancho y alto) o ajustado a lo escrito.
    NewText(Option<[f32; 2]>),
    NewNote,
    Edit(Annotation),
}

/// El editor que se abre sobre la página. (x, y) es su esquina superior
/// izquierda, en puntos de página.
pub(super) struct InlineEdit {
    page: usize,
    x: f32,
    y: f32,
    kind: InlineKind,
    text: String,
    /// Fotogramas en los que pedir el foco tras soltar el botón.
    focus: u8,
}

pub struct AnnotState {
    pub tool: AnnotTool,
    /// Color de cada herramienta (en el orden de `AnnotTool`).
    colors: [[u8; 3]; 7],
    pub ink_width: f32,
    pub font_size: f32,
    /// Resaltar, subrayar o tachar con la goma: arrastrar quita la marca.
    pub erasing: bool,
    /// Trazos del dibujo en curso (puntos de página) y su página.
    strokes: Vec<Vec<[f32; 2]>>,
    stroke_page: Option<usize>,
    drawing: bool,
    pub selected: Option<Annotation>,
    drag: Option<AnnotDrag>,
    zone: Option<ZoneDrag>,
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
            erasing: false,
            strokes: Vec::new(),
            stroke_page: None,
            drawing: false,
            selected: None,
            drag: None,
            zone: None,
            last_click: None,
        }
    }

    pub fn color(&self) -> [u8; 3] {
        self.colors[self.tool as usize]
    }

    pub fn set_color(&mut self, c: [u8; 3]) {
        self.colors[self.tool as usize] = c;
        self.erasing = false;
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
        let r = self.commit_pending();
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
        let r = self.commit_pending();
        if let Some(a) = self.annot.as_mut() {
            a.tool = t;
            a.selected = None;
            a.drag = None;
            a.zone = None;
            a.erasing = false;
        }
        self.selection = None;
        self.selection_rects.clear();
        r
    }

    /// Escribe lo que esté a medias: el texto del editor y el dibujo.
    fn commit_pending(&mut self) -> Result<bool, String> {
        let text = self.commit_inline()?;
        Ok(self.commit_ink()? || text)
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

    /// Abre el editor para cambiar el texto de una nota o de un texto libre.
    pub fn edit_annotation(&mut self, a: &Annotation) {
        let [left, _, _, top] = a.rect;
        self.inline = Some(InlineEdit { page: a.page, x: left, y: top, kind: InlineKind::Edit(a.clone()), text: a.contents.clone(), focus: 3 });
        if let Some(s) = self.annot.as_mut() {
            s.selected = None;
            s.drag = None;
        }
    }

    /// Escribe en el documento lo del editor. true si cambió algo.
    pub(super) fn commit_inline(&mut self) -> Result<bool, String> {
        let Some(e) = self.inline.take() else { return Ok(false) };
        let text = e.text.trim_end();
        let (size, color, note_color) = self.annot.as_ref().map_or((12.0, AnnotTool::FreeText.default_color(), AnnotTool::Note.default_color()), |a| {
            (a.font_size, a.colors[AnnotTool::FreeText as usize], a.colors[AnnotTool::Note as usize])
        });
        let (x, y, page) = (e.x, e.y, e.page);
        let r = match &e.kind {
            InlineKind::NewText(_) | InlineKind::NewNote if text.is_empty() => return Ok(false),
            InlineKind::NewText(None) => self.doc.add_free_text(page, x, y, text, size, color),
            InlineKind::NewText(Some([w, h])) => self.doc.add_free_text_in(page, [x, y - h, x + w, y], text, size, color),
            InlineKind::NewNote => self.doc.add_note(page, x, y, text, note_color),
            InlineKind::Edit(a) if text == a.contents => return Ok(false),
            InlineKind::Edit(a) if a.kind == AnnotKind::FreeText && text.is_empty() => self.doc.remove_annotation(page, a.index),
            InlineKind::Edit(a) => self.doc.set_annotation_text(page, a.index, text),
        };
        r.map_err(|e| format!("{e:#}"))?;
        self.invalidate_page(page);
        // Un texto nuevo queda elegido: se puede colocar y ajustar enseguida.
        if matches!(e.kind, InlineKind::NewText(_)) {
            let added = self.doc.annotations(page).pop();
            if let Some(a) = self.annot.as_mut() {
                a.selected = added;
            }
        }
        Ok(true)
    }

    /// El editor sobre la página. Pulsar en el documento fuera de él lo
    /// escribe (y esa pulsación no hace nada más); Esc lo descarta y
    /// Ctrl + Intro lo escribe.
    pub(super) fn inline_editor_ui(&mut self, ctx: &egui::Context, events: Vec<Event>, ev: &mut ViewEvents, area: Rect) -> Vec<Event> {
        let Some(e) = self.inline.as_ref() else { return events };
        let Some(p) = self.page_rects.iter().find(|(i, _)| *i == e.page).map(|(_, pr)| (self.page_to_screen(e.page, *pr, e.x as f64, e.y as f64), *pr)) else {
            // Su página ya no se ve.
            self.finish_inline(ev);
            return events;
        };
        let (top_left, page_rect) = p;
        let k = self.zoom * PT_TO_PX;
        let state = self.annot.as_ref();
        let (font_size, color, zone, min, note) = match &e.kind {
            InlineKind::NewText(z) => (state.map_or(12.0, |a| a.font_size), state.map_or(AnnotTool::FreeText.default_color(), |a| a.colors[AnnotTool::FreeText as usize]), *z, None, false),
            InlineKind::NewNote => (0.0, [0, 0, 0], None, None, true),
            InlineKind::Edit(a) if a.kind == AnnotKind::Note => (0.0, [0, 0, 0], None, None, true),
            InlineKind::Edit(a) => (a.font_size, a.color, a.zone.then_some([a.rect[2] - a.rect[0], a.rect[3] - a.rect[1]]), Some([a.rect[2] - a.rect[0], a.rect[3] - a.rect[1]]), false),
        };
        // Un texto se ve como quedará; el de una nota va en un recuadro junto
        // a su icono.
        let (pos, font, pad, fill, ink) = if note {
            (top_left + egui::vec2(NOTE_ICON * k + 4.0, 0.0), egui::FontId::proportional(14.0), 6.0, Color32::from_rgb(255, 249, 196), Color32::from_rgb(30, 30, 30))
        } else {
            (top_left, egui::FontId::proportional(font_size * k), FREE_TEXT_PAD * k, Color32::WHITE, color32(color))
        };
        let room = (page_rect.right() - pos.x - 2.0 * pad).max(60.0);
        let measured = ctx.fonts_mut(|f| {
            e.text.lines().map(|l| f.layout_no_wrap(l.to_string(), font.clone(), ink).size().x).fold(0.0_f32, f32::max)
        });
        let width = match zone {
            Some([w, _]) => w * k - 2.0 * pad,
            None if note => 240.0,
            None => (measured + font.size).max(min.map_or(120.0, |m| m[0] * k - 2.0 * pad)).min(room),
        };
        let min_h = zone.or(min).map_or(0.0, |z| z[1] * k - 2.0 * pad);
        let id = self.area_id.with("editor_en_linea");
        let accent = ctx.global_style().visuals.selection.bg_fill;
        let Some(e) = self.inline.as_mut() else { return events };
        let shown = egui::Area::new(id).order(egui::Order::Middle).fixed_pos(pos).constrain_to(area).show(ctx, |ui| {
            egui::Frame::new().fill(fill).stroke(Stroke::new(1.5, accent)).inner_margin(pad).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut e.text)
                        .id(id.with("texto"))
                        .frame(egui::Frame::NONE)
                        .font(font.clone())
                        .text_color(ink)
                        .desired_width(width)
                        .desired_rows(1)
                        .min_size(egui::vec2(width, min_h.max(font.size))),
                )
            })
        });
        let edit = shown.inner.inner;
        // Hasta unos fotogramas después de soltar el botón: al soltarlo, el
        // visor pide el foco para sí.
        if e.focus > 0 {
            edit.request_focus();
            if !ctx.input(|i| i.pointer.any_down()) {
                e.focus -= 1;
            }
        }
        let rect = shown.response.rect;
        let outside = ctx.input(|i| i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|p| area.contains(p) && !rect.contains(p)));
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.inline = None;
        } else if outside || ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if modifiers.command))) {
            self.finish_inline(ev);
        }
        ctx.request_repaint();
        if outside {
            return events.into_iter().filter(|e| !matches!(e, Event::PointerButton { button: PointerButton::Primary, pressed: true, .. })).collect();
        }
        events
    }

    fn finish_inline(&mut self, ev: &mut ViewEvents) {
        match self.commit_inline() {
            Ok(c) => ev.modified |= c,
            Err(e) => ev.annot_error = Some(e),
        }
    }

    /// Cambia el color de la anotación seleccionada.
    pub fn set_selected_color(&mut self, color: [u8; 3]) -> Result<bool, String> {
        let Some(sel) = self.annot.as_ref().and_then(|a| a.selected.clone()) else { return Ok(false) };
        let i = self.doc.set_annotation_color(sel.page, sel.index, color).map_err(|e| format!("{e:#}"))?;
        self.reselect(sel.page, i);
        Ok(true)
    }

    /// Cambia el tamaño de letra del texto libre seleccionado.
    pub fn set_selected_font_size(&mut self, size: f32) -> Result<bool, String> {
        let Some(sel) = self.annot.as_ref().and_then(|a| a.selected.clone()) else { return Ok(false) };
        let i = self.doc.set_free_text_size(sel.page, sel.index, size).map_err(|e| format!("{e:#}"))?;
        self.reselect(sel.page, i);
        Ok(true)
    }

    /// Repinta la página y elige la anotación que se acaba de rehacer.
    fn reselect(&mut self, page: usize, index: usize) {
        self.invalidate_page(page);
        let again = self.doc.annotations(page).into_iter().find(|a| a.index == index);
        if let Some(a) = self.annot.as_mut() {
            a.selected = again;
        }
    }

    /// Tirador de tamaño de la anotación seleccionada, en la pantalla.
    fn annot_handle(&self, sel: &Annotation, r: [f32; 4]) -> Option<Rect> {
        let sr = self.sig_screen_rect(sel.page, r)?;
        sel.kind.can_resize().then(|| Rect::from_center_size(sr.expand(3.0).right_bottom(), egui::Vec2::splat(HANDLE)))
    }

    /// Pulsar en el tirador de la anotación seleccionada. true si se ha
    /// pulsado ahí.
    fn press_annot_handle(&mut self, pos: Pos2) -> bool {
        let Some(sel) = self.annot.as_ref().and_then(|a| a.selected.clone()) else { return false };
        let on_handle = self.annot_handle(&sel, sel.rect).is_some_and(|h| h.expand(4.0).contains(pos));
        if on_handle
            && let Some(start) = self.page_point(sel.page, pos)
            && let Some(a) = self.annot.as_mut()
        {
            a.drag = Some(AnnotDrag { resize: true, start, rect0: sel.rect, cur: sel.rect });
        }
        on_handle
    }

    /// El puntero se mueve con una anotación agarrada. true si había una.
    fn drag_annotation(&mut self, pos: Pos2) -> bool {
        let Some(a) = self.annot.as_ref() else { return false };
        let (Some(d), Some(page)) = (a.drag, a.selected.as_ref().map(|s| s.page)) else { return false };
        if let Some((x, y)) = self.page_point(page, pos) {
            let r = d.rect0;
            let cur = if d.resize {
                // La esquina de arriba a la izquierda queda fija, y la proporción.
                let w = (x as f32 - r[0]).max(MIN_W);
                [r[0], r[3] - w * (r[3] - r[1]) / (r[2] - r[0]), r[0] + w, r[3]]
            } else {
                let (dx, dy) = ((x - d.start.0) as f32, (y - d.start.1) as f32);
                [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]
            };
            let cur = self.clamp_to_page(page, cur);
            if let Some(a) = self.annot.as_mut() {
                a.drag = Some(AnnotDrag { cur, ..d });
            }
        }
        true
    }

    /// El puntero se mueve marcando una zona. true si se está marcando.
    fn drag_zone(&mut self, pos: Pos2) -> bool {
        let Some(z) = self.annot.as_ref().and_then(|a| a.zone) else { return false };
        if let Some((x, y)) = self.page_point(z.page, pos) {
            let (w, h) = self.doc.page_size(z.page);
            if let Some(a) = self.annot.as_mut() {
                a.zone = Some(ZoneDrag { cur: [(x as f32).clamp(0.0, w), (y as f32).clamp(0.0, h)], ..z });
            }
        }
        true
    }

    /// Suelta la zona: se escribe en ella o, si era un clic, donde se pulsó.
    fn drop_zone(&mut self) {
        let Some(z) = self.annot.as_mut().and_then(|a| a.zone.take()) else { return };
        let [x0, y0] = z.start;
        let [x1, y1] = z.cur;
        let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
        let (x, y, size) = if w >= MIN_ZONE_W && h >= MIN_ZONE_H { (x0.min(x1), y0.max(y1), Some([w, h])) } else { (x0, y0, None) };
        self.inline = Some(InlineEdit { page: z.page, x, y, kind: InlineKind::NewText(size), text: String::new(), focus: 3 });
    }

    /// Suelta la anotación agarrada: la escribe en su sitio nuevo.
    fn drop_annotation(&mut self, ev: &mut ViewEvents) {
        let Some(a) = self.annot.as_mut() else { return };
        let Some(d) = a.drag.take() else { return };
        let Some(sel) = a.selected.clone() else { return };
        if d.cur == d.rect0 {
            return;
        }
        match self.doc.set_annotation_rect(sel.page, sel.index, d.cur) {
            Ok(i) => {
                ev.modified = true;
                self.reselect(sel.page, i);
            }
            Err(e) => ev.annot_error = Some(format!("{e:#}")),
        }
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
            let (pg, x, y) = self.hit(p).unwrap_or_default();
            let over = self.doc.annotation_at(pg, x as f32, y as f32).filter(|f| tool.picks(f.kind));
            let selected = self.annot.as_ref().and_then(|a| a.selected.clone());
            let on_handle = selected.as_ref().and_then(|s| self.annot_handle(s, s.rect)).is_some_and(|h| h.expand(4.0).contains(p));
            ctx.set_cursor_icon(match tool {
                _ if on_handle => CursorIcon::ResizeNwSe,
                _ if over.as_ref().is_some_and(|f| f.kind.can_move()) => CursorIcon::Move,
                AnnotTool::Select if over.is_some() => CursorIcon::PointingHand,
                AnnotTool::Select => CursorIcon::Default,
                AnnotTool::Highlight | AnnotTool::Underline | AnnotTool::StrikeOut => CursorIcon::Text,
                AnnotTool::Note | AnnotTool::FreeText | AnnotTool::Ink => CursorIcon::Crosshair,
            });
        }
        let mut took = false;
        for e in events {
            match e {
                Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers } if area.contains(*pos) => {
                    took = true;
                    if self.press_annot_handle(*pos) {
                        continue;
                    }
                    let Some((page, x, y)) = self.hit(*pos) else {
                        if let Some(a) = self.annot.as_mut() {
                            a.selected = None;
                        }
                        continue;
                    };
                    let (x, y) = (x as f32, y as f32);
                    let found = self.doc.annotation_at(page, x, y).filter(|f| tool.picks(f.kind));
                    match tool {
                        AnnotTool::Select | AnnotTool::Note | AnnotTool::FreeText if tool == AnnotTool::Select || found.is_some() => {
                            let now = Instant::now();
                            let start = (x as f64, y as f64);
                            let Some(a) = self.annot.as_mut() else { continue };
                            let double = a.last_click.is_some_and(|(t, q)| now.duration_since(t) < Duration::from_millis(400) && q.distance(*pos) < 5.0);
                            a.last_click = Some((now, *pos));
                            if double
                                && let Some(f) = found.as_ref().filter(|f| matches!(f.kind, AnnotKind::Note | AnnotKind::FreeText))
                            {
                                let f = f.clone();
                                self.edit_annotation(&f);
                                continue;
                            }
                            // Pulsar ya agarra lo elegido: se puede arrastrar sin más.
                            a.drag = found.as_ref().filter(|f| f.kind.can_move() && !double).map(|f| AnnotDrag { resize: false, start, rect0: f.rect, cur: f.rect });
                            a.selected = found;
                        }
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
                                a.selected = None;
                            }
                            self.inline = Some(InlineEdit { page, x, y, kind: InlineKind::NewNote, text: String::new(), focus: 3 });
                        }
                        AnnotTool::FreeText => {
                            if let Some(a) = self.annot.as_mut() {
                                a.selected = None;
                                // Al soltar: un clic o la zona arrastrada.
                                a.zone = Some(ZoneDrag { page, start: [x, y], cur: [x, y] });
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
                        // (Atendida arriba.)
                        AnnotTool::Select => {}
                    }
                }
                Event::PointerMoved(pos) if self.drag_annotation(*pos) => {}
                Event::PointerMoved(pos) if self.drag_zone(*pos) => {}
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
                    self.drop_annotation(ev);
                    self.drop_zone();
                    if let Some(a) = self.annot.as_mut() {
                        a.drawing = false;
                    }
                    if self.selecting {
                        self.selecting = false;
                        if let Some(kind) = tool.markup() {
                            let color = self.annot.as_ref().map_or(tool.default_color(), |a| a.color());
                            let erasing = self.annot.as_ref().is_some_and(|a| a.erasing);
                            for (p, start, count) in self.selection_ranges() {
                                let r = if erasing { self.doc.remove_markup_range(p, kind, start, count) } else { self.doc.add_markup_range(p, kind, start, count, color).map(|_| true) };
                                match r {
                                    Ok(changed) => {
                                        ev.modified |= changed;
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
                        a.drag = None;
                    }
                }
                _ => {}
            }
        }
        // El comentario de una nota al pasar por encima.
        if !took
            && self.inline.is_none()
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
        if let Some(z) = a.zone {
            let r = [z.start[0].min(z.cur[0]), z.start[1].min(z.cur[1]), z.start[0].max(z.cur[0]), z.start[1].max(z.cur[1])];
            if let Some(sr) = self.sig_screen_rect(z.page, r) {
                painter.rect_filled(sr, 2.0, accent.gamma_multiply(0.2));
                painter.rect_stroke(sr, 2.0, Stroke::new(1.5, accent), StrokeKind::Outside);
            }
        }
        if let Some(sel) = &a.selected {
            let r = a.drag.map_or(sel.rect, |d| d.cur);
            let Some(sr) = self.sig_screen_rect(sel.page, r) else { return };
            if let Some(d) = a.drag.filter(|d| d.cur != d.rect0) {
                painter.rect_filled(sr.expand(3.0), 3.0, accent.gamma_multiply(0.25));
                // Un texto se ve mientras se arrastra (con la letra de la interfaz).
                if sel.kind == AnnotKind::FreeText {
                    let scale = (d.cur[2] - d.cur[0]) / (d.rect0[2] - d.rect0[0]);
                    let px = sel.font_size * scale * self.zoom * PT_TO_PX;
                    painter.text(sr.left_top() + egui::Vec2::splat(3.0 * scale * self.zoom * PT_TO_PX), egui::Align2::LEFT_TOP, &sel.contents, egui::FontId::proportional(px), color32(sel.color));
                }
            }
            painter.rect_stroke(sr.expand(3.0), 3.0, Stroke::new(2.0, accent), StrokeKind::Outside);
            if let Some(h) = self.annot_handle(sel, r) {
                painter.rect(h, 2.0, Color32::WHITE, Stroke::new(1.5, accent), StrokeKind::Inside);
            }
        }
    }
}

/// El menú de una marca de texto (clic derecho): dónde se abrió y la marca.
pub(super) struct MarkupMenu {
    pos: Pos2,
    mark: Annotation,
}

/// Lo que se ha elegido en el menú de una marca.
enum MarkupAction {
    Color([u8; 3]),
    Remove,
    Copy,
}

impl DocView {
    /// La marca de texto de encima en la posición de la pantalla.
    fn markup_at(&self, pos: Pos2) -> Option<Annotation> {
        let (page, x, y) = self.hit(pos)?;
        self.doc.markup_at(page, x as f32, y as f32)
    }

    /// Clic derecho sobre una marca de texto: abre su menú. Devuelve los
    /// eventos sin los del botón derecho que lo han abierto (no van al
    /// formulario).
    pub(super) fn open_markup_menu(&mut self, events: Vec<Event>, area: Rect) -> Vec<Event> {
        if !self.can_annotate() {
            return events;
        }
        let opened = events.iter().find_map(|e| match e {
            Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, .. } if area.contains(*pos) => self.markup_at(*pos).map(|m| (*pos, m)),
            _ => None,
        });
        let Some((pos, mark)) = opened else { return events };
        self.markup_menu = Some(MarkupMenu { pos, mark });
        events.into_iter().filter(|e| !matches!(e, Event::PointerButton { button: PointerButton::Secondary, .. })).collect()
    }

    /// El menú de la marca, si está abierto.
    pub(super) fn markup_menu_ui(&mut self, ctx: &egui::Context, ev: &mut ViewEvents) {
        let Some(menu) = self.markup_menu.as_ref() else { return };
        let mark = menu.mark.clone();
        let mut action = None;
        let area = egui::Area::new(self.area_id.with("menu_marca")).order(egui::Order::Foreground).fixed_pos(menu.pos).constrain(true).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for c in ANNOT_COLORS {
                        let (rect, r) = ui.allocate_exact_size(egui::Vec2::splat(22.0), egui::Sense::click());
                        ui.painter().circle_filled(rect.center(), 8.0, color32(c));
                        if c == mark.color {
                            ui.painter().circle_stroke(rect.center(), 10.5, Stroke::new(2.0, ui.visuals().selection.stroke.color));
                        } else if r.hovered() {
                            ui.painter().circle_stroke(rect.center(), 10.5, Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color));
                        }
                        if r.clicked() {
                            action = Some(MarkupAction::Color(c));
                        }
                    }
                });
                ui.separator();
                let remove = match mark.kind {
                    AnnotKind::Underline => t!("annot.remove_underline"),
                    AnnotKind::StrikeOut => t!("annot.remove_strikeout"),
                    _ => t!("annot.remove_highlight"),
                };
                if ui.add(egui::Button::new(format!("{}  {remove}", icon::TRASH)).frame(false)).clicked() {
                    action = Some(MarkupAction::Remove);
                }
                if !mark.contents.is_empty() && ui.add(egui::Button::new(format!("{}  {}", icon::COPY, t!("annot.copy_text"))).frame(false)).clicked() {
                    action = Some(MarkupAction::Copy);
                }
            });
        });
        // Se cierra al elegir, con Esc o al pulsar fuera.
        let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
        if action.is_some() || outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.markup_menu = None;
        }
        let r = match action {
            Some(MarkupAction::Color(c)) if c != mark.color => self.doc.set_annotation_color(mark.page, mark.index, c).map(|_| true),
            Some(MarkupAction::Remove) => self.doc.remove_annotation(mark.page, mark.index).map(|_| true),
            Some(MarkupAction::Copy) => {
                ctx.copy_text(mark.contents.clone());
                Ok(false)
            }
            _ => Ok(false),
        };
        match r {
            Ok(true) => {
                ev.modified = true;
                self.invalidate_page(mark.page);
                // La anotación seleccionada ha cambiado de sitio en la lista.
                if let Some(a) = self.annot.as_mut() {
                    a.selected = None;
                }
            }
            Ok(false) => {}
            Err(e) => ev.annot_error = Some(format!("{e:#}")),
        }
    }
}
