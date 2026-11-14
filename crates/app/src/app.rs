use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{
    self, Align, Color32, CornerRadius, Key, KeyboardShortcut, Layout, Modifiers, Rect,
    Sense, Stroke, StrokeKind, Vec2, ViewportCommand,
};
use std::sync::Arc;

use rogeha_core::{DocInfo, Event, HitQuad, PageImage, RenderKey, Request, Worker};

use crate::icons::{self, Icon};
use crate::theme;

mod about;
pub use about::version_text;
mod assistant;
mod comments;
mod compress;
mod dsign;
mod forms;
mod navigate;
mod ocr;
mod pages_panel;
mod print;
mod session;
mod sign;

/// PDF points (1/72") to screen pixels at 100% zoom (96 dpi).
const PT_TO_PX: f32 = 96.0 / 72.0;
const PAGE_GAP: f32 = 10.0;
const MARGIN: f32 = 12.0;
/// Upper bound for page textures kept in memory.
const TEXTURE_BUDGET: usize = 40 << 20;
/// Largest page image we render (pixels). Higher zoom levels are upscaled from this.
const MAX_PAGE_PIXELS: f32 = 12.0e6;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.0;
const ZOOM_SETTLE: Duration = Duration::from_millis(70);

struct PageTex {
    key: RenderKey,
    image: Arc<PageImage>,
}

/// What the runner should draw underneath the UI this frame: the document area and its
/// pages. Page pixels are copied straight into the window's frame buffer, so each rendered
/// page exists in memory exactly once (no GPU texture, no second copy inside the UI library).
pub struct PageLayer {
    /// Document area in egui points; nothing is drawn outside it.
    pub clip: Rect,
    pub background: Color32,
    /// Page rectangles in egui points, with the image if it has been rendered.
    pub pages: Vec<(Rect, Option<Arc<PageImage>>)>,
}

/// How pages are laid out.
#[derive(PartialEq, Eq, Clone, Copy)]
enum ViewMode {
    /// One page per row.
    Single,
    /// Two pages side by side, starting with pages 1 and 2 (page 1 is a left-hand page).
    Spread,
    /// Two pages side by side like a printed magazine or book: page 1 stands alone on the
    /// right as the cover, then 2-3, 4-5, ... so even pages are on the left.
    SpreadCover,
}

/// The rows of a layout: the page on the left and the page on the right (either may be
/// missing). In single-page view every row holds one page, stored on the left.
fn layout_rows(count: usize, view: ViewMode) -> Vec<[Option<usize>; 2]> {
    let pair = |left: usize| [Some(left), (left + 1 < count).then_some(left + 1)];
    match view {
        ViewMode::Single => (0..count).map(|p| [Some(p), None]).collect(),
        ViewMode::Spread => (0..count).step_by(2).map(pair).collect(),
        ViewMode::SpreadCover => {
            let mut rows = Vec::new();
            if count > 0 {
                rows.push([None, Some(0)]);
            }
            rows.extend((1..count).step_by(2).map(pair));
            rows
        }
    }
}

/// Gap between the two pages of a spread, in screen points.
const SPREAD_GAP: f32 = 4.0;

#[derive(PartialEq, Clone, Copy)]
enum Fit {
    Width,
    Page,
    Custom,
}

#[derive(Default)]
struct Search {
    open: bool,
    focus: bool,
    query: String,
    /// The query the current hits belong to.
    searched: String,
    busy: bool,
    hits: Vec<(usize, HitQuad)>,
    active: usize,
}

enum AfterSave {
    Nothing,
    CloseApp,
    /// Close the document and show the start screen.
    CloseDoc,
    Open(PathBuf),
}

/// Hidden developer options used for automated checks (see main.rs).
#[derive(Default)]
pub struct DevOptions {
    pub screenshot: Option<PathBuf>,
    pub script: Option<String>,
}

pub struct ReaderApp {
    dev: DevOptions,
    dev_steps: Vec<String>,
    dev_wait: u32,
    worker: Worker,
    doc: Option<DocInfo>,
    opening: bool,

    zoom: f32,
    fit: Fit,
    zoom_changed_at: Option<Instant>,

    textures: HashMap<usize, PageTex>,
    layer: PageLayer,
    /// On-screen pages plus read-ahead; never evicted.
    visible_pages: Vec<usize>,
    failed: HashSet<RenderKey>,
    last_want: Vec<RenderKey>,

    // Layout cache: top of each page in content coordinates (egui points).
    tops: Vec<f32>,
    /// Left edge of each page relative to the centre line of the view, at the current zoom.
    lefts: Vec<f32>,
    rows: Vec<[Option<usize>; 2]>,
    view: ViewMode,
    fullscreen: bool,
    /// The fit mode to return to when full screen ends, and when its hint disappears.
    fit_before_fullscreen: Fit,
    fullscreen_since: Option<Instant>,
    layout_for: (u64, u32, ViewMode),
    content_size: Vec2,

    current_page: usize,
    scroll_offset: Vec2,
    viewport_size: Vec2,
    pending_scroll: Option<Vec2>,
    /// Jump to this page once the layout for the new document state exists.
    pending_page: Option<usize>,

    search: Search,
    comments: comments::Comments,
    compress: compress::CompressUi,
    about: about::About,
    ocr: ocr::OcrUi,
    sign: sign::SignUi,
    digital: dsign::DigitalUi,
    forms: forms::FormsUi,
    pages: pages_panel::PagesUi,
    session: session::Session,
    nav: navigate::Nav,
    print_busy: bool,
    assistant: assistant::AssistantUi,
    message: Option<(String, Instant, bool)>,
    after_save: AfterSave,
    allow_close: bool,

    started: Instant,
    bench: bool,
    bench_frames: u32,
    first_page_ms: Option<f64>,
}

impl ReaderApp {
    pub fn new(
        ctx: &egui::Context,
        file: Option<PathBuf>,
        started: Instant,
        bench: bool,
        dev: DevOptions,
    ) -> Self {
        let ctx = ctx.clone();
        // Ctrl +/- zooms the document, not the whole UI.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        theme::apply(&ctx);
        let wake_ctx = ctx.clone();
        let worker = Worker::spawn(move || wake_ctx.request_repaint());

        let dev_steps = dev
            .script
            .as_deref()
            .unwrap_or_default()
            .split(';')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .rev()
            .collect();
        let mut app = Self {
            dev,
            dev_steps,
            dev_wait: 0,
            worker,
            doc: None,
            opening: false,
            zoom: 1.0,
            fit: Fit::Width,
            zoom_changed_at: None,
            textures: HashMap::new(),
            visible_pages: Vec::new(),
            layer: PageLayer {
                clip: Rect::NOTHING,
                background: Color32::BLACK,
                pages: Vec::new(),
            },
            failed: HashSet::new(),
            last_want: Vec::new(),
            tops: Vec::new(),
            lefts: Vec::new(),
            rows: Vec::new(),
            view: ViewMode::Single,
            fullscreen: false,
            fit_before_fullscreen: Fit::Width,
            fullscreen_since: None,
            layout_for: (u64::MAX, 0, ViewMode::Single),
            content_size: Vec2::ZERO,
            current_page: 0,
            scroll_offset: Vec2::ZERO,
            viewport_size: Vec2::ZERO,
            pending_scroll: None,
            pending_page: None,
            search: Search::default(),
            comments: comments::Comments::default(),
            compress: compress::CompressUi::default(),
            about: about::About::default(),
            ocr: ocr::OcrUi::default(),
            sign: sign::SignUi::default(),
            digital: dsign::DigitalUi::default(),
            forms: forms::FormsUi::default(),
            pages: pages_panel::PagesUi::default(),
            session: session::Session::load(),
            nav: navigate::Nav::default(),
            print_busy: false,
            assistant: assistant::AssistantUi::default(),
            message: None,
            after_save: AfterSave::Nothing,
            allow_close: false,
            started,
            bench,
            bench_frames: 0,
            first_page_ms: None,
        };
        if let Some(path) = file {
            app.open_path(path);
        }
        app
    }

