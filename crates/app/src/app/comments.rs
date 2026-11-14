//! Comments UI: text selection, markup (highlight / underline / strike-out), sticky notes,
//! the comment editor and the Comments side panel. The document work itself happens in
//! `rogeha_core::annotations`; everything is stored as standard PDF annotations.

use egui::{Align2, CursorIcon, Id, Order, Pos2, Rect, Response, Stroke, StrokeKind, Vec2};
use rogeha_core::{AnnotInfo, AnnotKind, HitQuad, MarkupKind, Request};

use super::ReaderApp;

pub(super) struct Selection {
    pub page: usize,
    pub quads: Vec<HitQuad>,
    pub text: String,
}

/// The comment being written or edited.
pub(super) struct Editor {
    page: usize,
    /// `None` while writing a new note that does not exist in the document yet.
    xref: Option<i32>,
    /// Where a new note goes, in page points.
    at: (f32, f32),
    title: String,
    text: String,
    original: String,
    pos: Pos2,
    focus: bool,
}

pub(super) struct Comments {
    pub panel_open: bool,
    pub author: String,
    pub list: Vec<AnnotInfo>,
    /// The comment list has been asked for (it then stays up to date by itself).
    pub requested: bool,
    /// "Add note" is armed: the next click on a page places a note.
    pub note_tool: bool,
    drag: Option<(usize, (f32, f32))>,
    last_drag_end: (f32, f32),
    pub selection: Option<Selection>,
    pub selection_seq: u64,
    pub selected: Option<(usize, i32)>,
    pub editor: Option<Editor>,
    /// Open the editor for this comment as soon as it shows up in the list.
    pub open_when_listed: Option<(usize, i32)>,
}

impl Default for Comments {
    fn default() -> Self {
        let author = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "Reader".to_owned());
        Self {
            panel_open: false,
            author,
            list: Vec::new(),
            requested: false,
            note_tool: false,
            drag: None,
            last_drag_end: (f32::NAN, f32::NAN),
            selection: None,
            selection_seq: 0,
            selected: None,
            editor: None,
            open_when_listed: None,
        }
    }
}

impl Comments {
    /// Forget everything tied to the previous document.
    pub fn reset_for_new_document(&mut self) {
        self.list.clear();
        self.requested = false;
        self.note_tool = false;
        self.drag = None;
        self.selection = None;
        self.selected = None;
        self.editor = None;
        self.open_when_listed = None;
    }
}

pub(super) fn quad_bounds(q: &HitQuad) -> (Vec2, Vec2) {
    let min = Vec2::new(
        q.iter().map(|p| p.0).fold(f32::MAX, f32::min),
        q.iter().map(|p| p.1).fold(f32::MAX, f32::min),
    );
    let max = Vec2::new(
        q.iter().map(|p| p.0).fold(f32::MIN, f32::max),
        q.iter().map(|p| p.1).fold(f32::MIN, f32::max),
    );
    (min, max)
}

/// Is the page point inside this comment's visible area?
fn hit(annot: &AnnotInfo, p: (f32, f32)) -> bool {
    let inside = |min: Vec2, max: Vec2| p.0 >= min.x && p.0 <= max.x && p.1 >= min.y && p.1 <= max.y;
    if annot.quads.is_empty() {
        let (x0, y0, x1, y1) = annot.rect;
        inside(Vec2::new(x0 - 2.0, y0 - 2.0), Vec2::new(x1.max(x0 + 16.0) + 2.0, y1.max(y0 + 16.0) + 2.0))
    } else {
        annot.quads.iter().any(|q| {
            let (min, max) = quad_bounds(q);
            inside(min, max)
        })
    }
}

fn snippet(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > max {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    } else {
        one_line
    }
}

