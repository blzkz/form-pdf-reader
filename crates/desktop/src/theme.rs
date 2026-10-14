//! Aspecto de la interfaz: la fuente del sistema, los iconos (Phosphor) y
//! los colores, claros u oscuros según el sistema, al estilo de Windows 11.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Response, RichText, Shadow, Stroke, TextStyle, Theme, Ui,
    Vec2, WidgetText,
};

pub use egui_phosphor::regular as icon;

/// Colores de la ventana, para lo que se dibuja a mano.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Barra de título (detrás de las pestañas).
    pub title_bar: Color32,
    /// Barra de herramientas, pestaña activa, paneles.
    pub surface: Color32,
    /// Fondo detrás de las páginas.
    pub canvas: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    /// Acento: amarillo yema, como la app de Android. Para rellenos y
    /// contornos; el texto encima va en `on_accent`, y el texto o los iconos
    /// de color acento sobre el fondo, en `accent_text` (más legible).
    pub accent: Color32,
    pub on_accent: Color32,
    pub accent_text: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    /// Botón de cerrar la ventana al pasar el ratón.
    pub close_hover: Color32,
    pub warn: Color32,
    /// Estados: correcto y error.
    pub ok: Color32,
    pub danger: Color32,
}

const LIGHT: Palette = Palette {
    title_bar: Color32::from_rgb(0xE9, 0xEC, 0xF0),
    surface: Color32::from_rgb(0xF9, 0xFA, 0xFB),
    canvas: Color32::from_rgb(0xE3, 0xE6, 0xEA),
    border: Color32::from_rgb(0xD5, 0xD9, 0xDE),
    text: Color32::from_rgb(0x1B, 0x1C, 0x1F),
    text_weak: Color32::from_rgb(0x5C, 0x61, 0x68),
    accent: Color32::from_rgb(0xF4, 0xB4, 0x00),
    on_accent: Color32::from_rgb(0x26, 0x1A, 0x00),
    accent_text: Color32::from_rgb(0x8A, 0x65, 0x00),
    hover: Color32::from_rgba_premultiplied(0, 0, 0, 12),
    pressed: Color32::from_rgba_premultiplied(0, 0, 0, 22),
    close_hover: Color32::from_rgb(0xC4, 0x2B, 0x1C),
    warn: Color32::from_rgb(0xB2, 0x5E, 0x00),
    ok: Color32::from_rgb(0x10, 0x7C, 0x10),
    danger: Color32::from_rgb(0xC4, 0x2B, 0x1C),
};

const DARK: Palette = Palette {
    title_bar: Color32::from_rgb(0x1C, 0x1C, 0x1E),
    surface: Color32::from_rgb(0x2B, 0x2B, 0x2D),
    canvas: Color32::from_rgb(0x16, 0x16, 0x18),
    border: Color32::from_rgb(0x3D, 0x3D, 0x41),
    text: Color32::from_rgb(0xEC, 0xEC, 0xEE),
    text_weak: Color32::from_rgb(0xA4, 0xA6, 0xAB),
    accent: Color32::from_rgb(0xF8, 0xC5, 0x4A),
    on_accent: Color32::from_rgb(0x3F, 0x2E, 0x00),
    accent_text: Color32::from_rgb(0xF8, 0xC5, 0x4A),
    hover: Color32::from_rgba_premultiplied(15, 15, 15, 15),
    pressed: Color32::from_rgba_premultiplied(26, 26, 26, 26),
    close_hover: Color32::from_rgb(0xC4, 0x2B, 0x1C),
    warn: Color32::from_rgb(0xF0, 0xA0, 0x3C),
    ok: Color32::from_rgb(0x6C, 0xCB, 0x5F),
    danger: Color32::from_rgb(0xFF, 0x99, 0xA4),
};

/// El acento para contornos sobre blanco (campos en el tema claro, el papel
/// de las páginas): algo más oscuro, para que se vea.
pub const ACCENT_ON_WHITE: Color32 = Color32::from_rgb(0xD4, 0x9A, 0x00);

pub fn palette(ui: &Ui) -> Palette {
    palette_of(ui.visuals().dark_mode)
}

pub fn palette_of(dark: bool) -> Palette {
    if dark { DARK } else { LIGHT }
}

/// Fuentes, iconos y estilo claro y oscuro. El tema sigue al del sistema.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Some(data) = system_ui_font() {
        fonts.font_data.insert("ui".into(), Arc::new(FontData::from_owned(data)));
        if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
            f.insert(0, "ui".into());
        }
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    for theme in [Theme::Light, Theme::Dark] {
        ctx.set_style_of(theme, style(theme == Theme::Dark));
    }
}

