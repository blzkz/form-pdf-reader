//! Organizar páginas: miniaturas de todas las páginas, que se eligen (clic,
//! Ctrl+clic, Mayús+clic), se arrastran para cambiarlas de sitio, se giran,
//! se borran o se extraen a un PDF nuevo; y se insertan las de otro PDF.
//!
//! Cada cambio lo aplica la aplicación al momento (ver
//! `form_pdf_reader::pdfium::pages`) y la ventana vuelve a dibujar las
//! miniaturas del documento resultante.

use crate::theme::{self, icon};
use eframe::egui::{self, Color32, Key, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use form_pdf_reader::pdfium::Document;
use form_pdf_reader::pdfium::pages::PageEdit;
use form_pdf_reader::t;
use std::collections::BTreeSet;

/// Ancho de una miniatura.
const THUMB_W: f32 = 112.0;
/// Miniaturas que se dibujan por fotograma (las demás, en los siguientes).
const PER_FRAME: usize = 6;

/// Lo que pide la ventana a la aplicación.
pub enum Outcome {
    Close,
    Edit(Vec<PageEdit>),
    /// Guardar estas páginas como un PDF nuevo.
    Extract(Vec<usize>),
    /// Elegir un PDF para insertar sus páginas delante de la página `at`.
    Insert(usize),
}

/// Un PDF por insertar que pide contraseña.
struct PendingInsert {
    pdf: Vec<u8>,
    name: String,
    at: usize,
    password: String,
    wrong: bool,
    focus: bool,
}

pub struct PagesDialog {
    thumbs: Vec<Option<TextureHandle>>,
    selected: BTreeSet<usize>,
    /// Última página pulsada (para Mayús+clic).
    anchor: Option<usize>,
    /// Página que se arrastra.
    dragging: Option<usize>,
    insert: Option<PendingInsert>,
    /// Nombre del último PDF elegido para insertar.
    insert_name: Option<String>,
    /// El documento tiene firmas digitales (cambiar las páginas las
    /// invalida).
    signed: bool,
}

impl PagesDialog {
    pub fn new(signed: bool) -> PagesDialog {
        PagesDialog { thumbs: Vec::new(), selected: BTreeSet::new(), anchor: None, dragging: None, insert: None, insert_name: None, signed }
    }

    /// El documento ha cambiado: volver a dibujar las miniaturas.
    pub fn reset(&mut self, pages: usize, select: impl IntoIterator<Item = usize>) {
        self.thumbs.clear();
        self.selected = select.into_iter().filter(|&p| p < pages).collect();
        self.anchor = self.selected.iter().next().copied();
        self.dragging = None;
        self.insert = None;
    }

    /// Nombre del PDF elegido para insertar (para pedir su contraseña).
    pub fn set_insert_name(&mut self, name: String) {
        self.insert_name = Some(name);
    }

    /// El PDF elegido para insertar pide contraseña.
    pub fn ask_password(&mut self, pdf: Vec<u8>, at: usize, wrong: bool) {
        let password = self.insert.take().map(|i| i.password).unwrap_or_default();
        let name = self.insert_name.clone().unwrap_or_else(|| t!("pages.other_pdf"));
        self.insert = Some(PendingInsert { pdf, name, at, password, wrong, focus: true });
    }

    fn thumb(&mut self, ctx: &egui::Context, doc: &Document, i: usize, budget: &mut usize) -> Option<TextureHandle> {
        if self.thumbs.len() != doc.page_count() {
            self.thumbs = vec![None; doc.page_count()];
        }
        if self.thumbs[i].is_none() && *budget > 0 {
            *budget -= 1;
            let (pw, ph) = doc.page_size(i);
            let scale = ctx.pixels_per_point();
            let w = (THUMB_W * scale).round().max(1.0) as i32;
            let h = ((THUMB_W * ph / pw.max(1.0)) * scale).round().clamp(1.0, 4000.0) as i32;
            if let Some(px) = doc.render(i, w, h) {
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
                self.thumbs[i] = Some(ctx.load_texture(format!("miniatura-{i}"), img, TextureOptions::LINEAR));
            }
        }
        self.thumbs[i].clone()
    }

    fn click(&mut self, i: usize, mods: egui::Modifiers) {
        if mods.shift
            && let Some(a) = self.anchor
        {
            let (lo, hi) = (a.min(i), a.max(i));
            if !(mods.ctrl || mods.command) {
                self.selected.clear();
            }
            self.selected.extend(lo..=hi);
            return;
        }
        if mods.ctrl || mods.command {
            if !self.selected.remove(&i) {
                self.selected.insert(i);
            }
        } else {
            self.selected.clear();
            self.selected.insert(i);
        }
        self.anchor = Some(i);
    }

    pub fn show(&mut self, ctx: &egui::Context, doc: &Document) -> Option<Outcome> {
        let n = doc.page_count();
        self.selected.retain(|&p| p < n);
        let mut out = None;
        let mut budget = PER_FRAME;
        let modal = theme::modal(ctx, "organizar_paginas").show(ctx, |ui| {
            let p = theme::palette(ui);
            let screen = ctx.content_rect().size();
            ui.set_width((screen.x - 120.0).clamp(360.0, 900.0));
            ui.heading(t!("pages.title"));
            ui.add_space(4.0);
            if self.signed {
                ui.label(RichText::new(format!("{}  {}", icon::WARNING, t!("pages.signed_warning"))).color(p.warn));
            }
            ui.label(RichText::new(t!("pages.hint")).size(12.0).color(p.text_weak));
            ui.add_space(6.0);
            // Acciones sobre las páginas elegidas.
            let sel: Vec<usize> = self.selected.iter().copied().collect();
            let any = !sel.is_empty();
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(any, |ui| {
                    if ui.button(format!("{}  {}", icon::ARROW_COUNTER_CLOCKWISE, t!("pages.rotate_left"))).clicked() {
                        out = Some(Outcome::Edit(vec![PageEdit::Rotate { pages: sel.clone(), quarter_turns: -1 }]));
                    }
                    if ui.button(format!("{}  {}", icon::ARROW_CLOCKWISE, t!("pages.rotate_right"))).clicked() {
                        out = Some(Outcome::Edit(vec![PageEdit::Rotate { pages: sel.clone(), quarter_turns: 1 }]));
                    }
                    let all = sel.len() >= n;
                    let del = ui.add_enabled(!all, egui::Button::new(format!("{}  {}", icon::TRASH, t!("pages.delete"))));
                    if del.on_disabled_hover_text(t!("pages.delete_all")).clicked() {
                        out = Some(Outcome::Edit(vec![PageEdit::Delete { pages: sel.clone() }]));
                    }
                    if ui.button(format!("{}  {}", icon::EXPORT, t!("pages.extract"))).on_hover_text(t!("pages.extract_tip")).clicked() {
                        out = Some(Outcome::Extract(sel.clone()));
                    }
                });
                if ui.button(format!("{}  {}", icon::FILE_PLUS, t!("pages.insert"))).on_hover_text(t!("pages.insert_tip")).clicked() {
                    let at = sel.last().map_or(n, |&l| l + 1);
                    out = Some(Outcome::Insert(at));
                }
                ui.label(RichText::new(if any { t!("pages.selected", sel.len()) } else { String::new() }).color(p.text_weak));
            });
            ui.add_space(6.0);
            // Miniaturas.
            let pointer = ctx.input(|i| i.pointer.interact_pos());
            let mut cells = Vec::with_capacity(n);
            egui::ScrollArea::vertical().max_height((screen.y - 280.0).max(200.0)).auto_shrink([false, true]).show(ui, |ui| {
                // Margen para el marco de la miniatura elegida.
                ui.add_space(5.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(5.0);
                    ui.spacing_mut().item_spacing = Vec2::new(14.0, 14.0);
                    for i in 0..n {
                        let (pw, ph) = doc.page_size(i);
                        let th = (THUMB_W * ph / pw.max(1.0)).clamp(20.0, 400.0);
                        let (rect, r) = ui.allocate_exact_size(Vec2::new(THUMB_W, th + 22.0), Sense::click_and_drag());
                        cells.push(rect);
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let img = egui::Rect::from_min_size(rect.min, Vec2::new(THUMB_W, th));
                        let painter = ui.painter();
                        painter.rect_filled(img.translate(Vec2::new(1.5, 2.0)), 2.0, Color32::from_black_alpha(40));
                        painter.rect_filled(img, 0.0, Color32::WHITE);
                        if let Some(tex) = self.thumb(ctx, doc, i, &mut budget) {
                            painter.image(tex.id(), img, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        let selected = self.selected.contains(&i);
                        if selected {
                            painter.rect_stroke(img.expand(3.0), 3.0, Stroke::new(2.5, p.accent), StrokeKind::Outside);
                        } else if r.hovered() {
                            painter.rect_stroke(img.expand(2.0), 3.0, Stroke::new(1.0, p.border), StrokeKind::Outside);
                        }
                        painter.text(
                            egui::pos2(rect.center().x, img.max.y + 12.0),
                            egui::Align2::CENTER_CENTER,
                            format!("{}", i + 1),
                            egui::FontId::proportional(13.0),
                            if selected { p.accent_text } else { p.text_weak },
                        );
                        if r.clicked() {
                            let mods = ui.input(|i| i.modifiers);
                            self.click(i, mods);
                        }
                        if r.drag_started() {
                            self.dragging = Some(i);
                        }
                    }
                });
            });
            // Arrastrar: dónde quedarían las páginas (delante o detrás de la
            // miniatura bajo el puntero).
            if let (Some(src), Some(pos)) = (self.dragging, pointer) {
                let target = cells.iter().enumerate().find(|(_, r)| r.expand(7.0).contains(pos)).map(|(k, r)| if pos.x > r.center().x { k + 1 } else { k });
                if let Some(t) = target {
                    let x = if t < cells.len() { cells[t].min.x - 7.0 } else { cells[t - 1].max.x + 7.0 };
                    let r = if t < cells.len() { cells[t] } else { cells[t - 1] };
                    ui.painter().line_segment([egui::pos2(x, r.min.y), egui::pos2(x, r.max.y - 18.0)], Stroke::new(3.0, p.accent));
                }
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                if ctx.input(|i| i.pointer.any_released()) {
                    self.dragging = None;
                    if let Some(t) = target {
                        let pages: Vec<usize> = if self.selected.contains(&src) { self.selected.iter().copied().collect() } else { vec![src] };
                        let before = pages.iter().filter(|&&q| q < t).count();
                        let to = t - before;
                        // Sin cambio si ya están ahí, seguidas.
                        let unchanged = pages.windows(2).all(|w| w[1] == w[0] + 1) && pages[0] == to;
                        if !unchanged {
                            out = Some(Outcome::Edit(vec![PageEdit::Move { pages, to }]));
                        }
                    }
                }
            }
            if ui.input(|i| i.key_pressed(Key::A) && (i.modifiers.ctrl || i.modifiers.command)) {
                self.selected = (0..n).collect();
            }
            if ui.input(|i| i.key_pressed(Key::Delete)) && !self.selected.is_empty() && self.selected.len() < n {
                out = Some(Outcome::Edit(vec![PageEdit::Delete { pages: self.selected.iter().copied().collect() }]));
            }
            if theme::dialog_footer(ui, &[(t!("dialog.close"), true, true)]).is_some() {
                out = Some(Outcome::Close);
            }
        });
        if modal.should_close() && self.insert.is_none() {
            out = Some(Outcome::Close);
        }
        if budget == 0 {
            ctx.request_repaint();
        }
        if let Some(ins) = self.insert.as_mut() {
            let mut answer = None;
            theme::modal(ctx, "contraseña_insertar").show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading(t!("pages.insert_password_title"));
                ui.label(t!("pages.insert_password", ins.name.clone()));
                if ins.wrong {
                    ui.label(RichText::new(t!("dialog.password_wrong")).color(theme::palette(ui).danger));
                }
                let r = ui.add(egui::TextEdit::singleline(&mut ins.password).password(true).desired_width(f32::INFINITY));
                if ins.focus {
                    r.request_focus();
                    ins.focus = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    answer = Some(0);
                }
                answer = answer.or(theme::dialog_footer(ui, &[(t!("dialog.ok"), true, !ins.password.is_empty()), (t!("dialog.cancel"), false, true)]));
            });
            match answer {
                Some(0) => {
                    let ins = self.insert.take().expect("hay un PDF por insertar");
                    out = Some(Outcome::Edit(vec![PageEdit::Insert { pdf: ins.pdf, password: Some(ins.password), at: ins.at }]));
                }
                Some(_) => self.insert = None,
                None => {}
            }
        }
        out
    }
}