impl ReaderApp {
    /// Mouse handling on the visible pages. `pages` holds each page's screen rectangle and
    /// its click/drag response for this frame.
    pub(super) fn page_interactions(&mut self, ctx: &egui::Context, pages: &[(usize, Rect, Response)], scale: f32) {
        let Some(doc) = &self.doc else { return };
        let is_pdf = doc.is_pdf;
        let sizes = doc.page_sizes.clone();
        for (page, rect, response) in pages {
            let page = *page;
            let (pw, ph) = sizes[page];
            let to_page = |pos: Pos2| {
                let v = (pos - rect.min) / scale;
                (v.x.clamp(0.0, pw), v.y.clamp(0.0, ph))
            };
            let over_field = !self.comments.note_tool
                && response.hover_pos().is_some_and(|pos| self.over_form_field(page, to_page(pos)));
            if response.hovered() {
                ctx.set_cursor_icon(if self.comments.note_tool {
                    CursorIcon::Crosshair
                } else if over_field {
                    CursorIcon::PointingHand
                } else {
                    CursorIcon::Text
                });
            }

            if response.drag_started() && !self.comments.note_tool && !over_field {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.comments.drag = Some((page, to_page(pos)));
                    self.comments.last_drag_end = (f32::NAN, f32::NAN);
                    self.comments.selection = None;
                    self.comments.selected = None;
                }
            }
            if response.dragged() {
                if let (Some((drag_page, a)), Some(pos)) = (self.comments.drag, response.interact_pointer_pos()) {
                    let b = to_page(pos);
                    if drag_page == page && b != self.comments.last_drag_end {
                        self.comments.last_drag_end = b;
                        self.comments.selection_seq += 1;
                        self.worker.send(Request::SelectText {
                            page,
                            a,
                            b,
                            seq: self.comments.selection_seq,
                        });
                    }
                }
            }
            if response.drag_stopped() {
                self.comments.drag = None;
            }

            if response.clicked() {
                let Some(pos) = response.interact_pointer_pos() else { continue };
                let p = to_page(pos);
                self.comments.selection = None;
                // Form fields come first: a click in one fills it in.
                if !self.comments.note_tool && self.form_click(page, p) {
                    continue;
                }
                if self.comments.note_tool {
                    self.comments.note_tool = false;
                    if is_pdf {
                        self.comments.selected = None;
                        self.comments.editor = Some(Editor {
                            page,
                            xref: None,
                            at: p,
                            title: format!("New note on page {}", page + 1),
                            text: String::new(),
                            original: String::new(),
                            pos: pos + Vec2::new(12.0, 12.0),
                            focus: true,
                        });
                    }
                    continue;
                }
                // Topmost comment under the pointer, if any.
                let target = self
                    .comments
                    .list
                    .iter()
                    .rev()
                    .find(|a| a.page == page && hit(a, p))
                    .map(|a| (a.page, a.xref));
                match target {
                    Some(key) => self.open_comment(key, Some(pos + Vec2::new(12.0, 12.0))),
                    None => {
                        self.comments.selected = None;
                        if self.comments.editor.as_ref().is_some_and(|e| e.text == e.original) {
                            self.comments.editor = None;
                        }
                    }
                }
            }
        }
    }

    /// Select a comment and open its editor.
    pub(super) fn open_comment(&mut self, key: (usize, i32), pos: Option<Pos2>) {
        let Some(annot) = self.comments.list.iter().find(|a| (a.page, a.xref) == key) else {
            return;
        };
        let who = if annot.author.is_empty() {
            String::new()
        } else {
            format!(" by {}", annot.author)
        };
        self.comments.selected = Some(key);
        self.comments.editor = Some(Editor {
            page: annot.page,
            xref: Some(annot.xref),
            at: (annot.rect.0, annot.rect.1),
            title: format!("{}{who}, page {}", annot.kind.label(), annot.page + 1),
            text: annot.contents.clone(),
            original: annot.contents.clone(),
            pos: pos.unwrap_or(self.layer.clip.min + Vec2::new(40.0, 40.0)),
            focus: true,
        });
    }

    /// Selection boxes, the outline of the selected comment and the selection action bar.
    pub(super) fn comment_overlays(&mut self, ui: &egui::Ui, pages: &[(usize, Rect, Response)], scale: f32) {
        let painter = ui.painter().with_clip_rect(self.layer.clip);
        let rect_of = |page: usize| pages.iter().find(|(p, ..)| *p == page).map(|(_, r, _)| *r);

        if let Some((page, xref)) = self.comments.selected {
            if let (Some(rect), Some(annot)) = (
                rect_of(page),
                self.comments.list.iter().find(|a| (a.page, a.xref) == (page, xref)),
            ) {
                let (x0, y0, x1, y1) = annot.rect;
                let r = Rect::from_min_max(
                    rect.min + Vec2::new(x0, y0) * scale,
                    rect.min + Vec2::new(x1, y1) * scale,
                )
                .expand(3.0);
                painter.rect_stroke(
                    r,
                    egui::CornerRadius::same(2),
                    Stroke::new(2.0, egui::Color32::from_rgb(0, 120, 215)),
                    StrokeKind::Outside,
                );
            }
        }

        let mut bar_at = None;
        if let Some(sel) = &self.comments.selection {
            if let Some(rect) = rect_of(sel.page) {
                for q in &sel.quads {
                    let (min, max) = quad_bounds(q);
                    let r = Rect::from_min_max(rect.min + min * scale, rect.min + max * scale);
                    painter.rect_filled(
                        r,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_rgba_unmultiplied(0, 120, 215, 70),
                    );
                    bar_at = Some(r.right_bottom() + Vec2::new(0.0, 6.0));
                }
            }
        }

        // Action bar under the end of the selection, once the mouse button is released.
        let (Some(pos), true) = (bar_at, self.comments.drag.is_none()) else {
            return;
        };
        let can_mark = self.can_edit();
        let mut action = None;
        let mut copy = false;
        egui::Area::new(Id::unique("selection-bar"))
            .order(Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(pos.clamp(self.layer.clip.min + Vec2::new(260.0, 0.0), self.layer.clip.max - Vec2::new(0.0, 40.0)))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let hint = "Works on PDF files";
                        if ui.add_enabled(can_mark, egui::Button::new("Highlight")).on_disabled_hover_text(hint).clicked() {
                            action = Some(MarkupKind::Highlight);
                        }
                        if ui.add_enabled(can_mark, egui::Button::new("Underline")).on_disabled_hover_text(hint).clicked() {
                            action = Some(MarkupKind::Underline);
                        }
                        if ui.add_enabled(can_mark, egui::Button::new("Strike out")).on_disabled_hover_text(hint).clicked() {
                            action = Some(MarkupKind::StrikeOut);
                        }
                        if ui.button("Copy").on_hover_text("Copy the text (Ctrl+C)").clicked() {
                            copy = true;
                        }
                    });
                });
            });
        if copy {
            self.copy_selection(ui.ctx());
        }
        if let Some(kind) = action {
            self.mark_selection(kind);
        }
    }

    pub(super) fn copy_selection(&mut self, ctx: &egui::Context) {
        if let Some(sel) = &self.comments.selection {
            if !sel.text.is_empty() {
                ctx.copy_text(sel.text.clone());
                self.info("Copied");
            }
        }
    }

    /// Turn the current text selection into a highlight, underline or strike-out.
    pub(super) fn mark_selection(&mut self, kind: MarkupKind) {
        if !self.can_edit() {
            return;
        }
        if let Some(sel) = self.comments.selection.take() {
            self.worker.send(Request::AddMarkup {
                page: sel.page,
                kind,
                quads: sel.quads,
                author: self.comments.author.clone(),
                contents: String::new(),
            });
        }
    }

    /// The small window for reading, writing and deleting one comment.
    pub(super) fn comment_editor(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.comments.editor else { return };
        let can_edit = self.doc.as_ref().is_some_and(|d| d.is_pdf);
        let (mut save, mut delete, mut close) = (false, false, false);
        let mut open = true;
        egui::Window::new(&editor.title)
            .id(Id::unique("comment-editor"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(editor.pos)
            .show(ctx, |ui| {
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut editor.text)
                        .desired_width(300.0)
                        .desired_rows(4)
                        .hint_text("Write a comment"),
                );
                if std::mem::take(&mut editor.focus) {
                    resp.request_focus();
                }
                ui.horizontal(|ui| {
                    let changed = editor.text != editor.original;
                    let can_save = can_edit && changed && (editor.xref.is_some() || !editor.text.trim().is_empty());
                    if ui.add_enabled(can_save, egui::Button::new("Save")).on_hover_text("Ctrl+Enter").clicked() {
                        save = true;
                    }
                    if can_save && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
                        save = true;
                    }
                    if editor.xref.is_some() && ui.add_enabled(can_edit, egui::Button::new("Delete")).clicked() {
                        delete = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });

        if save {
            let editor = self.comments.editor.take().unwrap();
            match editor.xref {
                Some(xref) => self.worker.send(Request::EditAnnotation {
                    page: editor.page,
                    xref,
                    contents: editor.text,
                }),
                None => self.worker.send(Request::AddNote {
                    page: editor.page,
                    at: editor.at,
                    contents: editor.text,
                    author: self.comments.author.clone(),
                }),
            }
        } else if delete {
            let editor = self.comments.editor.take().unwrap();
            if let Some(xref) = editor.xref {
                self.worker.send(Request::DeleteAnnotation { page: editor.page, xref });
            }
            self.comments.selected = None;
        } else if close || !open {
            self.comments.editor = None;
        }
    }

    /// Side panel listing every comment in the document.
    pub(super) fn comments_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.heading(format!("Comments ({})", self.comments.list.len()));
        ui.horizontal(|ui| {
            ui.label("Your name:");
            ui.add(egui::TextEdit::singleline(&mut self.comments.author).desired_width(140.0))
                .on_hover_text("Saved as the author of the comments you add");
        });
        if self.doc.as_ref().is_some_and(|d| !d.is_pdf) {
            ui.label(egui::RichText::new("Comments can be added to PDF files.").weak());
        } else {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Add note").selected(self.comments.note_tool))
                    .on_hover_text("Click, then click on the page where the note should go")
                    .clicked()
                {
                    self.comments.note_tool = !self.comments.note_tool;
                }
                if self.comments.note_tool {
                    ui.label(egui::RichText::new("Now click on the page.").weak());
                }
            });
        }
        ui.separator();

        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.comments.list.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "No comments yet.\nSelect text to highlight it, or use \"Add note\".",
                    )
                    .weak(),
                );
            }
            for annot in &self.comments.list {
                let key = (annot.page, annot.xref);
                let selected = self.comments.selected == Some(key);
                let mut head = format!("Page {} · {}", annot.page + 1, annot.kind.label());
                if !annot.author.is_empty() {
                    head.push_str(&format!(" · {}", annot.author));
                }
                let body = if annot.contents.trim().is_empty() {
                    egui::RichText::new("(no text)").weak().italics()
                } else {
                    egui::RichText::new(snippet(&annot.contents, 140))
                };
                let frame = egui::Frame::group(ui.style()).fill(if selected {
                    ui.visuals().selection.bg_fill.gamma_multiply(0.35)
                } else {
                    egui::Color32::TRANSPARENT
                });
                let resp = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(egui::RichText::new(head).small().strong());
                        ui.label(body);
                    })
                    .response
                    .interact(egui::Sense::click());
                if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    clicked = Some((key, annot.rect.1, matches!(annot.kind, AnnotKind::Note)));
                }
            }
        });

        if let Some((key, y, _)) = clicked {
            // Bring the comment into view, then open it.
            if let Some(&top) = self.tops.get(key.0) {
                let scale = self.zoom * super::PT_TO_PX;
                let target = top + y * scale - self.viewport_size.y * 0.3;
                self.pending_scroll = Some(Vec2::new(self.scroll_offset.x, target.max(0.0)));
            }
            self.open_comment(key, None);
        }
    }
}
