//! The AI assistant panel.
//!
//! The assistant itself runs in a separate program (`rogeha-agent`), started the first time
//! this panel is opened. This side shows the conversation and the settings, and runs the
//! assistant's tools on the open document: reading pages, searching, and the actions it can
//! take (rotate, comment, highlight, sign, fill in forms).

use rogeha_core::{FieldKind, Request};
use serde_json::{Value, json};

use super::ReaderApp;
use crate::agent_host::AgentHost;
use crate::theme;

const CLAUDE_MODELS: [(&str, &str); 4] = [
    ("claude-opus-5-5", "Claude Opus 5.5 (recommended)"),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5 (faster, cheaper)"),
    ("claude-haiku-5-5", "Claude Haiku 5.5 (fastest, cheapest)"),
    ("claude-fable-5-2", "Claude Fable 5.2 (most capable, highest price)"),
];
const TRANSLATE_TO: [&str; 6] = ["English", "German", "French", "Italian", "Spanish", "Portuguese"];

enum Line {
    User(String),
    Assistant(String),
    /// What the assistant is doing ("Reading pages 1-4").
    Activity(String),
    Problem(String),
}

/// A tool call waiting for the document thread.
enum Pending {
    Pages,
    Search,
    /// An edit: resolved by the next document update (success) or error.
    Edit(String),
}

pub(super) struct AssistantUi {
    pub open: bool,
    host: Option<AgentHost>,
    start_error: Option<String>,
    status: String,
    log: Vec<Line>,
    input: String,
    busy: bool,
    pending: Option<(String, Pending)>,

    show_settings: bool,
    provider: String,
    model: String,
    base_url: String,
    effort: String,
    has_key: bool,
    key_input: String,
    /// Send this as soon as the agent is ready (scripted checks).
    pub queued: Option<String>,
}

impl Default for AssistantUi {
    fn default() -> Self {
        Self {
            open: false,
            host: None,
            start_error: None,
            status: String::new(),
            log: Vec::new(),
            input: String::new(),
            busy: false,
            pending: None,
            show_settings: false,
            provider: "anthropic".into(),
            model: CLAUDE_MODELS[0].0.into(),
            base_url: String::new(),
            effort: "medium".into(),
            has_key: false,
            key_input: String::new(),
            queued: None,
        }
    }
}

/// Split text into plain pieces and page citations like `[p. 12]` or `[p. 3-5]`.
fn citations(text: &str) -> Vec<(String, Option<usize>)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[p.") {
        let Some(len) = rest[start..].find(']') else { break };
        let inside = rest[start + 3..start + len].trim();
        let first: String = inside.chars().take_while(|c| c.is_ascii_digit()).collect();
        let valid = !first.is_empty() && inside.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '–' | ' ' | ','));
        match (valid, first.parse::<usize>()) {
            (true, Ok(page)) if page > 0 => {
                if start > 0 {
                    out.push((rest[..start].to_owned(), None));
                }
                out.push((format!("p. {inside}"), Some(page - 1)));
            }
            _ => out.push((rest[..start + len + 1].to_owned(), None)),
        }
        rest = &rest[start + len + 1..];
    }
    if !rest.is_empty() {
        out.push((rest.to_owned(), None));
    }
    out
}

fn page_list(input: &Value, count: usize) -> Result<Vec<usize>, String> {
    let pages: Vec<usize> = input["pages"].as_array().into_iter().flatten().filter_map(|p| p.as_u64()).map(|p| p as usize).collect();
    match pages.iter().find(|p| **p == 0 || **p > count) {
        Some(bad) => Err(format!("Page {bad} does not exist; the document has {count} pages.")),
        None if pages.is_empty() => Err("No pages were given.".into()),
        None => Ok(pages.into_iter().map(|p| p - 1).collect()),
    }
}

fn numbers(pages: &[usize]) -> String {
    pages.iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(", ")
}

