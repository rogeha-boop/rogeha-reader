//! "Black out (redact)": mark areas of pages, then write a copy in which they are blacked out
//! for good. Meant for documents sent as proof of something (a bank statement as proof of
//! address) where the rest is nobody's business. The work itself is `rogeha_core::redact`;
//! here the areas are marked: by dragging a box, by giving a text to mark wherever it
//! appears, and optionally the same box on every page (a statement's list of payments).

use std::path::PathBuf;

use egui::{Color32, CornerRadius, CursorIcon, Pos2, Rect, Response, Stroke, StrokeKind, Vec2};
use rogeha_core::{HitQuad, RedactArea, Request};

use super::ReaderApp;
use super::comments::quad_bounds;

#[derive(Default)]
pub(super) struct RedactUi {
    pub open: bool,
    pub marks: Vec<RedactArea>,
    /// A box being dragged: page, where it started, where the pointer is (page points).
    drag: Option<(usize, (f32, f32), (f32, f32))>,
    /// A dragged box is marked at the same place on every page.
    pub every_page: bool,
    pub find: String,
    /// Waiting for the places where this text appears.
    finding: Option<String>,
    /// Blacking out and writing the copy.
    pub busy: bool,
    note: Option<String>,
}

impl RedactUi {
    pub fn reset(&mut self) {
        self.marks.clear();
        self.drag = None;
        self.finding = None;
        self.note = None;
    }
}

fn ordered(a: (f32, f32), b: (f32, f32)) -> (f32, f32, f32, f32) {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn contains(r: (f32, f32, f32, f32), p: (f32, f32)) -> bool {
    p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3
}

/// A text hit as an area, a little larger than the letters so their edges go too.
fn hit_area(page: usize, quad: &HitQuad) -> RedactArea {
    let (min, max) = quad_bounds(quad);
    RedactArea { page, rect: (min.x - 1.0, min.y - 1.0, max.x + 1.0, max.y + 1.0) }
}

/// "Statement.pdf" -> "Statement (redacted).pdf", next to it.
pub(super) fn redacted_name(path: &std::path::Path) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Document".into());
    path.with_file_name(format!("{stem} (redacted).pdf"))
}

