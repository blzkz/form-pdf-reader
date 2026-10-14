//! Firmas guardadas en este equipo: el menú para elegir una y el diálogo
//! para crear otra, como en DocuSign: el nombre escrito con letra de mano,
//! las iniciales o una firma dibujada.

use std::collections::HashMap;

use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke, StrokeKind, TextureHandle, Vec2};
use form_pdf_reader::sign::library::{DEFAULT_INK, initials_of};
use form_pdf_reader::sign::{Library, SavedSignature, SignatureKind, Stroke as InkStroke, TypeStyle};
use form_pdf_reader::t;

use crate::theme::{self, icon};
use crate::viewer::signature_texture;

pub enum Action {
    /// Poner esta firma en el documento.
    Place(SavedSignature),
    /// Abrir el diálogo para crear una.
    Create,
    /// Firmar con certificado (firma digital).
    Digital,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Typed,
    Initials,
    Drawn,
}

const BLACK_INK: [u8; 3] = [0x14, 0x14, 0x14];
const PAD_SIZE: Vec2 = Vec2::new(480.0, 170.0);
const PEN_WIDTH: f32 = 2.6;
/// Tinta de los ejemplos, antes de escribir el nombre.
const SAMPLE_INK: [u8; 3] = [0xB4, 0xB6, 0xBC];

struct CreateDialog {
    mode: Mode,
    name: String,
    initials: String,
    /// Las iniciales se escribieron a mano (no seguir al nombre).
    initials_edited: bool,
    style: TypeStyle,
    color: [u8; 3],
    /// Trazos en coordenadas del lienzo.
    strokes: Vec<Vec<Pos2>>,
    previews: HashMap<(String, TypeStyle, [u8; 3]), TextureHandle>,
}

pub struct Signatures {
    lib: Library,
    items: Vec<SavedSignature>,
    thumbs: HashMap<String, TextureHandle>,
    dialog: Option<CreateDialog>,
}

impl Signatures {
    pub fn new() -> Signatures {
        let dir = crate::platform::config_dir().unwrap_or_else(|| std::env::temp_dir().join("form-pdf-reader")).join("signatures");
        let lib = Library::new(dir);
        let items = lib.list();
        Signatures { lib, items, thumbs: HashMap::new(), dialog: None }
    }

    fn thumb(&mut self, ctx: &egui::Context, sig: &SavedSignature) -> Option<TextureHandle> {
        if !self.thumbs.contains_key(&sig.id) {
            let tex = signature_texture(ctx, sig, 72)?;
            self.thumbs.insert(sig.id.clone(), tex);
        }
        self.thumbs.get(&sig.id).cloned()
    }

    /// Contenido del menú del botón de firmar.
    /// Las firmas guardadas en este equipo.
    pub fn saved(&self) -> &[SavedSignature] {
        &self.items
    }