impl ReaderApp {
    fn agent_send(&mut self, message: Value) {
        if let Some(host) = &mut self.assistant.host {
            if let Err(e) = host.send_line(&message.to_string()) {
                self.assistant.log.push(Line::Problem(format!("The assistant stopped: {e}")));
                self.assistant.host = None;
                self.assistant.busy = false;
            }
        }
    }

    fn tool_result(&mut self, id: &str, content: impl Into<String>, is_error: bool) {
        self.agent_send(json!({ "type": "tool_result", "id": id, "content": content.into(), "is_error": is_error }));
    }

    /// What the assistant is told about the document with every question.
    fn assistant_context(&self) -> String {
        let Some(doc) = &self.doc else { return "[No document is open]".into() };
        let name = doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut context = format!("[Document: {name}, {} pages, the user is on page {}]", doc.page_sizes.len(), self.current_page + 1);
        if let Some(sel) = self.comments.selection.as_ref().filter(|s| !s.text.is_empty()) {
            context.push_str(&format!("\n[Text the user has selected on page {}: \"{}\"]", sel.page + 1, sel.text));
        }
        context
    }

    pub(super) fn assistant_ask(&mut self, text: String) {
        if text.trim().is_empty() || self.assistant.busy || self.assistant.host.is_none() {
            return;
        }
        let context = self.assistant_context();
        self.assistant.log.push(Line::User(text.clone()));
        self.assistant.busy = true;
        self.agent_send(json!({ "type": "ask", "text": text, "context": context }));
    }

    /// Stop the answer in progress. The agent is simply restarted; it starts in milliseconds.
    fn assistant_stop(&mut self) {
        self.assistant.host = None;
        self.assistant.busy = false;
        self.assistant.pending = None;
        self.assistant.log.push(Line::Activity("Stopped. The conversation starts fresh.".into()));
    }

    // ---------------------------------------------------------------- the tools