impl ReaderApp {
    /// Mouse handling while marking: drag a box to mark it, click a marked box to take it away.
    pub(super) fn redact_interactions(&mut self, ctx: &egui::Context, pages: &[(usize, Rect, Response)], scale: f32) {
        let Some(sizes) = self.doc.as_ref().map(|d| d.page_sizes.clone()) else { return };
        let busy = self.redact.busy;
        for (page, rect, response) in pages {
            let page = *page;
            let (pw, ph) = sizes[page];
            let to_page = |pos: Pos2| {
                let v = (pos - rect.min) / scale;
                (v.x.clamp(0.0, pw), v.y.clamp(0.0, ph))
            };
            if busy {
                continue;
            }
            if response.hovered() {
                let over_mark = response
                    .hover_pos()
                    .is_some_and(|pos| self.redact.marks.iter().any(|m| m.page == page && contains(m.rect, to_page(pos))));
                ctx.set_cursor_icon(if over_mark { CursorIcon::PointingHand } else { CursorIcon::Crosshair });
            }
            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let p = to_page(pos);
                    self.redact.drag = Some((page, p, p));
                }
            }
            if response.dragged() {
                if let (Some((drag_page, a, _)), Some(pos)) = (self.redact.drag, response.interact_pointer_pos()) {
                    if drag_page == page {
                        self.redact.drag = Some((page, a, to_page(pos)));
                    }
                }
            }
            if response.drag_stopped() {
                if let Some((drag_page, a, b)) = self.redact.drag.take().filter(|d| d.0 == page) {
                    self.add_box(drag_page, ordered(a, b));
                }
            }
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let p = to_page(pos);
                    if let Some(i) = self.redact.marks.iter().rposition(|m| m.page == page && contains(m.rect, p)) {
                        self.redact.marks.remove(i);
                    }
                }
            }
        }
    }

    /// Mark a dragged box, on its page or on every page.
    pub(super) fn add_box(&mut self, page: usize, rect: (f32, f32, f32, f32)) {
        if rect.2 - rect.0 < 2.0 || rect.3 - rect.1 < 2.0 {
            return; // a click, not a box
        }
        let Some(sizes) = self.doc.as_ref().map(|d| d.page_sizes.clone()) else { return };
        let pages: Vec<usize> = if self.redact.every_page { (0..sizes.len()).collect() } else { vec![page] };
        for p in pages {
            let (w, h) = sizes[p];
            let clipped = (rect.0.min(w), rect.1.min(h), rect.2.min(w), rect.3.min(h));
            if clipped.2 - clipped.0 >= 2.0 && clipped.3 - clipped.1 >= 2.0 {
                self.redact.marks.push(RedactArea { page: p, rect: clipped });
            }
        }
    }

    /// The marked boxes, and the one being dragged.
    pub(super) fn redact_overlay(&self, ui: &egui::Ui, pages: &[(usize, Rect, Response)], scale: f32) {
        let painter = ui.painter().with_clip_rect(self.layer.clip);
        let red = Color32::from_rgb(0xd9, 0x2d, 0x20);
        let dragged = self.redact.drag.map(|(page, a, b)| RedactArea { page, rect: ordered(a, b) });
        for mark in self.redact.marks.iter().chain(dragged.iter()) {
            let Some((_, rect, _)) = pages.iter().find(|(p, ..)| *p == mark.page) else { continue };
            let (x0, y0, x1, y1) = mark.rect;
            let r = Rect::from_min_max(rect.min + Vec2::new(x0, y0) * scale, rect.min + Vec2::new(x1, y1) * scale);
            // Dark enough to see what will go, light enough to still read what is under it.
            painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(0, 0, 0, 110));
            painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, red), StrokeKind::Outside);
        }
    }

    /// Mark every place a text appears (answered by [`Self::redact_found`]).
    pub(super) fn redact_find(&mut self) {
        let query = self.redact.find.trim().to_owned();
        if query.is_empty() {
            return;
        }
        self.redact.finding = Some(query.clone());
        self.worker.send(Request::FindAll { query });
    }

    pub(super) fn redact_found(&mut self, query: String, hits: Vec<(usize, HitQuad)>) {
        if self.redact.finding.as_deref() != Some(query.as_str()) {
            return;
        }
        self.redact.finding = None;
        let before = self.redact.marks.len();
        for (page, quad) in &hits {
            let area = hit_area(*page, quad);
            if !self.redact.marks.contains(&area) {
                self.redact.marks.push(area);
            }
        }
        let added = self.redact.marks.len() - before;
        self.redact.note = Some(match (hits.len(), added) {
            (0, _) => format!("\u{201c}{query}\u{201d} does not appear in the document."),
            (_, 0) => format!("Every \u{201c}{query}\u{201d} was marked already."),
            (_, n) => format!("Marked \u{201c}{query}\u{201d} {n} time{}.", if n == 1 { "" } else { "s" }),
        });
    }

    /// Black out the marked areas and write the result to a new file (asked for when `target` is `None`).
    pub(super) fn redact_apply(&mut self, target: Option<PathBuf>) {
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else { return };
        if self.redact.marks.is_empty() {
            return;
        }
        let target = match target {
            Some(t) => t,
            None => {
                let suggested = redacted_name(&path);
                let mut dialog = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_title("Save the blacked-out copy as");
                if let Some(dir) = suggested.parent() {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(name) = suggested.file_name() {
                    dialog = dialog.set_file_name(name.to_string_lossy());
                }
                let Some(t) = dialog.save_file() else { return };
                t
            }
        };
        let same = |a: &std::path::Path, b: &std::path::Path| match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        };
        if same(&target, &path) {
            self.error("Choose a new name for the blacked-out copy, so the original stays as it is.");
            return;
        }
        self.redact.busy = true;
        self.redact.note = None;
        // The tab follows the document to the new file, as with "Save as".
        self.session.save_as_target = Some(target.clone());
        self.worker.send(Request::Redact { areas: self.redact.marks.clone(), target });
    }

    pub(super) fn redact_done(&mut self, path: PathBuf, report: rogeha_core::RedactReport) {
        self.redact.busy = false;
        self.redact.open = false;
        self.redact.reset();
        let pages = if report.pages == 1 { "1 page".to_owned() } else { format!("{} pages", report.pages) };
        let extra = match report.annotations_removed {
            0 => String::new(),
            1 => " A comment or link under a box was removed too.".to_owned(),
            n => format!(" {n} comments or links under the boxes were removed too."),
        };
        self.info(format!(
            "Blacked out on {pages} and saved as {}. This is the copy you see now; the original is unchanged.{extra}",
            path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
        ));
    }

    pub(super) fn redact_window(&mut self, ctx: &egui::Context) {
        if !self.redact.open {
            return;
        }
        let is_pdf = self.can_edit();
        let dirty = self.is_dirty();
        let mut open = true;
        let mut find = false;
        let mut apply = false;
        egui::Window::new("Black out (redact)")
            .id(egui::Id::unique("redact-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(self.layer.clip.right_top() + Vec2::new(-400.0, 30.0))
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label("Drag a box over what nobody should see: payments, amounts, account numbers. Click a box to take it away again.");
                ui.label(
                    egui::RichText::new(
                        "The copy you save has the content under the boxes taken out, not just covered: it cannot be copied, searched or uncovered. The original file is not changed.",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(6.0);
                let busy = self.redact.busy;
                ui.add_enabled_ui(!busy && is_pdf, |ui| {
                    ui.checkbox(&mut self.redact.every_page, "Same box on every page")
                        .on_hover_text("For lists that sit at the same place on each page, like the payments on a statement");
                    ui.horizontal(|ui| {
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.redact.find)
                                .hint_text("A name, a number…")
                                .desired_width(200.0),
                        );
                        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            find = true;
                        }
                        if ui
                            .add_enabled(self.redact.finding.is_none() && !self.redact.find.trim().is_empty(), egui::Button::new("Mark everywhere"))
                            .on_hover_text("Marks every place in the document where this text appears")
                            .clicked()
                        {
                            find = true;
                        }
                    });
                });
                if let Some(note) = &self.redact.note {
                    ui.label(egui::RichText::new(note).weak().small());
                }
                ui.add_space(6.0);
                let count = self.redact.marks.len();
                let mut pages: Vec<usize> = self.redact.marks.iter().map(|m| m.page).collect();
                pages.sort_unstable();
                pages.dedup();
                ui.horizontal(|ui| {
                    ui.label(match (count, pages.len()) {
                        (0, _) => "Nothing marked yet.".to_owned(),
                        (1, _) => "1 box marked.".to_owned(),
                        (n, 1) => format!("{n} boxes marked, on 1 page."),
                        (n, p) => format!("{n} boxes marked, on {p} pages."),
                    });
                    if count > 0 && !busy && ui.small_button("Clear all").clicked() {
                        self.redact.marks.clear();
                    }
                });
                ui.add_space(6.0);
                if !is_pdf {
                    ui.label("Only PDF files can be blacked out.");
                } else if dirty {
                    ui.label(egui::RichText::new("Save or undo your other changes first.").weak());
                }
                ui.horizontal(|ui| {
                    if busy {
                        ui.spinner();
                        ui.label("Blacking out…");
                    } else if ui
                        .add_enabled(is_pdf && !dirty && count > 0, egui::Button::new("Black out and save as…"))
                        .on_hover_text("Writes a new file with the marked areas blacked out for good")
                        .clicked()
                    {
                        apply = true;
                    }
                });
            });
        if find {
            self.redact_find();
        }
        if apply {
            self.redact_apply(None);
        }
        if !open && !self.redact.busy {
            self.redact.open = false;
            self.redact.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_is_named_after_the_original() {
        let p = std::path::Path::new(r"C:\Docs\Statement October.pdf");
        assert_eq!(redacted_name(p), std::path::Path::new(r"C:\Docs\Statement October (redacted).pdf"));
    }

    #[test]
    fn a_text_hit_is_marked_a_little_larger_than_its_letters() {
        let area = hit_area(2, &[(10.0, 20.0), (50.0, 20.0), (10.0, 32.0), (50.0, 32.0)]);
        assert_eq!(area, RedactArea { page: 2, rect: (9.0, 19.0, 51.0, 33.0) });
        assert!(contains(ordered((5.0, 9.0), (1.0, 2.0)), (3.0, 4.0)));
    }
}
