//! Piloto de pruebas de la interfaz (solo con la variable PDFRE_AUTOTEST).
//!
//! Ejemplo:
//!   PDFRE_AUTOTEST="wait:30;click:550,700;type:HOLA;key:Tab;shot:/tmp/a.png;quit"
//!
//! Pasos: wait:N (fotogramas), click:X,Y, down:X,Y, up:X,Y (pulsar y soltar
//! por separado, para arrastrar), move:X,Y, copy, type:TEXTO, key:Nombre
//! (Enter, Tab, ArrowDown...), scroll:DY, shot:RUTA.png, quit.

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Vec2};
use std::collections::VecDeque;

enum Step {
    Wait(u32),
    Move(Pos2),
    Press(Pos2),
    Release(Pos2),
    Type(String),
    Copy,
    Key(Key, bool, Modifiers),
    Scroll(f32),
    Shot(String),
    Quit,
}

pub struct AutoTest {
    steps: VecDeque<Step>,
    waiting_shot: Option<String>,
}

impl AutoTest {
    pub fn from_env() -> Option<AutoTest> {
        let spec = std::env::var("PDFRE_AUTOTEST").ok()?;
        let mut steps = VecDeque::new();
        for part in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (cmd, arg) = part.split_once(':').unwrap_or((part, ""));
            let pos = || {
                let (x, y) = arg.split_once(',').unwrap_or(("0", "0"));
                Pos2::new(x.trim().parse().unwrap_or(0.0), y.trim().parse().unwrap_or(0.0))
            };
            match cmd {
                "wait" => steps.push_back(Step::Wait(arg.parse().unwrap_or(1))),
                "move" => steps.push_back(Step::Move(pos())),
                "click" => {
                    steps.push_back(Step::Move(pos()));
                    steps.push_back(Step::Press(pos()));
                    steps.push_back(Step::Release(pos()));
                }
                "down" => {
                    steps.push_back(Step::Move(pos()));
                    steps.push_back(Step::Press(pos()));
                }
                "up" => {
                    steps.push_back(Step::Move(pos()));
                    steps.push_back(Step::Release(pos()));
                }
                "copy" => steps.push_back(Step::Copy),
                "type" => steps.push_back(Step::Type(arg.to_string())),
                "key" => {
                    let mut m = Modifiers::NONE;
                    let mut name = arg;
                    while let Some((pre, rest)) = name.split_once('+') {
                        match pre {
                            "Ctrl" => m = m | Modifiers::COMMAND | Modifiers::CTRL,
                            "Shift" => m |= Modifiers::SHIFT,
                            "Alt" => m |= Modifiers::ALT,
                            _ => {}
                        }
                        name = rest;
                    }
                    if let Some(k) = Key::from_name(name) {
                        steps.push_back(Step::Key(k, true, m));
                        steps.push_back(Step::Key(k, false, m));
                    } else {
                        eprintln!("[autotest] tecla desconocida: {name}");
                    }
                }
                "scroll" => steps.push_back(Step::Scroll(arg.parse().unwrap_or(0.0))),
                "shot" => steps.push_back(Step::Shot(arg.to_string())),
                "quit" => steps.push_back(Step::Quit),
                _ => eprintln!("[autotest] paso desconocido: {part}"),
            }
            if !matches!(cmd, "wait" | "quit") {
                steps.push_back(Step::Wait(3));
            }
        }
        Some(AutoTest { steps, waiting_shot: None })
    }

    pub fn raw_input(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        ctx.request_repaint();
        if self.waiting_shot.is_some() {
            return;
        }
        let Some(step) = self.steps.pop_front() else { return };
        match step {
            Step::Wait(n) => {
                if n > 1 {
                    self.steps.push_front(Step::Wait(n - 1));
                }
            }
            Step::Move(p) => raw.events.push(Event::PointerMoved(p)),
            Step::Press(p) => raw.events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }),
            Step::Release(p) => raw.events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }),
            Step::Type(t) => raw.events.push(Event::Text(t)),
            Step::Copy => raw.events.push(Event::Copy),
            Step::Key(k, pressed, m) => {
                raw.events.push(Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers: m });
            }
            Step::Scroll(dy) => raw.events.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, dy), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE }),
            Step::Shot(path) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.waiting_shot = Some(path);
            }
            Step::Quit => std::process::exit(0),
        }
    }

    /// Guarda la captura cuando llega.
    pub fn after_frame(&mut self, ctx: &egui::Context) {
        let Some(path) = self.waiting_shot.clone() else { return };
        let img = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = img {
            let [w, h] = img.size;
            let mut bytes = Vec::with_capacity(w * h * 4);
            for p in &img.pixels {
                bytes.extend_from_slice(&p.to_array());
            }
            match std::fs::File::create(&path) {
                Ok(f) => {
                    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
                    e.set_color(png::ColorType::Rgba);
                    let _ = e.write_header().and_then(|mut wr| wr.write_image_data(&bytes));
                    eprintln!("[autotest] captura guardada en {path} ({w}x{h})");
                }
                Err(err) => eprintln!("[autotest] no se pudo guardar {path}: {err}"),
            }
            self.waiting_shot = None;
        }
    }
}

/// true si se está ejecutando el piloto de pruebas.
pub fn active() -> bool {
    std::env::var_os("PDFRE_AUTOTEST").is_some()
}
