//! Vista de documento: renderizado por teselas, entrada de ratón/teclado
//! hacia PDFium y búsqueda.

use eframe::egui::{
    self, Color32, CursorIcon, Event, EventFilter, Id, Key, Modifiers, PointerButton, Pos2, Rect, Sense, TextureHandle,
    TextureOptions, Vec2,
};
use form_pdf_reader::pdfium::{Document, SearchHit, Updates, keys};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Puntos tipográficos -> píxeles lógicos al 100 %.
const PT_TO_PX: f32 = 96.0 / 72.0;
const PAGE_GAP: f32 = 16.0;
const MARGIN: f32 = 20.0;
/// Alto de tesela en píxeles físicos.
const TILE_PX: i32 = 1024;
/// Máximo de teselas renderizadas por fotograma (mantiene la UI fluida).
const TILE_BUDGET: usize = 8;

struct Tile {
    tex: TextureHandle,
    scale_key: u32,
    dirty: bool,
}

/// Cambios que la vista comunica a la aplicación tras cada fotograma.
#[derive(Default)]
pub struct ViewEvents {
    pub modified: bool,
    /// El formulario cambió de estructura y se recolocaron los controles.
    pub restructured: bool,
    pub print_requested: bool,
    pub submit_requested: Option<String>,
    pub uris: Vec<String>,
    /// Adjuntos que el formulario pide ver (ficheros temporales).
    pub open_files: Vec<std::path::PathBuf>,
}

static NEXT_VIEW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Qué hace el ratón sobre el documento.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Rellenar formularios (clics y teclado van a PDFium).
    Form,
    /// Mano: arrastrar desplaza el documento.
    Hand,
    /// Seleccionar texto para copiarlo.
    Text,
}

/// Disposición de las páginas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageLayout {
    /// Todas las páginas seguidas, una debajo de otra (por defecto).
    Continuous,
    /// Una página entera cada vez; flechas y rueda pasan de página.
    Single,
    /// Dos columnas: páginas por parejas, desplazamiento continuo.
    TwoColumns,
}

/// Selección de texto: del carácter `a` al `b` (página, índice), en el orden
/// en que se arrastró.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Selection {
    a: (usize, i32),
    b: (usize, i32),
}

pub struct DocView {
    pub doc: Document,
    pub zoom: f32,
    pub fit_width: bool,
    tiles: HashMap<(usize, i32), Tile>,
    /// Rectángulos de página en pantalla del último fotograma.
    page_rects: Vec<(usize, Rect)>,
    scale_px: f32,
    scroll_to: Option<f32>,
    pub current_page: usize,
    /// Página que recibió el botón pulsado (para arrastrar seleccionando).
    pressed_page: Option<usize>,
    last_click: Option<(Instant, Pos2)>,
    has_focus: bool,
    /// Fotogramas restantes en los que comprobar cambios de estructura.
    struct_checks: u32,
    pub search_hits: Vec<SearchHit>,
    pub search_index: usize,
    area_id: Id,
    pub tool: Tool,
    pub layout: PageLayout,
    /// Desplazamiento a aplicar en el próximo fotograma.
    pending_offset: Option<Vec2>,
    /// Desplazamiento y tamaño de la zona visible del último fotograma.
    offset: Vec2,
    viewport: Vec2,
    content: Vec2,
    /// Herramienta mano: última posición del puntero mientras se arrastra.
    hand_last: Option<Pos2>,
    selection: Option<Selection>,
    selecting: bool,
    /// Rectángulos de la selección por página (coordenadas de página).
    selection_rects: Vec<(usize, Vec<[f64; 4]>)>,
    /// Rueda acumulada en vista de una página (para pasar de página).
    wheel_acc: f32,
}

impl DocView {
    pub fn new(doc: Document) -> DocView {
        DocView {
            doc,
            zoom: 1.0,
            fit_width: true,
            tiles: HashMap::new(),
            page_rects: Vec::new(),
            scale_px: 1.0,
            scroll_to: None,
            current_page: 0,
            pressed_page: None,
            last_click: None,
            has_focus: false,
            struct_checks: 0,
            search_hits: Vec::new(),
            search_index: 0,
            // Un identificador por vista: cada pestaña conserva su propio
            // desplazamiento y su foco.
            area_id: Id::new(("pdf_document_area", NEXT_VIEW.fetch_add(1, std::sync::atomic::Ordering::Relaxed))),
            tool: Tool::Form,
            layout: PageLayout::Continuous,
            pending_offset: None,
            offset: Vec2::ZERO,
            viewport: Vec2::new(800.0, 600.0),
            content: Vec2::ZERO,
            hand_last: None,
            selection: None,
            selecting: false,
            selection_rects: Vec::new(),
            wheel_acc: 0.0,
        }
    }

    pub fn page_count(&self) -> usize {
        self.doc.page_count()
    }

    pub fn set_zoom(&mut self, z: f32) {
        self.fit_width = false;
        self.zoom = z.clamp(0.25, 5.0);
    }

