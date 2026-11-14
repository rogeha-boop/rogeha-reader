//! Background document thread. The GUI never touches MuPDF directly: it sends
//! [`Request`]s and receives [`Event`]s, so opening, rendering and saving never block a frame.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use crate::annotations::{AnnotInfo, MarkupKind, TextSelection};
use crate::engine::{Engine, HitQuad, PageImage};
use crate::dsig::{Identity, SignOptions, SignatureInfo};
use crate::forms::FormField;
use crate::signature::SignatureArt;

/// Identifies one rendered image: which page, at which scale, for which document state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderKey {
    pub page: usize,
    /// Scale in 1/1000ths (1000 = 72 dpi), so the key is hashable.
    pub scale_milli: u32,
    /// Bumped every time the document changes (open, rotate, delete, ...).
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct DocInfo {
    pub path: PathBuf,
    pub title: String,
    pub is_pdf: bool,
    pub dirty: bool,
    pub generation: u64,
    /// Page sizes in points, rotation applied.
    pub page_sizes: Vec<(f32, f32)>,
}

pub enum Request {
    Open(PathBuf),
    /// Replace the set of wanted renders. Listed first = most urgent.
    Want(Vec<RenderKey>),
    Rotate { pages: Vec<usize>, delta: i32 },
    Delete { pages: Vec<usize> },
    Move { from: usize, to: usize },
    /// Merge other PDFs into the open one, in order. `before = None` appends.
    InsertPdfs { paths: Vec<PathBuf>, before: Option<usize> },
    Save { target: Option<PathBuf> },
    Search { query: String },
    PageText { page: usize },
    /// Text between two page points; answered with [`Event::Selection`] carrying `seq`.
    SelectText { page: usize, a: (f32, f32), b: (f32, f32), seq: u64 },
    /// Send the list of comments now, and again after every change.
    ListAnnotations,
    AddMarkup { page: usize, kind: MarkupKind, quads: Vec<HitQuad>, author: String, contents: String },
    AddNote { page: usize, at: (f32, f32), contents: String, author: String },
    EditAnnotation { page: usize, xref: i32, contents: String },
    DeleteAnnotation { page: usize, xref: i32 },
    /// Stamp a visual signature; `rect` is (x0, y0, x1, y1) in displayed page points.
    StampSignature { page: usize, rect: (f32, f32, f32, f32), art: SignatureArt, ink: [u8; 3] },
    /// Send the document's form fields now, and again after every change.
    ListFields,
    /// Type into a text field or choose a dropdown / list option.
    SetField { page: usize, xref: i32, value: String },
    /// Tick or clear a checkbox, or select a radio button.
    SetChecked { page: usize, xref: i32, checked: bool },
    /// Digitally sign with a PKCS#12 identity and write to `target` (`None`: the opened file).
    SignDocument { target: Option<PathBuf>, p12: Vec<u8>, password: String, options: SignOptions },
    /// Send the document's digital signatures now, and again after every change.
    ListSignatures,
    Close,
}

pub enum Event {
    /// Sent after open and after every change to the document.
    Doc(DocInfo),
    Rendered { key: RenderKey, image: PageImage },
    /// The GUI should not ask for this key again.
    RenderFailed { key: RenderKey, message: String },
    Saved(PathBuf),
    SearchResults { query: String, hits: Vec<(usize, HitQuad)> },
    PageText { page: usize, text: String },
    Selection { page: usize, seq: u64, selection: TextSelection },
    /// All comments in the document, for the given document generation.
    Annotations { generation: u64, list: Vec<AnnotInfo> },
    /// A comment was just created (so the GUI can select it).
    AnnotationAdded { page: usize, xref: i32 },
    /// The document's form fields, for the given document generation.
    Fields { generation: u64, list: Vec<FormField> },
    /// The document's digital signatures, checked, for the given document generation.
    Signatures { generation: u64, list: Vec<SignatureInfo> },
    /// The document was signed and written to this file.
    Signed(PathBuf),
    Closed,
    Error(String),
}

pub struct Worker {
    tx: Sender<Request>,
    pub events: Receiver<Event>,
}

impl Worker {
    /// `wake` is called from the worker thread whenever an event is ready
    /// (the GUI uses it to request a repaint).
    pub fn spawn(wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (etx, events) = mpsc::channel();
        thread::Builder::new()
            .name("rr-document".into())
            // MuPDF can recurse deeply on complex pages.
            .stack_size(8 * 1024 * 1024)
            .spawn(move || run(rx, etx, wake))
            .expect("could not start the document thread");
        Self { tx, events }
    }

