//! The Pages panel: a thumbnail of every page, for getting around and for organising the
//! document. Click to go to a page, Ctrl- or Shift-click to select several, drag to reorder,
//! and use the buttons at the top to rotate or delete the selection or to add another PDF.

use std::collections::{BTreeSet, HashMap};

use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use rogeha_core::{PageImage, RenderKey, Request};

use super::ReaderApp;
use crate::icons::{self, Icon};
use crate::theme;

/// The box a thumbnail is fitted into, and the height of one row (thumbnail + page number).
const THUMB_BOX: Vec2 = Vec2::new(120.0, 150.0);
const ROW_HEIGHT: f32 = 178.0;
/// Thumbnails further than this many pages from the visible ones are dropped from memory.
const KEEP_AROUND: usize = 30;

#[derive(Default)]
pub(super) struct PagesUi {
    pub open: bool,
    selected: BTreeSet<usize>,
    /// Where a Shift-click range starts.
    anchor: Option<usize>,
    thumbs: HashMap<usize, (RenderKey, TextureHandle)>,
    /// Thumbnails to ask the document thread for this frame.
    pub want: Vec<RenderKey>,
    /// A drag is in progress, and where the pages would land (insert before this index).
    dragging: bool,
    drop_before: Option<usize>,
    last_current: usize,
    scroll_to: Option<usize>,
}

impl PagesUi {
    /// The document changed shape (pages added, removed, moved, rotated): start over.
    pub fn reset(&mut self, keep_selection: bool) {
        self.thumbs.clear();
        self.dragging = false;
        self.drop_before = None;
        if !keep_selection {
            self.selected.clear();
            self.anchor = None;
        }
    }
}

impl ReaderApp {
    /// A finished thumbnail from the document thread.
    pub(super) fn thumbnail_arrived(&mut self, ctx: &egui::Context, key: RenderKey, image: PageImage) {
        let pixels = image.pixels.iter().map(|p| Color32::from_rgb((p >> 16) as u8, (p >> 8) as u8, *p as u8)).collect();
        let texture = ctx.load_texture(
            format!("thumb-{}", key.page),
            egui::ColorImage::new([image.width, image.height], pixels),
            TextureOptions::LINEAR,
        );
        self.pages.thumbs.insert(key.page, (key, texture));
    }

    /// The pages the panel's buttons act on: the selection, or the current page if none.
    fn pages_target(&self) -> Vec<usize> {
        if self.pages.selected.is_empty() {
            vec![self.current_page]
        } else {
            self.pages.selected.iter().copied().collect()
        }
    }

    fn reorder(&mut self, pages: Vec<usize>, before: usize) {
        if !self.can_edit() || pages.is_empty() {
            return;
        }
        // The moved pages end up together starting here; keep them selected.
        let start = before - pages.iter().filter(|p| **p < before).count();
        self.pages.selected = (start..start + pages.len()).collect();
        self.pages.anchor = Some(start);
        self.pending_page = Some(start);
        self.worker.send(Request::Reorder { pages, before });
    }

