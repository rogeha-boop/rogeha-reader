//! Digital (certificate) signatures in the UI: the "Certificate" part of the Sign window
//! (choose a .p12 / .pfx file, or create a self-signed one) and the Signatures window that
//! explains the signatures found in a document.
//!
//! Passwords are typed by the user, used once and never stored.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use egui::{Color32, RichText};
use rogeha_core::{Identity, Request, SignOptions, SignatureInfo, SignatureStatus, format_time};

use super::ReaderApp;
use crate::theme;

#[derive(Default)]
pub(super) struct DigitalUi {
    /// The Signatures window.
    pub window_open: bool,
    pub list: Vec<SignatureInfo>,
    pub requested: bool,
    pub busy: bool,

    certificate: Option<PathBuf>,
    password: String,
    reason: String,
    location: String,
    message: Option<(bool, String)>, // (is error, text)

    // Creating a self-signed identity.
    creating: bool,
    new_name: String,
    new_organization: String,
    new_password: String,
    new_password_again: String,
    create_job: Option<(Receiver<Result<Vec<u8>, String>>, PathBuf)>,
}

impl DigitalUi {
    pub fn reset_for_new_document(&mut self) {
        self.list.clear();
        self.requested = false;
    }
}

impl ReaderApp {
    /// The "Certificate" tab of the Sign window.
    pub(super) fn certificate_tab(&mut self, ui: &mut egui::Ui) {
        // Finish creating an identity, if one is under way.
        if let Some((rx, path)) = &self.digital.create_job {
            if let Ok(result) = rx.try_recv() {
                let path = path.clone();
                self.digital.create_job = None;
                match result.and_then(|p12| std::fs::write(&path, p12).map_err(|e| format!("Could not save the certificate: {e}"))) {
                    Ok(()) => {
                        self.digital.certificate = Some(path);
                        self.digital.password = std::mem::take(&mut self.digital.new_password);
                        self.digital.new_password_again.clear();
                        self.digital.creating = false;
                        self.digital.message = Some((false, "Certificate created. Keep the file and its password safe: together they are your signature.".into()));
                    }
                    Err(e) => self.digital.message = Some((true, e)),
                }
            }
        }
        let t = theme::tokens(ui.visuals().dark_mode);
        ui.label("Signs the whole document with your certificate. Readers can then check who signed and that nothing was changed afterwards.");
        ui.add_space(4.0);

        if !self.digital.creating {
            ui.horizontal(|ui| {
                ui.label("Certificate file:");
                match &self.digital.certificate {
                    Some(path) => ui.label(RichText::new(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).strong()),
                    None => ui.label(RichText::new("none chosen").weak()),
                };
                if ui.button("Choose…").on_hover_text("A .p12 or .pfx file with your certificate and key").clicked() {
                    let picked = rfd::FileDialog::new()
                        .set_title("Choose your certificate file")
                        .add_filter("Certificate files", &["p12", "pfx"])
                        .pick_file();
                    if let Some(path) = picked {
                        self.digital.certificate = Some(path);
                        self.digital.password.clear();
                        self.digital.message = None;
                    }
                }
            });
            egui::Grid::new("dsign-fields").num_columns(2).show(ui, |ui| {
                ui.label("Password:");
                ui.add(egui::TextEdit::singleline(&mut self.digital.password).password(true).desired_width(260.0))
                    .on_hover_text("The password of the certificate file. It is used once and not stored.");
                ui.end_row();
                ui.label("Reason:");
                ui.add(egui::TextEdit::singleline(&mut self.digital.reason).desired_width(260.0).hint_text("optional, e.g. I approve this document"));
                ui.end_row();
                ui.label("Place:");
                ui.add(egui::TextEdit::singleline(&mut self.digital.location).desired_width(260.0).hint_text("optional"));
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ready = self.digital.certificate.is_some() && !self.digital.busy;
                let button = egui::Button::new(RichText::new("Sign and save as…").color(t.on_steel)).fill(t.steel_500);
                if ui.add_enabled(ready, button).clicked() {
                    self.sign_digitally(None);
                }
                if self.digital.busy {
                    ui.spinner();
                    ui.label("Signing…");
                }
                if ui.button("Create a certificate…").on_hover_text("Make a new self-signed certificate if you do not have one").clicked() {
                    self.digital.creating = true;
                    self.digital.message = None;
                }
            });
        } else {
            ui.label(RichText::new("New self-signed certificate").strong());
            ui.label(RichText::new("It proves a document is unchanged and was signed with this certificate. Other people's readers will show the signer as not verified until they decide to trust it. For signatures that are recognised automatically, you need a certificate from a certificate authority.").small().color(t.ink_muted));
            egui::Grid::new("dsign-create").num_columns(2).show(ui, |ui| {
                ui.label("Your name:");
                ui.add(egui::TextEdit::singleline(&mut self.digital.new_name).desired_width(260.0));
                ui.end_row();
                ui.label("Organisation:");
                ui.add(egui::TextEdit::singleline(&mut self.digital.new_organization).desired_width(260.0).hint_text("optional"));
                ui.end_row();
                ui.label("Password:");
                ui.add(egui::TextEdit::singleline(&mut self.digital.new_password).password(true).desired_width(260.0));
                ui.end_row();
                ui.label("Password again:");
                ui.add(egui::TextEdit::singleline(&mut self.digital.new_password_again).password(true).desired_width(260.0));
                ui.end_row();
            });
            let working = self.digital.create_job.is_some();
            let mismatch = self.digital.new_password != self.digital.new_password_again;
            let ready = !working && !self.digital.new_name.trim().is_empty() && !self.digital.new_password.is_empty() && !mismatch;
            if mismatch && !self.digital.new_password_again.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, "The two passwords are different.");
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, egui::Button::new("Create and save…")).clicked() {
                    let picked = rfd::FileDialog::new()
                        .set_title("Save your new certificate as")
                        .add_filter("Certificate file", &["p12"])
                        .set_file_name(format!("{}.p12", self.digital.new_name.trim()))
                        .save_file();
                    if let Some(path) = picked {
                        self.create_identity(ui.ctx(), path);
                    }
                }
                if working {
                    ui.spinner();
                    ui.label("Making the key… this takes a few seconds.");
                } else if ui.button("Back").clicked() {
                    self.digital.creating = false;
                }
            });
        }
        if let Some((is_error, text)) = &self.digital.message {
            let color = if *is_error { ui.visuals().error_fg_color } else { t.ink };
            ui.colored_label(color, text);
        }
    }

    fn create_identity(&mut self, ctx: &egui::Context, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        let (name, organization, password) =
            (self.digital.new_name.clone(), self.digital.new_organization.clone(), self.digital.new_password.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Identity::create_self_signed(&name, &organization, &password, 5));
            ctx.request_repaint();
        });
        self.digital.message = None;
        self.digital.create_job = Some((rx, path));
    }

    /// Ask where to save (unless `target` is given) and send the signing request.
    pub(super) fn sign_digitally(&mut self, target: Option<PathBuf>) {
        let (Some(doc), Some(certificate)) = (&self.doc, self.digital.certificate.clone()) else { return };
        let p12 = match std::fs::read(&certificate) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.digital.message = Some((true, format!("Could not read the certificate file: {e}")));
                return;
            }
        };
        // Check the password before asking where to save.
        if let Err(e) = Identity::from_pkcs12(&p12, &self.digital.password) {
            self.digital.message = Some((true, e));
            return;
        }
        let target = match target {
            Some(t) => t,
            None => {
                let stem = doc.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save the signed document as")
                    .add_filter("PDF", &["pdf"])
                    .set_file_name(format!("{stem}-signed.pdf"));
                if let Some(dir) = doc.path.parent() {
                    dialog = dialog.set_directory(dir);
                }
                match dialog.save_file() {
                    Some(t) => t,
                    None => return,
                }
            }
        };
        self.digital.busy = true;
        self.digital.message = None;
        self.worker.send(Request::SignDocument {
            target: Some(target),
            p12,
            // Used for this one request; the field is cleared so it does not linger.
            password: std::mem::take(&mut self.digital.password),
            options: SignOptions { reason: self.digital.reason.clone(), location: self.digital.location.clone() },
        });
    }

    /// For scripted checks: `spec` is `<certificate file>|<password>|<output file>`.
    pub(super) fn dev_sign_digitally(&mut self, spec: &str) {
        let parts: Vec<&str> = spec.split('|').collect();
        if let [certificate, password, output] = parts[..] {
            self.digital.certificate = Some(PathBuf::from(certificate));
            self.digital.password = password.to_owned();
            self.digital.reason = "Checked by script".to_owned();
            self.sign_digitally(Some(PathBuf::from(output)));
        }
    }

    pub(super) fn digital_busy(&self) -> bool {
        self.digital.busy
    }

    /// One line for the status bar, and whether it is a warning.
    pub(super) fn signature_summary(&self) -> Option<(String, bool)> {
        let list = &self.digital.list;
        let first = list.first()?;
        let broken = list.iter().filter(|s| matches!(s.status, SignatureStatus::Broken(_))).count();
        Some(if broken > 0 {
            (format!("Signature problem ({broken} of {})", list.len()), true)
        } else if list.len() == 1 {
            (format!("Signed by {}", first.signer), false)
        } else {
            (format!("{} signatures", list.len()), false)
        })
    }

    /// The window that explains each signature in plain words.
    pub(super) fn signatures_window(&mut self, ctx: &egui::Context) {
        if !self.digital.window_open {
            return;
        }
        let t = theme::tokens(ctx.theme() == egui::Theme::Dark);
        let mut open = true;
        egui::Window::new("Signatures")
            .id(egui::Id::unique("signatures-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(self.layer.clip.min + egui::Vec2::new(40.0, 30.0))
            .show(ctx, |ui| {
                ui.set_width(430.0);
                if self.digital.list.is_empty() {
                    ui.label("This document has no digital signatures.");
                }
                for (i, s) in self.digital.list.iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    let (headline, color): (String, Color32) = match &s.status {
                        SignatureStatus::Intact => (format!("Signed by {}", s.signer), t.ink),
                        SignatureStatus::Broken(_) => ("This signature is not valid".into(), ui.visuals().error_fg_color),
                        SignatureStatus::Unsupported(_) => ("This signature could not be checked".into(), ui.visuals().warn_fg_color),
                    };
                    ui.label(RichText::new(headline).strong().color(color));
                    match &s.status {
                        SignatureStatus::Intact => {
                            if s.covers_whole_file {
                                ui.label("The document has not been changed since it was signed.");
                            } else {
                                ui.label("The signed content is unchanged. Something was added to the document afterwards (for example a comment or another signature).");
                            }
                            let identity = if s.self_signed {
                                "The certificate is self-signed: anyone can make one with any name, so the signer's identity is not confirmed."
                            } else {
                                "The signer's identity was not checked against trusted certificate authorities (not available in this version)."
                            };
                            ui.label(RichText::new(identity).color(t.ink_muted));
                            if !s.certificate_valid_at_signing {
                                ui.colored_label(ui.visuals().warn_fg_color, "The certificate was outside its validity period at the signing time.");
                            }
                        }
                        SignatureStatus::Broken(why) | SignatureStatus::Unsupported(why) => {
                            ui.label(why);
                        }
                    }
                    let mut details = Vec::new();
                    if let Some(at) = s.signed_at {
                        details.push(format!("Signed {} (the signer's own clock)", format_time(at)));
                    }
                    if !s.reason.is_empty() {
                        details.push(format!("Reason: {}", s.reason));
                    }
                    if !s.location.is_empty() {
                        details.push(format!("Place: {}", s.location));
                    }
                    for line in details {
                        ui.label(RichText::new(line).small().color(t.ink_muted));
                    }
                }
            });
        if !open {
            self.digital.window_open = false;
        }
    }
}
