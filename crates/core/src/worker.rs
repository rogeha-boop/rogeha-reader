//! Background document thread. The GUI never touches MuPDF directly: it sends
//! [`Request`]s and receives [`Event`]s, so opening, rendering and saving never block a frame.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use crate::engine::{Engine, HitQuad, PageImage};

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
}

fn run(rx: Receiver<Request>, etx: Sender<Event>, wake: impl Fn()) {
    let mut st = State {
        engine: None,
        generation: 0,
        queue: Vec::new(),
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
        Some(info) => emit(Event::Doc(info)),
        None => true,
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
        Request::Close => {
            st.engine = None;
            st.generation += 1;
            st.queue.clear();
            emit(Event::Closed)
        }
    }
}
