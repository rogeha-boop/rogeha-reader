//! rogeha-core: the document engine behind Rogeha Reader.
//!
//! * [`engine`]: synchronous MuPDF wrapper (open, render, page operations, save, text).
//! * [`worker`]: a background thread that owns the engine and talks to the GUI over channels.

pub mod engine;
pub mod worker;

pub use engine::{Engine, HitQuad, PageImage};
pub use worker::{DocInfo, Event, RenderKey, Request, Worker};
