//! What the reader remembers and keeps open: recent files with the place you were in each,
//! document tabs, the password prompt for protected files, and the dark-pages setting.
//!
//! Tabs are light: only the tab in front holds its document in memory. Switching tabs opens
//! that file again at the page, zoom and layout it was left at, which takes a fraction of a
//! second and keeps memory use at one document however many tabs are open.

use std::path::{Path, PathBuf};

use rogeha_core::Request;
use serde_json::{Value, json};

use super::{AfterSave, Fit, MAX_ZOOM, MIN_ZOOM, ReaderApp, ViewMode, app_data_dir, ask_save_changes};

const MAX_RECENT: usize = 20;

/// A document and where the reader was in it.
#[derive(Clone)]
pub(super) struct Place {
    pub path: PathBuf,
    pub page: usize,
    pub zoom: f32,
    pub fit: Fit,
    pub view: ViewMode,
}

impl Place {
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf(), page: 0, zoom: 1.0, fit: Fit::Width, view: ViewMode::Single }
    }

    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn to_json(&self) -> Value {
        json!({
            "path": self.path.to_string_lossy(),
            "page": self.page,
            "zoom": self.zoom,
            "fit": match self.fit { Fit::Width => "width", Fit::Page => "page", Fit::Custom => "custom" },
            "view": match self.view { ViewMode::Single => "single", ViewMode::Spread => "spread", ViewMode::SpreadCover => "cover" },
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            path: PathBuf::from(v.get("path")?.as_str()?),
            page: v.get("page").and_then(Value::as_u64).unwrap_or(0) as usize,
            zoom: (v.get("zoom").and_then(Value::as_f64).unwrap_or(1.0) as f32).clamp(MIN_ZOOM, MAX_ZOOM),
            fit: match v.get("fit").and_then(Value::as_str) {
                Some("page") => Fit::Page,
                Some("custom") => Fit::Custom,
                _ => Fit::Width,
            },
            view: match v.get("view").and_then(Value::as_str) {
                Some("spread") => ViewMode::Spread,
                Some("cover") => ViewMode::SpreadCover,
                _ => ViewMode::Single,
            },
        })
    }
}

pub(super) struct PasswordPrompt {
    path: PathBuf,
    text: String,
    wrong: bool,
    focus: bool,
}

#[derive(Default)]
pub(super) struct Session {
    /// Most recent first.
    pub recent: Vec<Place>,
    pub tabs: Vec<Place>,
    pub active: usize,
    /// Show pages light-on-dark.
    pub dark_pages: bool,
    /// The language "Translate" translates into.
    pub translate_to: String,
    /// "Save as" is writing the open document to this file; the tab follows it there.
    pub save_as_target: Option<PathBuf>,
    /// The tab to remove once the next document has arrived.
    pub closing_tab: Option<PathBuf>,
    pub password: Option<PasswordPrompt>,
}

fn state_file() -> PathBuf {
    app_data_dir().join("state.json")
}

impl Session {
    pub fn load() -> Self {
        let mut session = Self { translate_to: "English".into(), ..Self::default() };
        let Ok(text) = std::fs::read_to_string(state_file()) else { return session };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { return session };
        session.dark_pages = v.get("dark_pages").and_then(Value::as_bool).unwrap_or(false);
        if let Some(language) = v.get("translate_to").and_then(Value::as_str).filter(|l| !l.trim().is_empty()) {
            session.translate_to = language.to_owned();
        }
        if let Some(list) = v.get("recent").and_then(Value::as_array) {
            session.recent = list.iter().filter_map(Place::from_json).take(MAX_RECENT).collect();
        }
        session
    }

    /// Passwords are never written here.
    fn store(&self) {
        let v = json!({
            "dark_pages": self.dark_pages,
            "translate_to": self.translate_to,
            "recent": self.recent.iter().map(Place::to_json).collect::<Vec<_>>(),
        });
        let dir = app_data_dir();
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(state_file(), serde_json::to_string_pretty(&v).unwrap_or_default());
        }
    }

    fn note_recent(&mut self, place: Place) {
        self.recent.retain(|r| r.path != place.path);
        self.recent.insert(0, place);
        self.recent.truncate(MAX_RECENT);
        self.store();
    }
}