    // ---------------------------------------------------------------- helpers

    fn page_count(&self) -> usize {
        self.doc.as_ref().map_or(0, |d| d.page_sizes.len())
    }

    fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.dirty)
    }

    fn can_edit(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.is_pdf)
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now(), false));
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now(), true));
    }

    fn open_path(&mut self, path: PathBuf) {
        self.remember_place();
        self.opening = true;
        self.worker.send(Request::Open(path));
    }

    /// Open a file (in its own tab), asking first if there are unsaved changes.
    /// Returns false when the user cancelled.
    fn request_open(&mut self, path: PathBuf) -> bool {
        if self.doc.as_ref().is_some_and(|d| d.path == path) {
            return true; // already in front
        }
        if self.is_dirty() {
            match ask_save_changes() {
                rfd::MessageDialogResult::Yes => {
                    self.after_save = AfterSave::Open(path);
                    self.worker.send(Request::Save { target: None });
                    return true;
                }
                rfd::MessageDialogResult::No => {}
                _ => return false,
            }
        }
        self.open_path(path);
        true
    }

    fn open_dialog(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter(
                "Documents",
                &["pdf", "epub", "cbz", "xps", "oxps", "fb2", "mobi", "svg"],
            )
            .add_filter("PDF", &["pdf"])
            .add_filter("EPUB", &["epub"])
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(path) = picked {
            self.request_open(path);
        }
    }

    fn save(&mut self) {
        self.commit_field_text();
        if self.is_dirty() {
            self.worker.send(Request::Save { target: None });
        }
    }

    fn save_as(&mut self) {
        let Some(doc) = &self.doc else { return };
        if !doc.is_pdf {
            return;
        }
        let mut dialog = rfd::FileDialog::new().add_filter("PDF", &["pdf"]);
        if let Some(dir) = doc.path.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(name) = doc.path.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(path) = dialog.save_file() {
            self.session.save_as_target = Some(path.clone());
            self.worker.send(Request::Save { target: Some(path) });
        }
    }

    fn set_zoom(&mut self, zoom: f32, fit: Fit) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.fit = fit;
        if (zoom - self.zoom).abs() < 1e-4 {
            return;
        }
        // Keep the centre of the view in place.
        let ratio = zoom / self.zoom;
        let centre = self.scroll_offset + self.viewport_size * 0.5;
        let new_offset = centre * ratio - self.viewport_size * 0.5;
        self.pending_scroll = Some(Vec2::new(new_offset.x.max(0.0), new_offset.y.max(0.0)));
        self.zoom = zoom;
        self.zoom_changed_at = Some(Instant::now());
    }

    fn zoom_step(&mut self, up: bool) {
        const STEPS: [f32; 15] = [
            0.1, 0.25, 0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0,
        ];
        let z = self.zoom;
        let next = if up {
            STEPS.iter().copied().find(|&s| s > z * 1.01)
        } else {
            STEPS.iter().rev().copied().find(|&s| s < z * 0.99)
        };
        if let Some(next) = next {
            self.set_zoom(next, Fit::Custom);
        }
    }

    fn go_to_page(&mut self, page: usize) {
        let n = self.page_count();
        if n == 0 {
            return;
        }
        let page = page.min(n - 1);
        if let Some(&top) = self.tops.get(page) {
            self.pending_scroll = Some(Vec2::new(self.scroll_offset.x, (top - 4.0).max(0.0)));
            self.current_page = page;
        } else {
            self.pending_page = Some(page);
        }
    }

    /// Go to the next or previous row: one page in single view, one spread in two-page view.
    fn step_row(&mut self, forward: bool) {
        let Some(row) = self.rows.iter().position(|r| r.contains(&Some(self.current_page))) else { return };
        let target = if forward { row + 1 } else { row.saturating_sub(1) };
        if let Some(page) = self.rows.get(target).and_then(|r| r[0].or(r[1])) {
            self.go_to_page(page);
        }
    }

    fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        if on == self.fullscreen {
            return;
        }
        self.fullscreen = on;
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(on));
        if on {
            // Full screen shows whole pages; the previous fit comes back afterwards.
            self.fit_before_fullscreen = self.fit;
            self.fit = Fit::Page;
            self.fullscreen_since = Some(Instant::now());
        } else {
            self.fit = self.fit_before_fullscreen;
            self.fullscreen_since = None;
        }
        self.pending_page = Some(self.current_page);
        self.layout_for.0 = u64::MAX; // lay out again
    }

    fn set_view(&mut self, view: ViewMode) {
        if view != self.view {
            self.view = view;
            self.pending_page = Some(self.current_page);
        }
    }

    fn scroll_by(&mut self, dy: f32) {
        let max_y = (self.content_size.y - self.viewport_size.y).max(0.0);
        let base = self.pending_scroll.unwrap_or(self.scroll_offset);
        self.pending_scroll = Some(Vec2::new(base.x, (base.y + dy).clamp(0.0, max_y)));
    }

    fn rotate(&mut self, pages: Vec<usize>, delta: i32) {
        if self.can_edit() {
            self.pending_page = Some(self.current_page);
            self.worker.send(Request::Rotate { pages, delta });
        }
    }

    fn run_search(&mut self) {
        let query = self.search.query.trim().to_owned();
        if query.is_empty() {
            self.search.hits.clear();
            self.search.searched.clear();
            return;
        }
        if query == self.search.searched {
            self.next_hit(true);
            return;
        }
        self.search.busy = true;
        self.worker.send(Request::Search { query });
    }

    fn next_hit(&mut self, forward: bool) {
        let n = self.search.hits.len();
        if n == 0 {
            return;
        }
        self.search.active = if forward {
            (self.search.active + 1) % n
        } else {
            (self.search.active + n - 1) % n
        };
        self.show_active_hit();
    }

    fn show_active_hit(&mut self) {
        let Some(&(page, quad)) = self.search.hits.get(self.search.active) else {
            return;
        };
        let Some(&top) = self.tops.get(page) else { return };
        let scale = self.zoom * PT_TO_PX;
        let y = top + quad[0].1 * scale - self.viewport_size.y * 0.35;
        self.pending_scroll = Some(Vec2::new(self.scroll_offset.x, y.max(0.0)));
    }

    // ----------------------------------------------------------- worker events

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Doc(info) => {
                    let same_file = self.doc.as_ref().is_some_and(|d| d.path == info.path);
                    let previous = self.current_place();
                    self.nav.links_changed();
                    self.opening = false;
                    // After an edit that keeps the page layout (a comment, a save), keep showing
                    // the old page images until the new ones arrive, so nothing flashes white.
                    let same_layout = same_file
                        && self.doc.as_ref().is_some_and(|d| d.page_sizes == info.page_sizes);
                    if !same_layout {
                        self.textures.clear();
                        // Thumbnails are redrawn; a selection survives only a pure reorder or rotation.
                        let same_count = self.doc.as_ref().is_some_and(|d| same_file && d.page_sizes.len() == info.page_sizes.len());
                        self.pages.reset(same_count);
                    }
                    if !same_file {
                        self.comments.reset_for_new_document();
                        self.digital.reset_for_new_document();
                        self.forms.reset_for_new_document();
                        self.nav.reset_for_new_document();
                    }
                    self.failed.clear();
                    self.last_want.clear();
                    if !same_file {
                        self.current_page = 0;
                        self.scroll_offset = Vec2::ZERO;
                        self.pending_scroll = Some(Vec2::ZERO);
                        self.pending_page = None;
                        self.search.hits.clear();
                        self.search.searched.clear();
                        if self.fit == Fit::Custom {
                            self.fit = Fit::Width;
                        }
                    } else if !self.search.searched.is_empty() {
                        // Page numbers may have shifted; search again.
                        self.search.busy = true;
                        self.worker.send(Request::Search {
                            query: self.search.searched.clone(),
                        });
                    }
                    self.doc = Some(info);
                    if !same_file {
                        self.document_arrived(previous);
                    }
                    self.update_title(ctx);
                    self.assistant_edit_outcome(Ok(()));
                }
                Event::Rendered { key, image } => {
                    let current = self.doc.as_ref().is_some_and(|d| d.generation == key.generation);
                    if !current {
                        continue;
                    }
                    if key.thumb {
                        self.thumbnail_arrived(ctx, key, image);
                        continue;
                    }
                    let mut image = image;
                    if self.session.dark_pages {
                        session::darken(&mut image.pixels);
                    }
                    self.textures.insert(key.page, PageTex { key, image: Arc::new(image) });
                    self.evict_textures();
                    if self.first_page_ms.is_none() {
                        self.first_page_ms = Some(self.started.elapsed().as_secs_f64() * 1000.0);
                    }
                }
                Event::RenderFailed { key, message } => {
                    self.failed.insert(key);
                    self.error(format!("Page {}: {message}", key.page + 1));
                }
                Event::Saved(path) => {
                    self.info(format!("Saved {}", path.display()));
                    match std::mem::replace(&mut self.after_save, AfterSave::Nothing) {
                        AfterSave::Nothing => {}
                        AfterSave::CloseApp => {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                        AfterSave::CloseDoc => {
                            self.remember_place();
                            self.worker.send(Request::Close);
                        }
                        AfterSave::Open(next) => self.open_path(next),
                    }
                }
                Event::SearchResults { query, hits } => {
                    self.search.busy = false;
                    self.search.searched = query;
                    self.search.hits = hits;
                    // Start at the first hit on or after the current page.
                    self.search.active = self
                        .search
                        .hits
                        .iter()
                        .position(|(p, _)| *p >= self.current_page)
                        .unwrap_or(0);
                    self.show_active_hit();
                    self.assistant_search_done();
                }
                Event::PageText { .. } => {}
                Event::PagesText(result) => self.assistant_pages_text(result),
                Event::Selection { page, seq, selection } => {
                    if seq == self.comments.selection_seq {
                        self.comments.selection = (!selection.quads.is_empty()).then_some(
                            comments::Selection { page, quads: selection.quads, text: selection.text },
                        );
                    }
                }
                Event::Annotations { generation, list } => {
                    if self.doc.as_ref().is_some_and(|d| d.generation == generation) {
                        self.comments.list = list;
                        if let Some(sel) = self.comments.selected {
                            if !self.comments.list.iter().any(|a| (a.page, a.xref) == sel) {
                                self.comments.selected = None;
                            }
                        }
                        if let Some(key) = self.comments.open_when_listed.take() {
                            self.comments.selected = Some(key);
                        }
                    }
                }
                Event::Fields { generation, list } => {
                    if self.doc.as_ref().is_some_and(|d| d.generation == generation) {
                        self.set_form_fields(list);
                    }
                }
                Event::Signatures { generation, list } => {
                    if self.doc.as_ref().is_some_and(|d| d.generation == generation) {
                        self.digital.list = list;
                    }
                }
                Event::Signed(path) => {
                    self.digital.busy = false;
                    self.sign.open = false;
                    self.digital.window_open = true;
                    self.info(format!("Signed and saved as {}", path.display()));
                }
                Event::AnnotationAdded { page, xref } => {
                    self.comments.open_when_listed = Some((page, xref));
                }
                Event::PasswordNeeded { path, wrong } => self.ask_password(path, wrong),
                Event::Outline(list) => self.nav.outline = list,
                Event::Links { generation, page, links } => {
                    if self.doc.as_ref().is_some_and(|d| d.generation == generation) {
                        self.nav.links.insert(page, links);
                    }
                }
                Event::Done(text) => {
                    self.print_busy = false;
                    self.info(text);
                }
                Event::Closed => {
                    self.doc = None;
                    self.session.tabs.clear();
                    self.session.active = 0;
                    self.nav.reset_for_new_document();
                    self.textures.clear();
                    self.update_title(ctx);
                }
                Event::Error(message) => {
                    self.opening = false;
                    self.digital.busy = false;
                    self.search.busy = false;
                    self.after_save = AfterSave::Nothing;
                    self.print_busy = false;
                    self.session.save_as_target = None;
                    self.session.closing_tab = None;
                    self.assistant_edit_outcome(Err(&message));
                    self.error(message);
                }
            }
        }
    }

    fn update_title(&self, ctx: &egui::Context) {
        let title = match &self.doc {
            Some(doc) => {
                let name = doc
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let star = if doc.dirty { "*" } else { "" };
                format!("{name}{star} - Rogeha Reader")
            }
            None => "Rogeha Reader".to_owned(),
        };
        ctx.send_viewport_cmd(ViewportCommand::Title(title));
    }

    fn evict_textures(&mut self) {
        let mut total: usize = self.textures.values().map(|t| t.image.bytes()).sum();
        if total <= TEXTURE_BUDGET {
            return;
        }
        let wanted: HashSet<usize> = self.last_want.iter().filter(|k| !k.thumb).map(|k| k.page).collect();
        let mut pages: Vec<usize> = self.textures.keys().copied().collect();
        let here = self.current_page;
        pages.sort_by_key(|p| std::cmp::Reverse(p.abs_diff(here)));
        for page in pages {
            if total <= TEXTURE_BUDGET {
                break;
            }
            if self.visible_pages.contains(&page) || wanted.contains(&page) {
                continue;
            }
            if let Some(t) = self.textures.remove(&page) {
                total -= t.image.bytes();
            }
        }
    }

    // ------------------------------------------------------------------ input

    fn handle_input(&mut self, ctx: &egui::Context) {
        // Files dropped on the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        // The first one opens; the others wait in tabs.
        for extra in dropped.iter().skip(1) {
            if !self.session.tabs.iter().any(|t| &t.path == extra) {
                self.session.tabs.push(session::Place::new(extra));
            }
        }
        if let Some(path) = dropped.into_iter().next() {
            self.request_open(path);
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            self.remember_place();
        }

        // Closing with unsaved changes.
        if ctx.input(|i| i.viewport().close_requested()) && self.is_dirty() && !self.allow_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            match ask_save_changes() {
                rfd::MessageDialogResult::Yes => {
                    self.after_save = AfterSave::CloseApp;
                    self.worker.send(Request::Save { target: None });
                }
                rfd::MessageDialogResult::No => {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                _ => {}
            }
        }

        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let hit = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));

        if hit(ctrl_shift, Key::S) {
            self.save_as();
        }
        if hit(ctrl, Key::S) {
            self.save();
        }
        if hit(ctrl, Key::O) || hit(ctrl, Key::T) {
            self.open_dialog();
        }
        if hit(ctrl, Key::P) {
            self.print();
        }
        if hit(ctrl, Key::W) {
            self.close_tab(self.session.active);
        }
        if hit(ctrl_shift, Key::Tab) {
            self.next_tab(false);
        }
        if hit(ctrl, Key::Tab) {
            self.next_tab(true);
        }
        if hit(ctrl, Key::F) {
            self.search.open = true;
            self.search.focus = true;
        }
        if hit(ctrl, Key::Plus) || hit(ctrl, Key::Equals) {
            self.zoom_step(true);
        }
        if hit(ctrl, Key::Minus) {
            self.zoom_step(false);
        }
        if hit(ctrl, Key::Num0) {
            self.fit = Fit::Width;
        }
        if hit(ctrl, Key::Num1) {
            self.set_zoom(1.0, Fit::Custom);
        }

        // Ctrl + wheel / pinch.
        let zoom_delta = ctx.input(|i| i.zoom_delta());
        if (zoom_delta - 1.0).abs() > 1e-3 && self.doc.is_some() {
            self.set_zoom(self.zoom * zoom_delta, Fit::Custom);
        }

        if hit(Modifiers::NONE, Key::F11) {
            self.set_fullscreen(ctx, !self.fullscreen);
        }
        if self.fullscreen && !self.search.open && hit(Modifiers::NONE, Key::Escape) {
            self.set_fullscreen(ctx, false);
        }
        if self.search.open && hit(Modifiers::NONE, Key::Escape) {
            self.search.open = false;
        }
        if !ctx.egui_wants_keyboard_input() {
            if hit(ctrl_shift, Key::Z) || hit(ctrl, Key::Y) {
                self.worker.send(Request::Redo);
            }
            if hit(ctrl, Key::Z) {
                self.worker.send(Request::Undo);
            }
            // Ctrl+C copies the selected page text.
            if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
                self.copy_selection(ctx);
            }
            if hit(Modifiers::NONE, Key::Escape) {
                self.comments.selection = None;
                self.comments.note_tool = false;
            }
        }

        // Plain keys only when no text field has focus.
        if ctx.egui_wants_keyboard_input() || self.doc.is_none() {
            return;
        }
        let page_h = self.viewport_size.y * 0.9;
        if hit(Modifiers::NONE, Key::PageDown) || hit(Modifiers::NONE, Key::Space) {
            self.scroll_by(page_h);
        }
        if hit(Modifiers::NONE, Key::PageUp) || hit(Modifiers::SHIFT, Key::Space) {
            self.scroll_by(-page_h);
        }
        if hit(Modifiers::NONE, Key::ArrowDown) {
            self.scroll_by(60.0);
        }
        if hit(Modifiers::NONE, Key::ArrowUp) {
            self.scroll_by(-60.0);
        }
        if hit(Modifiers::ALT, Key::ArrowLeft) {
            self.go_back();
        }
        if hit(Modifiers::ALT, Key::ArrowRight) {
            self.go_forward();
        }
        if hit(Modifiers::NONE, Key::ArrowRight) {
            self.step_row(true);
        }
        if hit(Modifiers::NONE, Key::ArrowLeft) {
            self.step_row(false);
        }
        if hit(Modifiers::NONE, Key::Home) {
            self.go_to_page(0);
        }
        if hit(Modifiers::NONE, Key::End) {
            self.go_to_page(usize::MAX);
        }
        if hit(Modifiers::SHIFT, Key::R) {
            self.rotate(vec![self.current_page], -90);
        }
        if hit(Modifiers::NONE, Key::R) {
            self.rotate(vec![self.current_page], 90);
        }
        if hit(Modifiers::NONE, Key::F3) {
            self.next_hit(true);
        }
        if hit(Modifiers::SHIFT, Key::F3) {
            self.next_hit(false);
        }
    }

    // --------------------------------------------------------------------- UI

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let has_doc = self.doc.is_some();
        let can_edit = self.can_edit();
        let edit_hint = if has_doc && !can_edit {
            "Page editing works on PDF files"
        } else {
            ""
        };

        ui.horizontal_wrapped(|ui| {
            if icons::icon_button(ui, Icon::Open, false, true).on_hover_text("Open a file (Ctrl+O)").clicked() {
                self.open_dialog();
            }
            let recent = icons::menu_button(ui, "", !self.session.recent.is_empty())
                .on_hover_text("Recent files")
                .on_disabled_hover_text("Recent files (none yet)");
            let mut reopen = None;
            egui::Popup::menu(&recent).show(|ui| {
                reopen = self.recent_list(ui, 12);
                if reopen.is_some() {
                    ui.close();
                }
            });
            if let Some(path) = reopen {
                self.request_open(path);
            }
            if icons::icon_button(ui, Icon::Save, false, self.is_dirty())
                .on_hover_text("Save changes into the file (Ctrl+S)")
                .on_disabled_hover_text("Save (nothing to save yet)")
                .clicked()
            {
                self.save();
            }
            if icons::icon_button(ui, Icon::SaveAs, false, can_edit)
                .on_hover_text("Save as: save a copy under a new name (Ctrl+Shift+S)")
                .on_disabled_hover_text("Save as")
                .clicked()
            {
                self.save_as();
            }
            if icons::icon_button(ui, Icon::Print, false, has_doc).on_hover_text("Print (Ctrl+P)").clicked() {
                self.print();
            }

            ui.separator();

            if icons::icon_button(ui, Icon::ZoomOut, false, has_doc).on_hover_text("Zoom out (Ctrl -)").clicked() {
                self.zoom_step(false);
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0)).on_hover_text("Zoom");
            if icons::icon_button(ui, Icon::ZoomIn, false, has_doc).on_hover_text("Zoom in (Ctrl +)").clicked() {
                self.zoom_step(true);
            }
            if icons::icon_button(ui, Icon::FitWidth, self.fit == Fit::Width, has_doc)
                .on_hover_text("Fit width (Ctrl+0)")
                .clicked()
            {
                self.fit = Fit::Width;
            }
            if icons::icon_button(ui, Icon::FitPage, self.fit == Fit::Page, has_doc)
                .on_hover_text("Fit whole page")
                .clicked()
            {
                self.fit = Fit::Page;
            }

            let view = icons::menu_button(ui, "View", has_doc).on_hover_text("Page layout and full screen");
            egui::Popup::menu(&view).show(|ui| {
                let mut choice = self.view;
                ui.radio_value(&mut choice, ViewMode::Single, "One page");
                ui.radio_value(&mut choice, ViewMode::Spread, "Two pages (1-2, 3-4, …)")
                    .on_hover_text("Page 1 is a left-hand page");
                ui.radio_value(&mut choice, ViewMode::SpreadCover, "Two pages, cover alone (1, 2-3, 4-5, …)")
                    .on_hover_text("Like a printed magazine: page 1 is the cover on the right");
                if choice != self.view {
                    self.set_view(choice);
                    ui.close();
                }
                ui.separator();
                let mut dark = self.session.dark_pages;
                if ui.checkbox(&mut dark, "Dark pages").on_hover_text("Light text on dark pages, for reading at night").changed() {
                    self.set_dark_pages(dark);
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(self.nav.can_go_back(), egui::Button::new("Back (Alt+Left)")).clicked() {
                    self.go_back();
                    ui.close();
                }
                if ui.add_enabled(self.nav.can_go_forward(), egui::Button::new("Forward (Alt+Right)")).clicked() {
                    self.go_forward();
                    ui.close();
                }
                ui.separator();
                if ui.button(if self.fullscreen { "Leave full screen (F11)" } else { "Full screen (F11)" }).clicked() {
                    let on = !self.fullscreen;
                    self.set_fullscreen(ui.ctx(), on);
                    ui.close();
                }
            });

            ui.separator();

            let (can_undo, can_redo) = self.doc.as_ref().map_or((false, false), |d| (d.can_undo, d.can_redo));
            if icons::icon_button(ui, Icon::Undo, false, can_undo)
                .on_hover_text("Undo the last change (Ctrl+Z)")
                .on_disabled_hover_text("Undo (nothing to undo)")
                .clicked()
            {
                self.worker.send(Request::Undo);
            }
            if icons::icon_button(ui, Icon::Redo, false, can_redo)
                .on_hover_text("Redo (Ctrl+Y)")
                .on_disabled_hover_text("Redo (nothing to redo)")
                .clicked()
            {
                self.worker.send(Request::Redo);
            }

            ui.separator();

            let page = self.current_page;
            if icons::icon_button(ui, Icon::RotateLeft, false, can_edit)
                .on_hover_text("Rotate this page left (Shift+R)")
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.rotate(vec![page], -90);
            }
            if icons::icon_button(ui, Icon::RotateRight, false, can_edit)
                .on_hover_text("Rotate this page right (R)")
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.rotate(vec![page], 90);
            }
            if icons::icon_button(ui, Icon::Pages, self.pages.open, has_doc)
                .on_hover_text("Pages: thumbnails, reorder, rotate, delete, add PDFs")
                .clicked()
            {
                self.pages.open = !self.pages.open;
                // One side panel on the left at a time.
                if self.pages.open {
                    self.comments.panel_open = false;
                    self.nav.outline_open = false;
                    self.pages.reset(true);
                }
            }
            if icons::icon_button(ui, Icon::Outline, self.nav.outline_open, has_doc)
                .on_hover_text("Outline: the document's table of contents")
                .clicked()
            {
                self.nav.outline_open = !self.nav.outline_open;
                if self.nav.outline_open {
                    self.pages.open = false;
                    self.comments.panel_open = false;
                }
            }

            ui.separator();

            if icons::icon_button(ui, Icon::Sign, self.sign.open || self.sign.placing.is_some(), can_edit)
                .on_hover_text("Sign: add your signature, or sign with a certificate")
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.sign.placing = None;
                self.sign.open = !self.sign.open;
            }
            // The number of comments is shown next to the icon.
            let count = match self.comments.list.len() {
                0 => String::new(),
                n => n.to_string(),
            };
            if icons::icon_button_with_text(ui, Icon::Comments, &count, self.comments.panel_open, has_doc)
                .on_hover_text("Comments: list, add a note")
                .clicked()
            {
                self.comments.panel_open = !self.comments.panel_open;
                if self.comments.panel_open {
                    self.pages.open = false;
                    self.nav.outline_open = false;
                }
            }

            ui.separator();

            // Whole-document tools live in a menu (marked by the chevron).
            let tools = icons::menu_button(ui, "Tools", true).on_hover_text("Tools for the whole document, and settings");
            egui::Popup::menu(&tools).show(|ui| {
                let pdf_only = if edit_hint.is_empty() { "Open a PDF first" } else { "Works on PDF files" };
                if ui
                    .add_enabled(can_edit, egui::Button::new("Recognize text (OCR)…"))
                    .on_hover_text("Make a scanned PDF searchable")
                    .on_disabled_hover_text(pdf_only)
                    .clicked()
                {
                    ui.close();
                    self.ocr.open = true;
                }
                if ui
                    .add_enabled(can_edit, egui::Button::new("Compress PDF…"))
                    .on_hover_text("Make a smaller copy of a scanned PDF")
                    .on_disabled_hover_text(pdf_only)
                    .clicked()
                {
                    ui.close();
                    self.compress.open = true;
                }
                ui.separator();
                let language = self.session.translate_to.clone();
                if ui
                    .add_enabled(has_doc, egui::Button::new(format!("Translate this page into {language}")))
                    .on_hover_text("The assistant translates the page you are on. To translate a passage, select it and choose Translate.")
                    .clicked()
                {
                    ui.close();
                    self.translate(false);
                }
                ui.menu_button("Translate into", |ui| {
                    let mut choice = language.clone();
                    for name in Self::LANGUAGES {
                        ui.radio_value(&mut choice, name.to_owned(), name);
                    }
                    ui.horizontal(|ui| {
                        ui.label("Other:");
                        let mut typed = if Self::LANGUAGES.contains(&language.as_str()) { String::new() } else { language.clone() };
                        if ui.add(egui::TextEdit::singleline(&mut typed).desired_width(110.0)).changed() && !typed.trim().is_empty() {
                            choice = typed;
                        }
                    });
                    self.set_translate_to(&choice);
                });
                ui.separator();
                if ui
                    .button("Make Rogeha Reader the default PDF viewer…")
                    .on_hover_text("Registers the reader with Windows and opens the Default apps page, where you confirm the choice")
                    .clicked()
                {
                    ui.close();
                    match crate::install::make_default() {
                        Ok(text) => self.info(text),
                        Err(e) => self.error(e),
                    }
                }
            });

            ui.separator();

            if icons::icon_button(ui, Icon::Find, self.search.open, has_doc).on_hover_text("Find in the document (Ctrl+F)").clicked() {
                self.search.open = !self.search.open;
                self.search.focus = self.search.open;
            }
            if icons::icon_button(ui, Icon::Assistant, self.assistant.open, true)
                .on_hover_text("AI assistant")
                .clicked()
            {
                self.assistant.open = !self.assistant.open;
            }
            if icons::icon_button(ui, Icon::About, self.about.open, true).on_hover_text("About Rogeha Reader").clicked() {
                self.about.open = true;
            }
        });

        if self.search.open {
            ui.horizontal(|ui| {
                ui.label("Find:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search.query)
                        .desired_width(240.0)
                        .hint_text("text to search for"),
                );
                if std::mem::take(&mut self.search.focus) {
                    resp.request_focus();
                }
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if enter {
                    self.run_search();
                    resp.request_focus();
                }
                if ui.button("Previous").clicked() {
                    self.next_hit(false);
                }
                if ui.button("Next").clicked() {
                    if self.search.query.trim() == self.search.searched {
                        self.next_hit(true);
                    } else {
                        self.run_search();
                    }
                }
                if self.search.busy {
                    ui.spinner();
                } else if !self.search.searched.is_empty() {
                    if self.search.hits.is_empty() {
                        ui.label("No matches");
                    } else {
                        ui.label(format!("{} of {}", self.search.active + 1, self.search.hits.len()));
                    }
                }
                if ui.button("Close").clicked() {
                    self.search.open = false;
                }
            });
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let mut jump: Option<usize> = None;
        ui.horizontal(|ui| {
            if let Some(doc) = &self.doc {
                let kind = doc
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().to_uppercase())
                    .unwrap_or_else(|| "Document".into());
                // The page number doubles as "go to page": click it and type, or drag it.
                let count = doc.page_sizes.len();
                let mut page_no = self.current_page + 1;
                ui.label("Page");
                let field = ui
                    .add(egui::DragValue::new(&mut page_no).range(1..=count.max(1)).speed(0.1))
                    .on_hover_text("Click to type a page number");
                if field.changed() && page_no != self.current_page + 1 {
                    jump = Some(page_no - 1);
                }
                ui.label(format!("of {count}"));
                ui.separator();
                ui.label(kind);
                if doc.dirty {
                    ui.separator();
                    ui.label("Unsaved changes");
                }
                if let Some((text, warning)) = self.signature_summary() {
                    ui.separator();
                    let color = if warning { ui.visuals().error_fg_color } else { ui.visuals().hyperlink_color };
                    if ui.link(egui::RichText::new(text).color(color)).on_hover_text("Show the signature details").clicked() {
                        self.digital.window_open = true;
                    }
                }
            } else {
                ui.label("No document");
            }
            if let Some((text, at, is_error)) = &self.message {
                if at.elapsed() < Duration::from_secs(8) {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let color = if *is_error {
                            ui.visuals().error_fg_color
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.add(egui::Label::new(egui::RichText::new(text).color(color)).truncate());
                    });
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                } else {
                    self.message = None;
                }
            }
        });
        if let Some(page) = jump {
            self.go_to_page(page);
        }
    }

    fn ensure_layout(&mut self, avail: Vec2) {
        let Some(doc) = &self.doc else { return };
        if doc.page_sizes.is_empty() {
            return;
        }
        let generation = doc.generation;
        let sizes = &doc.page_sizes.clone();
        let rows = layout_rows(sizes.len(), self.view);
        let two_up = self.view != ViewMode::Single;
        let gutter = if two_up { SPREAD_GAP } else { 0.0 };
        // Width of a row in points. In a spread each side gets the wider page's width, so the
        // line between the pages stays in the middle even when one side is empty (the cover).
        let row_width = |row: &[Option<usize>; 2]| {
            let w = |p: Option<usize>| p.map_or(0.0, |p| sizes[p].0);
            if two_up { 2.0 * w(row[0]).max(w(row[1])) } else { w(row[0]) }
        };
        let row_height = |row: &[Option<usize>; 2]| row.iter().flatten().map(|p| sizes[*p].1).fold(1.0, f32::max);
        let max_w = rows.iter().map(row_width).fold(1.0, f32::max);

        // Fit modes follow the window size.
        let scrollbar = 14.0;
        match self.fit {
            Fit::Width => {
                let z = (avail.x - 2.0 * MARGIN - scrollbar - gutter) / (max_w * PT_TO_PX);
                self.set_zoom(z, Fit::Width);
            }
            Fit::Page => {
                let current = self.current_page.min(sizes.len() - 1);
                let row = rows.iter().find(|r| r.contains(&Some(current))).copied().unwrap_or([Some(current), None]);
                let zw = (avail.x - 2.0 * MARGIN - scrollbar - gutter) / (row_width(&row) * PT_TO_PX);
                let zh = (avail.y - 2.0 * PAGE_GAP) / (row_height(&row) * PT_TO_PX);
                self.set_zoom(zw.min(zh), Fit::Page);
            }
            Fit::Custom => {}
        }

        let key = (generation, (self.zoom * 10_000.0) as u32, self.view);
        if self.layout_for == key && self.tops.len() == sizes.len() {
            return;
        }
        let scale = self.zoom * PT_TO_PX;
        let mut y = PAGE_GAP;
        self.tops = vec![0.0; sizes.len()];
        self.lefts = vec![0.0; sizes.len()];
        for row in &rows {
            if let Some(left) = row[0] {
                self.tops[left] = y;
                // Single view: centred. Spread: its right edge sits at the gutter.
                self.lefts[left] = if two_up { -gutter / 2.0 - sizes[left].0 * scale } else { -sizes[left].0 * scale / 2.0 };
            }
            if let Some(right) = row[1] {
                self.tops[right] = y;
                self.lefts[right] = gutter / 2.0;
            }
            y += row_height(row) * scale + PAGE_GAP;
        }
        self.content_size = Vec2::new(max_w * scale + gutter + 2.0 * MARGIN, y);
        self.rows = rows;
        self.layout_for = key;
        if let Some(page) = self.pending_page.take() {
            self.go_to_page(page);
        }
    }

    fn document_view(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        self.ensure_layout(avail);
        let Some(doc) = &self.doc else { return };
        let generation = doc.generation;
        let n = doc.page_sizes.len();
        let scale = self.zoom * PT_TO_PX;
        let ppp = ui.ctx().pixels_per_point();

        // Dragging with the mouse selects text; the wheel, scroll bars and touch scroll.
        let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
        if let Some(offset) = self.pending_scroll.take() {
            area = area.scroll_offset(offset);
        }

        let mut visible: Vec<usize> = Vec::new();
        let mut page_hits: Vec<(usize, Rect, egui::Response)> = Vec::new();
        let content_size = self.content_size;
        let out = area.show_viewport(ui, |ui, viewport| {
            let size = Vec2::new(content_size.x.max(viewport.width()), content_size.y);
            let (content, _) = ui.allocate_exact_size(size, Sense::hover());
            let origin = content.min;
            let painter = ui.painter();

            // Start one row early: in a spread two pages share a top.
            let first = self
                .tops
                .partition_point(|&top| top < viewport.min.y)
                .saturating_sub(2);
            for page in first..n {
                let top = self.tops[page];
                if top > viewport.max.y {
                    break;
                }
                let (w, h) = doc.page_sizes[page];
                let (w, h) = (w * scale, h * scale);
                if top + h < viewport.min.y {
                    continue;
                }
                let x = size.x * 0.5 + self.lefts[page];
                let rect = Rect::from_min_size(origin + Vec2::new(x, top), Vec2::new(w, h));
                visible.push(page);
                let response = ui.interact(rect, ui.scope_id().with(("page", page)), Sense::click_and_drag());
                page_hits.push((page, rect, response));

                let image = self.textures.get(&page).map(|t| t.image.clone());
                self.layer.pages.push((rect, image));
                painter.rect_stroke(
                    rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, theme::tokens(ui.visuals().dark_mode).border),
                    StrokeKind::Outside,
                );

                // Search hits on this page.
                for (i, (hit_page, q)) in self.search.hits.iter().enumerate() {
                    if *hit_page != page {
                        continue;
                    }
                    let xs = [q[0].0, q[1].0, q[2].0, q[3].0];
                    let ys = [q[0].1, q[1].1, q[2].1, q[3].1];
                    let min = Vec2::new(xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min));
                    let max = Vec2::new(xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max));
                    let r = Rect::from_min_max(rect.min + min * scale, rect.min + max * scale);
                    // The current match is the screen's one accent (signal-500); the others
                    // are a quiet steel tint.
                    let color = if i == self.search.active {
                        let c = theme::LIGHT.signal_500;
                        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 120)
                    } else {
                        let c = theme::LIGHT.steel_500;
                        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 60)
                    };
                    painter.rect_filled(r, CornerRadius::ZERO, color);
                }
            }

            // The current page is the one under a line 30% down the view.
            let probe = viewport.min.y + viewport.height() * 0.3;
            let mut current = self.tops.partition_point(|&top| top <= probe).saturating_sub(1);
            // In a spread, the current page is the left one of its row.
            while current > 0 && self.tops[current - 1] == self.tops[current] {
                current -= 1;
            }
            (current, viewport.size())
        });
        let (current, viewport_size) = out.inner;
        self.current_page = current.min(n.saturating_sub(1));
        self.viewport_size = viewport_size;
        self.scroll_offset = out.state.clamped_offset();

        // Comments: mouse handling and overlays. Ask for the comment list once the first
        // page is on screen (so it never delays opening a document).
        let ctx = ui.ctx().clone();
        if self.sign.placing.is_some() {
            // Placing a signature takes over the mouse from text selection.
            self.placement_interactions(&ctx, &page_hits, scale);
            self.placement_overlay(ui, &page_hits, scale);
        } else {
            self.page_interactions(&ctx, &page_hits, scale);
            self.link_interactions(&ctx, &page_hits, scale);
            self.comment_overlays(ui, &page_hits, scale);
            self.forms_overlay(ui, &page_hits, scale);
        }
        if !self.comments.requested && !self.textures.is_empty() {
            self.comments.requested = true;
            self.worker.send(Request::ListAnnotations);
        }
        if !self.forms.requested && !self.textures.is_empty() {
            self.forms.requested = true;
            self.worker.send(Request::ListFields);
        }
        if !self.digital.requested && !self.textures.is_empty() {
            self.digital.requested = true;
            self.worker.send(Request::ListSignatures);
        }
        let Some(doc) = &self.doc else { return };

        // Ask the worker for what is visible (plus a little read-ahead).
        let zoom_settled = self.zoom_changed_at.is_none_or(|t| t.elapsed() >= ZOOM_SETTLE);
        if !zoom_settled {
            ui.ctx().request_repaint_after(ZOOM_SETTLE);
        }
        // Pages to have ready: what is on screen plus the next page. The same set is what the
        // cache refuses to evict, so a page is never dropped and then asked for again.
        let mut pages = visible.clone();
        if let Some(&last) = visible.last() {
            if last + 1 < n {
                pages.push(last + 1);
            }
        }
        self.visible_pages.clone_from(&pages);
        let mut want = Vec::new();
        for page in pages {
            let (w, h) = doc.page_sizes[page];
            let s = (scale * ppp).min((MAX_PAGE_PIXELS / (w * h)).sqrt());
            let key = RenderKey {
                page,
                scale_milli: (s * 1000.0).round().max(1.0) as u32,
                generation,
                thumb: false,
            };
            let have = self.textures.get(&page);
            if have.is_some_and(|t| t.key == key) || self.failed.contains(&key) {
                continue;
            }
            // While zooming, keep showing the stretched old image instead of re-rendering every step.
            if have.is_some() && !zoom_settled {
                continue;
            }
            want.push(key);
        }
        // Thumbnails for the Pages panel come after what the page view needs.
        want.append(&mut self.pages.want);
        if want != self.last_want {
            self.worker.send(Request::Want(want.clone()));
            self.last_want = want;
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            if self.session.password.is_some() {
                return; // the password window is in front
            }
            if self.opening {
                ui.spinner();
                ui.label("Opening…");
                return;
            }
            ui.heading("Rogeha Reader");
            ui.label("Open a PDF, EPUB, CBZ, XPS, FB2 or MOBI file, or drop one here.");
            ui.add_space(8.0);
            if ui.button("Open a file").clicked() {
                self.open_dialog();
            }
            if !self.session.recent.is_empty() {
                ui.add_space(20.0);
                ui.label(egui::RichText::new("Recent files").strong());
                ui.add_space(4.0);
                if let Some(path) = self.recent_list(ui, 8) {
                    self.request_open(path);
                }
            }
        });
    }

    /// Developer automation: run scripted actions one at a time, then take a screenshot.
    fn run_dev(&mut self, ctx: &egui::Context) {
        if self.dev.screenshot.is_none() && self.dev_steps.is_empty() {
            return;
        }
        ctx.request_repaint();
        // A protected file waits for its password before anything else can run.
        if self.session.password.is_some() {
            if let Some(password) = self.dev_steps.last().and_then(|s| s.strip_prefix("password=")).map(str::to_owned) {
                self.dev_steps.pop();
                self.submit_password(password);
                return;
            }
        }
        let idle = (self.doc.is_some() || self.session.password.is_some() || self.dev_steps.last().is_some_and(|s| s == "welcome"))
            && !self.print_busy
            && !self.opening
            && self.last_want.is_empty()
            && !self.search.busy
            && !self.compress_busy()
            && !self.ocr_busy()
            && !self.digital_busy()
            && !self.assistant_busy();
        if !idle {
            self.dev_wait = 0;
            return;
        }
        self.dev_wait += 1;
        if self.dev_wait < 20 {
            return; // let the UI settle between steps
        }
        self.dev_wait = 0;
        match self.dev_steps.pop() {
            Some(step) => {
                let (name, arg) = step.split_once('=').unwrap_or((&step, ""));
                match name {
                    "page" => self.go_to_page(arg.parse::<usize>().unwrap_or(1).saturating_sub(1)),
                    "rotate" => self.rotate(vec![self.current_page], 90),
                    "zoom" => self.set_zoom(arg.parse().unwrap_or(1.0), Fit::Custom),
                    "fitpage" => self.fit = Fit::Page,
                    "find" => {
                        self.search.open = true;
                        self.search.query = arg.to_owned();
                        self.run_search();
                    }
                    "ai" => self.assistant.open = true,
                    "ask" => {
                        self.assistant.open = true;
                        self.assistant.queued = Some(arg.to_owned());
                    }
                    "save" => self.save(),
                    // select=x0,y0,x1,y1 in page points on the current page
                    "select" => {
                        let v: Vec<f32> = arg.split(',').filter_map(|n| n.parse().ok()).collect();
                        if let [x0, y0, x1, y1] = v[..] {
                            self.comments.selection_seq += 1;
                            self.worker.send(Request::SelectText {
                                page: self.current_page,
                                a: (x0, y0),
                                b: (x1, y1),
                                seq: self.comments.selection_seq,
                            });
                        }
                    }
                    "highlight" => self.mark_selection(rogeha_core::MarkupKind::Highlight),
                    "underline" => self.mark_selection(rogeha_core::MarkupKind::Underline),
                    "note" => self.worker.send(Request::AddNote {
                        page: self.current_page,
                        at: (120.0, 160.0),
                        contents: arg.to_owned(),
                        author: self.comments.author.clone(),
                        color: self.my_comment_color(),
                    }),
                    "comments" => self.comments.panel_open = true,
                    "pages" => self.pages.open = true,
                    "welcome" => {}
                    "undo" => self.worker.send(Request::Undo),
                    "redo" => self.worker.send(Request::Redo),
                    "translatepage" => self.translate(false),
                    "translatesel" => self.translate(true),
                    "tools" => {}
                    "darkpages" => self.set_dark_pages(true),
                    "lightpages" => self.set_dark_pages(false),
                    "outline" => self.nav.outline_open = true,
                    // tab=path opens another file in a new tab; tab=2 switches to the second tab
                    "tab" => match arg.parse::<usize>() {
                        Ok(n) => self.switch_tab(n.saturating_sub(1)),
                        Err(_) => {
                            self.request_open(PathBuf::from(arg));
                        }
                    },
                    "closetab" => self.close_tab(self.session.active),
                    // link=N follows the N-th link on the current page
                    "link" => {
                        let target = self.nav.links.get(&self.current_page).and_then(|l| l.get(arg.parse::<usize>().unwrap_or(1).saturating_sub(1))).map(|l| l.target.clone());
                        match target {
                            Some(rogeha_core::LinkTarget::Page(page)) => self.jump_to(page),
                            other => self.info(format!("link: {other:?}")),
                        }
                    }
                    // firstlink jumps through the first link on this page that leads to another page
                    "firstlink" => {
                        let here = self.current_page;
                        let page = self.nav.links.get(&here).and_then(|l| l.iter().find_map(|l| match l.target {
                            rogeha_core::LinkTarget::Page(p) if p != here => Some(p),
                            _ => None,
                        }));
                        match page {
                            Some(page) => self.jump_to(page),
                            None => self.error("no internal link on this page"),
                        }
                    }
                    "back" => self.go_back(),
                    "print" => self.dev_print(arg),
                    // view=single | spread | cover
                    "view" => self.set_view(match arg {
                        "spread" => ViewMode::Spread,
                        "cover" => ViewMode::SpreadCover,
                        _ => ViewMode::Single,
                    }),
                    "fullscreen" => self.set_fullscreen(ctx, true),
                    // reorder=2,5>1: move pages 2 and 5 in front of page 1
                    "reorder" => self.dev_reorder(arg),
                    // author=<name>: comment as this person
                    "author" => self.comments.author = arg.to_owned(),
                    "about" => self.about.open = true,
                    "sign" => self.sign.open = true,
                    // signat=<x>,<y>,<draw | a name>: start placing at that point on the current page
                    "signat" => {
                        let parts: Vec<&str> = arg.splitn(3, ',').collect();
                        if let [x, y, kind] = parts[..] {
                            self.dev_sign(kind, x.parse().ok().zip(y.parse().ok()));
                        }
                    }
                    "signapply" => self.apply_signature(),
                    "signatures" => self.digital.window_open = true,
                    // fill=<part of field name>|<value>   editfield=<same> opens the editor instead
                    "fill" => self.dev_fill(arg, false),
                    "editfield" => self.dev_fill(arg, true),
                    "certtab" => self.dev_certificate_tab(),
                    "dsign" => self.dev_sign_digitally(arg),
                    "ocrdownload" => {
                        self.ocr.open = true;
                        self.ocr_download_missing(ctx);
                    }
                    // ocr=<output path>, or just "ocr" to show the dialog
                    "ocr" => {
                        self.ocr.open = true;
                        if !arg.is_empty() {
                            self.start_ocr(ctx, Some(PathBuf::from(arg)));
                        }
                    }
                    "light" => ctx.set_theme(egui::Theme::Light),
                    "dark" => ctx.set_theme(egui::Theme::Dark),
                    // compress=<output path>
                    "compress" => {
                        self.compress.open = true;
                        self.start_compress(ctx, Some(PathBuf::from(arg)));
                    }
                    "opencomment" => {
                        if let Some(a) = self.comments.list.last() {
                            self.open_comment((a.page, a.xref), None);
                        }
                    }
                    _ => {}
                }
            }
            None => {
                if let Some(path) = self.dev.screenshot.take() {
                    ctx.request_screenshot(move |image| {
                        let mut out =
                            format!("P6
{} {}
255
", image.width(), image.height()).into_bytes();
                        out.extend(image.pixels.iter().flat_map(|p| [p.r(), p.g(), p.b()]));
                        let _ = std::fs::write(&path, out);
                        std::process::exit(0);
                    });
                }
            }
        }
    }

    fn finish_bench(&mut self, ctx: &egui::Context) {
        if !self.bench {
            return;
        }
        self.bench_frames += 1;
        let first_ms = match self.first_page_ms {
            Some(ms) if self.last_want.is_empty() => ms, // read-ahead finished too
            // No file given: report the empty-window baseline after a few frames.
            None if self.doc.is_none() && !self.opening && self.bench_frames > 5 => 0.0,
            _ => {
                ctx.request_repaint();
                return;
            }
        };
        self.bench = false;
        let (working_set, private) = memory_usage();
        println!(
            "first_page_ms={first_ms:.1} settled_ms={:.1} pages={} working_set_mb={:.1} private_mb={:.1} textures_mb={:.1}",
            self.started.elapsed().as_secs_f64() * 1000.0,
            self.page_count(),
            working_set as f64 / 1048576.0,
            private as f64 / 1048576.0,
            self.textures.values().map(|t| t.image.bytes()).sum::<usize>() as f64 / 1048576.0,
        );
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}

impl ReaderApp {
    /// The document area to draw under the UI, as recorded by the last [`Self::ui`] call.
    pub fn page_layer(&self) -> &PageLayer {
        &self.layer
    }

    /// One frame of UI. Called by the runner whenever a repaint is needed.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.handle_input(&ctx);

        // Full screen: only the document. F11 or Esc brings everything back.
        let chrome = !self.fullscreen;
        if chrome {
            egui::Panel::top("toolbar").show(ui, |ui| {
                ui.add_space(3.0);
                self.toolbar(ui);
                ui.add_space(3.0);
            });
            if self.session.tabs.len() > 1 {
                egui::Panel::top("tabs").show(ui, |ui| self.tab_strip(ui));
            }
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        } else if self.fullscreen_since.is_some_and(|t| t.elapsed() < Duration::from_secs(4)) {
            egui::Area::new(egui::Id::unique("fullscreen-hint"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 16.0))
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label("Full screen. Press Esc or F11 to leave; arrow keys turn pages.");
                    });
                });
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        self.pages.want.clear();
        if chrome && self.pages.open && self.doc.is_some() {
            egui::Panel::left("pages")
                .default_size(190.0)
                .show(ui, |ui| self.pages_panel(ui));
        }
        if chrome && self.nav.outline_open && self.doc.is_some() {
            egui::Panel::left("outline")
                .default_size(260.0)
                .show(ui, |ui| self.outline_panel(ui));
        }
        if chrome && self.comments.panel_open && self.doc.is_some() {
            egui::Panel::left("comments")
                .default_size(280.0)
                .show(ui, |ui| self.comments_panel(ui));
        }
        if chrome && self.assistant.open {
            egui::Panel::right("assistant")
                .default_size(360.0)
                .show(ui, |ui| self.assistant_panel(ui));
        }
        self.layer.pages.clear();
        // The document sits on `canvas`, the design system's page ground.
        let tokens = theme::tokens(ui.visuals().dark_mode);
        self.layer.background = tokens.canvas;
        // No fill here: the runner paints the background and the pages under the UI.
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                self.layer.clip = ui.max_rect();
                if self.doc.is_some() {
                    self.document_view(ui);
                } else {
                    self.welcome(ui);
                }
            });

        self.password_window(&ctx);
        self.comment_editor(&ctx);
        self.compress_window(&ctx);
        self.about_window(&ctx);
        self.ocr_window(&ctx);
        self.sign_window(&ctx);
        self.signatures_window(&ctx);
        self.run_dev(&ctx);
        self.finish_bench(&ctx);
    }
}