    fn run_tool(&mut self, id: String, name: &str, input: &Value) {
        let Some(doc) = &self.doc else {
            return self.tool_result(&id, "No document is open.", true);
        };
        let count = doc.page_sizes.len();
        let can_edit = doc.is_pdf;
        let page_arg = || match input["page"].as_u64() {
            Some(p) if p >= 1 && p as usize <= count => Ok(p as usize - 1),
            _ => Err(format!("That page does not exist; the document has {count} pages.")),
        };
        let author = self.comments.author.clone();
        let not_pdf = "This document is not a PDF, so it cannot be changed.";
        let activity = |app: &mut Self, text: String| app.assistant.log.push(Line::Activity(text));

        match name {
            "get_document_info" => {
                let landscape: Vec<usize> = doc.page_sizes.iter().enumerate().filter(|(_, s)| s.0 > s.1 * 1.05).map(|(i, _)| i).collect();
                let name = doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let info = json!({
                    "file": name,
                    "kind": if doc.is_pdf { "PDF" } else { "not a PDF (read-only)" },
                    "pages": count,
                    "current_page": self.current_page + 1,
                    "landscape_pages": landscape.iter().map(|p| p + 1).collect::<Vec<_>>(),
                    "comments": self.comments.list.len(),
                    "form_fields": self.forms.list.len(),
                    "digital_signatures": self.digital.list.len(),
                    "unsaved_changes": doc.dirty,
                });
                self.tool_result(&id, info.to_string(), false);
            }
            "get_pages" => {
                let (from, to) = (input["from"].as_u64().unwrap_or(0) as usize, input["to"].as_u64().unwrap_or(0) as usize);
                if from == 0 || to < from || to > count {
                    return self.tool_result(&id, format!("Pages {from}-{to} are not in this document; it has {count} pages."), true);
                }
                activity(self, if from == to { format!("Reading page {from}") } else { format!("Reading pages {from}-{to}") });
                self.worker.send(Request::PagesText { from: from - 1, to: to - 1 });
                self.assistant.pending = Some((id, Pending::Pages));
            }
            "search" => {
                let query = input["query"].as_str().unwrap_or_default().trim().to_owned();
                if query.is_empty() {
                    return self.tool_result(&id, "Nothing to search for.", true);
                }
                activity(self, format!("Searching for \"{query}\""));
                self.search.query = query.clone();
                self.search.busy = true;
                self.worker.send(Request::Search { query });
                self.assistant.pending = Some((id, Pending::Search));
            }
            "go_to_page" => match page_arg() {
                Ok(page) => {
                    self.go_to_page(page);
                    self.tool_result(&id, format!("The reader now shows page {}.", page + 1), false);
                }
                Err(e) => self.tool_result(&id, e, true),
            },
            _ if !can_edit => self.tool_result(&id, not_pdf, true),
            "rotate_pages" => {
                let degrees = input["degrees"].as_i64().unwrap_or(0);
                match page_list(input, count) {
                    _ if ![90, 180, 270].contains(&degrees) => self.tool_result(&id, "degrees must be 90, 180 or 270.", true),
                    Err(e) => self.tool_result(&id, e, true),
                    Ok(pages) => {
                        let done = format!("Rotated page(s) {} by {degrees} degrees. Not saved yet.", numbers(&pages));
                        activity(self, format!("Rotating page(s) {}", numbers(&pages)));
                        self.worker.send(Request::Rotate { pages, delta: degrees as i32 });
                        self.assistant.pending = Some((id, Pending::Edit(done)));
                    }
                }
            }
            "delete_pages" => match page_list(input, count) {
                Err(e) => self.tool_result(&id, e, true),
                Ok(pages) => {
                    // Deleting is the one action the user must approve each time.
                    let answer = rfd::MessageDialog::new()
                        .set_title("Delete pages")
                        .set_description(format!(
                            "The assistant wants to delete page(s) {}.\nThe file changes only when you save. Allow it?",
                            numbers(&pages)
                        ))
                        .set_buttons(rfd::MessageButtons::YesNo)
                        .show();
                    if matches!(answer, rfd::MessageDialogResult::Yes) {
                        let done = format!("Deleted page(s) {}. Not saved yet.", numbers(&pages));
                        activity(self, format!("Deleting page(s) {}", numbers(&pages)));
                        self.worker.send(Request::Delete { pages });
                        self.assistant.pending = Some((id, Pending::Edit(done)));
                    } else {
                        self.tool_result(&id, "The user did not allow deleting these pages.", false);
                    }
                }
            },
            "add_note" => match page_arg() {
                Err(e) => self.tool_result(&id, e, true),
                Ok(page) => {
                    let contents = input["text"].as_str().unwrap_or_default().to_owned();
                    activity(self, format!("Adding a note on page {}", page + 1));
                    self.worker.send(Request::AddNote { page, at: (36.0, 36.0), contents, author });
                    self.assistant.pending = Some((id, Pending::Edit(format!("Added a note on page {}.", page + 1))));
                }
            },
            "highlight_text" => match page_arg() {
                Err(e) => self.tool_result(&id, e, true),
                Ok(page) => {
                    let text = input["text"].as_str().unwrap_or_default().to_owned();
                    let comment = input["comment"].as_str().unwrap_or_default().to_owned();
                    activity(self, format!("Highlighting \"{text}\" on page {}", page + 1));
                    let done = format!("Highlighted \"{text}\" on page {}.", page + 1);
                    self.worker.send(Request::HighlightText { page, text, author, comment });
                    self.assistant.pending = Some((id, Pending::Edit(done)));
                }
            },
            "place_signature" => match (page_arg(), self.first_saved_signature()) {
                (Err(e), _) => self.tool_result(&id, e, true),
                (_, None) => self.tool_result(&id, "The user has no saved signature yet. Ask them to create one with the Sign button (with \"Remember this signature\" ticked).", true),
                (Ok(page), Some((art, aspect, ink))) => {
                    let (pw, ph) = self.doc.as_ref().map(|d| d.page_sizes[page]).unwrap_or((595.0, 842.0));
                    let w = 150f32.min(pw * 0.4);
                    let h = w / aspect;
                    let x = match input["position"].as_str().unwrap_or("bottom-right") {
                        "bottom-left" => 50.0,
                        "bottom-center" => (pw - w) / 2.0,
                        _ => pw - w - 50.0,
                    };
                    let y = ph - h - 60.0;
                    activity(self, format!("Placing your signature on page {}", page + 1));
                    self.worker.send(Request::StampSignature { page, rect: (x, y, x + w, y + h), art, ink });
                    self.assistant.pending = Some((id, Pending::Edit(format!("Placed the saved signature on page {}. Not saved yet.", page + 1))));
                }
            },
            "get_form_fields" => {
                let fields: Vec<Value> = self
                    .forms
                    .list
                    .iter()
                    .filter(|f| !f.read_only && f.kind != FieldKind::Other)
                    .map(|f| {
                        let kind = match f.kind {
                            FieldKind::Text { .. } => "text",
                            FieldKind::CheckBox => "checkbox",
                            FieldKind::Radio => "radio button",
                            FieldKind::Dropdown | FieldKind::List { .. } => "choice",
                            FieldKind::Other => "other",
                        };
                        let value = match f.kind {
                            FieldKind::CheckBox | FieldKind::Radio => if f.checked { "on" } else { "off" }.to_owned(),
                            _ => f.value.clone(),
                        };
                        json!({ "name": f.name, "page": f.page + 1, "kind": kind, "description": f.label, "value": value,
                                "choices": f.options.iter().map(|o| o.0.clone()).collect::<Vec<_>>() })
                    })
                    .collect();
                let text = if fields.is_empty() { "This document has no fillable form fields.".to_owned() } else { Value::Array(fields).to_string() };
                self.tool_result(&id, text, false);
            }
            "fill_form_field" => {
                let (wanted, value) = (input["name"].as_str().unwrap_or_default(), input["value"].as_str().unwrap_or_default().to_owned());
                let field = self.forms.list.iter().find(|f| f.name == wanted && !f.read_only).cloned();
                match field {
                    None => self.tool_result(&id, format!("There is no fillable field named \"{wanted}\". Use get_form_fields for the names."), true),
                    Some(f) => {
                        activity(self, format!("Filling in \"{}\"", if f.label.is_empty() { &f.name } else { &f.label }));
                        let request = match f.kind {
                            FieldKind::CheckBox | FieldKind::Radio => {
                                Request::SetChecked { page: f.page, xref: f.xref, checked: !matches!(value.to_lowercase().as_str(), "off" | "no" | "false" | "") }
                            }
                            _ => Request::SetField { page: f.page, xref: f.xref, value: value.clone() },
                        };
                        self.worker.send(request);
                        self.assistant.pending = Some((id, Pending::Edit(format!("Set \"{wanted}\" to \"{value}\"."))));
                    }
                }
            }
            other => self.tool_result(&id, format!("The reader has no tool called {other}."), true),
        }
    }