/// Turn a rendered page light-on-dark: lightness is flipped and hue kept, so black text on
/// white becomes white on black while a red stays red.
pub(super) fn darken(pixels: &mut [u32]) {
    for px in pixels {
        let (r, g, b) = ((*px >> 16 & 0xFF) as i32, (*px >> 8 & 0xFF) as i32, (*px & 0xFF) as i32);
        let shift = 255 - r.max(g).max(b) - r.min(g).min(b);
        // Not quite black: easier on the eyes and matches the dark theme.
        let lift = |c: i32| (((c + shift) * 225 / 255) + 30) as u32;
        *px = lift(r) << 16 | lift(g) << 8 | lift(b);
    }
}

impl ReaderApp {
    pub(super) fn current_place(&self) -> Option<Place> {
        let doc = self.doc.as_ref()?;
        Some(Place { path: doc.path.clone(), page: self.current_page, zoom: self.zoom, fit: self.fit, view: self.view })
    }

    /// Note where the reader is in the open document (before leaving it).
    pub(super) fn remember_place(&mut self) {
        let Some(place) = self.current_place() else { return };
        if let Some(tab) = self.session.tabs.iter_mut().find(|t| t.path == place.path) {
            *tab = place.clone();
        }
        self.session.note_recent(place);
    }

    /// A different document has just been opened: give it a tab and go back to where the
    /// reader left it. `previous` is the place in the document shown until now.
    pub(super) fn document_arrived(&mut self, previous: Option<Place>) {
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else { return };
        let session = &mut self.session;
        let place = if session.save_as_target.take().as_deref() == Some(path.as_path()) {
            // "Save as": the same document under a new name, so stay where we are.
            let active = session.active;
            previous.map(|mut p| {
                p.path = path.clone();
                if let Some(tab) = session.tabs.get_mut(active) {
                    *tab = p.clone();
                }
                p
            })
        } else {
            let known = |list: &[Place]| list.iter().find(|p| p.path == path).cloned();
            known(&session.tabs).or_else(|| known(&session.recent))
        };
        if let Some(closing) = session.closing_tab.take() {
            session.tabs.retain(|t| t.path != closing);
        }
        let place = place.unwrap_or_else(|| Place::new(&path));
        session.active = match session.tabs.iter().position(|t| t.path == path) {
            Some(i) => i,
            None => {
                session.tabs.push(place.clone());
                session.tabs.len() - 1
            }
        };
        self.view = place.view;
        self.fit = place.fit;
        if place.fit == Fit::Custom {
            self.zoom = place.zoom;
        }
        if place.page > 0 {
            self.pending_page = Some(place.page);
        }
        self.session.note_recent(place);
    }

    pub(super) fn switch_tab(&mut self, index: usize) {
        if index != self.session.active {
            if let Some(path) = self.session.tabs.get(index).map(|t| t.path.clone()) {
                self.request_open(path);
            }
        }
    }

    pub(super) fn next_tab(&mut self, forward: bool) {
        let n = self.session.tabs.len();
        if n > 1 {
            let step = if forward { 1 } else { n - 1 };
            self.switch_tab((self.session.active + step) % n);
        }
    }

    pub(super) fn close_tab(&mut self, index: usize) {
        let session = &mut self.session;
        if index >= session.tabs.len() {
            return;
        }
        if index != session.active {
            session.tabs.remove(index);
            if index < session.active {
                session.active -= 1;
            }
            return;
        }
        let closing = session.tabs[index].path.clone();
        let neighbour = session.tabs.get(index + 1).or_else(|| index.checked_sub(1).and_then(|i| session.tabs.get(i)));
        match neighbour.map(|t| t.path.clone()) {
            Some(next) => {
                session.closing_tab = Some(closing);
                if !self.request_open(next) {
                    self.session.closing_tab = None;
                }
            }
            None => self.close_document(),
        }
    }

    /// Close the open document and go back to the start screen.
    pub(super) fn close_document(&mut self) {
        if self.doc.is_none() {
            return;
        }
        if self.is_dirty() {
            match ask_save_changes() {
                rfd::MessageDialogResult::Yes => {
                    self.after_save = AfterSave::CloseDoc;
                    self.worker.send(Request::Save { target: None });
                    return;
                }
                rfd::MessageDialogResult::No => {}
                _ => return,
            }
        }
        self.remember_place();
        self.worker.send(Request::Close);
    }

    /// Languages offered in the Tools menu; any other can be typed.
    pub(super) const LANGUAGES: [&str; 8] = ["English", "German", "French", "Italian", "Spanish", "Portuguese", "Serbian", "Croatian"];

    pub(super) fn set_translate_to(&mut self, language: &str) {
        let language = language.trim();
        if !language.is_empty() && language != self.session.translate_to {
            self.session.translate_to = language.to_owned();
            self.session.store();
        }
    }