    /// `handwritten`: el documento admite firmas manuscritas (no en XFA
    /// dinámico); la firma digital se puede siempre.
    pub fn menu(&mut self, ui: &mut egui::Ui, handwritten: bool) -> Option<Action> {
        // Más aire que los demás menús: aquí hay imágenes.
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                let p = theme::palette(ui);
                let mut action = None;
                if handwritten {
                    action = self.menu_contents(ui);
                } else {
                    ui.set_min_width(300.0);
                    ui.label(RichText::new(t!("sign.cannot")).small().color(p.text_weak));
                }
                ui.separator();
                if ui.button(format!("{}   {}", icon::SEAL_CHECK, t!("dsig.menu"))).on_hover_text(t!("dsig.menu_tip")).clicked() {
                    action = Some(Action::Digital);
                    ui.close();
                }
                action
            })
            .inner
    }

    fn menu_contents(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let p = theme::palette(ui);
        ui.set_min_width(300.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        let mut action = None;
        if self.items.is_empty() {
            // Sin firmas: una tarjeta vacía que explica qué hacer.
            egui::Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(egui::Margin::symmetric(16, 14)).show(ui, |ui| {
                ui.set_width(268.0);
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(RichText::new(icon::SIGNATURE).size(30.0).color(p.text_weak));
                    ui.label(RichText::new(t!("sign.none")).color(p.text));
                    ui.label(RichText::new(t!("sign.none_hint")).small().color(p.text_weak));
                    ui.add_space(8.0);
                    if theme::primary_button(ui, format!("{}  {}", icon::PLUS, t!("sign.create"))).clicked() {
                        action = Some(Action::Create);
                        ui.close();
                    }
                });
            });
            return action;
        }
        let mut remove = None;
        for sig in self.items.clone() {
            ui.horizontal(|ui| {
                let Some(tex) = self.thumb(ui.ctx(), &sig) else { return };
                let size = tex.size_vec2() * (36.0 / tex.size_vec2().y);
                let size = if size.x > 220.0 { size * (220.0 / size.x) } else { size };
                let img = egui::Image::new((tex.id(), size));
                // Sobre una tarjeta blanca, como en el papel (en tema oscuro la
                // tinta no se vería).
                let b = egui::Button::image(img).fill(Color32::WHITE).stroke(Stroke::new(1.0, p.border)).corner_radius(8).min_size(Vec2::new(240.0, 48.0));
                if ui.add(b).on_hover_text(t!("sign.place_tip")).clicked() {
                    action = Some(Action::Place(sig.clone()));
                    ui.close();
                }
                if theme::icon_button(ui, icon::TRASH, t!("sign.delete")).clicked() {
                    remove = Some(sig.id.clone());
                }
            });
        }
        if let Some(id) = remove {
            if let Err(e) = self.lib.remove(&id) {
                log::warn!("no se pudo borrar la firma {id}: {e:#}");
            }
            self.thumbs.remove(&id);
            self.items = self.lib.list();
        }
        ui.separator();
        if ui.button(format!("{}   {}", icon::PLUS, t!("sign.create"))).clicked() {
            action = Some(Action::Create);
            ui.close();
        }
        action
    }

    pub fn open_create(&mut self, name: &str) {
        self.dialog = Some(CreateDialog {
            mode: Mode::Typed,
            name: name.to_string(),
            initials: initials_of(name),
            initials_edited: false,
            style: TypeStyle::GreatVibes,
            color: DEFAULT_INK,
            strokes: Vec::new(),
            previews: HashMap::new(),
        });
    }

    /// El diálogo de crear firma. Devuelve la firma guardada y el nombre
    /// escrito (para proponerlo la próxima vez).
    pub fn create_window(&mut self, ctx: &egui::Context) -> Option<(SavedSignature, String)> {
        let d = self.dialog.as_mut()?;
        let mut save = false;
        let mut cancel = false;
        let resp = theme::modal(ctx, "crear_firma").show(ctx, |ui| {
            let p = theme::palette(ui);
            ui.set_width(PAD_SIZE.x + 8.0);
            ui.heading(t!("sign.create_title"));
            ui.add_space(10.0);
            theme::segmented(
                ui,
                &mut d.mode,
                &[
                    (Mode::Typed, icon::TEXT_T, t!("sign.mode_typed"), t!("sign.mode_typed_tip")),
                    (Mode::Initials, icon::TEXT_AA, t!("sign.mode_initials"), t!("sign.mode_initials_tip")),
                    (Mode::Drawn, icon::SCRIBBLE, t!("sign.mode_drawn"), t!("sign.mode_drawn_tip")),
                ],
                true,
            );
            ui.add_space(14.0);
            match d.mode {
                Mode::Typed | Mode::Initials => {
                    let initials = d.mode == Mode::Initials;
                    ui.label(if initials { t!("sign.initials") } else { t!("sign.name") });
                    if initials {
                        let field = egui::TextEdit::singleline(&mut d.initials).hint_text(initials_of(&t!("sign.sample_name"))).desired_width(160.0).margin(egui::Margin::symmetric(8, 5));
                        if ui.add(field).changed() {
                            d.initials_edited = true;
                        }
                    } else if ui
                        .add(egui::TextEdit::singleline(&mut d.name).hint_text(t!("sign.name_hint")).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)))
                        .changed()
                        && !d.initials_edited
                    {
                        d.initials = initials_of(&d.name);
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new(t!("sign.style")).color(p.text_weak));
                    let text = if initials { d.initials.trim().to_string() } else { d.name.trim().to_string() };
                    // Sin texto todavía, cada estilo se ve con un nombre de
                    // ejemplo en gris, para saber cómo quedará.
                    let (text, ink) = if text.is_empty() {
                        let sample = t!("sign.sample_name");
                        (if initials { initials_of(&sample) } else { sample }, SAMPLE_INK)
                    } else {
                        (text, d.color)
                    };
                    if d.previews.len() > 40 {
                        d.previews.clear();
                    }
                    for style in TypeStyle::ALL {
                        let selected = d.style == style;
                        let frame = egui::Frame::new()
                            .fill(Color32::WHITE)
                            .stroke(Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { theme::ACCENT_ON_WHITE } else { p.border }))
                            .corner_radius(8)
                            .inner_margin(8);
                        let r = frame
                            .show(ui, |ui| {
                                ui.set_width(PAD_SIZE.x - 16.0);
                                ui.set_height(56.0);
                                let key = (text.clone(), style, ink);
                                if !d.previews.contains_key(&key) {
                                    let sig = typed(&text, style, ink, initials);
                                    if let Some(tex) = signature_texture(ui.ctx(), &sig, 96) {
                                        d.previews.insert(key.clone(), tex);
                                    }
                                }
                                if let Some(tex) = d.previews.get(&key) {
                                    let size = tex.size_vec2() * (52.0 / tex.size_vec2().y);
                                    let size = if size.x > PAD_SIZE.x - 20.0 { size * ((PAD_SIZE.x - 20.0) / size.x) } else { size };
                                    ui.centered_and_justified(|ui| ui.add(egui::Image::new((tex.id(), size))));
                                }
                            })
                            .response;
                        if ui.interact(r.rect, egui::Id::new(("estilo_firma", style.key())), Sense::click()).clicked() {
                            d.style = style;
                        }
                    }
                }
                Mode::Drawn => {
                    ui.label(RichText::new(t!("sign.draw_hint")).color(p.text_weak));
                    draw_pad(ui, &mut d.strokes, d.color);
                    if ui.button(format!("{}  {}", icon::ERASER, t!("sign.clear"))).clicked() {
                        d.strokes.clear();
                    }
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(t!("sign.ink"));
                for (ink, name) in [(DEFAULT_INK, t!("sign.ink_blue")), (BLACK_INK, t!("sign.ink_black"))] {
                    let c = Color32::from_rgb(ink[0], ink[1], ink[2]);
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 9.0, c);
                    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, p.border));
                    if d.color == ink {
                        let ring = if ui.visuals().dark_mode { p.accent } else { theme::ACCENT_ON_WHITE };
                        ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(2.0, ring));
                    }
                    if resp.on_hover_text(name).clicked() {
                        d.color = ink;
                    }
                }
            });
            let valid = match d.mode {
                Mode::Typed => !d.name.trim().is_empty(),
                Mode::Initials => !d.initials.trim().is_empty(),
                Mode::Drawn => d.strokes.iter().any(|s| !s.is_empty()),
            };
            match theme::dialog_footer(ui, &[(t!("sign.save"), true, valid), (t!("dialog.cancel"), false, true)]) {
                Some(0) => save = true,
                Some(_) => cancel = true,
                None => {}
            }
        });
        if cancel || resp.should_close() {
            self.dialog = None;
            return None;
        }
        if !save {
            return None;
        }
        let d = self.dialog.take()?;
        let kind = match d.mode {
            Mode::Typed => SignatureKind::Typed { text: d.name.trim().to_string(), style: d.style },
            Mode::Initials => SignatureKind::Initials { text: d.initials.trim().to_string(), style: d.style },
            Mode::Drawn => SignatureKind::Drawn {
                strokes: d
                    .strokes
                    .iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| InkStroke { width: PEN_WIDTH, points: s.iter().map(|p| (p.x, p.y)).collect() })
                    .collect(),
            },
        };
        match self.lib.add(kind, d.color) {
            Ok(sig) => {
                self.items = self.lib.list();
                Some((sig, d.name.trim().to_string()))
            }
            Err(e) => {
                log::warn!("no se pudo guardar la firma: {e:#}");
                None
            }
        }
    }
}

