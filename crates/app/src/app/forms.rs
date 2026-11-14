//! Filling out forms on the page: click a text field to type in it, click a checkbox or
//! radio button to switch it, click a dropdown to choose. The engine stores the values the
//! standard way, so other readers show them too.

use egui::{Color32, Id, Order, Pos2, Rect, Response, Stroke, StrokeKind, Vec2};
use rogeha_core::{FieldKind, FormField, Request};

use super::ReaderApp;
use crate::theme;

struct TextEditing {
    page: usize,
    xref: i32,
    text: String,
    original: String,
    multiline: bool,
    password: bool,
    focus: bool,
}

#[derive(Default)]
pub(super) struct FormsUi {
    pub list: Vec<FormField>,
    pub requested: bool,
    /// The text field being typed in.
    editing: Option<TextEditing>,
    /// The dropdown / list whose choices are showing.
    choosing: Option<(usize, i32)>,
    /// The hint about the form has been shown for this document.
    announced: bool,
}

impl FormsUi {
    pub fn reset_for_new_document(&mut self) {
        *self = Self::default();
    }
}

fn fillable(field: &FormField) -> bool {
    !field.read_only && field.kind != FieldKind::Other
}

fn contains(field: &FormField, p: (f32, f32)) -> bool {
    let (x0, y0, x1, y1) = field.rect;
    p.0 >= x0 && p.0 <= x1 && p.1 >= y0 && p.1 <= y1
}

impl ReaderApp {
    pub(super) fn set_form_fields(&mut self, list: Vec<FormField>) {
        self.forms.list = list;
        // Editors for fields that are gone (page deleted, other document state) close.
        let exists = |page: usize, xref: i32| self.forms.list.iter().any(|f| (f.page, f.xref) == (page, xref));
        if self.forms.editing.as_ref().is_some_and(|e| !exists(e.page, e.xref)) {
            self.forms.editing = None;
        }
        if self.forms.choosing.is_some_and(|(page, xref)| !exists(page, xref)) {
            self.forms.choosing = None;
        }
        if !self.forms.announced && self.forms.list.iter().any(fillable) {
            self.forms.announced = true;
            self.info("This document is a form: click a field to fill it in");
        }
    }

    /// Write the text being edited into the document (if it changed) and close the editor.
    pub(super) fn commit_field_text(&mut self) {
        if let Some(edit) = self.forms.editing.take() {
            if edit.text != edit.original {
                self.worker.send(Request::SetField { page: edit.page, xref: edit.xref, value: edit.text });
            }
        }
    }

    fn edit_text_field(&mut self, field: &FormField) {
        let FieldKind::Text { multiline, password } = field.kind else { return };
        self.commit_field_text();
        self.forms.choosing = None;
        self.forms.editing = Some(TextEditing {
            page: field.page,
            xref: field.xref,
            text: field.value.clone(),
            original: field.value.clone(),
            multiline,
            password,
            focus: true,
        });
    }

    /// A click on a page at `p` (page points). Returns true if a form field took it.
    pub(super) fn form_click(&mut self, page: usize, p: (f32, f32)) -> bool {
        let hit = self.forms.list.iter().rev().find(|f| f.page == page && fillable(f) && contains(f, p)).cloned();
        let Some(field) = hit else {
            // A click anywhere else finishes what was being typed.
            self.commit_field_text();
            self.forms.choosing = None;
            return false;
        };
        match field.kind {
            FieldKind::Text { .. } => self.edit_text_field(&field),
            FieldKind::CheckBox => {
                self.commit_field_text();
                self.worker.send(Request::SetChecked { page, xref: field.xref, checked: !field.checked });
            }
            FieldKind::Radio => {
                self.commit_field_text();
                if !field.checked {
                    self.worker.send(Request::SetChecked { page, xref: field.xref, checked: true });
                }
            }
            FieldKind::Dropdown | FieldKind::List { .. } => {
                self.commit_field_text();
                let key = (page, field.xref);
                self.forms.choosing = (self.forms.choosing != Some(key)).then_some(key);
            }
            FieldKind::Other => {}
        }
        true
    }

    /// Is the point over a field that can be filled? (For the mouse cursor and to keep a
    /// drag that starts in a field from selecting page text.)
    pub(super) fn over_form_field(&self, page: usize, p: (f32, f32)) -> bool {
        self.forms.list.iter().any(|f| f.page == page && fillable(f) && contains(f, p))
    }