    pub(super) fn pages_panel(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &self.doc else { return };
        let (count, generation, can_edit) = (doc.page_sizes.len(), doc.generation, doc.is_pdf);
        let sizes = doc.page_sizes.clone();
        let t = theme::tokens(ui.visuals().dark_mode);
        self.pages.selected.retain(|p| *p < count);

        ui.add_space(4.0);
        ui.heading("Pages");
        let target = self.pages_target();
        let what = match target.len() {
            1 => format!("page {}", target[0] + 1),
            n => format!("{n} pages"),
        };
        let edit_hint = "Page editing works on PDF files";
        ui.horizontal_wrapped(|ui| {
            if icons::icon_button(ui, Icon::RotateLeft, false, can_edit)
                .on_hover_text(format!("Rotate {what} left"))
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.pending_page = Some(self.current_page);
                self.worker.send(Request::Rotate { pages: target.clone(), delta: -90 });
            }
            if icons::icon_button(ui, Icon::RotateRight, false, can_edit)
                .on_hover_text(format!("Rotate {what} right"))
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                self.pending_page = Some(self.current_page);
                self.worker.send(Request::Rotate { pages: target.clone(), delta: 90 });
            }
            if ui
                .add_enabled(can_edit && target.len() < count, egui::Button::new("Delete"))
                .on_hover_text(format!("Delete {what}"))
                .on_disabled_hover_text(if can_edit { "A document needs at least one page" } else { edit_hint })
                .clicked()
            {
                let numbers = target.iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(", ");
                let answer = rfd::MessageDialog::new()
                    .set_title("Delete pages")
                    .set_description(format!("Delete page(s) {numbers} from this document?\nThe file changes only when you save."))
                    .set_buttons(rfd::MessageButtons::OkCancel)
                    .show();
                if matches!(answer, rfd::MessageDialogResult::Ok) {
                    self.pending_page = Some(target[0].min(count.saturating_sub(target.len() + 1)));
                    self.pages.selected.clear();
                    self.worker.send(Request::Delete { pages: target.clone() });
                }
            }
            if ui
                .add_enabled(can_edit, egui::Button::new("Add PDF…"))
                .on_hover_text("Insert the pages of other PDF files after the selected page (or at the end)")
                .on_disabled_hover_text(edit_hint)
                .clicked()
            {
                let picked = rfd::FileDialog::new().set_title("Choose PDFs to add").add_filter("PDF", &["pdf"]).pick_files();
                if let Some(mut paths) = picked.filter(|p| !p.is_empty()) {
                    paths.sort();
                    // After the last selected page; with nothing selected, at the end.
                    let before = self.pages.selected.iter().next_back().map(|p| p + 1);
                    self.pending_page = Some(before.unwrap_or(count));
                    self.worker.send(Request::InsertPdfs { paths, before });
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            // Moving without dragging, one step at a time.
            let (first, last) = (target[0], target[target.len() - 1]);
            if ui.add_enabled(can_edit && first > 0, egui::Button::new("Move up")).on_hover_text(format!("Move {what} one place earlier")).clicked() {
                self.reorder(target.clone(), first - 1);
            }
            if ui.add_enabled(can_edit && last + 1 < count, egui::Button::new("Move down")).on_hover_text(format!("Move {what} one place later")).clicked() {
                self.reorder(target.clone(), last + 2);
            }
            if ui.small_button("Select all").clicked() {
                self.pages.selected = (0..count).collect();
            }
        });
        ui.label(
            egui::RichText::new("Click a page to go to it. Ctrl or Shift selects several. Drag to reorder.")
                .small()
                .color(t.ink_muted),
        );
        ui.separator();

        // Follow the page view: when it moves to another page, bring that thumbnail into view.
        if self.current_page != self.pages.last_current {
            self.pages.last_current = self.current_page;
            if !ui.rect_contains_pointer(ui.max_rect()) {
                self.pages.scroll_to = Some(self.current_page);
            }
        }
        let spacing = ui.spacing().item_spacing.y;
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(page) = self.pages.scroll_to.take() {
            let offset = page as f32 * (ROW_HEIGHT + spacing) - ui.available_height() / 3.0;
            area = area.vertical_scroll_offset(offset.max(0.0));
        }

        let ppp = ui.ctx().pixels_per_point();
        let modifiers = ui.input(|i| i.modifiers);
        let pointer = ui.ctx().pointer_latest_pos();
        let mut clicked: Option<usize> = None;
        let mut drag_started: Option<usize> = None;
        let mut drag_stopped = false;
        let mut still_dragging = false;
        let mut rows: Vec<(usize, Rect)> = Vec::new();
        let mut visible = 0..0;

        area.show_rows(ui, ROW_HEIGHT, count, |ui, range| {
            visible = range.clone();
            for page in range {
                let (row, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click_and_drag());
                rows.push((page, row));
                let selected = self.pages.selected.contains(&page);
                let painter = ui.painter();
                if selected {
                    painter.rect_filled(row, theme::RADIUS_SM, t.steel_100);
                } else if response.hovered() {
                    painter.rect_filled(row, theme::RADIUS_SM, t.surface_sunken);
                }

                // The thumbnail, fitted into its box.
                let (w, h) = sizes[page];
                let fit = (THUMB_BOX.x / w).min(THUMB_BOX.y / h);
                let size = Vec2::new(w * fit, h * fit);
                let centre = Pos2::new(row.center().x, row.top() + 6.0 + THUMB_BOX.y / 2.0);
                let rect = Rect::from_center_size(centre, size);
                painter.rect_filled(rect, 0, Color32::WHITE);
                let key = RenderKey { page, scale_milli: (fit * ppp * 1000.0).round().max(1.0) as u32, generation, thumb: true };
                match self.pages.thumbs.get(&page) {
                    Some((have, texture)) => {
                        painter.image(texture.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        if *have != key {
                            self.pages.want.push(key);
                        }
                    }
                    None => self.pages.want.push(key),
                }
                // The page on screen in the main view gets the stronger frame.
                let frame = if page == self.current_page { Stroke::new(2.0, t.steel_500) } else { Stroke::new(1.0, t.border) };
                painter.rect_stroke(rect, 0, frame, StrokeKind::Outside);
                painter.text(
                    Pos2::new(row.center().x, row.top() + THUMB_BOX.y + 12.0),
                    egui::Align2::CENTER_TOP,
                    (page + 1).to_string(),
                    egui::TextStyle::Small.resolve(ui.style()),
                    if selected { t.ink } else { t.ink_muted },
                );

                if response.clicked() {
                    clicked = Some(page);
                }
                if response.drag_started() {
                    drag_started = Some(page);
                }
                still_dragging |= response.dragged();
                drag_stopped |= response.drag_stopped();
            }

            // While dragging: show where the pages would be dropped.
            if self.pages.dragging {
                self.pages.drop_before = pointer.and_then(|pos| {
                    rows.iter().find(|(_, r)| pos.y >= r.top() && pos.y <= r.bottom() + spacing).map(|(page, r)| if pos.y < r.center().y { *page } else { page + 1 })
                });
                if let Some(before) = self.pages.drop_before {
                    let y = rows
                        .iter()
                        .find(|(p, _)| *p == before)
                        .map(|(_, r)| r.top() - spacing / 2.0)
                        .or_else(|| rows.iter().find(|(p, _)| p + 1 == before).map(|(_, r)| r.bottom() + spacing / 2.0));
                    if let (Some(y), Some((_, any))) = (y, rows.first()) {
                        ui.painter().line_segment([Pos2::new(any.left() + 8.0, y), Pos2::new(any.right() - 8.0, y)], Stroke::new(3.0, t.steel_500));
                    }
                }
            }
        });

        if let Some(page) = clicked {
            if modifiers.command {
                // Ctrl: add or remove this page.
                if !self.pages.selected.remove(&page) {
                    self.pages.selected.insert(page);
                }
                self.pages.anchor = Some(page);
            } else if modifiers.shift {
                // Shift: everything from the last clicked page to this one.
                let from = self.pages.anchor.unwrap_or(self.current_page);
                self.pages.selected = (from.min(page)..=from.max(page)).collect();
            } else {
                self.pages.selected = BTreeSet::from([page]);
                self.pages.anchor = Some(page);
                self.go_to_page(page);
            }
        }
        if let Some(page) = drag_started.filter(|_| can_edit) {
            // Dragging a page that is not selected drags just that page.
            if !self.pages.selected.contains(&page) {
                self.pages.selected = BTreeSet::from([page]);
                self.pages.anchor = Some(page);
            }
            self.pages.dragging = true;
        }
        if drag_stopped || (self.pages.dragging && !still_dragging && drag_started.is_none()) {
            if let (true, Some(before)) = (self.pages.dragging, self.pages.drop_before) {
                let pages: Vec<usize> = self.pages.selected.iter().copied().collect();
                self.reorder(pages, before);
            }
            self.pages.dragging = false;
            self.pages.drop_before = None;
        }
        if self.pages.dragging {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }

        // Keep the thumbnails near what is visible; let go of the rest.
        let (near_from, near_to) = (visible.start.saturating_sub(KEEP_AROUND), visible.end + KEEP_AROUND);
        self.pages.thumbs.retain(|page, _| (near_from..near_to).contains(page));
    }

    // ------------------------------------------------------------- scripted checks

    /// `spec` is like `2,5>1`: move pages 2 and 5 in front of page 1 (all numbers 1-based;
    /// a target one past the last page means the end).
    pub(super) fn dev_reorder(&mut self, spec: &str) {
        let Some((pages, before)) = spec.split_once('>') else { return };
        let pages: Vec<usize> = pages.split(',').filter_map(|p| p.trim().parse::<usize>().ok()).filter(|p| *p > 0).map(|p| p - 1).collect();
        if let Ok(before) = before.trim().parse::<usize>() {
            self.reorder(pages, before.saturating_sub(1));
        }
    }
}
