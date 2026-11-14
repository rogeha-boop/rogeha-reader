use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{
    self, Align, Color32, CornerRadius, Key, KeyboardShortcut, Layout, Modifiers, Rect,
    Sense, Stroke, StrokeKind, Vec2, ViewportCommand,
};
use std::sync::Arc;

use rogeha_core::{DocInfo, Event, HitQuad, PageImage, RenderKey, Request, Worker};

use crate::theme;

mod about;
mod assistant;
mod comments;
mod compress;
mod dsign;
mod forms;
mod ocr;
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
    layout_for: (u64, u32),
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
            layout_for: (u64::MAX, 0),
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
        self.opening = true;
        self.worker.send(Request::Open(path));
    }

    /// Open a file, asking first if there are unsaved changes.
    fn request_open(&mut self, path: PathBuf) {
        if self.is_dirty() {
            match ask_save_changes() {
                rfd::MessageDialogResult::Yes => {
                    self.after_save = AfterSave::Open(path);
                    self.worker.send(Request::Save { target: None });
                    return;
                }
                rfd::MessageDialogResult::No => {}
                _ => return,
            }
        }
        self.open_path(path);
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

    fn delete_current(&mut self) {
        if !self.can_edit() || self.page_count() < 2 {
            return;
        }
        let page = self.current_page;
        let ok = rfd::MessageDialog::new()
            .set_title("Delete page")
            .set_description(format!(
                "Delete page {} from this document?\nThe file changes only when you save.",
                page + 1
            ))
            .set_buttons(rfd::MessageButtons::OkCancel)
            .show();
        if matches!(ok, rfd::MessageDialogResult::Ok) {
            self.pending_page = Some(page.min(self.page_count().saturating_sub(2)));
            self.worker.send(Request::Delete { pages: vec![page] });
        }
    }

    fn move_current(&mut self, later: bool) {
        if !self.can_edit() {
            return;
        }
        let from = self.current_page;
        let to = if later {
            if from + 1 >= self.page_count() {
                return;
            }
            from + 1
        } else {
            if from == 0 {
                return;
            }
            from - 1
        };
        self.pending_page = Some(to);
        self.worker.send(Request::Move { from, to });
    }

    /// Merge: pick one or more PDFs and add their pages at the end, or after the current page.
    fn add_pdfs(&mut self, after_current: bool) {
        if !self.can_edit() {
            return;
        }
        let picked = rfd::FileDialog::new()
            .set_title("Choose PDFs to add (they are added in the order shown)")
            .add_filter("PDF", &["pdf"])
            .pick_files();
        let Some(mut paths) = picked else { return };
        if paths.is_empty() {
            return;
        }
        paths.sort();
        let before = after_current.then_some(self.current_page + 1);
        self.pending_page = Some(before.unwrap_or(self.page_count()));
        self.worker.send(Request::InsertPdfs { paths, before });
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
                    self.opening = false;
                    // After an edit that keeps the page layout (a comment, a save), keep showing
                    // the old page images until the new ones arrive, so nothing flashes white.
                    let same_layout = same_file
                        && self.doc.as_ref().is_some_and(|d| d.page_sizes == info.page_sizes);
                    if !same_layout {
                        self.textures.clear();
                    }
                    if !same_file {
                        self.comments.reset_for_new_document();
                        self.digital.reset_for_new_document();
                        self.forms.reset_for_new_document();
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
                    self.update_title(ctx);
                    self.assistant_edit_outcome(Ok(()));
                }
                Event::Rendered { key, image } => {
                    let current = self.doc.as_ref().is_some_and(|d| d.generation == key.generation);
                    if !current {
                        continue;
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
                Event::Closed => {
                    self.doc = None;
                    self.textures.clear();
                    self.update_title(ctx);
                }
                Event::Error(message) => {
                    self.opening = false;
                    self.digital.busy = false;
                    self.search.busy = false;
                    self.after_save = AfterSave::Nothing;
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
        let wanted: HashSet<usize> = self.last_want.iter().map(|k| k.page).collect();
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
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.request_open(path);
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
        if hit(ctrl, Key::O) {
            self.open_dialog();
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

        if self.search.open && hit(Modifiers::NONE, Key::Escape) {
            self.search.open = false;
        }
        if !ctx.egui_wants_keyboard_input() {
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
        if hit(Modifiers::NONE, Key::ArrowRight) {
            self.go_to_page(self.current_page + 1);
        }
        if hit(Modifiers::NONE, Key::ArrowLeft) {
            self.go_to_page(self.current_page.saturating_sub(1));
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
        let n = self.page_count();
        let edit_hint = if has_doc && !can_edit {
            "Page editing works on PDF files"
        } else {
            ""
        };

        ui.horizontal_wrapped(|ui| {
            if ui.button("Open").on_hover_text("Open a file (Ctrl+O)").clicked() {
                self.open_dialog();
            }
            if ui
                .add_enabled(self.is_dirty(), egui::Button::new("Save"))
                .on_hover_text("Save changes into the file (Ctrl+S)")
                .clicked()
            {
                self.save();
            }
            if ui
                .add_enabled(can_edit, egui::Button::new("Save as"))
                .on_hover_text("Save a copy (Ctrl+Shift+S)")
                .clicked()
            {
                self.save_as();
            }

            ui.separator();

            if ui.add_enabled(has_doc, egui::Button::new("<")).on_hover_text("Previous page").clicked() {
                self.go_to_page(self.current_page.saturating_sub(1));
            }
            let mut page_no = self.current_page + 1;
            let resp = ui.add_enabled(
                has_doc,
                egui::DragValue::new(&mut page_no).range(1..=n.max(1)).speed(0.1),
            );
            if resp.changed() && page_no != self.current_page + 1 {
                self.go_to_page(page_no - 1);
            }
            ui.label(format!("/ {n}"));
            if ui.add_enabled(has_doc, egui::Button::new(">")).on_hover_text("Next page").clicked() {
                self.go_to_page(self.current_page + 1);
            }

            ui.separator();

            if ui.add_enabled(has_doc, egui::Button::new("-")).on_hover_text("Zoom out (Ctrl -)").clicked() {
                self.zoom_step(false);
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
            if ui.add_enabled(has_doc, egui::Button::new("+")).on_hover_text("Zoom in (Ctrl +)").clicked() {
                self.zoom_step(true);
            }
            if ui
                .add_enabled(has_doc, egui::Button::new("Fit width").selected(self.fit == Fit::Width))
                .on_hover_text("Fit the page width (Ctrl+0)")
                .clicked()
            {
                self.fit = Fit::Width;
            }
            if ui
                .add_enabled(has_doc, egui::Button::new("Fit page").selected(self.fit == Fit::Page))
                .clicked()
            {
                self.fit = Fit::Page;
            }

            ui.separator();

            let page = self.current_page;
            if ui
                .add_enabled(can_edit, egui::Button::new("Rotate left"))
                .on_hover_text("Rotate this page 90° left (Shift+R)")
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.rotate(vec![page], -90);
            }
            if ui
                .add_enabled(can_edit, egui::Button::new("Rotate right"))
                .on_hover_text("Rotate this page 90° right (R)")
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.rotate(vec![page], 90);
            }
            ui.add_enabled_ui(can_edit, |ui| {
                ui.menu_button("Pages", |ui| {
                    if ui.button("Rotate all pages left").clicked() {
                        self.rotate((0..n).collect(), -90);
                        ui.close();
                    }
                    if ui.button("Rotate all pages right").clicked() {
                        self.rotate((0..n).collect(), 90);
                        ui.close();
                    }
                    if ui.button("Turn this page upside down").clicked() {
                        self.rotate(vec![page], 180);
                        ui.close();
                    }
                    ui.separator();
                    if ui.add_enabled(page > 0, egui::Button::new("Move page earlier")).clicked() {
                        self.move_current(false);
                        ui.close();
                    }
                    if ui.add_enabled(page + 1 < n, egui::Button::new("Move page later")).clicked() {
                        self.move_current(true);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Add PDFs at the end…").on_hover_text("Combine: append the pages of other PDF files").clicked() {
                        ui.close();
                        self.add_pdfs(false);
                    }
                    if ui.button("Insert PDFs after this page…").clicked() {
                        ui.close();
                        self.add_pdfs(true);
                    }
                    ui.separator();
                    if ui.button("Recognize text (OCR)…").on_hover_text("Make a scanned PDF searchable").clicked() {
                        ui.close();
                        self.ocr.open = true;
                    }
                    if ui.button("Compress PDF…").on_hover_text("Make a smaller copy of a scanned PDF").clicked() {
                        ui.close();
                        self.compress.open = true;
                    }
                    ui.separator();
                    if ui.add_enabled(n > 1, egui::Button::new("Delete this page")).clicked() {
                        self.delete_current();
                        ui.close();
                    }
                })
                .response
                .on_disabled_hover_text(edit_hint);
            });

            ui.separator();

            if ui
                .add_enabled(can_edit, egui::Button::new("Add note").selected(self.comments.note_tool))
                .on_hover_text("Click, then click on the page where the note should go")
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.comments.note_tool = !self.comments.note_tool;
            }
            if ui
                .add_enabled(can_edit, egui::Button::new("Sign").selected(self.sign.open || self.sign.placing.is_some()))
                .on_hover_text("Add your signature to a page")
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.sign.placing = None;
                self.sign.open = !self.sign.open;
            }
            let label = match self.comments.list.len() {
                0 => "Comments".to_owned(),
                n => format!("Comments ({n})"),
            };
            if ui
                .add_enabled(has_doc, egui::Button::new(label).selected(self.comments.panel_open))
                .on_hover_text("Show the list of comments")
                .clicked()
            {
                self.comments.panel_open = !self.comments.panel_open;
            }

            ui.separator();

            if ui.add_enabled(has_doc, egui::Button::new("Find")).on_hover_text("Search the document (Ctrl+F)").clicked() {
                self.search.open = !self.search.open;
                self.search.focus = self.search.open;
            }
            if ui
                .add(egui::Button::new("AI").selected(self.assistant.open))
                .on_hover_text("Open the AI assistant (loaded only when opened)")
                .clicked()
            {
                self.assistant.open = !self.assistant.open;
            }
            if ui.button("About").on_hover_text("About Rogeha Reader").clicked() {
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
        ui.horizontal(|ui| {
            if let Some(doc) = &self.doc {
                let kind = doc
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().to_uppercase())
                    .unwrap_or_else(|| "Document".into());
                ui.label(format!("Page {} of {}", self.current_page + 1, doc.page_sizes.len()));
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
    }

    fn ensure_layout(&mut self, avail: Vec2) {
        let Some(doc) = &self.doc else { return };
        if doc.page_sizes.is_empty() {
            return;
        }
        // Fit modes follow the window size.
        let max_w = doc.page_sizes.iter().map(|s| s.0).fold(1.0, f32::max);
        let scrollbar = 14.0;
        match self.fit {
            Fit::Width => {
                let z = (avail.x - 2.0 * MARGIN - scrollbar) / (max_w * PT_TO_PX);
                self.set_zoom(z, Fit::Width);
            }
            Fit::Page => {
                let (w, h) = doc.page_sizes[self.current_page.min(doc.page_sizes.len() - 1)];
                let zw = (avail.x - 2.0 * MARGIN - scrollbar) / (w * PT_TO_PX);
                let zh = (avail.y - 2.0 * PAGE_GAP) / (h * PT_TO_PX);
                self.set_zoom(zw.min(zh), Fit::Page);
            }
            Fit::Custom => {}
        }

        let doc = self.doc.as_ref().unwrap();
        let key = (doc.generation, (self.zoom * 10_000.0) as u32);
        if self.layout_for == key && self.tops.len() == doc.page_sizes.len() {
            return;
        }
        let scale = self.zoom * PT_TO_PX;
        let mut y = PAGE_GAP;
        self.tops.clear();
        for &(_, h) in &doc.page_sizes {
            self.tops.push(y);
            y += h * scale + PAGE_GAP;
        }
        self.content_size = Vec2::new(max_w * scale + 2.0 * MARGIN, y);
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

            let first = self
                .tops
                .partition_point(|&top| top < viewport.min.y)
                .saturating_sub(1);
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
                let x = ((size.x - w) * 0.5).max(MARGIN);
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
            let current = self.tops.partition_point(|&top| top <= probe).saturating_sub(1);
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
        if want != self.last_want {
            self.worker.send(Request::Want(want.clone()));
            self.last_want = want;
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
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
        });
    }

    /// Developer automation: run scripted actions one at a time, then take a screenshot.
    fn run_dev(&mut self, ctx: &egui::Context) {
        if self.dev.screenshot.is_none() && self.dev_steps.is_empty() {
            return;
        }
        ctx.request_repaint();
        let idle = self.doc.is_some()
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
                    }),
                    "comments" => self.comments.panel_open = true,
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

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui);
            ui.add_space(3.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.comments.panel_open && self.doc.is_some() {
            egui::Panel::left("comments")
                .default_size(280.0)
                .show(ui, |ui| self.comments_panel(ui));
        }
        if self.assistant.open {
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
