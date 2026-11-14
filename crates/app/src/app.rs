use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CornerRadius, Key, KeyboardShortcut, Layout, Modifiers, Pos2, Rect,
    Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2, ViewportCommand,
};
use rogeha_core::{DocInfo, Event, HitQuad, RenderKey, Request, Worker};

use crate::agent_host::AgentHost;

/// PDF points (1/72") to screen pixels at 100% zoom (96 dpi).
const PT_TO_PX: f32 = 96.0 / 72.0;
const PAGE_GAP: f32 = 10.0;
const MARGIN: f32 = 12.0;
/// Upper bound for page textures kept in memory.
const TEXTURE_BUDGET: usize = 64 << 20;
const MAX_TEXTURE_SIDE: f32 = 8192.0;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.0;
const ZOOM_SETTLE: Duration = Duration::from_millis(70);

struct PageTex {
    key: RenderKey,
    tex: TextureHandle,
    bytes: usize,
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

#[derive(Default)]
struct AiPanel {
    open: bool,
    host: Option<AgentHost>,
    log: Vec<(bool, String)>, // (from user, text)
    input: String,
    status: String,
}

enum AfterSave {
    Nothing,
    CloseApp,
    Open(PathBuf),
}

pub struct ReaderApp {
    worker: Worker,
    doc: Option<DocInfo>,
    opening: bool,

    zoom: f32,
    fit: Fit,
    zoom_changed_at: Option<Instant>,

    textures: HashMap<usize, PageTex>,
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
    ai: AiPanel,
    message: Option<(String, Instant, bool)>,
    after_save: AfterSave,
    allow_close: bool,

    started: Instant,
    bench: bool,
    first_page_ms: Option<f64>,
}

impl ReaderApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        file: Option<PathBuf>,
        started: Instant,
        bench: bool,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        // Ctrl +/- zooms the document, not the whole UI.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let wake_ctx = ctx.clone();
        let worker = Worker::spawn(move || wake_ctx.request_repaint());

