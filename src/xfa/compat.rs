//! Correcciones de compatibilidad para que PDFium ejecute bien formularios
//! XFA dinámicos pensados para Adobe Reader.
//!
//! Se aplican SOLO a la copia en memoria que se entrega a PDFium. El fichero
//! del usuario nunca se modifica: al guardar se escriben únicamente los datos.
//!
//! 1. Vista continua: el motor de paginación XFA de PDFium pierde o
//!    desordena secciones cuando el formulario usa varias páginas maestras y
//!    saltos a áreas de contenido. Se convierte la primera página maestra en
//!    una página muy alta y se anulan los saltos de página, de modo que todo el
//!    formulario fluye en una sola página continua (como un formulario web).
//! 2. Referencias SOM sin cualificar: Adobe resuelve `xfa.resolveNode("X")`
//!    buscando también en descendientes; PDFium no. El framework "DFE" de los
//!    formularios del Banco de España depende de ello para idiomas y
//!    desplegables. Se añade una búsqueda `$form..X` de respaldo.

use regex::{Captures, Regex};

/// Altura de la página continua. 5000 mm ≈ 14173 pt, por debajo del límite
/// de 14400 pt de PDF.
pub const TALL_MM: f64 = 5000.0;

/// Nombre del botón invisible que la aplicación pulsa para que PDFium
/// recoloque los controles tras un cambio de estructura (ver más abajo).
pub const REFRESH_FIELD: &str = "pdfreRefresh";
/// Posición (en puntos, coordenadas de la página continua) del botón.
pub const REFRESH_POINT: (f64, f64) = (5.0, 5.0);
/// Script del botón de órdenes: ejecuta lo que la aplicación tenga pendiente
/// (`app.response("PDFRE|PENDING")`): el click de un botón (expresión SOM) o
/// "@relayout", que fuerza un relayout. PDFium no marca el layout como
/// pendiente en instanceManager.removeInstance(), así que tras pulsar un botón
/// la aplicación lo pide para que desaparezcan las filas eliminadas.
const COMMAND_SCRIPT: &str = r#"
try {
  var r = app.response("PDFRE|PENDING", "pdfre");
  if (r == "@relayout") {
    xfa.layout.relayout();
  } else if (r == "@restore") {
    pdfreCompat.restore();
    xfa.layout.relayout();
  } else if (r.indexOf("@eval:") == 0) {
    var v;
    try { v = eval(r.substr(6)); } catch (ev) { v = "ERROR " + ev; }
    app.response("PDFRE|EVAL|" + v, "pdfre");
  } else if (r == "@capture") {
    app.response("PDFRE|STATE|" + pdfreCompat.capture(), "pdfre");
  } else if (r != null && r != "") {
    var n = xfa.resolveNode(r);
    if (n != null) { n.execEvent("click"); }
  }
} catch (e) {}
"#;

/// Botón invisible que ejecuta órdenes pendientes (ver COMMAND_SCRIPT).
pub const COMMAND_FIELD: &str = "pdfreCommand";
pub const COMMAND_POINT: (f64, f64) = (17.0, 5.0);

/// Script del botón. PDFium tiene un fallo: tras un relayout, los controles
/// nativos de los campos (cajas, casillas, desplegables) no se recolocan
/// (CXFA_FFNotify::OnLayoutItemAdded compara una referencia consigo misma y
/// nunca llama a PerformLayout). Reasignar un atributo de cada campo pasa por
/// CXFA_FFNotify::OnValueChanged, que sí llama a PerformLayout.
const REFRESH_SCRIPT: &str = r#"
function pdfreWalk(n) {
  var c = n.nodes;
  for (var i = 0; i != c.length; i++) {
    var k = c.item(i);
    if (k == null) continue;
    var cn = k.className;
    if (cn == "field") {
      if (k.name != "pdfreRefresh" && k.name != "pdfreCommand") {
        try { var a = k.access; k.access = (a == "nonInteractive") ? "readOnly" : "nonInteractive"; k.access = a; } catch (e1) {}
      }
    } else if (cn == "subform" || cn == "subformSet" || cn == "area" || cn == "exclGroup") {
      pdfreWalk(k);
    }
  }
}
pdfreWalk(xfa.form);
"#;

/// Objeto de script que sustituye a las funciones de Acrobat para adjuntos
/// (`doc.dataObjects`, `importDataObject`, `getDataObject`,
/// `removeDataObject`, `exportDataObject`), que PDFium no implementa. Habla
/// con la aplicación a través de `app.response("PDFRE|...")`, que PDFium sí
/// implementa (ver pdfium::host::attach_command).
const ATTACH_SCRIPT: &str = r#"
function call(cmd) {
  var r = app.response("PDFRE|" + cmd, "pdfre");
  return (r == null) ? "" : String(r);
}
function item(s) {
  if (s == null || s == "") return null;
  var p = s.split("|");
  return { name: p[0], size: Number(p[1]), path: p.slice(2).join("|") };
}
function list() {
  var n = Number(call("COUNT"));
  if (!(n > 0)) return null;
  var a = [];
  for (var i = 0; i != n; i++) { a.push(item(call("ITEM|" + i))); }
  return a;
}
// Botones que PDFium no deja pulsar (ver open_readonly_buttons): al recibir
// el foco durante un clic de ratón se apuntan como candidatos; la aplicación
// ejecuta después el click del último (el pulsado) con runPending().
function candidate(node) {
  if (call("CLICKING") != "") { call("CANDIDATE|" + node.somExpression); }
}
function doc() {
  var o = {};
  o.dataObjects = list();
  o.importDataObject = function (cName) { return item(call("IMPORT|" + cName)) != null; };
  o.getDataObject = function (cName) {
    var it = item(call("GET|" + cName));
    if (it == null) { throw new Error("No existe el adjunto " + cName); }
    return it;
  };
  o.removeDataObject = function (cName) { call("REMOVE|" + cName); };
  o.exportDataObject = function (p) {
    var c = (p != null && typeof p == "object") ? p.cName : p;
    call("OPEN|" + c);
  };
  return o;
}
"#;

#[derive(Debug, Default, Clone)]
pub struct CompatReport {
    pub attachment_bridge: bool,
    /// Eventos change de desplegables protegidos contra la reentrada.
    pub choice_change_guards: usize,
    /// Campos de fecha sin peine de celdas.
    pub date_fields_fixed: usize,
    /// Correcciones del framework DFE para funciones que PDFium no tiene.
    pub dfe_fixes: usize,
    /// Llamadas a removeInstance() precedidas de ocultar la instancia.
    pub remove_instance_patches: usize,
    /// Botones con access="readOnly" abiertos (Adobe ejecuta su clic; PDFium no).
    pub readonly_buttons_opened: usize,
    pub refresh_button: bool,
    pub continuous_layout: bool,
    pub breaks_neutralized: usize,
    pub footer_items_hidden: usize,
    pub resolve_node_patches: usize,
    /// subformSet de un solo hijo sustituidos por subform (ver
    /// `replace_single_subformsets`).
    pub subformsets_replaced: usize,
    /// Nombres de esos subformSet (para corregir también el paquete `form`).
    pub subformset_names: Vec<String>,
    /// Desplegables cuyo change vuelve a ejecutar su initialize.
    pub initialize_after_change: usize,
    /// Eventos change que solo se ejecutan por acción del usuario.
    pub change_events_guarded: usize,
    /// Desplegables que rellenan al abrir su lista dependiente.
    pub dependent_dropdowns_initialized: usize,
    /// Eventos "listen=refAndDescendents" copiados a los campos descendientes.
    pub listen_events_propagated: usize,
    /// Objetos de script envueltos para aislar sus variables.
    pub script_objects_isolated: usize,
}

