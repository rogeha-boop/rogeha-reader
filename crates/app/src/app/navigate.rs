//! Getting around a document: the outline (table of contents) panel, clickable links, and
//! Back / Forward after a jump.

use std::collections::{HashMap, HashSet};

use egui::{CursorIcon, Rect, Response, RichText, Sense};
use rogeha_core::{LinkTarget, OutlineItem, PageLink, Request};

use super::ReaderApp;

#[derive(Default)]
pub(super) struct Nav {
    pub outline: Vec<OutlineItem>,
    pub outline_open: bool,
    /// Links per page, for the current document state.
    pub links: HashMap<usize, Vec<PageLink>>,
    asked: HashSet<usize>,
    back: Vec<usize>,
    forward: Vec<usize>,
}

impl Nav {
    pub fn reset_for_new_document(&mut self) {
        self.outline.clear();
        self.back.clear();
        self.forward.clear();
        self.links_changed();
    }

    /// The document changed; link positions and targets have to be read again.
    pub fn links_changed(&mut self) {
        self.links.clear();
        self.asked.clear();
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

impl ReaderApp {
    /// Jump to a page from a link or the outline, remembering where we came from.
    pub(super) fn jump_to(&mut self, page: usize) {
        if page == self.current_page {
            return;
        }
        self.nav.back.push(self.current_page);
        self.nav.forward.clear();
        let from = self.current_page + 1;
        self.go_to_page(page);
        self.info(format!("Alt+Left goes back to page {from}"));
    }

    pub(super) fn go_back(&mut self) {
        if let Some(page) = self.nav.back.pop() {
            self.nav.forward.push(self.current_page);
            self.go_to_page(page);
        }
    }

    pub(super) fn go_forward(&mut self) {
        if let Some(page) = self.nav.forward.pop() {
            self.nav.back.push(self.current_page);
            self.go_to_page(page);
        }
    }

    pub(super) fn follow_link(&mut self, ctx: &egui::Context, target: &LinkTarget) {
        match target {
            LinkTarget::Page(page) => self.jump_to(*page),
            LinkTarget::Url(url) => {
                // Only ordinary web and mail addresses are handed to the system.
                let lower = url.to_ascii_lowercase();
                if ["http://", "https://", "mailto:"].iter().any(|p| lower.starts_with(p)) {
                    ctx.open_url(egui::OpenUrl::new_tab(url));
                } else {
                    self.error(format!("This kind of link is not opened: {url}"));
                }
            }
        }
    }

    /// Links on the visible pages: a hand cursor and the target on hover, a jump on click.
    pub(super) fn link_interactions(&mut self, ctx: &egui::Context, pages: &[(usize, Rect, Response)], scale: f32) {
        if self.comments.note_tool {
            return;
        }
        let mut follow = None;
        for (page, rect, response) in pages {
            if self.nav.asked.insert(*page) {
                self.worker.send(Request::Links { page: *page });
            }
            let Some(pos) = response.hover_pos() else { continue };
            let p = (pos - rect.min) / scale;
            let Some(link) = self.nav.links.get(page).and_then(|links| {
                links.iter().find(|l| p.x >= l.rect.0 && p.x <= l.rect.2 && p.y >= l.rect.1 && p.y <= l.rect.3)
            }) else {
                continue;
            };
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            let hint = match &link.target {
                LinkTarget::Page(page) => format!("Go to page {}", page + 1),
                LinkTarget::Url(url) => url.clone(),
            };
            response.clone().on_hover_text_at_pointer(hint);
            if response.clicked() {
                follow = Some(link.target.clone());
            }
        }
        if let Some(target) = follow {
            self.follow_link(ctx, &target);
        }
    }

    pub(super) fn outline_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.heading("Outline");
        ui.separator();
        if self.nav.outline.is_empty() {
            ui.label("This document has no outline.");
            return;
        }
        // The section being read: the last entry that starts on or before this page.
        let here = self.current_page;
        let current = self
            .nav
            .outline
            .iter()
            .enumerate()
            .filter(|(_, o)| o.page.is_some_and(|p| p <= here))
            .max_by_key(|(i, o)| (o.page, *i))
            .map(|(i, _)| i);
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
        let mut jump = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
            ui,
            row_height,
            self.nav.outline.len(),
            |ui, rows| {
                for i in rows {
                    let item = &self.nav.outline[i];
                    ui.horizontal(|ui| {
                        ui.add_space(item.level as f32 * 14.0);
                        let mut text = RichText::new(&item.title);
                        if current == Some(i) {
                            text = text.strong();
                        } else if item.page.is_none() {
                            text = text.weak();
                        }
                        let label = ui.add(egui::Label::new(text).truncate().sense(Sense::click()));
                        if let Some(page) = item.page {
                            if label.on_hover_text(format!("Page {}", page + 1)).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                                jump = Some(page);
                            }
                        }
                    });
                }
            },
        );
        if let Some(page) = jump {
            self.jump_to(page);
        }
    }
}