    pub fn send(&self, req: Request) {
        let _ = self.tx.send(req);
    }
}

struct State {
    engine: Option<Engine>,
    generation: u64,
    queue: Vec<RenderKey>,
    /// The GUI asked for the comment list, so keep it up to date.
    wants_annotations: bool,
    wants_signatures: bool,
    wants_fields: bool,
}

fn run(rx: Receiver<Request>, etx: Sender<Event>, wake: impl Fn()) {
    let mut st = State {
        engine: None,
        generation: 0,
        queue: Vec::new(),
        wants_annotations: false,
        wants_signatures: false,
        wants_fields: false,
    };
    let emit = |e: Event| {
        let ok = etx.send(e).is_ok();
        wake();
        ok
    };

    let mut rendered_since_idle = false;
    loop {
        // Block only when there is nothing left to render.
        let first = if st.queue.is_empty() {
            if std::mem::take(&mut rendered_since_idle) {
                crate::engine::release_render_cache();
            }
            match rx.recv() {
                Ok(r) => Some(r),
                Err(_) => return,
            }
        } else {
            match rx.try_recv() {
                Ok(r) => Some(r),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };

        if let Some(req) = first {
            if !handle(&mut st, req, &emit) {
                return;
            }
            continue; // drain all pending requests before rendering
        }

        let key = st.queue.remove(0);
        if key.generation != st.generation {
            continue;
        }
        let Some(engine) = &st.engine else { continue };
        rendered_since_idle = true;
        match engine.render(key.page, key.scale_milli as f32 / 1000.0) {
            Ok(image) => {
                if !emit(Event::Rendered { key, image }) {
                    return;
                }
            }
            Err(message) => {
                emit(Event::RenderFailed { key, message });
            }
        }
    }
}

fn doc_info(st: &State) -> Option<DocInfo> {
    st.engine.as_ref().map(|e| DocInfo {
        path: e.path().to_path_buf(),
        title: e.title(),
        is_pdf: e.is_pdf(),
        dirty: e.is_dirty(),
        generation: st.generation,
        page_sizes: e.page_sizes().to_vec(),
    })
}

fn changed(st: &mut State, emit: &impl Fn(Event) -> bool) -> bool {
    st.generation += 1;
    st.queue.clear();
    match doc_info(st) {
        Some(info) => emit(Event::Doc(info)) && send_annotations(st, emit) && send_signatures(st, emit) && send_fields(st, emit),
        None => true,
    }
}

fn send_fields(st: &State, emit: &impl Fn(Event) -> bool) -> bool {
    match &st.engine {
        Some(engine) if st.wants_fields => emit(Event::Fields {
            generation: st.generation,
            list: engine.form_fields(),
        }),
        _ => true,
    }
}

fn send_signatures(st: &State, emit: &impl Fn(Event) -> bool) -> bool {
    match &st.engine {
        Some(engine) if st.wants_signatures => emit(Event::Signatures {
            generation: st.generation,
            list: engine.signatures(),
        }),
        _ => true,
    }
}

fn send_annotations(st: &State, emit: &impl Fn(Event) -> bool) -> bool {
    match &st.engine {
        Some(engine) if st.wants_annotations => emit(Event::Annotations {
            generation: st.generation,
            list: engine.all_annotations(),
        }),
        _ => true,
    }
}

fn handle(st: &mut State, req: Request, emit: &impl Fn(Event) -> bool) -> bool {
    // Run an edit on the open document, then announce the new state.
    macro_rules! edit {
        ($f:expr) => {{
            let Some(engine) = st.engine.as_mut() else {
                return emit(Event::Error("No document is open".into()));
            };
            #[allow(clippy::redundant_closure_call)]
            match ($f)(engine) {
                Ok(_) => changed(st, emit),
                Err(e) => emit(Event::Error(e)),
            }
        }};
    }

    match req {
        Request::Open(path) => match Engine::open(&path) {
            Ok(engine) => {
                st.engine = Some(engine);
                changed(st, emit)
            }
            Err(e) => emit(Event::Error(format!("Could not open {}: {e}", path.display()))),
        },
        Request::Want(keys) => {
            st.queue = keys
                .into_iter()
                .filter(|k| k.generation == st.generation)
                .collect();
            true
        }
        Request::Rotate { pages, delta } => edit!(|e: &mut Engine| e.rotate_pages(&pages, delta)),
        Request::Delete { pages } => edit!(|e: &mut Engine| e.delete_pages(&pages)),
        Request::Move { from, to } => edit!(|e: &mut Engine| e.move_page(from, to)),
        Request::InsertPdfs { paths, before } => edit!(|e: &mut Engine| {
            let mut at = before;
            for path in &paths {
                let added = e
                    .insert_pdf(path, at)
                    .map_err(|err| format!("{}: {err}", path.display()))?;
                at = at.map(|i| i + added);
            }
            Ok::<(), String>(())
        }),
        Request::Save { target } => {
            let Some(engine) = st.engine.as_mut() else {
                return emit(Event::Error("No document is open".into()));
            };
            match engine.save(target.as_deref()) {
                Ok(path) => changed(st, emit) && emit(Event::Saved(path)),
                Err(e) => {
                    // The engine may have been reopened from disk; resync the GUI either way.
                    changed(st, emit) && emit(Event::Error(e))
                }
            }
        }
        Request::Search { query } => {
            let Some(engine) = &st.engine else { return true };
            let mut hits = Vec::new();
            if !query.is_empty() {
                for page in 0..engine.page_count() {
                    if let Ok(quads) = engine.search_page(page, &query) {
                        hits.extend(quads.into_iter().map(|q| (page, q)));
                    }
                }
            }
            emit(Event::SearchResults { query, hits })
        }
        Request::PageText { page } => {
            let Some(engine) = &st.engine else { return true };
            match engine.page_text(page) {
                Ok(text) => emit(Event::PageText { page, text }),
                Err(e) => emit(Event::Error(e)),
            }
        }
        Request::SelectText { page, a, b, seq } => {
            let Some(engine) = &st.engine else { return true };
            let selection = engine.select_text(page, a, b).unwrap_or_default();
            emit(Event::Selection { page, seq, selection })
        }
        Request::ListAnnotations => {
            st.wants_annotations = true;
            send_annotations(st, emit)
        }
        Request::AddMarkup { page, kind, quads, author, contents } => {
            let Some(engine) = st.engine.as_mut() else { return true };
            match engine.add_markup(page, kind, &quads, &author, &contents) {
                Ok(xref) => changed(st, emit) && emit(Event::AnnotationAdded { page, xref }),
                Err(e) => emit(Event::Error(e)),
            }
        }
        Request::AddNote { page, at, contents, author } => {
            let Some(engine) = st.engine.as_mut() else { return true };
            match engine.add_note(page, at, &contents, &author) {
                Ok(xref) => changed(st, emit) && emit(Event::AnnotationAdded { page, xref }),
                Err(e) => emit(Event::Error(e)),
            }
        }
        Request::EditAnnotation { page, xref, contents } => {
            edit!(|e: &mut Engine| e.set_annotation_contents(page, xref, &contents))
        }
        Request::DeleteAnnotation { page, xref } => {
            edit!(|e: &mut Engine| e.delete_annotation(page, xref))
        }
        Request::StampSignature { page, rect, art, ink } => {
            edit!(|e: &mut Engine| e.stamp_signature(page, rect, &art, ink))
        }
        Request::ListFields => {
            st.wants_fields = true;
            send_fields(st, emit)
        }
        Request::SetField { page, xref, value } => {
            edit!(|e: &mut Engine| e.set_field_value(page, xref, &value))
        }
        Request::SetChecked { page, xref, checked } => {
            edit!(|e: &mut Engine| e.set_field_checked(page, xref, checked))
        }
        Request::ListSignatures => {
            st.wants_signatures = true;
            send_signatures(st, emit)
        }
        Request::SignDocument { target, p12, password, options } => {
            let Some(engine) = st.engine.as_mut() else {
                return emit(Event::Error("No document is open".into()));
            };
            let result = Identity::from_pkcs12(&p12, &password)
                .and_then(|identity| engine.sign_document(target.as_deref(), &identity, &options));
            match result {
                Ok(path) => changed(st, emit) && emit(Event::Signed(path)),
                // The engine may have gone back to the file on disk; resync the GUI either way.
                Err(e) => changed(st, emit) && emit(Event::Error(e)),
            }
        }
        Request::Close => {
            st.engine = None;
            st.generation += 1;
            st.queue.clear();
            emit(Event::Closed)
        }
    }
}