fn typed(text: &str, style: TypeStyle, color: [u8; 3], initials: bool) -> SavedSignature {
    let text = text.to_string();
    let kind = if initials { SignatureKind::Initials { text, style } } else { SignatureKind::Typed { text, style } };
    SavedSignature { id: format!("vista-{}-{}", style.key(), initials), kind, color }
}

/// Lienzo blanco para dibujar la firma con el ratón, una tableta o el dedo.
fn draw_pad(ui: &mut egui::Ui, strokes: &mut Vec<Vec<Pos2>>, color: [u8; 3]) {
    let p = theme::palette(ui);
    let (rect, resp) = ui.allocate_exact_size(PAD_SIZE, Sense::drag());
    let painter = ui.painter_at(rect);
    painter.rect(rect, 8.0, Color32::WHITE, Stroke::new(1.0, p.border), StrokeKind::Inside);
    // Línea de firma, como en un papel.
    let base = rect.bottom() - 40.0;
    painter.line_segment([Pos2::new(rect.left() + 24.0, base), Pos2::new(rect.right() - 24.0, base)], Stroke::new(1.0, Color32::from_gray(200)));
    painter.text(Pos2::new(rect.left() + 26.0, base - 4.0), egui::Align2::LEFT_BOTTOM, "×", egui::FontId::proportional(18.0), Color32::from_gray(170));
    if strokes.iter().all(|s| s.is_empty()) {
        // Vacío: qué hacer aquí.
        painter.text(
            Pos2::new(rect.center().x, rect.top() + (base - rect.top()) / 2.0),
            egui::Align2::CENTER_CENTER,
            format!("{}  {}", icon::SCRIBBLE, t!("sign.pad_hint")),
            egui::FontId::proportional(15.0),
            Color32::from_gray(160),
        );
    }
    if resp.drag_started() {
        strokes.push(Vec::new());
    }
    if (resp.dragged() || resp.drag_started())
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(s) = strokes.last_mut()
    {
        let local = (pos - rect.min).to_pos2();
        if s.last().is_none_or(|l| l.distance(local) > 1.0) {
            s.push(local);
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let ink = Color32::from_rgb(color[0], color[1], color[2]);
    for s in strokes.iter() {
        let pts: Vec<Pos2> = s.iter().map(|p| rect.min + p.to_vec2()).collect();
        match pts.len() {
            0 => {}
            1 => {
                painter.circle_filled(pts[0], PEN_WIDTH / 2.0, ink);
            }
            _ => {
                painter.add(egui::Shape::line(pts, Stroke::new(PEN_WIDTH, ink)));
            }
        }
    }
}
