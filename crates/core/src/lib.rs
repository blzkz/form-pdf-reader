//! Núcleo de form_pdf_reader: integración con PDFium (V8 + XFA).
pub mod i18n;
pub mod pades;
pub mod pdfium;
pub mod scan;
#[cfg(feature = "ocr")]
pub mod ocr;
pub mod sign;
pub mod xfa;