    // Results from the document thread that a pending tool call is waiting for.

    pub(super) fn assistant_pages_text(&mut self, result: Result<String, String>) {
        if let Some((id, Pending::Pages)) = self.assistant.pending.take() {
            match result {
                Ok(text) => self.tool_result(&id, text, false),
                Err(e) => self.tool_result(&id, e, true),
            }
        }
    }

    pub(super) fn assistant_search_done(&mut self) {
        if !matches!(self.assistant.pending, Some((_, Pending::Search))) {
            return;
        }
        let Some((id, _)) = self.assistant.pending.take() else { return };
        let mut per_page: Vec<(usize, usize)> = Vec::new();
        for (page, _) in &self.search.hits {
            match per_page.last_mut() {
                Some((p, n)) if p == page => *n += 1,
                _ => per_page.push((*page, 1)),
            }
        }
        let text = if per_page.is_empty() {
            "No matches in the document's text.".to_owned()
        } else {
            let pages: Vec<String> = per_page.iter().take(200).map(|(p, n)| format!("page {}: {n}", p + 1)).collect();
            format!("{} matches. Matches per page: {}", self.search.hits.len(), pages.join("; "))
        };
        self.tool_result(&id, text, false);
    }

    /// The document changed (`Ok`) or an operation failed (`Err`).
    pub(super) fn assistant_edit_outcome(&mut self, outcome: Result<(), &str>) {
        if !matches!(self.assistant.pending, Some((_, Pending::Edit(_)))) {
            return;
        }
        if let Some((id, Pending::Edit(done))) = self.assistant.pending.take() {
            match outcome {
                Ok(()) => self.tool_result(&id, done, false),
                Err(e) => self.tool_result(&id, e.to_owned(), true),
            }
        }
    }