    /// Disposición efectiva: los XFA dinámicos en vista continua son una sola
    /// página muy larga y siempre se muestran en continuo.
    pub fn effective_layout(&self) -> PageLayout {
        if self.doc.is_continuous() { PageLayout::Continuous } else { self.layout }
    }

    pub fn set_layout(&mut self, l: PageLayout) {
        if l != self.layout {
            self.layout = l;
            let p = self.current_page;
            self.go_to_page(p);
        }
    }

    pub fn go_to_page(&mut self, idx: usize) {
        let idx = idx.min(self.page_count().saturating_sub(1));
        if self.effective_layout() == PageLayout::Single {
            self.current_page = idx;
            self.doc.set_current_page(idx);
            self.pending_offset = Some(Vec2::new(self.offset.x, 0.0));
            return;
        }
        let sizes: Vec<Vec2> = (0..self.page_count()).map(|i| self.display_size(i)).collect();
        let (rects, _) = self.arrange(&sizes, self.viewport.x);
        if let Some((_, r)) = rects.iter().find(|(i, _)| *i == idx) {
            self.pending_offset = Some(Vec2::new(self.offset.x, (r.min.y - 8.0).max(0.0)));
        }
    }

    /// Posición de cada página dentro del contenido (origen arriba a la
    /// izquierda) y tamaño total del contenido, según la disposición.
    fn arrange(&self, sizes: &[Vec2], avail_w: f32) -> (Vec<(usize, Rect)>, Vec2) {
        let mut rects = Vec::new();
        match self.effective_layout() {
            PageLayout::Continuous => {
                let w = sizes.iter().map(|s| s.x).fold(0.0, f32::max) + 2.0 * MARGIN;
                let total_w = w.max(avail_w);
                let mut y = MARGIN;
                for (i, sz) in sizes.iter().enumerate() {
                    rects.push((i, Rect::from_min_size(Pos2::new((total_w - sz.x) / 2.0, y), *sz)));
                    y += sz.y + PAGE_GAP;
                }
                (rects, Vec2::new(total_w, (y - PAGE_GAP + MARGIN).max(0.0)))
            }
            PageLayout::Single => {
                let i = self.current_page.min(sizes.len().saturating_sub(1));
                let Some(sz) = sizes.get(i) else { return (rects, Vec2::ZERO) };
                let total_w = (sz.x + 2.0 * MARGIN).max(avail_w);
                rects.push((i, Rect::from_min_size(Pos2::new((total_w - sz.x) / 2.0, MARGIN), *sz)));
                (rects, Vec2::new(total_w, sz.y + 2.0 * MARGIN))
            }
            PageLayout::TwoColumns => {
                let rows: Vec<(f32, f32)> = sizes
                    .chunks(2)
                    .map(|c| (c.iter().map(|s| s.x).sum::<f32>() + PAGE_GAP * (c.len() as f32 - 1.0), c.iter().map(|s| s.y).fold(0.0, f32::max)))
                    .collect();
                let w = rows.iter().map(|r| r.0).fold(0.0, f32::max) + 2.0 * MARGIN;
                let total_w = w.max(avail_w);
                let mut y = MARGIN;
                for (k, (row_w, row_h)) in rows.iter().enumerate() {
                    let mut x = (total_w - row_w) / 2.0;
                    for j in 0..2 {
                        let i = k * 2 + j;
                        if let Some(sz) = sizes.get(i) {
                            rects.push((i, Rect::from_min_size(Pos2::new(x, y), *sz)));
                            x += sz.x + PAGE_GAP;
                        }
                    }
                    y += row_h + PAGE_GAP;
                }
                (rects, Vec2::new(total_w, (y - PAGE_GAP + MARGIN).max(0.0)))
            }
        }
    }

    /// Desplaza la vista `dy` píxeles.
    fn scroll_by(&mut self, dy: f32) {
        let max_y = (self.content.y - self.viewport.y).max(0.0);
        let y = (self.offset.y + dy).clamp(0.0, max_y);
        self.pending_offset = Some(Vec2::new(self.offset.x, y));
    }

    fn page_step(&mut self, forward: bool) {
        let n = self.page_count();
        if forward && self.current_page + 1 < n {
            let p = self.current_page + 1;
            self.go_to_page(p);
        } else if !forward && self.current_page > 0 {
            let p = self.current_page - 1;
            self.go_to_page(p);
            // Al retroceder se muestra el final de la página anterior.
            if self.effective_layout() == PageLayout::Single {
                self.pending_offset = Some(Vec2::new(self.offset.x, f32::MAX));
            }
        }
    }

