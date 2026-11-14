//! rogeha-core: the document engine behind Rogeha Reader.
//!
//! * [`engine`]: synchronous MuPDF wrapper (open, render, page operations, save, text).
//! * [`annotations`]: comments and markup as standard PDF annotations, and text selection.
//! * [`compress`]: shrink scanned PDFs by re-encoding their images (run in a helper process).
//! * [`worker`]: a background thread that owns the engine and talks to the GUI over channels.

pub mod annotations;
pub mod compress;
pub mod engine;
pub mod worker;

pub use annotations::{AnnotInfo, AnnotKind, MarkupKind, TextSelection};
pub use compress::{CompressStats, Preset, compress_pdf};
pub use engine::{Engine, HitQuad, PageImage};
pub use worker::{DocInfo, Event, RenderKey, Request, Worker};