fn ask_save_changes() -> rfd::MessageDialogResult {
    rfd::MessageDialog::new()
        .set_title("Unsaved changes")
        .set_description("This document has unsaved changes. Save them?")
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show()
}

/// (working set, private bytes) of this process. Used by `--bench`.
#[cfg(windows)]
fn memory_usage() -> (usize, usize) {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters::default();
    c.cb = std::mem::size_of::<Counters>() as u32;
    // SAFETY: `c` is a correctly sized PROCESS_MEMORY_COUNTERS_EX for the current process.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (c.working_set_size, c.private_usage)
}

#[cfg(not(windows))]
fn memory_usage() -> (usize, usize) {
    (0, 0)
}

/// Folder for things the app remembers on this computer (OCR language data, signatures).
fn app_data_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    // `ROGEHA_DATA_DIR` moves it (used by the scripted checks).
    std::env::var_os("ROGEHA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| base.unwrap_or_else(std::env::temp_dir).join("Rogeha Reader"))
}

#[cfg(test)]
mod tests {
    use super::{ViewMode, layout_rows};

    #[test]
    fn spreads_pair_pages_the_way_a_magazine_does() {
        let s = Some;
        assert_eq!(layout_rows(3, ViewMode::Single), vec![[s(0), None], [s(1), None], [s(2), None]]);
        // Starting with a left-hand page: 1-2, 3-4, and a last page alone on the left.
        assert_eq!(layout_rows(5, ViewMode::Spread), vec![[s(0), s(1)], [s(2), s(3)], [s(4), None]]);
        // Cover alone on the right, then 2-3, 4-5, 6 alone on the left.
        assert_eq!(layout_rows(6, ViewMode::SpreadCover), vec![[None, s(0)], [s(1), s(2)], [s(3), s(4)], [s(5), None]]);
        assert_eq!(layout_rows(1, ViewMode::SpreadCover), vec![[None, s(0)]]);
        assert!(layout_rows(0, ViewMode::SpreadCover).is_empty());
    }
}
