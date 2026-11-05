//! Enlaces del documento: las anotaciones de enlace (a una web o a otra
//! página) y las direcciones web escritas en el texto, que muchos PDF no
//! convierten en enlace.
//!
//! PDFium no sigue los enlaces al hacer clic (`FORM_OnLButtonUp` solo
//! atiende a los campos del formulario): lo hace `left_up` con `link_at`, y
//! el resultado llega a la interfaz como el de un formulario (`Updates::uris`
//! y `Updates::goto_page`).

use super::super::{from_wide_buf, sys};
use super::Document;
use std::ffi::{c_ulong, c_void};

// fpdf_doc.h (las constantes no están en sys.rs).
const PDFACTION_GOTO: c_ulong = 1;
const PDFACTION_URI: c_ulong = 3;

/// Distancia (en puntos) entre la pulsación y la suelta por debajo de la que
/// un clic sigue el enlace; más es un arrastre.
pub(super) const CLICK_SLOP: f64 = 4.0;
/// Margen alrededor de una dirección escrita en el texto, en puntos: las
/// cajas del texto son justas y con el dedo cuesta acertar.
const TEXT_LINK_MARGIN: f64 = 2.0;

/// Adónde lleva un enlace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// Una dirección (web, correo…), tal como está en el PDF.
    Uri(String),
    /// Una página de este documento (desde 0).
    Page(usize),
}

/// Una dirección escrita en el texto de una página y lo que ocupa
/// (izquierda, arriba, derecha, abajo en coordenadas de página).
#[derive(Debug, Clone)]
pub(super) struct TextLink {
    url: String,
    rects: Vec<[f64; 4]>,
}

impl Document {
    /// El enlace en (x, y) de una página (coordenadas de página): una
    /// anotación de enlace o, si no hay, una dirección web escrita en el texto.
    pub fn link_at(&self, idx: usize, x: f64, y: f64) -> Option<Link> {
        self.annot_link_at(idx, x, y).or_else(|| self.text_link_at(idx, x, y))
    }

    /// Sigue el enlace en (x, y), si lo hay: lo deja en `Updates` (`uris` o
    /// `goto_page`) para que la interfaz abra la dirección o vaya a la página.
    pub fn follow_link_at(&mut self, idx: usize, x: f64, y: f64) -> bool {
        let Some(link) = self.link_at(idx, x, y) else {
            return false;
        };
        let mut st = self.host.state.borrow_mut();
        match link {
            Link::Uri(u) => st.uris.push(u),
            Link::Page(p) => st.goto_page = i32::try_from(p).ok(),
        }
        true
    }

    fn annot_link_at(&self, idx: usize, x: f64, y: f64) -> Option<Link> {
        let page = self.page(idx);
        if page.is_null() {
            return None;
        }
        unsafe {
            let link = sys::FPDFLink_GetLinkAtPoint(page, x, y);
            if link.is_null() {
                return None;
            }
            let mut dest = sys::FPDFLink_GetDest(self.doc, link);
            let action = sys::FPDFLink_GetAction(link);
            if !action.is_null() {
                match sys::FPDFAction_GetType(action) {
                    PDFACTION_URI => {
                        let n = sys::FPDFAction_GetURIPath(self.doc, action, std::ptr::null_mut(), 0);
                        if n == 0 {
                            return None;
                        }
                        let mut buf = vec![0u8; n as usize];
                        sys::FPDFAction_GetURIPath(self.doc, action, buf.as_mut_ptr() as *mut c_void, n);
                        let uri = String::from_utf8_lossy(&buf).trim_end_matches('\0').trim().to_string();
                        return (!uri.is_empty()).then_some(Link::Uri(uri));
                    }
                    PDFACTION_GOTO => dest = sys::FPDFAction_GetDest(self.doc, action),
                    _ => {}
                }
            }
            if dest.is_null() {
                return None;
            }
            let p = sys::FPDFDest_GetDestPageIndex(self.doc, dest);
            usize::try_from(p).ok().filter(|&p| p < self.page_count()).map(Link::Page)
        }
    }

    fn text_link_at(&self, idx: usize, x: f64, y: f64) -> Option<Link> {
        let mut cache = self.text_links.borrow_mut();
        let links = match cache.get(&idx) {
            Some(l) => l,
            None => {
                let l = self.load_text_links(idx);
                cache.entry(idx).or_insert(l)
            }
        };
        let m = TEXT_LINK_MARGIN;
        links
            .iter()
            .find(|l| l.rects.iter().any(|&[l, t, r, b]| x >= l - m && x <= r + m && y >= b.min(t) - m && y <= b.max(t) + m))
            .map(|l| Link::Uri(l.url.clone()))
    }

    /// Las direcciones web del texto de una página (PDFium las reconoce con
    /// o sin «http://», y también las de correo).
    fn load_text_links(&self, idx: usize) -> Vec<TextLink> {
        self.with_text_page(idx, |tp| unsafe {
            let pl = sys::FPDFLink_LoadWebLinks(tp);
            if pl.is_null() {
                return Vec::new();
            }
            let mut out = Vec::new();
            for i in 0..sys::FPDFLink_CountWebLinks(pl) {
                let n = sys::FPDFLink_GetURL(pl, i, std::ptr::null_mut(), 0);
                if n <= 1 {
                    continue;
                }
                let mut buf = vec![0u16; n as usize];
                sys::FPDFLink_GetURL(pl, i, buf.as_mut_ptr(), n);
                let url = from_wide_buf(&buf);
                let rects = (0..sys::FPDFLink_CountRects(pl, i))
                    .filter_map(|r| {
                        let (mut l, mut t, mut rr, mut b) = (0.0, 0.0, 0.0, 0.0);
                        (sys::FPDFLink_GetRect(pl, i, r, &mut l, &mut t, &mut rr, &mut b) != 0).then_some([l, t, rr, b])
                    })
                    .collect();
                out.push(TextLink { url, rects });
            }
            sys::FPDFLink_CloseWebLinks(pl);
            out
        })
        .unwrap_or_default()
    }
}