fn style(dark: bool) -> egui::Style {
    let p = palette_of(dark);
    let mut s = egui::Style {
        visuals: if dark { egui::Visuals::dark() } else { egui::Visuals::light() },
        text_styles: [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into(),
        ..Default::default()
    };

    let sp = &mut s.spacing;
    sp.item_spacing = Vec2::new(6.0, 6.0);
    sp.button_padding = Vec2::new(10.0, 5.0);
    sp.interact_size.y = 28.0;
    sp.menu_margin = Margin::same(6);
    sp.window_margin = Margin::same(20);
    sp.menu_spacing = 2.0;

    let v = &mut s.visuals;
    let radius = CornerRadius::same(6);
    v.panel_fill = p.surface;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.canvas;
    v.faint_bg_color = p.title_bar;
    v.text_edit_bg_color = Some(if dark { Color32::from_rgb(0x1F, 0x1F, 0x21) } else { Color32::WHITE });
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 40 }) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 30 }) };
    v.hyperlink_color = p.accent_text;
    v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.40 } else { 0.35 });
    // El contorno de foco, algo más oscuro en el tema claro: el amarillo
    // sobre blanco apenas se ve.
    v.selection.stroke = Stroke::new(1.0, if dark { p.accent } else { ACCENT_ON_WHITE });
    v.warn_fg_color = p.warn;

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;
    for (st, fill) in [(&mut w.inactive, p.hover), (&mut w.hovered, p.hover), (&mut w.active, p.pressed), (&mut w.open, p.pressed)] {
        st.corner_radius = radius;
        st.weak_bg_fill = fill;
        st.bg_fill = fill;
        st.bg_stroke = Stroke::NONE;
        st.fg_stroke = Stroke::new(1.0, p.text);
        st.expansion = 0.0;
    }
    // Botones en reposo: un fondo apenas visible, como Fluent.
    w.inactive.weak_bg_fill = if dark { Color32::from_rgba_premultiplied(10, 10, 10, 10) } else { Color32::from_rgba_premultiplied(0, 0, 0, 6) };
    w.inactive.bg_fill = if dark { Color32::from_rgb(0x3A, 0x3A, 0x3D) } else { Color32::from_rgb(0xD5, 0xD9, 0xDE) };
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    // Borde fino en campos de texto y botones normales, como Fluent (los de
    // solo icono no tienen marco en reposo).
    // El mismo grosor de borde en todos los estados: egui suma el borde al
    // tamaño del botón, y si cambiaba al pasar el ratón o pulsar, los botones
    // de al lado se movían.
    for st in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        st.bg_stroke = Stroke::new(1.0, p.border);
    }
    s
}

/// Botones planos, como los de la barra de herramientas: sin fondo ni borde
/// en reposo, pero ocupando lo mismo en reposo, con el ratón encima y
/// pulsados. egui solo mide el borde de un botón cuando lo dibuja; por eso
/// aquí siempre lo tienen, transparente, y los botones de dentro deben
/// dibujar su marco también en reposo (`frame_when_inactive(true)`).
pub fn flat<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        for st in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            st.bg_stroke = Stroke::new(1.0, Color32::TRANSPARENT);
        }
        add(ui)
    })
    .inner
}

/// Botón de solo icono, sin fondo hasta pasar el ratón.
pub fn icon_button(ui: &mut Ui, glyph: &str, tip: impl Into<WidgetText>) -> Response {
    flat(ui, |ui| {
        let b = egui::Button::new(RichText::new(glyph).size(18.0)).frame_when_inactive(true).min_size(Vec2::splat(32.0));
        ui.add(b).on_hover_text(tip)
    })
}

/// Igual, marcado cuando está activo.
pub fn icon_toggle(ui: &mut Ui, on: bool, glyph: &str, tip: impl Into<WidgetText>) -> Response {
    let p = palette(ui);
    flat(ui, |ui| {
        let text = RichText::new(glyph).size(18.0).color(if on { p.accent_text } else { p.text });
        let mut b = egui::Button::new(text).frame_when_inactive(true).min_size(Vec2::splat(32.0));
        if on {
            b = b.fill(p.accent.gamma_multiply(0.22));
        }
        ui.add(b).on_hover_text(tip)
    })
}