    /// Teclas de navegación (flechas, Av/Re Pág, Inicio/Fin). Devuelve true
    /// si la tecla se ha usado para moverse por el documento.
    fn navigate(&mut self, key: Key) -> bool {
        let single = self.effective_layout() == PageLayout::Single;
        let line = 48.0;
        let pagesz = (self.viewport.y - 40.0).max(line);
        let max_y = (self.content.y - self.viewport.y).max(0.0);
        let at_top = self.offset.y <= 0.5;
        let at_bottom = self.offset.y >= max_y - 0.5;
        match key {
            Key::ArrowDown | Key::PageDown => {
                if single && (key == Key::PageDown || at_bottom) {
                    self.page_step(true);
                } else {
                    self.scroll_by(if key == Key::PageDown { pagesz } else { line });
                }
            }
            Key::ArrowUp | Key::PageUp => {
                if single && (key == Key::PageUp || at_top) {
                    self.page_step(false);
                } else {
                    self.scroll_by(if key == Key::PageUp { -pagesz } else { -line });
                }
            }
            Key::Home => self.go_to_page(0),
            Key::End => {
                let n = self.page_count();
                self.go_to_page(n.saturating_sub(1));
            }
            _ => return false,
        }
        true
    }

    /// Recalcula los rectángulos de la selección de texto.
    fn update_selection_rects(&mut self) {
        self.selection_rects.clear();
        for (p, start, count) in self.selection_ranges() {
            self.selection_rects.push((p, self.doc.text_rects(p, start, count)));
        }
    }

    /// Tramos (página, inicio, cantidad) de la selección, en orden.
    fn selection_ranges(&self) -> Vec<(usize, i32, i32)> {
        let Some(sel) = self.selection else { return Vec::new() };
        let (a, b) = if sel.a <= sel.b { (sel.a, sel.b) } else { (sel.b, sel.a) };
        let mut v = Vec::new();
        for p in a.0..=b.0 {
            let n = self.doc.text_char_count(p);
            if n == 0 {
                continue;
            }
            let start = if p == a.0 { a.1 } else { 0 };
            let end = if p == b.0 { b.1 } else { n - 1 };
            if end >= start {
                v.push((p, start, end - start + 1));
            }
        }
        v
    }

    /// Texto seleccionado con la herramienta de texto.
    pub fn selected_text(&self) -> String {
        self.selection_ranges().into_iter().map(|(p, s, c)| self.doc.text_range(p, s, c)).collect::<Vec<_>>().join("\n")
    }

    pub fn select_all_text(&mut self) {
        let n = self.page_count();
        let last = (0..n).rev().find(|&p| self.doc.text_char_count(p) > 0);
        if let Some(lp) = last {
            let first = (0..n).find(|&p| self.doc.text_char_count(p) > 0).unwrap_or(0);
            self.selection = Some(Selection { a: (first, 0), b: (lp, self.doc.text_char_count(lp) - 1) });
            self.update_selection_rects();
        }
    }

    pub fn set_tool(&mut self, t: Tool) {
        if t != self.tool {
            if self.tool == Tool::Form {
                self.commit_focus();
            }
            self.tool = t;
            self.selection = None;
            self.selection_rects.clear();
            self.selecting = false;
            self.hand_last = None;
        }
    }

    /// Tamaño lógico de la página tal como se muestra (en vista continua se
    /// recorta al final del contenido).
    fn display_size(&self, i: usize) -> Vec2 {
        let (w, _) = self.doc.page_size(i);
        let h = self.doc.content_height(i);
        Vec2::new(w, h) * self.zoom * PT_TO_PX
    }

    fn invalidate_page(&mut self, page: usize) {
        for ((p, _), t) in self.tiles.iter_mut() {
            if *p == page {
                t.dirty = true;
            }
        }
    }

    fn invalidate_all(&mut self) {
        for t in self.tiles.values_mut() {
            t.dirty = true;
        }
    }

    /// Envía a PDFium el foco perdido para que confirme el valor del campo
    /// que se está editando (necesario antes de guardar).
    pub fn commit_focus(&mut self) {
        self.doc.kill_focus();
    }

    /// Procesa timers y notificaciones de PDFium.
    fn pump(&mut self, ctx: &egui::Context, ev: &mut ViewEvents) {
        if let Some(next) = self.doc.process_timers() {
            ctx.request_repaint_after(next.max(Duration::from_millis(10)));
        }
        let u: Updates = self.doc.take_updates();
        if u.invalidate_all || u.layout_changed {
            self.invalidate_all();
            if u.layout_changed {
                // El número o el tamaño de páginas puede haber cambiado.
                let n = self.page_count();
                self.tiles.retain(|(p, _), _| *p < n);
            }
        }
        for p in &u.invalidated {
            self.invalidate_page(*p);
        }
        if u.form_changed {
            ev.modified = true;
        }
        if let Some(g) = u.goto_page {
            self.go_to_page(g);
        }
        ev.print_requested |= u.print_requested;
        if u.submit_requested.is_some() {
            ev.submit_requested = u.submit_requested;
        }
        ev.uris.extend(u.uris);
        ev.open_files.extend(u.open_files);
        if !u.invalidated.is_empty() || u.invalidate_all || u.layout_changed {
            ctx.request_repaint();
        }
    }

