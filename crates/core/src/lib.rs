//! rogeha-core: the document engine behind Rogeha Reader.
//!
//! * [`engine`]: synchronous MuPDF wrapper (open, render, page operations, save, text).
//! * [`annotations`]: comments and markup as standard PDF annotations, and text selection.
//! * [`compress`]: shrink scanned PDFs by re-encoding their images (run in a helper process).
//! * [`ocr`]: recognise text in scans and add it as an invisible layer (run in a helper process).
//! * [`signature`]: stamp a drawn, typed or photographed signature onto a page.
//! * [`dsig`]: digital (certificate) signatures: sign with a PKCS#12 identity, verify.
//! * [`fonts`]: fonts installed on the computer, for documents that do not carry their own.
//! * [`forms`]: fill out PDF forms (text, checkboxes, radio buttons, dropdowns).
//! * [`worker`]: a background thread that owns the engine and talks to the GUI over channels.

pub mod annotations;
pub mod compress;
pub mod dsig;
pub mod engine;
pub mod fonts;
pub mod forms;
pub mod ocr;
pub mod signature;
pub mod worker;

pub use annotations::{AnnotInfo, AnnotKind, COMMENT_PALETTE, MarkupKind, TextSelection, author_color};
pub use dsig::{Identity, SignOptions, SignatureInfo, SignatureStatus, format_time};
pub use compress::{CompressStats, Preset, compress_pdf};
pub use signature::{INK_BLACK, INK_BLUE, SignatureArt, load_signature_image, render_signature};
pub use ocr::{OcrStats, ocr_pdf};
pub use forms::{FieldKind, FormField};
pub use engine::{Engine, HitQuad, LinkTarget, OutlineItem, PageImage, PageLink, PageRgb, render_svg};
pub use worker::{DocInfo, Event, RenderKey, Request, Worker};
