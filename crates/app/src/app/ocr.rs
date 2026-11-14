//! "Recognize text (OCR)" dialog. Like compression, the work runs in a helper process (this
//! program started with `--ocr`), which reports progress page by page.
//!
//! Language data is not shipped with the app. It is downloaded once per language, on request,
//! into the user's app data folder, and OCR then works offline.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

use super::ReaderApp;

/// (Tesseract code, name shown). The "fast" models from the Tesseract project, a few MB each.
const LANGUAGES: [(&str, &str); 8] = [
    ("eng", "English"),
    ("deu", "German"),
    ("fra", "French"),
    ("ita", "Italian"),
    ("spa", "Spanish"),
    ("por", "Portuguese"),
    ("nld", "Dutch"),
    ("pol", "Polish"),
];
const DATA_URL: &str = "https://github.com/tesseract-ocr/tessdata_fast/raw/main";

enum Msg {
    Progress(usize, usize),
    Done { recognised: u64, with_text: u64, words: u64, output: PathBuf },
    Failed(String),
    Downloaded(Result<(), String>),
}

pub(super) struct OcrUi {
    pub open: bool,
    chosen: [bool; LANGUAGES.len()],
    running: Option<Receiver<Msg>>,
    downloading: bool,
    progress: (usize, usize),
    outcome: Option<Msg>,
}

impl Default for OcrUi {
    fn default() -> Self {
        let mut chosen = [false; LANGUAGES.len()];
        chosen[0] = true;
        Self { open: false, chosen, running: None, downloading: false, progress: (0, 0), outcome: None }
    }
}

/// Where language data lives. `ROGEHA_TESSDATA` overrides it (for people who already have
/// Tesseract language files somewhere).
fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ROGEHA_TESSDATA") {
        return PathBuf::from(dir);
    }
    super::app_data_dir().join("tessdata")
}

fn installed(code: &str) -> bool {
    data_dir().join(format!("{code}.traineddata")).is_file()
}

fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Fetch language files with the system's `curl` (part of Windows 10+, macOS and most Linux
/// systems), so the reader itself carries no networking code.
fn download(codes: Vec<&'static str>) -> Result<(), String> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    for code in codes {
        let target = dir.join(format!("{code}.traineddata"));
        let partial = dir.join(format!("{code}.traineddata.part"));
        let status = quiet(
            Command::new("curl")
                .args(["--location", "--fail", "--silent", "--show-error", "--output"])
                .arg(&partial)
                .arg(format!("{DATA_URL}/{code}.traineddata"))
                .stdin(Stdio::null()),
        )
        .status();
        let ok = matches!(status, Ok(s) if s.success())
            && std::fs::metadata(&partial).is_ok_and(|m| m.len() > 100_000);
        if !ok {
            let _ = std::fs::remove_file(&partial);
            return Err(format!(
                "Could not download the language data. Check your internet connection, or put {code}.traineddata into {}",
                dir.display()
            ));
        }
        std::fs::rename(&partial, &target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn run_helper(input: PathBuf, output: PathBuf, language: String, tx: &Sender<Msg>, wake: &dyn Fn()) -> Msg {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Msg::Failed(e.to_string()),
    };
    let mut cmd = Command::new(exe);
    cmd.arg("--ocr").arg(&input).arg(&output).arg(&language).arg(data_dir());
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = match quiet(&mut cmd).spawn() {
        Ok(c) => c,
        Err(e) => return Msg::Failed(format!("Could not start text recognition: {e}")),
    };
    let mut last = None;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if let (Some(page), Some(of)) = (v["page"].as_u64(), v["of"].as_u64()) {
                let _ = tx.send(Msg::Progress(page as usize, of as usize));
                wake();
            } else {
                last = Some(v);
            }
        }
    }
    let _ = child.wait();
    match last {
        Some(v) if v["ok"].as_bool() == Some(true) => Msg::Done {
            recognised: v["recognised"].as_u64().unwrap_or(0),
            with_text: v["with_text"].as_u64().unwrap_or(0),
            words: v["words"].as_u64().unwrap_or(0),
            output,
        },
        Some(v) => Msg::Failed(v["error"].as_str().unwrap_or("Text recognition failed").to_owned()),
        None => {
            // The helper stopped without an answer: MuPDF gave up on the file.
            let _ = std::fs::remove_file(&output);
            Msg::Failed("The text in this file could not be recognized (it may be damaged).".into())
        }
    }
}

impl ReaderApp {
    fn ocr_language(&self) -> String {
        LANGUAGES
            .iter()
            .zip(self.ocr.chosen)
            .filter(|(_, on)| *on)
            .map(|((code, _), _)| *code)
            .collect::<Vec<_>>()
            .join("+")
    }

