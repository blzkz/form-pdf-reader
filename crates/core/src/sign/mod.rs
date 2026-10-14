//! Firmas manuscritas, como en DocuSign: el nombre escrito con letra de
//! mano, las iniciales o un trazo dibujado. Se guardan en cada dispositivo
//! para reutilizarlas (`library`) y se convierten en trazados vectoriales
//! (`shape`) para ponerlas en el PDF como anotación (ver
//! `pdfium::Document::add_signature`).

pub mod library;
pub mod shape;

pub use library::{Library, SavedSignature, SignatureKind, Stroke, TypeStyle};
pub use shape::{Segment, Shape, ShapePath};