    /// Field tints, the text editor and the dropdown choices.
    pub(super) fn forms_overlay(&mut self, ui: &egui::Ui, pages: &[(usize, Rect, Response)], scale: f32) {
        if self.forms.list.is_empty() {
            return;
        }
        let clip = self.layer.clip;
        let t = theme::tokens(ui.visuals().dark_mode);
        let painter = ui.painter().with_clip_rect(clip);
        let screen = |page_rect: &Rect, f: &FormField| {
            Rect::from_min_max(page_rect.min + Vec2::new(f.rect.0, f.rect.1) * scale, page_rect.min + Vec2::new(f.rect.2, f.rect.3) * scale)
        };
        let page_rect = |page: usize| pages.iter().find(|(p, ..)| *p == page).map(|(_, r, _)| *r);

        // A light tint shows where the form can be filled in (pages are always light).
        let tint = Color32::from_rgba_unmultiplied(theme::LIGHT.steel_500.r(), theme::LIGHT.steel_500.g(), theme::LIGHT.steel_500.b(), 26);
        for field in self.forms.list.iter().filter(|f| fillable(f)) {
            if let Some(r) = page_rect(field.page) {
                painter.rect_filled(screen(&r, field), 1, tint);
            }
        }

        // Typing into a text field: an editor exactly over the field.
        let mut commit = false;
        let mut cancel = false;
        let mut next_field = false;
        if let Some(edit) = &mut self.forms.editing {
            let field = self.forms.list.iter().find(|f| (f.page, f.xref) == (edit.page, edit.xref));
            if let (Some(field), Some(r)) = (field, page_rect(edit.page)) {
                let rect = screen(&r, field);
                let size = if edit.multiline { 13.0 } else { (rect.height() * 0.62).clamp(9.0, 18.0) };
                egui::Area::new(Id::unique("form-text-editor"))
                    .order(Order::Foreground)
                    .fixed_pos(rect.min)
                    .constrain(false)
                    .show(ui.ctx(), |ui| {
                        ui.set_clip_rect(clip);
                        // Paper-coloured editor with dark ink, whatever the app's theme.
                        let visuals = ui.visuals_mut();
                        visuals.extreme_bg_color = Color32::WHITE;
                        visuals.override_text_color = Some(theme::LIGHT.ink);
                        visuals.selection.stroke = Stroke::new(1.5, theme::LIGHT.steel_500);
                        let font = egui::FontId::proportional(size);
                        let widget = if edit.multiline {
                            egui::TextEdit::multiline(&mut edit.text)
                        } else {
                            egui::TextEdit::singleline(&mut edit.text)
                        };
                        let resp = ui.add_sized(
                            rect.size().max(Vec2::new(40.0, 18.0)),
                            widget.font(font).password(edit.password).margin(egui::Margin::symmetric(2, 0)),
                        );
                        if std::mem::take(&mut edit.focus) {
                            resp.request_focus();
                        }
                        let (enter, escape, tab) = ui.input(|i| {
                            (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Tab))
                        });
                        if escape {
                            cancel = true;
                        } else if tab {
                            next_field = true;
                        } else if resp.lost_focus() && (enter || !edit.multiline) {
                            commit = true;
                        } else if resp.lost_focus() {
                            commit = true;
                        }
                    });
            }
        }
        if cancel {
            self.forms.editing = None;
        } else if next_field {
            // Tab: keep what was typed and go on to the next text field.
            let current = self.forms.editing.as_ref().map(|e| (e.page, e.xref));
            let texts: Vec<FormField> = self
                .forms
                .list
                .iter()
                .filter(|f| fillable(f) && matches!(f.kind, FieldKind::Text { .. }))
                .cloned()
                .collect();
            let at = texts.iter().position(|f| Some((f.page, f.xref)) == current);
            self.commit_field_text();
            if let Some(next) = at.and_then(|i| texts.get(i + 1)) {
                self.edit_text_field(next);
            }
        } else if commit {
            self.commit_field_text();
        }

        // Choosing from a dropdown or list: the choices under the field.
        let mut chosen: Option<String> = None;
        let mut close = false;
        if let Some((page, xref)) = self.forms.choosing {
            let field = self.forms.list.iter().find(|f| (f.page, f.xref) == (page, xref));
            if let (Some(field), Some(r)) = (field, page_rect(page)) {
                let rect = screen(&r, field);
                let area = egui::Area::new(Id::unique("form-choices"))
                    .order(Order::Foreground)
                    .fixed_pos(Pos2::new(rect.left(), rect.bottom() + 2.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_min_width(rect.width().max(120.0));
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                for (stored, shown) in &field.options {
                                    let label = if shown.is_empty() { stored } else { shown };
                                    if ui.selectable_label(*stored == field.value, label).clicked() {
                                        chosen = Some(stored.clone());
                                    }
                                }
                                if field.options.is_empty() {
                                    ui.label(egui::RichText::new("This field has no choices").weak());
                                }
                            });
                        });
                    });
                if area.response.clicked_elsewhere() && ui.input(|i| i.pointer.any_released()) {
                    close = true;
                }
                painter.rect_stroke(rect, 1, Stroke::new(1.5, t.steel_500), StrokeKind::Outside);
                if let Some(value) = chosen {
                    self.worker.send(Request::SetField { page, xref, value });
                    close = true;
                }
            } else {
                close = true;
            }
        }
        if close {
            self.forms.choosing = None;
        }
    }

    // ------------------------------------------------------------- scripted checks

    /// `spec` is `<part of a field name>|<value>`; a value of `on` ticks a box.
    pub(super) fn dev_fill(&mut self, spec: &str, open_editor: bool) {
        let (name, value) = spec.split_once('|').unwrap_or((spec, ""));
        let Some(field) = self.forms.list.iter().find(|f| f.name.contains(name) && fillable(f)).cloned() else { return };
        match field.kind {
            FieldKind::Text { .. } if open_editor => {
                self.edit_text_field(&field);
                if let Some(edit) = &mut self.forms.editing {
                    edit.text = value.to_owned();
                }
            }
            FieldKind::CheckBox | FieldKind::Radio => {
                self.worker.send(Request::SetChecked { page: field.page, xref: field.xref, checked: true });
            }
            FieldKind::Dropdown | FieldKind::List { .. } if open_editor => {
                self.forms.choosing = Some((field.page, field.xref));
            }
            _ => self.worker.send(Request::SetField { page: field.page, xref: field.xref, value: value.to_owned() }),
        }
    }
}