    /// Ask the assistant for a translation of the selected text, or of the current page.
    pub(super) fn translate(&mut self, selection: bool) {
        let language = self.session.translate_to.clone();
        let question = if selection {
            // The selected text travels with every question as context.
            format!("Translate the selected text into {language}. Reply with the translation only.")
        } else {
            format!(
                "Translate page {} into {language}. Reply with the translation only, keeping the paragraphs.",
                self.current_page + 1
            )
        };
        self.assistant_ask_when_ready(question);
    }

    pub(super) fn set_dark_pages(&mut self, on: bool) {
        if on != self.session.dark_pages {
            self.session.dark_pages = on;
            self.session.store();
            // Draw the pages again in the other colours.
            self.textures.clear();
            self.failed.clear();
            self.last_want.clear();
        }
    }

    /// The row of tabs under the toolbar, shown when more than one document is open.
    pub(super) fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let mut switch = None;
        let mut close = None;
        let dirty = self.is_dirty();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, tab) in self.session.tabs.iter().enumerate() {
                    let active = i == self.session.active;
                    let mut name = tab.name();
                    if name.chars().count() > 32 {
                        name = name.chars().take(31).collect::<String>() + "…";
                    }
                    if active && dirty {
                        name.push('*');
                    }
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if ui.selectable_label(active, name).on_hover_text(tab.path.display().to_string()).clicked() {
                        switch = Some(i);
                    }
                    if ui.small_button("×").on_hover_text("Close this tab (Ctrl+W for the open one)").clicked() {
                        close = Some(i);
                    }
                    ui.add_space(8.0);
                }
            });
        });
        if let Some(i) = close {
            self.close_tab(i);
        } else if let Some(i) = switch {
            self.switch_tab(i);
        }
    }

    /// The list of recent files, as rows to click. Returns the one chosen.
    pub(super) fn recent_list(&self, ui: &mut egui::Ui, limit: usize) -> Option<PathBuf> {
        let mut chosen = None;
        for place in self.session.recent.iter().take(limit) {
            let folder = place.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
            let text = format!("{}  ·  page {}", place.name(), place.page + 1);
            if ui.button(text).on_hover_text(folder).clicked() {
                chosen = Some(place.path.clone());
            }
        }
        chosen
    }

    pub(super) fn ask_password(&mut self, path: PathBuf, wrong: bool) {
        self.opening = false;
        self.session.password = Some(PasswordPrompt { path, text: String::new(), wrong, focus: true });
    }

    pub(super) fn submit_password(&mut self, password: String) {
        if let Some(prompt) = self.session.password.take() {
            self.opening = true;
            self.worker.send(Request::OpenWithPassword { path: prompt.path, password });
        }
    }

    pub(super) fn password_window(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.session.password else { return };
        let name = prompt.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new("Password needed")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("{name} is protected. Type its password to open it."));
                if prompt.wrong {
                    ui.colored_label(ui.visuals().error_fg_color, "That password did not work.");
                }
                let field = ui.add(egui::TextEdit::singleline(&mut prompt.text).password(true).desired_width(280.0));
                if std::mem::take(&mut prompt.focus) {
                    field.request_focus();
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    submit |= ui.button("Open").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
                ui.small("The password is used for this session only and is not saved.");
            });
        if submit {
            let password = std::mem::take(&mut prompt.text);
            self.submit_password(password);
        } else if cancel {
            self.session.password = None;
            self.session.closing_tab = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_pages_flip_lightness_and_keep_hue() {
        let mut px = [0x00FF_FFFF, 0x0000_0000, 0x00FF_0000, 0x0080_8080];
        darken(&mut px);
        assert_eq!(px[0], 0x001E_1E1E, "white paper becomes near black");
        assert_eq!(px[1], 0x00FF_FFFF, "black text becomes white");
        assert_eq!(px[2], 0x00FF_1E1E, "pure red stays red");
        let grey = px[3] & 0xFF;
        assert!((0x80..0xA0).contains(&grey), "mid grey stays in the middle");
    }

    #[test]
    fn a_place_survives_being_written_and_read() {
        let place = Place { path: PathBuf::from("C:/x/a b.pdf"), page: 41, zoom: 1.5, fit: Fit::Custom, view: ViewMode::SpreadCover };
        let back = Place::from_json(&place.to_json()).unwrap();
        assert_eq!((back.path, back.page, back.zoom), (place.path, 41, 1.5));
        assert!(back.fit == Fit::Custom && back.view == ViewMode::SpreadCover);
    }
}