    pub(super) fn start_ocr(&mut self, ctx: &egui::Context, output: Option<PathBuf>) {
        let Some(doc) = &self.doc else { return };
        let input = doc.path.clone();
        let output = match output {
            Some(o) => o,
            None => {
                let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save the searchable copy as")
                    .add_filter("PDF", &["pdf"])
                    .set_file_name(format!("{stem}-searchable.pdf"));
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
            self.ocr.outcome = Some(Msg::Failed("Choose a different name: the original file is always kept.".into()));
            return;
        }
        let language = self.ocr_language();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let wake = || ctx.request_repaint();
            let result = run_helper(input, output, language, &tx, &wake);
            let _ = tx.send(result);
            wake();
        });
        self.ocr.outcome = None;
        self.ocr.progress = (0, 0);
        self.ocr.running = Some(rx);
    }

    fn start_download(&mut self, ctx: &egui::Context, codes: Vec<&'static str>) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Downloaded(download(codes)));
            ctx.request_repaint();
        });
        self.ocr.outcome = None;
        self.ocr.downloading = true;
        self.ocr.running = Some(rx);
    }

    pub(super) fn ocr_window(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.ocr.running {
            let mut finished = false;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::Progress(done, total) => self.ocr.progress = (done, total),
                    Msg::Downloaded(Ok(())) => finished = true,
                    Msg::Downloaded(Err(e)) => {
                        self.ocr.outcome = Some(Msg::Failed(e));
                        finished = true;
                    }
                    other => {
                        self.ocr.outcome = Some(other);
                        finished = true;
                    }
                }
            }
            if finished {
                self.ocr.running = None;
                self.ocr.downloading = false;
            }
        }
        if !self.ocr.open {
            return;
        }
        let dirty = self.is_dirty();
        let is_pdf = self.can_edit();
        let busy = self.ocr.running.is_some();
        let mut open = true;
        let (mut start, mut fetch) = (false, false);
        let mut open_result = None;
        egui::Window::new("Recognize text (OCR)")
            .id(egui::Id::unique("ocr-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(self.layer.clip.min + egui::Vec2::new(40.0, 30.0))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label("Reads the text in scanned pages so you can search, select and copy it. The pages look exactly the same; the text is added as an invisible layer. You get a new file; the original is not changed.");
                ui.add_space(6.0);
                ui.label("Language of the document:");
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (i, (_, name)) in LANGUAGES.iter().enumerate() {
                            ui.checkbox(&mut self.ocr.chosen[i], *name);
                        }
                    });
                });
                let missing: Vec<&str> = LANGUAGES
                    .iter()
                    .zip(self.ocr.chosen)
                    .filter(|((code, _), on)| *on && !installed(code))
                    .map(|((_, name), _)| *name)
                    .collect();
                let any_chosen = self.ocr.chosen.iter().any(|c| *c);
                ui.add_space(8.0);
                if !is_pdf {
                    ui.label("Text recognition works on PDF files.");
                } else if dirty {
                    ui.label("Save your changes first, then recognize the text.");
                } else if busy && self.ocr.downloading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Downloading language data…");
                    });
                } else if busy {
                    let (done, total) = self.ocr.progress;
                    ui.horizontal(|ui| {
                        ui.spinner();
                        if total > 0 {
                            ui.label(format!("Reading page {} of {total}…", (done + 1).min(total)));
                        } else {
                            ui.label("Starting…");
                        }
                    });
                } else if !any_chosen {
                    ui.label("Choose at least one language.");
                } else if !missing.is_empty() {
                    ui.label(format!(
                        "Language data for {} is needed once (a few MB, from the Tesseract project on github.com). After that, text recognition works offline.",
                        missing.join(" and ")
                    ));
                    if ui.button("Download language data").clicked() {
                        fetch = true;
                    }
                } else if ui.button("Recognize text and save a copy…").clicked() {
                    start = true;
                }
                match &self.ocr.outcome {
                    Some(Msg::Done { recognised, with_text, words, output }) => {
                        ui.separator();
                        if *recognised > 0 {
                            ui.label(egui::RichText::new(format!("Recognized {words} words on {recognised} page(s).")).strong());
                        } else {
                            ui.label(egui::RichText::new("No pages needed text recognition.").strong());
                        }
                        if *with_text > 0 {
                            ui.label(format!("{with_text} page(s) already had text and were left as they are."));
                        }
                        ui.label(egui::RichText::new(output.display().to_string()).weak().small());
                        if ui.button("Open the searchable file").clicked() {
                            open_result = Some(output.clone());
                        }
                    }
                    Some(Msg::Failed(message)) => {
                        ui.separator();
                        ui.colored_label(ui.visuals().error_fg_color, message);
                    }
                    _ => {}
                }
            });
        if fetch {
            self.ocr_download_missing(ctx);
        }
        if start {
            self.start_ocr(ctx, None);
        }
        if let Some(path) = open_result {
            self.ocr.open = false;
            self.request_open(path);
        } else if !open {
            self.ocr.open = false;
        }
    }

    /// Download the data of every chosen language that is not installed yet.
    pub(super) fn ocr_download_missing(&mut self, ctx: &egui::Context) {
        let codes = LANGUAGES
            .iter()
            .zip(self.ocr.chosen)
            .filter(|((code, _), on)| *on && !installed(code))
            .map(|((code, _), _)| *code)
            .collect();
        self.start_download(ctx, codes);
    }

    /// For scripted checks: is OCR (or a language download) still running?
    pub(super) fn ocr_busy(&self) -> bool {
        self.ocr.running.is_some()
    }
}