impl CompatReport {
    pub fn any(&self) -> bool {
        self.continuous_layout || self.resolve_node_patches > 0 || self.refresh_button || self.attachment_bridge || self.readonly_buttons_opened > 0 || self.remove_instance_patches > 0 || self.dfe_fixes > 0 || self.choice_change_guards > 0 || self.date_fields_fixed > 0 || self.script_objects_isolated > 0 || self.listen_events_propagated > 0 || self.dependent_dropdowns_initialized > 0 || self.change_events_guarded > 0 || self.subformsets_replaced > 0 || self.initialize_after_change > 0
    }
}

/// Convierte una medida XFA ("30mm", "1.5in", "12pt", "2cm") a milímetros.
/// Sin unidad, XFA usa pulgadas.
pub fn measure_mm(s: &str) -> Option<f64> {
    let s = s.trim();
    let idx = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(idx);
    let v: f64 = num.trim().parse().ok()?;
    Some(match unit.trim() {
        "mm" => v,
        "cm" => v * 10.0,
        "in" | "" => v * 25.4,
        "pt" => v * 25.4 / 72.0,
        "mp" => v * 25.4 / 72000.0,
        _ => return None,
    })
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let re = Regex::new(&format!(r#"\b{}\s*=\s*"([^"]*)""#, regex::escape(name))).ok()?;
    re.captures(tag).and_then(|c| c.get(1)).map(|m| m.as_str())
}

fn set_attr(tag: &str, name: &str, value: &str) -> String {
    let re = Regex::new(&format!(r#"\b{}\s*=\s*"[^"]*""#, regex::escape(name))).unwrap();
    if re.is_match(tag) {
        re.replace(tag, format!(r#"{name}="{value}""#).as_str()).into_owned()
    } else {
        // Insertar tras el nombre de la etiqueta.
        let end = tag.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(tag.len());
        format!(r#"{} {name}="{value}"{}"#, &tag[..end], &tag[end..])
    }
}

/// Aplica todas las correcciones. Devuelve la plantilla nueva (o `None` si no
/// hizo falta ningún cambio) y un informe.
pub fn patch_template(template: &str, continuous: bool) -> (Option<String>, CompatReport) {
    patch_template_with_state(template, continuous, None)
}

/// Como `patch_template`, con el estado del formulario a restaurar al abrir
/// (array JSON, ver `xfa::formstate`).
pub fn patch_template_with_state(template: &str, continuous: bool, state: Option<&str>) -> (Option<String>, CompatReport) {
    let mut report = CompatReport::default();
    let mut t = template.to_string();
    t = patch_date_fields(&t, &mut report);
    t = replace_single_subformsets(&t, &mut report);
    t = propagate_listen_events(&t, &mut report);
    t = init_dependent_dropdowns(&t, &mut report);

    if continuous
        && let Some(new) = continuous_layout(&t, &mut report) {
            t = new;
        }
    t = patch_resolve_node(&t, &mut report);
    t = patch_dfe_framework(&t, &mut report);
    t = rerun_initialize_after_change(&t, &mut report);
    t = patch_choice_change_events(&t, &mut report);
    t = guard_change_events(&t, &mut report);
    t = insert_compat_script(&t, state);
    t = patch_attachments(&t, &mut report);
    t = open_readonly_buttons(&t, &mut report);
    t = patch_remove_instance(&t, &mut report);
    t = isolate_script_objects(&t, &mut report);
    if std::env::var_os("PDFRE_SCRIPT_DEBUG").is_some() {
        t = wrap_event_scripts_for_debug(&t);
    }

    if report.any() { (Some(t), report) } else { (None, report) }
}

fn continuous_layout(t: &str, report: &mut CompatReport) -> Option<String> {
    // Localizar la primera <pageArea ...> ... </pageArea>.
    let start = t.find("<pageArea")?;
    let end_rel = t[start..].find("</pageArea")?;
    let close = start + end_rel + t[start + end_rel..].find('>')? + 1;
    let area = &t[start..close];

    // Medio (tamaño de papel) y área de contenido.
    let re_medium = Regex::new(r"<medium\b[^>]*>").unwrap();
    let medium_tag = re_medium.find(area)?.as_str().to_string();
    let long = attr(&medium_tag, "long").and_then(measure_mm).unwrap_or(297.0);
    let delta = TALL_MM - long;
    if delta <= 0.0 {
        return None;
    }
    let re_ca = Regex::new(r"<contentArea\b[^>]*>").unwrap();
    let ca_tag = re_ca.find(area)?.as_str().to_string();
    let ca_y = attr(&ca_tag, "y").and_then(measure_mm).unwrap_or(0.0);
    let ca_h = attr(&ca_tag, "h").and_then(measure_mm).unwrap_or(long - ca_y);
    let content_bottom = ca_y + ca_h;

    // Botón invisible de refresco en la esquina superior izquierda.
    // Se inserta justo antes del cierre </pageArea> (encima del resto).
    let open_end = area.rfind("</pageArea")?;
    let button = format!(
        r#"<field name="{REFRESH_FIELD}" x="0.5mm" y="0.5mm" w="3mm" h="3mm" relevant="-print"><ui><button highlight="none"/></ui><border presence="hidden"/><caption presence="hidden"/><bind match="none"/><event activity="enter"><script contentType="application/x-javascript">{}</script></event></field>"#,
        REFRESH_SCRIPT.replace('&', "&amp;").replace('<', "&lt;")
    );
    let command = format!(
        r#"<field name="{COMMAND_FIELD}" x="4.5mm" y="0.5mm" w="3mm" h="3mm" relevant="-print"><ui><button highlight="none"/></ui><border presence="hidden"/><caption presence="hidden"/><bind match="none"/><event activity="enter"><script contentType="application/x-javascript">{}</script></event></field>"#,
        COMMAND_SCRIPT.replace('&', "&amp;").replace('<', "&lt;")
    );
    let area = format!("{}{}{}{}", &area[..open_end], button, command, &area[open_end..]);
    let area = area.as_str();
    report.refresh_button = true;

    let mut new_area = area.replacen(&medium_tag, &set_attr(&medium_tag, "long", &format!("{TALL_MM}mm")), 1);
    new_area = new_area.replacen(&ca_tag, &set_attr(&ca_tag, "h", &format!("{:.3}mm", ca_h + delta)), 1);

    // Ocultar los elementos de pie de página (debajo del área de contenido):
    // en la página continua quedarían en mitad del formulario.
    let re_tag = Regex::new(r"<(/?)([A-Za-z]+)\b[^>]*?(/?)>").unwrap();
    let mut depth = 0i32;
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for m in re_tag.captures_iter(&new_area) {
        let whole = m.get(0).unwrap();
        let closing = &m[1] == "/";
        let selfclose = &m[3] == "/";
        let name = &m[2];
        if closing {
            depth -= 1;
            continue;
        }
        // depth 0 = la propia <pageArea>; sus hijos directos están en depth 1.
        if depth == 1 && matches!(name, "draw" | "field" | "subform" | "exclGroup" | "area") {
            let tag = whole.as_str();
            if let Some(y) = attr(tag, "y").and_then(measure_mm)
                && y >= content_bottom - 0.5 {
                    edits.push((whole.start(), whole.end(), set_attr(tag, "presence", "hidden")));
                }
        }
        if !selfclose {
            depth += 1;
        }
    }
    report.footer_items_hidden = edits.len();
    for (s, e, rep) in edits.into_iter().rev() {
        new_area.replace_range(s..e, &rep);
    }

    let mut out = String::with_capacity(t.len() + 64);
    out.push_str(&t[..start]);
    out.push_str(&new_area);
    out.push_str(&t[close..]);

    // Anular saltos a otras páginas / áreas de contenido.
    let re_break = Regex::new(r"<(break|breakBefore|breakAfter)\b[^>]*>").unwrap();
    let mut n = 0usize;
    let out = re_break
        .replace_all(&out, |c: &Captures| {
            let tag = c.get(0).unwrap().as_str();
            let mut new = tag.to_string();
            let mut changed = false;
            for a in ["before", "after", "targetType"] {
                if let Some(v) = attr(&new, a)
                    && matches!(v, "pageArea" | "contentArea" | "pageEven" | "pageOdd") {
                        new = set_attr(&new, a, "auto");
                        changed = true;
                    }
            }
            for a in ["beforeTarget", "afterTarget", "target"] {
                if attr(&new, a).is_some() {
                    new = set_attr(&new, a, "");
                    changed = true;
                }
            }
            if changed {
                n += 1;
            }
            new
        })
        .into_owned();
    report.breaks_neutralized = n;
    report.continuous_layout = true;
    Some(out)
}

/// Adobe ejecuta el evento click de los botones con access="readOnly"
/// (por ejemplo "Ver" y "Eliminar" de la tabla de adjuntos); PDFium los trata
/// como inactivos. Los botones no tienen datos, así que abrirlos es inocuo.
fn open_readonly_buttons(t: &str, report: &mut CompatReport) -> String {
    let re = Regex::new(r#"<field\b[^>]*\baccess="readOnly"[^>]*>"#).unwrap();
    let bridge = report.attachment_bridge;
    let mut out = String::with_capacity(t.len());
    let mut last = 0;
    let mut n = 0;
    for m in re.find_iter(t) {
        let rest = &t[m.end()..];
        let field_end = rest.find("</field").unwrap_or(rest.len());
        let body = &rest[..field_end];
        let is_button = body.find("<ui").is_some_and(|u| {
            let ui = &body[u..];
            let ui_end = ui.find("</ui").unwrap_or(ui.len());
            ui[..ui_end].contains("<button")
        });
        out.push_str(&t[last..m.start()]);
        if !is_button {
            out.push_str(m.as_str());
            last = m.end();
            continue;
        }
        out.push_str(&m.as_str().replacen(r#"access="readOnly""#, r#"access="open""#, 1));
        n += 1;
        if bridge {
            // PDFium tampoco dispara el click de estos botones. Al recibir el
            // foco por un clic de ratón se apuntan como candidatos y la
            // aplicación ejecuta su click (Document::left_down).
            out.push_str(body);
            out.push_str(r#"<event activity="enter"><script contentType="application/x-javascript">try { pdfreAttach.candidate(this); } catch (e) {}</script></event>"#);
            last = m.end() + field_end;
        } else {
            last = m.end();
        }
    }
    out.push_str(&t[last..]);
    report.readonly_buttons_opened = n;
    out
}

/// Sustituye `event.target` (el documento de Acrobat) por el objeto puente en
/// los scripts que usan adjuntos, y añade ese objeto al subformulario raíz.
fn patch_attachments(t: &str, report: &mut CompatReport) -> String {
    if !t.contains("importDataObject") {
        return t.to_string();
    }
    let mut out = t.replace("var oObj = event.target;", "var oObj = pdfreAttach.doc();");
    out = out.replace("event.target.dataObjects", "pdfreAttach.doc().dataObjects");
    // Objeto de script en las variables del subformulario raíz (visible desde
    // cualquier script, como jsGlobal).
    let script = format!(
        r#"<script contentType="application/x-javascript" name="pdfreAttach">{}</script>"#,
        ATTACH_SCRIPT.replace('&', "&amp;").replace('<', "&lt;")
    );
    let inserted = if let Some(pos) = root_variables_pos(&out) {
        out.insert_str(pos, &script);
        true
    } else {
        false
    };
    report.attachment_bridge = inserted;
    if inserted { out } else { t.to_string() }
}

/// Añade el objeto de script `pdfreCompat` a las variables del subformulario
/// raíz, con la lista de estado a restaurar (`state`, array JSON).
fn insert_compat_script(t: &str, state: Option<&str>) -> String {
    let body = format!("var restoreList = {};\n{}", state.unwrap_or("[]"), COMPAT_SCRIPT);
    let script = format!(
        r#"<script contentType="application/x-javascript" name="pdfreCompat">{}</script>"#,
        body.replace('&', "&amp;").replace('<', "&lt;")
    );
    let mut out = t.to_string();
    if let Some(pos) = root_variables_pos(&out) {
        out.insert_str(pos, &script);
    }
    // Restaurar el estado en el "ready" del formulario: después de los
    // scripts de inicio y antes de crear los controles. Si se hace después,
    // PDFium no actualiza los controles ya creados (un desplegable abierto
    // por el estado seguía sin responder).
    if state.is_some()
        && let Some(tpl) = out.find("<template")
        && let Some(rs) = out[tpl..].find("<subform").map(|i| tpl + i)
        && let Some(end) = out[rs..].find('>').map(|i| rs + i + 1)
    {
        out.insert_str(end, r#"<event activity="initialize" name="pdfre_restore"><script contentType="application/x-javascript">pdfreCompat.restore();</script></event>"#);
    }
    out
}

/// Posición justo después de la etiqueta <variables> del subformulario raíz.
fn root_variables_pos(t: &str) -> Option<usize> {
    let tpl = t.find("<template")?;
    let root = tpl + t[tpl..].find("<subform")?;
    // Recorrer los hijos directos del subformulario raíz.
    let re = Regex::new(r"<(/?)([A-Za-z]+)\b[^>]*?(/?)>").unwrap();
    let mut depth = 0i32;
    for m in re.captures_iter(&t[root..]) {
        let whole = m.get(0).unwrap();
        let closing = &m[1] == "/";
        let selfclose = &m[3] == "/";
        if closing {
            depth -= 1;
            if depth == 0 {
                return None;
            }
            continue;
        }
        if depth == 1 && &m[2] == "variables" && !selfclose {
            return Some(root + whole.end());
        }
        if !selfclose {
            depth += 1;
        }
    }
    None
}

/// PDFium no recoloca el formulario tras instanceManager.removeInstance(i): la
/// fila eliminada se sigue viendo. Ocultar antes la instancia sí provoca el
/// relayout. `X.removeInstance(i)` pasa a ocultar la instancia i y quitarla.
fn patch_remove_instance(t: &str, report: &mut CompatReport) -> String {
    let re = Regex::new(r"\b([A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*)\.removeInstance\(([^()]*)\)").unwrap();
    let mut n = 0;
    let out = re.replace_all(t, |c: &Captures| {
        n += 1;
        format!(
            "(function (im, i) {{ try {{ var it = im.parent.resolveNode(im.name.substr(1) + \"[\" + i + \"]\"); if (it != null) {{ it.presence = \"hidden\"; }} }} catch (e) {{}} return im.removeInstance(i); }})({}, {})",
            &c[1], &c[2]
        )
    });
    report.remove_instance_patches = n;
    out.into_owned()
}

/// Depuración (PDFRE_SCRIPT_DEBUG=1): envuelve cada script de evento para
/// registrar sus excepciones en el log de la aplicación.
fn wrap_event_scripts_for_debug(t: &str) -> String {
    let re = Regex::new(r#"(?s)(<event\b[^>]*\bactivity="([^"]+)"[^>]*>\s*<script\b[^>]*>)(.*?)(</script)"#).unwrap();
    re.replace_all(t, |c: &Captures| {
        format!(
            "{}if (\"{}\" == \"change\") {{ app.response(\"PDFRE|LOG|change en \" + this.name + \" newText=\" + xfa.event.newText + \" raw=\" + this.rawValue, \"pdfre\"); }} try {{ {} \n}} catch (pdfreErr) {{ app.response(\"PDFRE|LOG|excepción en {}: \" + pdfreErr + \" en \" + this.somExpression + \" | \" + String(pdfreErr.stack).replace(/\\n/g, \" / \"), \"pdfre\"); }}{}",
            &c[1], &c[2], &c[3], &c[2], &c[4]
        )
    })
    .into_owned()
}

/// Framework DFE: `exData.loadXML()` no existe en PDFium y el plan B del
/// formulario (`caption.value.text.value = ...`) falla si la etiqueta es de
/// texto enriquecido. La excepción cortaba scripts enteros (p. ej. el botón
/// "x" de titulaciones no llegaba a eliminar la sección). Se sustituyen por
/// versiones que no lanzan excepción.
fn patch_dfe_framework(t: &str, report: &mut CompatReport) -> String {
    const FIXES: &[(&str, &str)] = &[
        (
            "field.caption.value.text.value = removeXMLTags(newText);",
            // Si no se puede, se deja la etiqueta de la plantilla (ya en español).
            "try { field.caption.value.exData.value = \" \"; field.caption.value.exData.value = removeXMLTags(newText); } catch (eC2) { try { field.caption.value.text.value = removeXMLTags(newText); } catch (eC3) {} }",
        ),
        (
            // Textos fijos (draw): mismo fallo de maquetación que las
            // etiquetas; sin el espacio previo, "Tamaño" salía duplicado y
            // desplazado sobre sí mismo.
            "field.rawValue = removeXMLTags(newText);",
            "try { field.rawValue = \" \"; field.rawValue = removeXMLTags(newText); } catch (eD) {}",
        ),
        ("label.assist.toolTip.value = newText;", "try { label.assist.toolTip.value = newText; } catch (eT) {}"),
    ];
    let mut out = t.to_string();
    for (old, new) in FIXES {
        let n = out.matches(old).count();
        if n > 0 {
            out = out.replace(old, new);
            report.dfe_fixes += n;
        }
    }
    out
}

/// Objeto de script de compatibilidad, siempre presente en el subformulario
/// raíz. `changed(nodo, valor)` recuerda el último valor procesado de cada
/// desplegable (los objetos de script conservan su estado entre eventos).
///
/// Solo cuenta como cambio el de un desplegable con el foco, es decir, el que
/// hace el usuario. PDFium también dispara `change` cuando un script rellena o
/// asigna un desplegable (al abrir el PDF, por ejemplo); Adobe no. Al abrir
/// un PDF relleno, el `change` de "Nivel" vaciaba "Área de estudio".
const COMPAT_SCRIPT: &str = r#"
var last = {};
function focused(node) {
  var f = null;
  try { f = xfa.host.getFocus(); } catch (e) { return true; }
  if (f == null) return false;
  var s = (typeof f == "string") ? f : f.somExpression;
  return s == node.somExpression;
}
function user(node) {
  var f = null;
  try { f = xfa.host.getFocus(); } catch (e) { return true; }
  if (f == null) return false;
  var s = (typeof f == "string") ? f : f.somExpression;
  var k = node.somExpression;
  return s == k || s.indexOf(k + ".") == 0;
}
// Estado del formulario (ver xfa::formstate): [som, access, presence, valor].
function restore() {
  var n = 0;
  if (restoreList.length > 0) app.response("PDFRE|LOG|restaurando estado: " + restoreList.length + " entradas", "pdfre");
  for (var i = 0; i < restoreList.length; i++) {
    var e = restoreList[i];
    var k = null;
    try { k = xfa.resolveNode(e[0]); } catch (x) { k = null; }
    if (k == null) continue;
    try { if (e[2] != null && k.presence != e[2]) k.presence = e[2]; } catch (x) {}
    try { if (e[1] != null && k.access != e[1]) k.access = e[1]; } catch (x) {}
    try { if (e[3] != null && k.rawValue != e[3]) k.rawValue = e[3]; } catch (x) {}
    n++;
  }
  return n;
}
function capture() {
  var out = [];
  function walk(n) {
    var c = n.nodes;
    for (var i = 0; i < c.length; i++) {
      var k = c.item(i);
      if (k == null) continue;
      var cn = k.className;
      if (cn != "subform" && cn != "subformSet" && cn != "area" && cn != "pageSet" && cn != "pageArea" && cn != "exclGroup" && cn != "field" && cn != "draw") continue;
      if (k.name != null && String(k.name).indexOf("pdfre") == 0) continue;
      var acc = null, pre = null, val = null;
      try { pre = k.presence; } catch (x) {}
      if (cn == "field" || cn == "exclGroup" || cn == "subform") { try { acc = k.access; } catch (x) {} }
      if (cn == "field" || cn == "exclGroup") {
        try {
          if (k.bind.match == "none" && (cn == "exclGroup" || k.ui.oneOfChild.className != "button")) {
            val = (k.rawValue == null) ? null : String(k.rawValue);
          }
        } catch (x) {}
      }
      out.push([k.somExpression, acc, pre, val]);
      if (cn != "field" && cn != "draw" && cn != "exclGroup") walk(k);
    }
  }
  walk(xfa.form);
  return JSON.stringify(out);
}
function changed(node, value) {
  var k = node.somExpression;
  var v = (value == null) ? "" : String(value);
  if (!focused(node)) { last[k] = v; return false; }
  if (last[k] === v) return false;
  last[k] = v;
  return true;
}
"#;

/// Eventos change de los desplegables. Los scripts del formulario hacen
///   if (this.rawValue != this.boundItem(xfa.event.newText)) {
///     this.rawValue = this.boundItem(xfa.event.newText); ... rellenar dependientes
/// En Adobe, durante change `rawValue` aún es el valor anterior. En PDFium:
///  - a veces el valor ya está confirmado y `newText` llega vacío (la
///    condición nunca se cumple: "Área de estudio" no se rellenaba);
///  - asignar rawValue dentro del evento vuelve a disparar change con
///    `newText` vacío y el script borraba el valor recién elegido.
///
/// Se sustituye la condición por `pdfreCompat.changed()` con el valor nuevo
/// (de newText si viene, si no el ya confirmado) y la asignación por una que
/// no borra el valor cuando newText está vacío.
fn patch_choice_change_events(t: &str, report: &mut CompatReport) -> String {
    const NEW_BOUND: &str = r#"((xfa.event.newText != "") ? this.boundItem(xfa.event.newText) : this.rawValue)"#;
    const NEW_TEXT: &str = r#"((xfa.event.newText != "") ? xfa.event.newText : this.rawValue)"#;
    let reps = [
        ("if (this.rawValue != this.boundItem(xfa.event.newText))".to_string(), format!("if (pdfreCompat.changed(this, {NEW_BOUND}))")),
        ("this.rawValue = this.boundItem(xfa.event.newText);".to_string(), format!("this.rawValue = {NEW_BOUND};")),
        ("if (this.rawValue != xfa.event.newText)".to_string(), format!("if (pdfreCompat.changed(this, {NEW_TEXT}))")),
        ("this.rawValue = xfa.event.newText;".to_string(), format!("this.rawValue = {NEW_TEXT};")),
    ];
    let mut out = t.to_string();
    for (old, new) in &reps {
        let n = out.matches(old.as_str()).count();
        if n > 0 {
            out = out.replace(old.as_str(), new);
            report.choice_change_guards += n;
        }
    }
    out
}

/// Campos de fecha (dateTimeEdit): se quita el "peine" de celdas (<comb>),
/// que PDFium dibuja con rayas blancas y mal alineado junto al botón del
/// calendario. Es solo visual.
///
/// No se añade patrón de edición: sin él, PDFium guarda lo tecleado tal cual
/// ("01/02/2020"), igual que Adobe con este formulario, y el script de salida
/// del propio formulario normaliza el formato. La validación del formulario
/// (`customValidateDate`) exige DD/MM/AAAA en el dato; guardar la fecha
/// canónica XFA (2020-02-01) la haría fallar.
fn patch_date_fields(t: &str, report: &mut CompatReport) -> String {
    let re_edit = Regex::new(r"(?s)<dateTimeEdit\b.*?</dateTimeEdit\s*>").unwrap();
    let re_comb = Regex::new(r"<comb\b[^>]*/>").unwrap();
    let mut n = 0;
    let out = re_edit.replace_all(t, |c: &Captures| {
        let edit = &c[0];
        if re_comb.is_match(edit) {
            n += 1;
            re_comb.replace_all(edit, "").into_owned()
        } else {
            edit.to_string()
        }
    });
    report.date_fields_fixed = n;
    out.into_owned()
}

/// PDFium decide si un campo es interactivo recorriendo sus contenedores
/// padres (`CXFA_Node::IsOpenAccess`) y exigiendo `access="open"` en todos.
/// Un `subformSet` no tiene atributo `access`, así que nada de lo que hay
/// dentro responde: en la lista de adjuntos no se podía elegir el tipo de
/// documento ni escribir la descripción. Un `subformSet` con un único
/// subformulario hijo equivale a un `subform` sin enlace a datos, y se
/// sustituye por él.
fn replace_single_subformsets(t: &str, report: &mut CompatReport) -> String {
    let re_tag = Regex::new(r"<(/?)([A-Za-z][\w:.-]*)\b[^>]*?(/?)>").unwrap();
    let re_name = Regex::new(r#"\bname="([^"]*)""#).unwrap();
    // (inicio etiqueta de apertura, fin de la misma, nº de subformularios hijos directos)
    let mut stack: Vec<(String, usize, usize, usize)> = Vec::new();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for m in re_tag.captures_iter(t) {
        let w = m.get(0).unwrap();
        let tag = &m[2];
        if &m[1] == "/" {
            if let Some((open, start, open_end, children)) = stack.pop()
                && open == "subformSet"
                && children == 1
            {
                let name = re_name.captures(&t[start..open_end]).map(|c| c[1].to_string());
                if let Some(n) = &name {
                    report.subformset_names.push(n.clone());
                }
                let name_attr = name.map(|n| format!(r#" name="{n}""#)).unwrap_or_default();
                edits.push((start, open_end, format!(r#"<subform{name_attr} layout="tb"><bind match="none"/>"#)));
                edits.push((w.start(), w.end(), "</subform>".into()));
                report.subformsets_replaced += 1;
            }
            continue;
        }
        if tag == "subform"
            && let Some(parent) = stack.last_mut()
        {
            parent.3 += 1;
        }
        if &m[3] != "/" {
            stack.push((tag.to_string(), w.start(), w.end(), 0));
        }
    }
    if edits.is_empty() {
        return t.to_string();
    }
    edits.sort_by_key(|e| e.0);
    let mut out = String::with_capacity(t.len());
    let mut last = 0;
    for (a, b, txt) in edits {
        out.push_str(&t[last..a]);
        out.push_str(&txt);
        last = b;
    }
    out.push_str(&t[last..]);
    out
}

/// Paquete `form` guardado por Adobe: PDFium reutiliza su estructura al
/// fusionar plantilla y datos, así que los `subformSet` sustituidos en la
/// plantilla (ver `replace_single_subformsets`) también se sustituyen aquí.
pub fn patch_form_packet(form: &str, names: &[String]) -> Option<String> {
    if names.is_empty() || !form.contains("<subformSet") {
        return None;
    }
    let re_tag = Regex::new(r"<(/?)([A-Za-z][\w:.-]*)\b([^>]*?)(/?)>").unwrap();
    let re_name = Regex::new(r#"\bname="([^"]*)""#).unwrap();
    let mut stack: Vec<bool> = Vec::new();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for m in re_tag.captures_iter(form) {
        let w = m.get(0).unwrap();
        let tag = &m[2];
        if &m[1] == "/" {
            if stack.pop() == Some(true) {
                edits.push((w.start(), w.end(), "</subform>".into()));
            }
            continue;
        }
        let hit = tag == "subformSet" && re_name.captures(&m[3]).is_some_and(|c| names.iter().any(|n| n == &c[1]));
        if hit {
            edits.push((w.start(), w.end(), format!("<subform{}{}>", &m[3], &m[4])));
        }
        if &m[4] != "/" {
            stack.push(hit);
        }
    }
    if edits.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(form.len());
    let mut last = 0;
    for (a, b, txt) in edits {
        out.push_str(&form[last..a]);
        out.push_str(&txt);
        last = b;
    }
    out.push_str(&form[last..]);
    Some(out)
}

/// Eventos change: Adobe solo los dispara cuando el usuario cambia el valor.
/// PDFium también cuando un script rellena o asigna el campo, por ejemplo al
/// abrir el PDF. Muchos scripts del formulario reaccionan a ese change falso
/// borrando datos: "Curso/Certificación" quedaba vacío (y "Validar" daba
/// error) y la descripción de cada adjunto se borraba y ocultaba. Cada script
/// de change pasa a ejecutarse solo si el campo (o uno de sus hijos, en un
/// grupo de casillas) tiene el foco.
fn guard_change_events(t: &str, report: &mut CompatReport) -> String {
    let re = Regex::new(r#"(?s)(<event\b[^>]*\bactivity="change"[^>]*>\s*<script\b[^>]*>)(.*?)(</script\s*>)"#).unwrap();
    let mut n = 0;
    let out = re.replace_all(t, |c: &Captures| {
        n += 1;
        format!("{}if (pdfreCompat.user(this)) {{\n{}\n}}{}", &c[1], &c[2], &c[3])
    });
    report.change_events_guarded = n;
    out.into_owned()
}

/// Tras un cambio del usuario en un desplegable, Adobe vuelve a ejecutar su
/// evento initialize: los eventos ready de layout llaman a
/// changeFormLanguage, que hace execEvent("initialize") en cada desplegable.
/// Algunos scripts dependen de ello. El change de "Tipo de documento" de los
/// adjuntos oculta la descripción y es su initialize el que la vuelve a
/// mostrar; en PDFium la descripción quedaba oculta para siempre.
///
/// Se añade la llamada al final del change de los desplegables cuyo
/// initialize no rellena listas: volver a rellenarlas en mitad del change
/// estropea el valor ("BACHILLERATOBACHILLERATO").
fn rerun_initialize_after_change(t: &str, report: &mut CompatReport) -> String {
    let re_field = Regex::new(r"(?s)<field\b[^>]*[^/]>.*?</field\s*>").unwrap();
    let re_init = Regex::new(r#"(?s)<event\b[^>]*activity="initialize"[^>]*>\s*<script\b[^>]*>(.*?)</script"#).unwrap();
    let re_change = Regex::new(r#"(?s)(<event\b[^>]*activity="change"[^>]*>\s*<script\b[^>]*>)(.*?)(</script\s*>)"#).unwrap();
    let mut n = 0;
    let out = re_field.replace_all(t, |c: &Captures| {
        let f = &c[0];
        if !f.contains("<choiceList") {
            return f.to_string();
        }
        let inits: Vec<String> = re_init.captures_iter(f).map(|m| m[1].to_string()).collect();
        if inits.is_empty() || inits.iter().any(|s| s.contains("populate") || s.contains("addItem") || s.contains("clearItems")) {
            return f.to_string();
        }
        re_change
            .replace_all(f, |m: &Captures| {
                n += 1;
                format!("{}{}\n;try {{ this.execEvent(\"initialize\"); }} catch (pdfreE) {{}}\n{}", &m[1], &m[2], &m[3])
            })
            .into_owned()
    });
    report.initialize_after_change = n;
    out.into_owned()
}

/// Desplegables dependientes: el evento change de "Tipo de titulación"
/// rellena la lista de "Área de estudio", pero nada la rellena al abrir el
/// PDF. Adobe la restaura del paquete `form`, que PDFium no usa, y el área
/// se veía como su código ("201", "4003") en vez del texto. Se añade un
/// evento initialize que rellena la lista dependiente con el valor guardado.
fn init_dependent_dropdowns(t: &str, report: &mut CompatReport) -> String {
    let re_field = Regex::new(r"(?s)<field\b[^>]*[^/]>.*?</field\s*>").unwrap();
    let re_call = Regex::new(r"[\w.]*populateDependentDropdown(?:SortByValue)?\(\s*this\s*,[^;]*\);").unwrap();
    let re_init = Regex::new(r#"(?s)<event\b[^>]*activity="initialize"[^>]*>.*?</event\s*>"#).unwrap();
    let re_change = Regex::new(r#"(?s)<event\b[^>]*activity="change"[^>]*>.*?</event\s*>"#).unwrap();
    let mut n = 0;
    let out = re_field.replace_all(t, |c: &Captures| {
        let f = &c[0];
        let Some(ch) = re_change.find(f) else { return f.to_string() };
        let Some(call) = re_call.find(ch.as_str()) else { return f.to_string() };
        if re_init.find_iter(f).any(|e| re_call.is_match(e.as_str())) {
            return f.to_string();
        }
        n += 1;
        let close = f.rfind("</field").unwrap();
        format!(
            r#"{}<event activity="initialize" name="pdfre_init"><script contentType="application/x-javascript">if (!this.isNull) {{ {} }}</script></event>{}"#,
            &f[..close],
            call.as_str(),
            &f[close..]
        )
    });
    report.dependent_dropdowns_initialized = n;
    out.into_owned()
}

/// Eventos con `listen="refAndDescendents"` (XFA 3.x): el evento de un
/// subformulario se dispara también por cada descendiente, con
/// `xfa.event.target` apuntando a él. PDFium ignora `listen`. El formulario
/// del Banco de España formatea y valida cada campo así: al salir de un campo
/// pasa a mayúsculas, normaliza fechas (la del calendario llega como
/// 2020-02-01 y debe quedar 01/02/2020) y marca errores.
///
/// Se copia el evento a cada campo descendiente. En PDFium `xfa.event.target`
/// no está definido, así que en la copia se sustituye por `this` (el campo).
fn propagate_listen_events(t: &str, report: &mut CompatReport) -> String {
    let re_tag = Regex::new(r"<(/?)([A-Za-z][\w:.-]*)\b[^>]*?(/?)>").unwrap();
    let re_act = Regex::new(r#"\bactivity="([^"]+)""#).unwrap();
    let re_script = Regex::new(r"(?s)<script\b[^>]*>(.*?)</script\s*>").unwrap();
    let re_name = Regex::new(r#"\bname="([^"]*)""#).unwrap();
    // (inicio, nombre) de los elementos abiertos.
    let mut stack: Vec<(usize, String)> = Vec::new();
    // Eventos a propagar: (actividad, script, inicio del padre).
    let mut listeners: Vec<(String, String, usize)> = Vec::new();
    // Rango de cada elemento que contiene un evento "listen": inicio -> fin.
    let mut parent_end: std::collections::HashMap<usize, usize> = Default::default();
    // Campos: (inicio, fin de su etiqueta de cierre, nombre).
    let mut fields: Vec<(usize, usize, String)> = Vec::new();
    for m in re_tag.captures_iter(t) {
        let whole = m.get(0).unwrap();
        let name = m[2].to_string();
        if &m[1] == "/" {
            if let Some((start, open)) = stack.pop() {
                if open == "field" {
                    let fname = re_name.captures(&t[start..start + 400.min(t.len() - start)]).map(|c| c[1].to_string()).unwrap_or_default();
                    fields.push((start, whole.start(), fname));
                }
                if let Some(e) = parent_end.get_mut(&start) {
                    *e = whole.end();
                }
            }
            continue;
        }
        if name == "event" && whole.as_str().contains("listen=\"refAndDescendents\"") {
            let ev_end = t[whole.end()..].find("</event").map(|e| whole.end() + e);
            if let (Some(act), Some(end), Some(&(pstart, _))) = (re_act.captures(whole.as_str()), ev_end, stack.last())
                && let Some(sc) = re_script.captures(&t[whole.end()..end])
            {
                listeners.push((act[1].to_string(), sc[1].replace("xfa.event.target", "this"), pstart));
                parent_end.entry(pstart).or_insert(usize::MAX);
            }
        }
        if &m[3] != "/" {
            stack.push((whole.start(), name));
        }
    }
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for (start, close, fname) in &fields {
        if fname.starts_with("pdfre") {
            continue;
        }
        let mut add = String::new();
        for (act, body, pstart) in &listeners {
            let pend = parent_end.get(pstart).copied().unwrap_or(0);
            if *start > *pstart && *close < pend {
                add.push_str(&format!(r#"<event activity="{act}" name="pdfre_listen"><script contentType="application/x-javascript">{body}</script></event>"#));
                report.listen_events_propagated += 1;
            }
        }
        if !add.is_empty() {
            inserts.push((*close, add));
        }
    }
    if inserts.is_empty() {
        return t.to_string();
    }
    inserts.sort_by_key(|i| i.0);
    let mut out = String::with_capacity(t.len() + inserts.iter().map(|i| i.1.len()).sum::<usize>());
    let mut last = 0;
    for (pos, text) in inserts {
        out.push_str(&t[last..pos]);
        out.push_str(&text);
        last = pos;
    }
    out.push_str(&t[last..]);
    out
}

/// Objetos de script (`<variables><script name="...">`). En PDFium, dentro de
/// las funciones de un objeto de script las variables globales del propio
/// script (p. ej. `langTable`) se resuelven contra un espacio global
/// compartido por todos los objetos de script: gana el último que se
/// ejecutó. El formulario tiene decenas de `jsLanguage` con su `langTable` y
/// su `updateLanguage()`, así que "Eliminar la titulación académica" tomaba
/// la tabla de "cursos" y perdía su número. El acceso por propiedad
/// (`jsLanguage.langTable`) sí es correcto.
///
/// Cada objeto de script se envuelve en una función: sus funciones cierran
/// sobre sus propias variables (como en Adobe), y las declaraciones de primer
/// nivel se reexportan como propiedades con get/set para que el resto del
/// formulario siga accediendo a ellas por nombre.
fn isolate_script_objects(t: &str, report: &mut CompatReport) -> String {
    let re_vars = Regex::new(r"(?s)(<variables\b[^>]*>)(.*?)(</variables\s*>)").unwrap();
    let re_script = Regex::new(r"(?s)(<script\b([^>]*)>)(.*?)(</script\s*>)").unwrap();
    let mut n = 0;
    let out = re_vars.replace_all(t, |v: &Captures| {
        let inner = re_script.replace_all(&v[2], |c: &Captures| {
            let attrs = &c[2];
            let body = &c[3];
            if !attrs.contains("javascript") || attrs.contains(r#"name="pdfre"#) {
                return c[0].to_string();
            }
            let names = top_level_names(&xml_unescape(body));
            if names.is_empty() {
                return c[0].to_string();
            }
            n += 1;
            let mut w = String::from("(function () {\n");
            w.push_str(body);
            w.push_str("\n;var pdfreG = this;\nfunction pdfreX(n, g, s) { try { Object.defineProperty(pdfreG, n, { get: g, set: s, enumerable: true, configurable: true }); } catch (e) { pdfreG[n] = g(); } }\n");
            for name in &names {
                w.push_str(&format!("pdfreX(\"{name}\", function () {{ return {name}; }}, function (v) {{ {name} = v; }});\n"));
            }
            w.push_str("}).call(this);\n");
            format!("{}{}{}", &c[1], w, &c[4])
        });
        format!("{}{}{}", &v[1], inner, &v[3])
    });
    report.script_objects_isolated = n;
    out.into_owned()
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&#xD;", "\r").replace("&#xA;", "\n").replace("&amp;", "&")
}

/// Nombres declarados en el primer nivel de un script JavaScript (`var x`,
/// `var a = 1, b`, `function f(`). Analizador mínimo: salta cadenas,
/// comentarios y expresiones regulares, y lleva la profundidad de llaves,
/// paréntesis y corchetes.
fn top_level_names(src: &str) -> Vec<String> {
    let b: Vec<char> = src.chars().collect();
    let mut names: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    // Último carácter significativo (para distinguir "/" de división y de regex).
    let mut prev = ';';
    let mut prev_word = String::new();
    let mut in_var = false;
    let mut expect_name = false;
    let mut expect_fn_name = false;
    let push = |n: &str, names: &mut Vec<String>| {
        if !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    while i < b.len() {
        let ch = b[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        // Comentarios.
        if ch == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // Cadenas.
        if ch == '"' || ch == '\'' || ch == '`' {
            i += 1;
            while i < b.len() && b[i] != ch {
                if b[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            prev = 'a';
            prev_word.clear();
            continue;
        }
        // Expresión regular: "/" donde no puede haber una división.
        if ch == '/' && ("(,=:[!&|?{};+-*%<>~^".contains(prev) || prev_word == "return" || prev_word == "typeof") {
            i += 1;
            let mut class = false;
            while i < b.len() && b[i] != '\n' {
                match b[i] {
                    '\\' => i += 1,
                    '[' => class = true,
                    ']' => class = false,
                    '/' if !class => break,
                    _ => {}
                }
                i += 1;
            }
            i += 1;
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            prev = 'a';
            prev_word.clear();
            continue;
        }
        // Identificadores y palabras clave.
        if ch.is_alphabetic() || ch == '_' || ch == '$' {
            let start = i;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_' || b[i] == '$') {
                i += 1;
            }
            let word: String = b[start..i].iter().collect();
            if depth == 0 {
                if expect_fn_name {
                    push(&word, &mut names);
                    expect_fn_name = false;
                } else if expect_name {
                    push(&word, &mut names);
                    expect_name = false;
                } else if word == "var" {
                    in_var = true;
                    expect_name = true;
                } else if word == "function" {
                    expect_fn_name = true;
                }
            }
            prev = 'a';
            prev_word = word;
            continue;
        }
        if ch.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '.') {
                i += 1;
            }
            prev = '0';
            prev_word.clear();
            continue;
        }
        match ch {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            ',' if depth == 0 && in_var => expect_name = true,
            ';' if depth == 0 => {
                in_var = false;
                expect_name = false;
            }
            _ => {}
        }
        // Una función anónima de primer nivel no declara nombre.
        if ch == '(' {
            expect_fn_name = false;
        }
        prev = ch;
        prev_word.clear();
        i += 1;
    }
    names
}

fn patch_resolve_node(t: &str, report: &mut CompatReport) -> String {
    // Framework DFE (Banco de España): jsGlobal.getReference().
    const OLD: &str = "reference = xfa.resolveNode(defaultReference);";
    const NEW: &str = "reference = xfa.resolveNode(defaultReference); if (reference == null) { try { reference = xfa.resolveNode(\"$form..\" + defaultReference); } catch (eCompat) { reference = null; } }";
    let n = t.matches(OLD).count();
    report.resolve_node_patches += n;
    if n > 0 { t.replace(OLD, NEW) } else { t.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medidas() {
        assert_eq!(measure_mm("10mm"), Some(10.0));
        assert_eq!(measure_mm("1in"), Some(25.4));
        assert_eq!(measure_mm("2cm"), Some(20.0));
        assert!((measure_mm("72pt").unwrap() - 25.4).abs() < 1e-9);
    }

    #[test]
    fn nombres_de_primer_nivel() {
        let js = r#"
            // var comentada;
            var langTable = { a: { b: "x{" } }, otra = 1;
            var re = /[}{]/g;
            function updateLanguage(n) { var interna = n; function anidada() {} }
            var s = 'it\'s }';
            (function () { var anon = 1; })();
            if (a < b) { var dentro = 2; }
            var x = 10 / 2, y = 3;
        "#;
        assert_eq!(top_level_names(js), vec!["langTable", "otra", "re", "updateLanguage", "s", "x", "y"]);
    }

    #[test]
    fn aislar_objetos_de_script() {
        let t = r#"<subform><variables><script contentType="application/x-javascript" name="jsLanguage">var t = 1; function f() { return t &lt; 2; }</script><script contentType="application/x-javascript" name="vacio">// nada</script></variables></subform>"#;
        let mut rep = CompatReport::default();
        let out = isolate_script_objects(t, &mut rep);
        assert_eq!(rep.script_objects_isolated, 1);
        assert!(out.contains("(function () {
var t = 1; function f() { return t &lt; 2; }"));
        assert!(out.contains(r#"pdfreX("t", function () { return t; }, function (v) { t = v; });"#));
        assert!(out.contains(r#"pdfreX("f", "#));
        assert!(out.contains(r#"name="vacio">// nada</script>"#));
    }

    #[test]
    fn propagar_listen() {
        let t = r#"<template><subform name="raiz"><event activity="exit" listen="refAndDescendents" name="e"><script contentType="application/x-javascript">f(xfa.event.target);</script></event><subform name="s"><field name="a"><ui/></field><field name="b"/></subform></subform><field name="fuera"></field></template>"#;
        let mut rep = CompatReport::default();
        let out = propagate_listen_events(t, &mut rep);
        assert_eq!(rep.listen_events_propagated, 1);
        assert!(out.contains(r#"<field name="a"><ui/><event activity="exit" name="pdfre_listen"><script contentType="application/x-javascript">f(this);</script></event></field>"#));
        assert!(out.contains(r#"<field name="fuera"></field>"#));
    }

    #[test]
    fn inicializar_dependientes() {
        let t = r#"<field name="nivel"><event activity="initialize"><script>j.populateDependentDropdown(this, a.tipo, "x");</script></event><event activity="change"><script>j.populateDependentDropdown(this, a.tipo, "x");</script></event></field><field name="tipo"><event activity="change"><script>if (c) { j.L.populateDependentDropdown(this, this.parent.area, "t_a"); }</script></event></field><field name="b"/>"#;
        let mut rep = CompatReport::default();
        let out = init_dependent_dropdowns(t, &mut rep);
        assert_eq!(rep.dependent_dropdowns_initialized, 1);
        assert!(out.contains(r#"</event><event activity="initialize" name="pdfre_init"><script contentType="application/x-javascript">if (!this.isNull) { j.L.populateDependentDropdown(this, this.parent.area, "t_a"); }</script></event></field><field name="b"/>"#));
    }

    #[test]
    fn subformset_de_un_hijo() {
        let t = r#"<subform name="t"><subformSet name="filas" relation="choice"><subform name="fila"><field name="a"/></subform></subformSet><subformSet name="dos"><subform name="x"/><subform name="y"/></subformSet></subform>"#;
        let mut rep = CompatReport::default();
        let out = replace_single_subformsets(t, &mut rep);
        assert_eq!(rep.subformsets_replaced, 1);
        assert!(out.contains(r#"<subform name="filas" layout="tb"><bind match="none"/><subform name="fila"><field name="a"/></subform></subform>"#), "{out}");
        assert!(out.contains(r#"<subformSet name="dos">"#));
    }

    #[test]
    fn layout_continuo() {
        let t = r#"<template><subform name="r"><pageSet><pageArea name="A"><medium stock="a4" short="210mm" long="297mm"/><contentArea x="10mm" y="30mm" w="190mm" h="245mm"/><draw name="cab" y="5mm"/><draw name="pie" y="280mm"><value/></draw></pageArea><pageArea name="B"><medium long="297mm"/></pageArea></pageSet><subform name="s"><break before="pageArea" beforeTarget="B"/><breakBefore targetType="contentArea" target="B.x"/></subform></subform></template>"#;
        let (out, rep) = patch_template(t, true);
        let out = out.unwrap();
        assert!(rep.continuous_layout);
        assert!(out.contains(r#"long="5000mm""#));
        assert!(out.contains(r#"h="4948.000mm""#));
        assert!(out.contains(r#"<draw presence="hidden" name="pie" y="280mm">"#));
        assert!(out.contains(r#"<draw name="cab" y="5mm"/>"#));
        assert!(out.contains(r#"<break before="auto" beforeTarget=""/>"#));
        assert!(out.contains(r#"<breakBefore targetType="auto" target=""/>"#));
        assert_eq!(rep.breaks_neutralized, 2);
        assert_eq!(rep.footer_items_hidden, 1);
    }
}