        let mut app = Self {
            worker,
            doc: None,
            opening: false,
            zoom: 1.0,
            fit: Fit::Width,
            zoom_changed_at: None,
            textures: HashMap::new(),
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
            ai: AiPanel::default(),
            message: None,
            after_save: AfterSave::Nothing,
            allow_close: false,
            started,
            bench,
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
                    self.textures.clear();
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
                }
                Event::Rendered { key, image } => {
                    let current = self.doc.as_ref().is_some_and(|d| d.generation == key.generation);
                    if !current {
                        continue;
                    }
                    let bytes = image.rgba.len();
                    let color = egui::ColorImage::from_rgba_premultiplied(
                        [image.width, image.height],
                        &image.rgba,
                    );
                    let tex = ctx.load_texture(
                        format!("page-{}", key.page),
                        color,
                        TextureOptions::LINEAR,
                    );
                    self.textures.insert(key.page, PageTex { key, tex, bytes });
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
                }
                Event::PageText { .. } => {}
                Event::Closed => {
                    self.doc = None;
                    self.textures.clear();
                    self.update_title(ctx);
                }
                Event::Error(message) => {
                    self.opening = false;
                    self.search.busy = false;
                    self.after_save = AfterSave::Nothing;
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
        let mut total: usize = self.textures.values().map(|t| t.bytes).sum();
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
            if page.abs_diff(here) <= 1 || wanted.contains(&page) {
                continue;
            }
            if let Some(t) = self.textures.remove(&page) {
                total -= t.bytes;
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
                    if ui.add_enabled(n > 1, egui::Button::new("Delete this page")).clicked() {
                        self.delete_current();
                        ui.close();
                    }
                })
                .response
                .on_disabled_hover_text(edit_hint);
            });

            ui.separator();

            if ui.add_enabled(has_doc, egui::Button::new("Find")).on_hover_text("Search the document (Ctrl+F)").clicked() {
                self.search.open = !self.search.open;
                self.search.focus = self.search.open;
            }
            if ui
                .add(egui::Button::new("AI").selected(self.ai.open))
                .on_hover_text("Open the AI assistant (loaded only when opened)")
                .clicked()
            {
                self.ai.open = !self.ai.open;
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
                let kind = if doc.is_pdf { "PDF" } else { "Reflowable / other" };
                ui.label(format!("Page {} of {}", self.current_page + 1, doc.page_sizes.len()));
                ui.separator();
                ui.label(kind);
                if doc.dirty {
                    ui.separator();
                    ui.label("Unsaved changes");
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

    fn ai_panel(&mut self, ui: &mut egui::Ui) {
        // First time the panel is shown: start the agent process.
        if self.ai.host.is_none() && self.ai.status.is_empty() {
            let ctx = ui.ctx().clone();
            match AgentHost::start(move || ctx.request_repaint()) {
                Ok(mut host) => {
                    let _ = host.send_line(r#"{"type":"hello"}"#);
                    self.ai.host = Some(host);
                    self.ai.status = "Starting…".into();
                }
                Err(e) => self.ai.status = e,
            }
        }
        if let Some(host) = &self.ai.host {
            while let Ok(line) = host.lines.try_recv() {
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(v) => {
                        let text = v["text"].as_str().unwrap_or_default().to_owned();
                        match v["type"].as_str() {
                            Some("ready") => self.ai.status = text,
                            Some("message") => self.ai.log.push((false, text)),
                            Some("error") => self.ai.log.push((false, format!("Error: {text}"))),
                            _ => {}
                        }
                    }
                    Err(_) => self.ai.log.push((false, line)),
                }
            }
        }

        ui.heading("AI assistant");
        ui.label(egui::RichText::new(&self.ai.status).weak());
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height((ui.available_height() - 40.0).max(60.0))
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (from_user, text) in &self.ai.log {
                    let who = if *from_user { "You" } else { "Assistant" };
                    ui.label(egui::RichText::new(who).strong());
                    ui.label(text);
                    ui.add_space(6.0);
                }
            });
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.ai.input)
                    .desired_width(ui.available_width() - 60.0)
                    .hint_text("Ask about this document"),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if (ui.button("Send").clicked() || enter) && !self.ai.input.trim().is_empty() {
                let text = std::mem::take(&mut self.ai.input);
                let msg = serde_json::json!({
                    "type": "ask",
                    "text": text,
                    "page": self.current_page + 1,
                    "document": self.doc.as_ref().map(|d| d.path.to_string_lossy().into_owned()),
                });
                match &mut self.ai.host {
                    Some(host) => {
                        if let Err(e) = host.send_line(&msg.to_string()) {
                            self.ai.log.push((false, format!("Error: {e}")));
                        }
                    }
                    None => self.ai.log.push((false, "The AI agent is not running.".into())),
                }
                self.ai.log.push((true, text));
                resp.request_focus();
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

        let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
        if let Some(offset) = self.pending_scroll.take() {
            area = area.scroll_offset(offset);
        }

        let mut visible: Vec<usize> = Vec::new();
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

                painter.rect_filled(rect.translate(Vec2::splat(2.0)), CornerRadius::ZERO, Color32::from_black_alpha(40));
                painter.rect_filled(rect, CornerRadius::ZERO, Color32::WHITE);
                if let Some(t) = self.textures.get(&page) {
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                    painter.image(t.tex.id(), rect, uv, Color32::WHITE);
                }
                painter.rect_stroke(
                    rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, Color32::from_gray(150)),
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
                    let color = if i == self.search.active {
                        Color32::from_rgba_unmultiplied(255, 140, 0, 110)
                    } else {
                        Color32::from_rgba_unmultiplied(255, 230, 0, 90)
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

        // Ask the worker for what is visible (plus a little read-ahead).
        let zoom_settled = self.zoom_changed_at.is_none_or(|t| t.elapsed() >= ZOOM_SETTLE);
        if !zoom_settled {
            ui.ctx().request_repaint_after(ZOOM_SETTLE);
        }
        let mut pages = visible.clone();
        if let (Some(&first), Some(&last)) = (visible.first(), visible.last()) {
            pages.extend([last + 1, last + 2].into_iter().filter(|&p| p < n));
            if first > 0 {
                pages.push(first - 1);
            }
        }
        let mut want = Vec::new();
        for page in pages {
            let (w, h) = doc.page_sizes[page];
            let s = (scale * ppp).min(MAX_TEXTURE_SIDE / w.max(h));
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

    fn finish_bench(&mut self, ctx: &egui::Context) {
        if !self.bench {
            return;
        }
        let Some(first_ms) = self.first_page_ms else { return };
        if !self.last_want.is_empty() {
            return; // read-ahead still rendering
        }
        self.bench = false;
        let (working_set, private) = memory_usage();
        println!(
            "first_page_ms={first_ms:.1} settled_ms={:.1} pages={} working_set_mb={:.1} private_mb={:.1} textures_mb={:.1}",
            self.started.elapsed().as_secs_f64() * 1000.0,
            self.page_count(),
            working_set as f64 / 1048576.0,
            private as f64 / 1048576.0,
            self.textures.values().map(|t| t.bytes).sum::<usize>() as f64 / 1048576.0,
        );
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}

impl eframe::App for ReaderApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.handle_input(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui);
            ui.add_space(3.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.ai.open {
            egui::Panel::right("ai")
                .default_size(320.0)
                .show(ui, |ui| self.ai_panel(ui));
        }
        let bg = if ui.visuals().dark_mode {
            Color32::from_gray(48)
        } else {
            Color32::from_gray(200)
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg))
            .show(ui, |ui| {
                if self.doc.is_some() {
                    self.document_view(ui);
                } else {
                    self.welcome(ui);
                }
            });

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