    /// Página y coordenadas de página bajo una posición de pantalla.
    fn hit(&self, pos: Pos2) -> Option<(usize, f64, f64)> {
        let (i, r) = self.page_rects.iter().find(|(_, r)| r.contains(pos))?;
        self.to_page_coords(*i, *r, pos).map(|(x, y)| (*i, x, y))
    }

    fn to_page_coords(&self, i: usize, r: Rect, pos: Pos2) -> Option<(f64, f64)> {
        let (w, h) = self.doc.page_size(i);
        let s = self.zoom * PT_TO_PX;
        let (dw, dh) = ((w * s) as f64, (h * s) as f64);
        let d = pos - r.min;
        Some(self.doc.device_to_page(i, dw, dh, d.x as f64, d.y as f64))
    }

    fn page_to_screen(&self, i: usize, r: Rect, px: f64, py: f64) -> Pos2 {
        let (w, h) = self.doc.page_size(i);
        let s = self.zoom * PT_TO_PX;
        let (dx, dy) = self.doc.page_to_device(i, (w * s) as f64, (h * s) as f64, px, py);
        r.min + Vec2::new(dx as f32, dy as f32)
    }

    pub fn search(&mut self, q: &str) -> usize {
        self.search_hits = self.doc.search(q);
        self.search_index = 0;
        if let Some(h) = self.search_hits.first() {
            let p = h.page;
            self.go_to_page(p);
        }
        self.search_hits.len()
    }

    pub fn search_step(&mut self, forward: bool) {
        if self.search_hits.is_empty() {
            return;
        }
        let n = self.search_hits.len();
        self.search_index = if forward { (self.search_index + 1) % n } else { (self.search_index + n - 1) % n };
        let p = self.search_hits[self.search_index].page;
        self.go_to_page(p);
    }

    // -----------------------------------------------------------------------
    // Entrada
    // -----------------------------------------------------------------------

    fn mods(m: &Modifiers) -> i32 {
        let mut f = 0;
        if m.shift {
            f |= keys::MOD_SHIFT;
        }
        if m.ctrl || m.command {
            f |= keys::MOD_CTRL;
        }
        if m.alt {
            f |= keys::MOD_ALT;
        }
        f
    }

    fn vk(key: Key) -> Option<i32> {
        use Key::*;
        Some(match key {
            ArrowDown => keys::VK_DOWN,
            ArrowUp => keys::VK_UP,
            ArrowLeft => keys::VK_LEFT,
            ArrowRight => keys::VK_RIGHT,
            Escape => keys::VK_ESCAPE,
            Tab => keys::VK_TAB,
            Backspace => keys::VK_BACK,
            Enter => keys::VK_RETURN,
            Space => keys::VK_SPACE,
            Insert => keys::VK_INSERT,
            Delete => keys::VK_DELETE,
            Home => keys::VK_HOME,
            End => keys::VK_END,
            PageUp => keys::VK_PRIOR,
            PageDown => keys::VK_NEXT,
            k => {
                let name = k.name();
                if name.len() == 1 {
                    let c = name.chars().next().unwrap();
                    if c.is_ascii_uppercase() {
                        keys::VK_A + (c as i32 - 'A' as i32)
                    } else if c.is_ascii_digit() {
                        keys::VK_0 + (c as i32 - '0' as i32)
                    } else {
                        return None;
                    }
                } else {
                    let n = name.strip_prefix('F').and_then(|s| s.parse::<i32>().ok())?;
                    if (1..=12).contains(&n) { keys::VK_F1 + n - 1 } else { return None }
                }
            }
        })
    }

    /// Herramientas mano y texto: el ratón no va a PDFium.
    fn handle_pointer_tool(&mut self, events: &[Event], area_rect: Rect) -> bool {
        let mut clicked = false;
        for e in events {
            match e {
                Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers } if area_rect.contains(*pos) => {
                    clicked = true;
                    match self.tool {
                        Tool::Hand => self.hand_last = Some(*pos),
                        Tool::Text => {
                            self.selecting = true;
                            let hit = self.hit(*pos).and_then(|(p, x, y)| self.doc.text_char_at(p, x, y).map(|c| (p, c)));
                            match (hit, self.selection) {
                                // Mayúsculas + clic: ampliar la selección.
                                (Some(h), Some(sel)) if modifiers.shift => self.selection = Some(Selection { a: sel.a, b: h }),
                                (Some(h), _) => self.selection = Some(Selection { a: h, b: h }),
                                (None, _) => self.selection = None,
                            }
                            self.update_selection_rects();
                        }
                        Tool::Form => {}
                    }
                }
                Event::PointerButton { button: PointerButton::Primary, pressed: false, .. } => {
                    self.hand_last = None;
                    self.selecting = false;
                }
                Event::PointerMoved(pos) => match self.tool {
                    Tool::Hand => {
                        if let Some(last) = self.hand_last {
                            let d = *pos - last;
                            let base = self.pending_offset.unwrap_or(self.offset);
                            let max = (self.content - self.viewport).max(Vec2::ZERO);
                            self.pending_offset = Some((base - d).clamp(Vec2::ZERO, max));
                            self.hand_last = Some(*pos);
                        }
                    }
                    Tool::Text if self.selecting => {
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
                    _ => {}
                },
                _ => {}
            }
        }
        clicked
    }