    /// For scripted checks: is a question being answered (or about to be sent)?
    pub(super) fn assistant_busy(&self) -> bool {
        self.assistant.busy || self.assistant.queued.is_some()
    }

    // --------------------------------------------------------------------- UI

    fn assistant_messages(&mut self) {
        let lines: Vec<String> = match &self.assistant.host {
            Some(host) => std::iter::from_fn(|| host.lines.try_recv().ok()).collect(),
            None => return,
        };
        for line in lines {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let text = v["text"].as_str().unwrap_or_default().to_owned();
            match v["type"].as_str() {
                Some("ready") => {
                    self.assistant.status = text;
                    let field = |k: &str, current: &str| v[k].as_str().unwrap_or(current).to_owned();
                    self.assistant.provider = field("provider", &self.assistant.provider);
                    self.assistant.model = field("model", &self.assistant.model);
                    self.assistant.base_url = field("base_url", &self.assistant.base_url);
                    self.assistant.effort = field("effort", &self.assistant.effort);
                    self.assistant.has_key = v["has_key"].as_bool().unwrap_or(false);
                    let needs_key = self.assistant.provider == "anthropic" || self.assistant.base_url.trim().is_empty();
                    if needs_key && !self.assistant.has_key {
                        self.assistant.show_settings = true;
                    }
                    if let Some(question) = self.assistant.queued.take() {
                        self.assistant_ask(question);
                    }
                }
                Some("delta") => match self.assistant.log.last_mut() {
                    Some(Line::Assistant(answer)) => answer.push_str(&text),
                    _ if text.trim().is_empty() => {}
                    _ => self.assistant.log.push(Line::Assistant(text)),
                },
                Some("tool_call") => {
                    let id = v["id"].as_str().unwrap_or_default().to_owned();
                    self.run_tool(id, v["name"].as_str().unwrap_or_default(), &v["input"]);
                }
                Some("done") => self.assistant.busy = false,
                Some("error") => {
                    self.assistant.busy = false;
                    self.assistant.pending = None;
                    self.assistant.log.push(Line::Problem(text));
                }
                _ => {}
            }
        }
    }

