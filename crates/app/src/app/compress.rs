//! "Compress PDF" dialog. The work runs in a helper process (this same program started with
//! `--compress`), so the reader stays responsive and a damaged file cannot take it down.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use rogeha_core::Preset;

use super::ReaderApp;

pub(super) enum Outcome {
    Done { before: u64, after: u64, restored: u64, images_untouched: bool, pictures: u64, output: PathBuf },
    Failed(String),
}

pub(super) struct CompressUi {
    pub open: bool,
    preset: Preset,
    running: Option<(Receiver<Outcome>, PathBuf)>,
    outcome: Option<Outcome>,
}

impl Default for CompressUi {
    fn default() -> Self {
        Self { open: false, preset: Preset::Balanced, running: None, outcome: None }
    }
}

fn megabytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        format!("{:.0} KB", bytes as f64 / 1e3)
    }
}

fn run_helper(input: PathBuf, output: PathBuf, preset: Preset) -> Outcome {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let mut cmd = Command::new(exe);
    cmd.arg("--compress").arg(&input).arg(&output).arg(preset.id());
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    crate::agent_host::keep_files_out_of_helpers();
    let result = match cmd.output() {
        Ok(out) => out,
        Err(e) => return Outcome::Failed(format!("Could not start the compressor: {e}")),
    };
    let line = String::from_utf8_lossy(&result.stdout);
    let Some(v) = line.lines().last().and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok()) else {
        // The helper stopped without an answer: MuPDF gave up on the file.
        let _ = std::fs::remove_file(&output);
        return Outcome::Failed("This file could not be compressed (it may be damaged).".into());
    };
    if v["ok"].as_bool() != Some(true) {
        return Outcome::Failed(v["error"].as_str().unwrap_or("Compression failed").to_owned());
    }
    Outcome::Done {
        before: v["before"].as_u64().unwrap_or(0),
        after: v["after"].as_u64().unwrap_or(0),
        restored: v["pages_restored"].as_u64().unwrap_or(0),
        images_untouched: v["images_untouched"].as_bool().unwrap_or(false),
        pictures: v["picture_bytes"].as_u64().unwrap_or(0),
        output,
    }
}

impl ReaderApp {
    pub(super) fn start_compress(&mut self, ctx: &egui::Context, output: Option<PathBuf>) {
        let Some(doc) = &self.doc else { return };
        let input = doc.path.clone();
        let output = match output {
            Some(o) => o,
            None => {
                let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save the compressed copy as")
                    .add_filter("PDF", &["pdf"])
                    .set_file_name(format!("{stem}-compressed.pdf"));
                if let Some(dir) = input.parent() {
                    dialog = dialog.set_directory(dir);
                }
                match dialog.save_file() {
                    Some(o) => o,
                    None => return,
                }
            }
        };
        if output == input {
            self.compress.outcome = Some(Outcome::Failed(
                "Choose a different name: the original file is always kept.".into(),
            ));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let preset = self.compress.preset;
        let ctx = ctx.clone();
        let out = output.clone();
        std::thread::spawn(move || {
            let _ = tx.send(run_helper(input, out, preset));
            ctx.request_repaint();
        });
        self.compress.outcome = None;
        self.compress.running = Some((rx, output));
    }

    pub(super) fn compress_window(&mut self, ctx: &egui::Context) {
        if let Some((rx, _)) = &self.compress.running {
            if let Ok(outcome) = rx.try_recv() {
                self.compress.outcome = Some(outcome);
                self.compress.running = None;
            }
        }
        if !self.compress.open {
            return;
        }
        let dirty = self.is_dirty();
        let is_pdf = self.can_edit();
        let size = self.doc.as_ref().and_then(|d| std::fs::metadata(&d.path).ok()).map(|m| m.len());
        let mut open = true;
        let mut start = false;
        let mut open_result = None;
        egui::Window::new("Compress PDF")
            .id(egui::Id::unique("compress-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(self.layer.clip.min + egui::Vec2::new(40.0, 30.0))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label("Makes a PDF smaller: pictures are stored more efficiently, and data that no reader shows is taken out. Text, comments and black-and-white pages are kept exactly. You get a new file; the original is not changed.");
                ui.label(egui::RichText::new("The three settings differ only in how pictures are stored. A file that is mostly text and drawings comes out about the same size with each.").weak().small());
                if let Some(size) = size {
                    ui.label(egui::RichText::new(format!("Current size: {}", megabytes(size))).weak());
                }
                ui.add_space(6.0);
                let busy = self.compress.running.is_some();
                ui.add_enabled_ui(!busy, |ui| {
                    for preset in Preset::ALL {
                        ui.radio_value(&mut self.compress.preset, preset, preset.label());
                        ui.label(egui::RichText::new(format!("      {}", preset.description())).weak().small());
                    }
                });
                ui.add_space(8.0);
                if !is_pdf {
                    ui.label("Only PDF files can be compressed.");
                } else if dirty {
                    ui.label("Save your changes first, then compress.");
                } else if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Compressing… large scans can take a minute.");
                    });
                } else if ui.button("Compress and save a copy…").clicked() {
                    start = true;
                }
                match &self.compress.outcome {
                    Some(Outcome::Done { before, after, restored, images_untouched, pictures, output }) => {
                        ui.separator();
                        let saved = if *before > 0 { 100.0 - *after as f64 * 100.0 / *before as f64 } else { 0.0 };
                        if saved >= 1.0 {
                            ui.label(egui::RichText::new(format!(
                                "{} to {}  ({saved:.0}% smaller)",
                                megabytes(*before),
                                megabytes(*after)
                            )).strong());
                        } else {
                            ui.label(egui::RichText::new("This file is already compact; the copy is not smaller.").strong());
                        }
                        // Say why the setting made little difference, when it did.
                        let share = if *before > 0 { *pictures as f64 * 100.0 / *before as f64 } else { 0.0 };
                        if share < 25.0 {
                            ui.label(format!(
                                "Pictures were only {share:.0}% of this file; the rest is text, drawings and fonts. The other settings would give about the same size."
                            ));
                        }
                        if *images_untouched {
                            ui.label("To be safe, the images were left as they are: re-encoding them would have changed other content in this file.");
                        } else if *restored > 0 {
                            ui.label(format!("{restored} page(s) were kept exactly as in the original to protect their text."));
                        }
                        ui.label(egui::RichText::new(output.display().to_string()).weak().small());
                        if ui.button("Open the compressed file").clicked() {
                            open_result = Some(output.clone());
                        }
                    }
                    Some(Outcome::Failed(message)) => {
                        ui.separator();
                        ui.colored_label(ui.visuals().error_fg_color, message);
                    }
                    None => {}
                }
            });
        if start {
            self.start_compress(ctx, None);
        }
        if let Some(path) = open_result {
            self.compress.open = false;
            self.request_open(path);
        } else if !open {
            self.compress.open = false;
        }
    }

    /// For scripted checks: is a compression still running?
    pub(super) fn compress_busy(&self) -> bool {
        self.compress.running.is_some()
    }
}