    fn handle_pointer(&mut self, events: &[Event], ev: &mut ViewEvents, area_rect: Rect) -> bool {
        if self.tool != Tool::Form {
            return self.handle_pointer_tool(events, area_rect);
        }
        let mut clicked_doc = false;
        for e in events {
            match e {
                Event::PointerMoved(pos) => {
                    if let Some(p) = self.pressed_page {
                        if let Some((_, r)) = self.page_rects.iter().find(|(i, _)| *i == p).copied()
                            && let Some((x, y)) = self.to_page_coords(p, r, *pos) {
                                self.doc.mouse_move(p, x, y, keys::MOD_LBUTTON);
                            }
                    } else if area_rect.contains(*pos)
                        && let Some((p, x, y)) = self.hit(*pos) {
                            self.doc.mouse_move(p, x, y, 0);
                        }
                }
                Event::PointerButton { pos, button, pressed, modifiers } => {
                    log::debug!("ratón {button:?} pressed={pressed} en {pos:?} -> {:?}", self.hit(*pos));
                    let m = Self::mods(modifiers);
                    match (button, pressed) {
                        (PointerButton::Primary, true) => {
                            if !area_rect.contains(*pos) {
                                continue;
                            }
                            let Some((p, x, y)) = self.hit(*pos) else {
                                // Clic fuera de las páginas: quitar el foco.
                                self.commit_focus();
                                continue;
                            };
                            clicked_doc = true;
                            let now = Instant::now();
                            let double = self.last_click.is_some_and(|(t, q)| now.duration_since(t) < Duration::from_millis(400) && q.distance(*pos) < 5.0);
                            if double {
                                self.doc.left_double_click(p, x, y, m);
                                self.last_click = None;
                            } else {
                                self.doc.left_down(p, x, y, m);
                                self.last_click = Some((now, *pos));
                            }
                            self.pressed_page = Some(p);
                            self.current_page = p;
                            ev.modified |= false;
                        }
                        (PointerButton::Primary, false) => {
                            if let Some(p) = self.pressed_page.take()
                                && let Some((_, r)) = self.page_rects.iter().find(|(i, _)| *i == p).copied()
                                    && let Some((x, y)) = self.to_page_coords(p, r, *pos) {
                                        self.doc.left_up(p, x, y, m);
                                    }
                        }
                        (PointerButton::Secondary, pressed) => {
                            if let Some((p, x, y)) = self.hit(*pos).filter(|_| area_rect.contains(*pos)) {
                                if *pressed {
                                    self.doc.right_down(p, x, y, m);
                                } else {
                                    self.doc.right_up(p, x, y, m);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        clicked_doc
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context, events: &[Event], ev: &mut ViewEvents) {
        for e in events {
            if !matches!(e, Event::PointerMoved(_) | Event::MouseMoved(_)) {
                log::debug!("teclado -> {e:?}");
            }
            // Navegación con el teclado, salvo que se esté editando un campo.
            if let Event::Key { key, pressed: true, modifiers, .. } = e
                && !(modifiers.ctrl || modifiers.command || modifiers.alt)
                && (self.tool != Tool::Form || !self.doc.editing_field())
                && self.navigate(*key)
            {
                continue;
            }
            if self.tool != Tool::Form {
                match e {
                    Event::Copy => {
                        let s = self.selected_text();
                        if !s.is_empty() {
                            log::info!("texto copiado ({} caracteres): {}", s.chars().count(), s.chars().take(200).collect::<String>());
                            ctx.copy_text(s);
                        }
                    }
                    Event::Key { key: Key::A, pressed: true, modifiers, .. } if (modifiers.ctrl || modifiers.command) && self.tool == Tool::Text => {
                        self.select_all_text();
                    }
                    Event::Key { key: Key::Escape, pressed: true, .. } => {
                        self.selection = None;
                        self.selection_rects.clear();
                    }
                    _ => {}
                }
                continue;
            }
            match e {
                Event::Text(t) => {
                    for c in t.chars() {
                        if !c.is_control() {
                            self.doc.char_input(c as u32, 0);
                            ev.modified = true;
                        }
                    }
                }
                Event::Ime(egui::ImeEvent::Commit(t)) => {
                    self.doc.type_text(t);
                    ev.modified = true;
                }
                Event::Copy => {
                    let s = self.doc.selected_text();
                    if !s.is_empty() {
                        ctx.copy_text(s);
                    }
                }
                Event::Cut => {
                    let s = self.doc.selected_text();
                    if !s.is_empty() {
                        ctx.copy_text(s);
                        self.doc.replace_selection("");
                        ev.modified = true;
                    }
                }
                Event::Paste(s) => {
                    // Los formularios no suelen admitir saltos de línea.
                    self.doc.replace_selection(s);
                    ev.modified = true;
                }
                Event::Key { key, pressed, modifiers, repeat, .. } => {
                    let ctrl = modifiers.ctrl || modifiers.command;
                    if ctrl && *pressed {
                        match key {
                            Key::A => {
                                self.doc.select_all();
                                continue;
                            }
                            Key::Z => {
                                if modifiers.shift { self.doc.redo() } else { self.doc.undo() };
                                ev.modified = true;
                                continue;
                            }
                            Key::Y => {
                                self.doc.redo();
                                ev.modified = true;
                                continue;
                            }
                            Key::C | Key::V | Key::X => continue,
                            _ => {}
                        }
                    }
                    let Some(vk) = Self::vk(*key) else { continue };
                    let mut m = Self::mods(modifiers);
                    if *repeat {
                        m |= sys_autorepeat();
                    }
                    if *pressed {
                        self.doc.key_down(vk, m);
                        if let Some(c) = keys::char_for_vk(vk) {
                            self.doc.char_input(c as u32, m);
                        }
                        if matches!(vk, keys::VK_BACK | keys::VK_DELETE | keys::VK_SPACE | keys::VK_RETURN | keys::VK_UP | keys::VK_DOWN) {
                            ev.modified = true;
                        }
                    } else {
                        self.doc.key_up(vk, m);
                    }
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Dibujo
    // -----------------------------------------------------------------------

    pub fn ui(&mut self, ui: &mut egui::Ui) -> ViewEvents {
        let ctx = ui.ctx().clone();
        let mut ev = ViewEvents::default();
        self.pump(&ctx, &mut ev);

        let ppp = ctx.pixels_per_point();
        let avail = ui.available_size();
        let n = self.page_count();
        if self.fit_width && n > 0 {
            let maxw = (0..n.min(50)).map(|i| self.doc.page_size(i).0).fold(0.0f32, f32::max);
            if maxw > 0.0 {
                let free_w = avail.x - 2.0 * MARGIN - 18.0;
                self.zoom = match self.effective_layout() {
                    PageLayout::Continuous => free_w / (maxw * PT_TO_PX),
                    // Dos páginas una al lado de otra.
                    PageLayout::TwoColumns => (free_w - PAGE_GAP) / (2.0 * maxw * PT_TO_PX),
                    // La página entera a la vista.
                    PageLayout::Single => {
                        let i = self.current_page.min(n - 1);
                        let (w, _) = self.doc.page_size(i);
                        let h = self.doc.content_height(i);
                        (free_w / (w * PT_TO_PX)).min((avail.y - 2.0 * MARGIN - 4.0) / (h * PT_TO_PX))
                    }
                }
                .clamp(0.25, 2.0);
            }
        }
        self.scale_px = self.zoom * PT_TO_PX * ppp;

        // Eventos de este fotograma. La rueda se decide antes de la ScrollArea.
        let events: Vec<Event> = ui.input(|i| i.events.clone());
        let my_layer = ui.layer_id();
        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let over_doc = pointer_pos.is_some_and(|p| self.page_rects.iter().any(|(_, r)| r.contains(p)));

        // Ctrl + rueda = zoom.
        let ctrl_wheel: f32 = events
            .iter()
            .filter_map(|e| match e {
                Event::MouseWheel { delta, modifiers, .. } if modifiers.ctrl || modifiers.command => Some(delta.y),
                _ => None,
            })
            .sum();
        if ctrl_wheel != 0.0 {
            let f = if ctrl_wheel > 0.0 { 1.1 } else { 1.0 / 1.1 };
            self.set_zoom(self.zoom * f);
            ui.input_mut(|i| {
                i.smooth_scroll_delta = Vec2::ZERO;
            });
        } else if self.doc.list_open() && over_doc {
            // Rueda sobre una lista desplegada: la desplaza PDFium.
            let dy: f32 = events
                .iter()
                .filter_map(|e| match e {
                    Event::MouseWheel { delta, .. } => Some(delta.y),
                    _ => None,
                })
                .sum();
            if dy != 0.0
                && let Some((p, x, y)) = pointer_pos.and_then(|pp| self.hit(pp)) {
                    let steps = (dy.signum() * 120.0) as i32;
                    if self.doc.wheel(p, x, y, 0, steps, 0) {
                        ui.input_mut(|i| {
                            i.smooth_scroll_delta = Vec2::ZERO;
                        });
                    }
                }
        }

        // Vista de una página: la rueda pasa de página al llegar al borde.
        if ctrl_wheel == 0.0 && !self.doc.list_open() && over_doc && self.effective_layout() == PageLayout::Single {
            let dy: f32 = events
                .iter()
                .filter_map(|e| match e {
                    Event::MouseWheel { delta, .. } => Some(delta.y),
                    _ => None,
                })
                .sum();
            let max_y = (self.content.y - self.viewport.y).max(0.0);
            let at_top = self.offset.y <= 0.5;
            let at_bottom = self.offset.y >= max_y - 0.5;
            if (dy < 0.0 && at_bottom) || (dy > 0.0 && at_top) {
                self.wheel_acc += dy;
                ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
                if self.wheel_acc.abs() >= 3.0 {
                    let fwd = self.wheel_acc < 0.0;
                    self.wheel_acc = 0.0;
                    self.page_step(fwd);
                }
            } else if dy != 0.0 {
                self.wheel_acc = 0.0;
            }
        }

        // Disposición de páginas (coordenadas de contenido).
        let sizes: Vec<Vec2> = (0..n).map(|i| self.display_size(i)).collect();
        let (layout_rects, total) = self.arrange(&sizes, avail.x);

        let mut area = egui::ScrollArea::both()
            .id_salt(self.area_id)
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource { drag: egui::scroll_area::DragScroll::Never, ..Default::default() });
        if let Some(y) = self.scroll_to.take() {
            area = area.vertical_scroll_offset(y.max(0.0));
        }
        if let Some(o) = self.pending_offset.take() {
            let max = (total - avail).max(Vec2::ZERO);
            area = area.scroll_offset(o.clamp(Vec2::ZERO, max));
        }

        let mut new_rects = Vec::new();
        let mut clicked = false;
        let out = area.show_viewport(ui, |ui, viewport| {
            let total = Vec2::new(total.x.max(ui.available_width()), total.y);
            let (rect, _) = ui.allocate_exact_size(total, Sense::hover());
            let resp = ui.interact(rect, self.area_id, Sense::click_and_drag());
            let painter = ui.painter_at(ui.clip_rect());
            painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

            let visible = viewport.translate(rect.min.to_vec2());
            let mut budget = TILE_BUDGET;
            let mut center_page = None;
            for (i, r) in layout_rects.iter() {
                let i = *i;
                let pr = r.translate(rect.min.to_vec2());
                if pr.min.y <= visible.center().y + PAGE_GAP && center_page.is_none() && pr.max.y + PAGE_GAP >= visible.center().y {
                    center_page = Some(i);
                }
                if !pr.intersects(visible.expand(200.0)) {
                    continue;
                }
                new_rects.push((i, pr));
                // Sombra y fondo de página.
                painter.rect_filled(pr.translate(Vec2::new(2.0, 3.0)), 2.0, Color32::from_black_alpha(40));
                painter.rect_filled(pr, 0.0, Color32::WHITE);
                self.draw_tiles(&ctx, &painter, i, pr, visible, &mut budget);
                self.draw_search(&painter, i, pr);
                self.draw_selection(&painter, i, pr);
            }
            if let Some(c) = center_page.filter(|_| self.effective_layout() != PageLayout::Single) {
                self.current_page = c;
                self.doc.set_current_page(c);
            }
            if budget == 0 {
                ctx.request_repaint();
            }

            if resp.clicked() || resp.drag_started() {
                resp.request_focus();
            }
            self.has_focus = resp.has_focus();
            if self.has_focus {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(self.area_id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
                });
            }
            resp
        });
        self.page_rects = new_rects;
        let area_rect = out.inner_rect;
        self.offset = out.state.offset;
        self.viewport = out.inner_rect.size();
        self.content = out.content_size;

        // Ratón (siempre) y teclado (si el documento tiene el foco). Se
        // descartan las pulsaciones y movimientos sobre otra capa (un diálogo,
        // un menú desplegable): el clic en "Entendido" llegaba también al PDF.
        let covered = |p: Pos2| ctx.layer_id_at(p).is_some_and(|l| l != my_layer);
        let pointer_events: Vec<Event> = events
            .iter()
            .filter(|e| match e {
                Event::PointerButton { pos, pressed: true, .. } | Event::PointerMoved(pos) => !covered(*pos),
                _ => true,
            })
            .cloned()
            .collect();
        clicked |= self.handle_pointer(&pointer_events, &mut ev, area_rect);
        if clicked {
            ctx.memory_mut(|m| m.request_focus(self.area_id));
            self.has_focus = true;
        }
        let focused = ctx.memory(|m| m.focused());
        if self.has_focus && (focused.is_none() || focused == Some(self.area_id)) {
            self.handle_keyboard(&ctx, &events, &mut ev);
        }

        // Cambios de estructura (secciones que aparecen, filas añadidas):
        // recolocar los controles de PDFium (ver xfa::compat).
        // Tras una interacción, PDFium puede tardar un evento más en maquetar:
        // se vuelve a comprobar durante unos fotogramas.
        let had_input = events.iter().any(|e| matches!(e, Event::PointerButton { .. } | Event::Key { .. } | Event::Text(_) | Event::Paste(_) | Event::Cut));
        if had_input {
            self.struct_checks = 6;
        }
        let check = self.struct_checks > 0;
        if check {
            self.struct_checks -= 1;
            ctx.request_repaint();
        }
        if check && self.doc.refresh_if_structure_changed() {
            self.invalidate_all();
            ev.restructured = true;
            ctx.request_repaint();
        }

        // Cursor según la herramienta y el tipo de campo bajo el puntero.
        if self.tool == Tool::Hand && pointer_pos.is_some_and(|p| area_rect.contains(p)) {
            ctx.set_cursor_icon(if self.hand_last.is_some() { CursorIcon::Grabbing } else { CursorIcon::Grab });
        } else if self.tool == Tool::Text && pointer_pos.is_some_and(|p| area_rect.contains(p)) {
            ctx.set_cursor_icon(CursorIcon::Text);
        } else if let Some(p) = pointer_pos.filter(|p| area_rect.contains(*p))
            && let Some((i, x, y)) = self.hit(p) {
                let t = self.doc.field_at(i, x, y);
                let icon = match t {
                    6 | 15 => Some(CursorIcon::Text),
                    1 | 2 | 3 | 4 | 5 | 9 | 10 | 11 | 12 | 13 => Some(CursorIcon::PointingHand),
                    _ => None,
                };
                if let Some(icon) = icon {
                    ctx.set_cursor_icon(icon);
                }
            }

        // Lo que haya pedido PDFium durante el manejo de eventos.
        self.pump(&ctx, &mut ev);
        ev
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_tiles(&mut self, ctx: &egui::Context, painter: &egui::Painter, i: usize, pr: Rect, visible: Rect, budget: &mut usize) {
        let (w, h) = self.doc.page_size(i);
        let page_w = (w * self.scale_px).round().max(1.0) as i32;
        let page_h = (h * self.scale_px).round().max(1.0) as i32;
        let shown_h = ((pr.height() / pr.width()) * page_w as f32).round() as i32;
        let key = (self.scale_px * 1000.0) as u32;
        let ppp = ctx.pixels_per_point();
        let ntiles = (shown_h + TILE_PX - 1) / TILE_PX;
        for t in 0..ntiles {
            let ty = t * TILE_PX;
            let th = TILE_PX.min(shown_h - ty);
            let tile_rect = Rect::from_min_size(pr.min + Vec2::new(0.0, ty as f32 / ppp), Vec2::new(pr.width(), th as f32 / ppp));
            if !tile_rect.intersects(visible.expand(300.0)) {
                continue;
            }
            let needs = match self.tiles.get(&(i, t)) {
                Some(tile) => tile.dirty || tile.scale_key != key,
                None => true,
            };
            if needs && *budget > 0 {
                *budget -= 1;
                if let Some(px) = self.doc.render_region(i, page_w, page_h, 0, ty, page_w, th) {
                    let img = egui::ColorImage::from_rgba_unmultiplied([page_w as usize, th as usize], &px);
                    match self.tiles.get_mut(&(i, t)) {
                        Some(tile) => {
                            tile.tex.set(img, TextureOptions::LINEAR);
                            tile.scale_key = key;
                            tile.dirty = false;
                        }
                        None => {
                            let tex = ctx.load_texture(format!("p{i}t{t}"), img, TextureOptions::LINEAR);
                            self.tiles.insert((i, t), Tile { tex, scale_key: key, dirty: false });
                        }
                    }
                }
            }
            if let Some(tile) = self.tiles.get(&(i, t)) {
                painter.image(tile.tex.id(), tile_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        }
    }

    fn draw_selection(&self, painter: &egui::Painter, i: usize, pr: Rect) {
        for (p, rects) in &self.selection_rects {
            if *p != i {
                continue;
            }
            for r in rects {
                let a = self.page_to_screen(i, pr, r[0], r[1]);
                let b = self.page_to_screen(i, pr, r[2], r[3]);
                painter.rect_filled(Rect::from_two_pos(a, b), 0.0, Color32::from_rgba_unmultiplied(40, 110, 230, 70));
            }
        }
    }

    fn draw_search(&self, painter: &egui::Painter, i: usize, pr: Rect) {
        for (k, hit) in self.search_hits.iter().enumerate() {
            if hit.page != i {
                continue;
            }
            let color = if k == self.search_index { Color32::from_rgba_unmultiplied(255, 140, 0, 90) } else { Color32::from_rgba_unmultiplied(255, 230, 0, 80) };
            for r in &hit.rects {
                let a = self.page_to_screen(i, pr, r[0], r[1]);
                let b = self.page_to_screen(i, pr, r[2], r[3]);
                painter.rect_filled(Rect::from_two_pos(a, b), 1.0, color);
            }
        }
    }

    /// Libera texturas de teselas lejos de la vista (ahorro de memoria).
    pub fn trim_cache(&mut self) {
        if self.tiles.len() > 256 {
            let visible: Vec<usize> = self.page_rects.iter().map(|(i, _)| *i).collect();
            self.tiles.retain(|(p, _), _| visible.contains(p));
        }
    }
}

fn sys_autorepeat() -> i32 {
    32 // FWL_EVENTFLAG_AutoRepeat
}