    pub(super) fn assistant_panel(&mut self, ui: &mut egui::Ui) {
        // First time the panel is shown (or after Stop): start the agent program.
        if self.assistant.host.is_none() && self.assistant.start_error.is_none() {
            let ctx = ui.ctx().clone();
            match AgentHost::start(move || ctx.request_repaint()) {
                Ok(host) => {
                    self.assistant.host = Some(host);
                    self.assistant.status = "Starting…".into();
                    self.agent_send(json!({ "type": "hello" }));
                }
                Err(e) => self.assistant.start_error = Some(e),
            }
        }
        self.assistant_messages();
        let t = theme::tokens(ui.visuals().dark_mode);

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("Assistant");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.assistant.show_settings, "Settings");
                if ui.add_enabled(!self.assistant.busy && !self.assistant.log.is_empty(), egui::Button::new("New chat")).clicked() {
                    self.assistant.log.clear();
                    self.agent_send(json!({ "type": "reset" }));
                }
            });
        });
        if let Some(e) = &self.assistant.start_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return;
        }
        ui.label(egui::RichText::new(&self.assistant.status).small().color(t.ink_muted));
        if self.assistant.show_settings {
            self.assistant_settings(ui);
        }
        ui.separator();

        // The input area is laid out first so it stays at the bottom.
        let mut ask: Option<String> = None;
        let mut go_to: Option<usize> = None;
        egui::Panel::bottom("assistant-input").show(ui, |ui| {
            ui.add_space(4.0);
            let ready = !self.assistant.busy && self.doc.is_some();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(ready, egui::Button::new("Summarize")).clicked() {
                    ask = Some("Summarize this document.".into());
                }
                if ui.add_enabled(ready, egui::Button::new("Explain this page")).clicked() {
                    ask = Some("Explain the page I am looking at in simple terms.".into());
                }
                ui.add_enabled_ui(ready, |ui| {
                    ui.menu_button("Translate page", |ui| {
                        for language in TRANSLATE_TO {
                            if ui.button(language).clicked() {
                                ask = Some(format!("Translate the page I am looking at into {language}. Give only the translation."));
                                ui.close();
                            }
                        }
                    });
                });
            });
            ui.horizontal(|ui| {
                let resp = ui.add_enabled(
                    !self.assistant.busy,
                    egui::TextEdit::multiline(&mut self.assistant.input)
                        .desired_rows(2)
                        .desired_width(ui.available_width() - 64.0)
                        .hint_text("Ask about this document, or tell me what to do with it"),
                );
                let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                if self.assistant.busy {
                    if ui.button("Stop").clicked() {
                        self.assistant_stop();
                    }
                } else if (ui.button("Send").clicked() || enter) && !self.assistant.input.trim().is_empty() {
                    ask = Some(std::mem::take(&mut self.assistant.input).trim().to_owned());
                }
            });
            ui.label(egui::RichText::new("Pages the assistant reads are sent to the provider you chose.").small().color(t.ink_muted));
            ui.add_space(2.0);
        });

        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            if self.assistant.log.is_empty() {
                ui.label(egui::RichText::new("Ask a question about the document, or ask for something to be done: \"rotate all landscape pages\", \"highlight every mention of the deadline\", \"fill in my name\".").color(t.ink_muted));
            }
            for line in &self.assistant.log {
                match line {
                    Line::User(text) => {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("You").small().strong().color(t.ink_muted));
                        ui.label(text);
                    }
                    Line::Assistant(text) => {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("Assistant").small().strong().color(t.ink_muted));
                        for paragraph in text.split('\n').filter(|p| !p.trim().is_empty()) {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (piece, page) in citations(paragraph) {
                                    match page {
                                        Some(page) => {
                                            if ui.link(format!("[{piece}]")).on_hover_text("Go to this page").clicked() {
                                                go_to = Some(page);
                                            }
                                        }
                                        None => {
                                            ui.label(piece);
                                        }
                                    }
                                }
                            });
                        }
                    }
                    Line::Activity(text) => {
                        ui.label(egui::RichText::new(format!("· {text}")).small().color(t.ink_muted));
                    }
                    Line::Problem(text) => {
                        ui.add_space(4.0);
                        ui.colored_label(ui.visuals().error_fg_color, text);
                    }
                }
            }
            if self.assistant.busy {
                ui.add_space(4.0);
                ui.spinner();
            }
        });

        if let Some(page) = go_to {
            self.go_to_page(page);
        }
        if let Some(question) = ask {
            self.assistant_ask(question);
        }
    }

    fn assistant_settings(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.visuals().dark_mode);
        let mut apply = false;
        let mut remove_key = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.assistant.provider, "anthropic".to_owned(), "Claude (Anthropic)");
                ui.radio_value(&mut self.assistant.provider, "openai".to_owned(), "OpenAI-compatible or local");
            });
            if self.assistant.provider == "anthropic" {
                let shown = CLAUDE_MODELS.iter().find(|m| m.0 == self.assistant.model).map_or(self.assistant.model.as_str(), |m| m.1).to_owned();
                egui::ComboBox::from_label("Model").selected_text(shown).show_ui(ui, |ui| {
                    for (id, name) in CLAUDE_MODELS {
                        ui.selectable_value(&mut self.assistant.model, id.to_owned(), name);
                    }
                });
                egui::ComboBox::from_label("Thoroughness").selected_text(self.assistant.effort.clone()).show_ui(ui, |ui| {
                    for effort in ["low", "medium", "high"] {
                        ui.selectable_value(&mut self.assistant.effort, effort.to_owned(), effort);
                    }
                });
                ui.label(egui::RichText::new("You need your own API key from console.anthropic.com. Usage is billed to your Anthropic account.").small().color(t.ink_muted));
            } else {
                ui.horizontal(|ui| {
                    ui.label("Address:");
                    ui.add(egui::TextEdit::singleline(&mut self.assistant.base_url).desired_width(220.0).hint_text("https://api.openai.com/v1"));
                });
                ui.horizontal_wrapped(|ui| {
                    for (name, url) in [("OpenAI", "https://api.openai.com/v1"), ("OpenRouter", "https://openrouter.ai/api/v1"), ("Ollama (local)", "http://localhost:11434/v1"), ("LM Studio (local)", "http://localhost:1234/v1")] {
                        if ui.small_button(name).clicked() {
                            self.assistant.base_url = url.to_owned();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Model:");
                    ui.add(egui::TextEdit::singleline(&mut self.assistant.model).desired_width(220.0).hint_text("the model's name at that provider"));
                });
                ui.label(egui::RichText::new("The model must support tool (function) calling. Local servers usually need no key.").small().color(t.ink_muted));
            }
            ui.horizontal(|ui| {
                ui.label("API key:");
                let hint = if self.assistant.has_key { "a key is saved; type to replace it" } else { "paste your key" };
                ui.add(egui::TextEdit::singleline(&mut self.assistant.key_input).password(true).desired_width(200.0).hint_text(hint));
            });
            ui.label(egui::RichText::new("The key is kept in the Windows Credential Manager, not in a file.").small().color(t.ink_muted));
            ui.horizontal(|ui| {
                let button = egui::Button::new(egui::RichText::new("Save settings").color(t.on_steel)).fill(t.steel_500);
                if ui.add_enabled(!self.assistant.busy, button).clicked() {
                    apply = true;
                }
                if self.assistant.has_key && ui.add_enabled(!self.assistant.busy, egui::Button::new("Remove key")).clicked() {
                    remove_key = true;
                }
            });
        });
        if apply || remove_key {
            let mut message = json!({
                "type": "configure",
                "provider": self.assistant.provider,
                "model": self.assistant.model,
                "base_url": self.assistant.base_url,
                "effort": self.assistant.effort,
            });
            // The key is sent only when the user typed one (or asked to remove it), then forgotten here.
            let key = std::mem::take(&mut self.assistant.key_input);
            if remove_key {
                message["api_key"] = json!("");
            } else if !key.trim().is_empty() {
                message["api_key"] = json!(key.trim());
            }
            self.assistant.log.clear();
            self.agent_send(message);
            if apply {
                self.assistant.show_settings = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::citations;

    #[test]
    fn citations_become_links() {
        let parts = citations("It starts here [p. 1] and goes on [p. 3-5]. See [note] and [p. x].");
        let links: Vec<(String, usize)> = parts.iter().filter_map(|(t, p)| p.map(|p| (t.clone(), p))).collect();
        assert_eq!(links, vec![("p. 1".to_owned(), 0), ("p. 3-5".to_owned(), 2)]);
        let plain: String = parts.iter().filter(|(_, p)| p.is_none()).map(|(t, _)| t.as_str()).collect();
        assert_eq!(plain, "It starts here  and goes on . See [note] and [p. x].");
        assert_eq!(citations("no pages here"), vec![("no pages here".to_owned(), None)]);
    }
}