/// Botón de diálogo: el principal relleno con el color de acento, los demás
/// claros con un borde fino, como en Windows 11. Con estados al pasar el
/// ratón y al pulsar.
pub fn dialog_button(ui: &mut Ui, text: impl Into<WidgetText>, primary: bool, enabled: bool) -> Response {
    let p = palette(ui);
    let dark = ui.visuals().dark_mode;
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = Vec2::new(18.0, 6.0);
        let (base, ink, stroke) = if primary {
            (p.accent, p.on_accent, Stroke::NONE)
        } else {
            (if dark { Color32::from_rgb(0x37, 0x37, 0x3A) } else { Color32::WHITE }, p.text, Stroke::new(1.0, p.border))
        };
        let toward = if dark { Color32::WHITE } else { Color32::BLACK };
        let w = &mut ui.visuals_mut().widgets;
        for (st, fill) in [
            (&mut w.inactive, base),
            (&mut w.hovered, base.lerp_to_gamma(toward, 0.08)),
            (&mut w.active, base.lerp_to_gamma(toward, 0.16)),
        ] {
            st.weak_bg_fill = fill;
            st.bg_fill = fill;
            st.bg_stroke = stroke;
            st.fg_stroke = Stroke::new(1.0, ink);
            st.corner_radius = CornerRadius::same(6);
        }
        ui.add_enabled(enabled, egui::Button::new(text).min_size(Vec2::new(96.0, 32.0)))
    })
    .inner
}

/// Diálogo modal con el marco de una ventana: más margen que los menús
/// (el marco por defecto de egui es el de un menú emergente).
pub fn modal(ctx: &egui::Context, id: &str) -> egui::Modal {
    let style = ctx.global_style();
    let v = &style.visuals;
    let frame = egui::Frame::new()
        .fill(v.window_fill)
        .stroke(v.window_stroke)
        .corner_radius(v.window_corner_radius)
        .shadow(v.window_shadow)
        .inner_margin(Margin::same(DIALOG_MARGIN));
    egui::Modal::new(egui::Id::new(id)).frame(frame)
}

/// Margen interior de los diálogos.
pub const DIALOG_MARGIN: i8 = 22;

/// Botón principal suelto (p. ej. "Abrir" en la ventana vacía).
pub fn primary_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    dialog_button(ui, text.into(), true, true)
}

/// Pie de un diálogo: los botones (texto, principal, activo) a la derecha,
/// en ese orden. Devuelve el que se pulsó.
pub fn dialog_footer(ui: &mut Ui, buttons: &[(String, bool, bool)]) -> Option<usize> {
    ui.add_space(14.0);
    let mut clicked = None;
    ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (i, (text, primary, enabled)) in buttons.iter().enumerate().rev() {
            if dialog_button(ui, text.as_str(), *primary, *enabled).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Control segmentado: varias opciones juntas, una elegida.
/// Con `labels` false, solo los iconos (el texto queda en la ayuda).
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str, String, String)], labels: bool) {
    let p = palette(ui);
    let frame = egui::Frame::new().fill(p.hover).corner_radius(8).inner_margin(Margin::same(2));
    frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        flat(ui, |ui| ui.horizontal(|ui| {
            for (opt, glyph, label, tip) in options {
                let on = *value == *opt;
                let shown = if labels { format!("{glyph}  {label}") } else { glyph.to_string() };
                let text = RichText::new(shown).color(if on { p.text } else { p.text_weak });
                let mut b = egui::Button::new(text).frame_when_inactive(true).min_size(Vec2::new(0.0, 28.0)).corner_radius(6);
                if on {
                    b = b.fill(p.surface).stroke(Stroke::new(1.0, p.border));
                }
                let tip = if labels { tip.clone() } else { format!("{label}: {tip}") };
                if ui.add(b).on_hover_text(tip).clicked() {
                    *value = *opt;
                }
            }
        }));
    });
}

/// Fuente de la interfaz del sistema: Segoe UI en Windows; en Linux, la
/// "sans-serif" de fontconfig (Noto Sans en KDE, Cantarell en GNOME...).
fn system_ui_font() -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        std::fs::read(dir.join("Fonts").join("segoeui.ttf")).ok()
    }
    #[cfg(not(windows))]
    {
        let out = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans-serif:style=Regular"]).output().ok()?;
        let path = String::from_utf8(out.stdout).ok()?;
        // Solo TrueType/OpenType sencillos (no colecciones .ttc ni fuentes variables).
        let ok = path.ends_with(".ttf") || path.ends_with(".otf");
        if !ok || path.contains('[') {
            return None;
        }
        std::fs::read(path.trim()).ok()
    }
}
