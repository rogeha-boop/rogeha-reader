// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent_host;
mod app;

use std::path::PathBuf;
use std::time::Instant;

fn main() -> eframe::Result {
    let started = Instant::now();

    let mut file: Option<PathBuf> = None;
    let mut bench = false;
    let mut dev = app::DevOptions::default();
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bench" {
            bench = true;
        } else if arg == "--screenshot" {
            // Developer option: save the window contents as a PPM image, then exit.
            dev.screenshot = args.next().map(PathBuf::from);
        } else if arg == "--script" {
            // Developer option: actions to run before the screenshot, e.g. "page=3;rotate;find=text;ai".
            dev.script = args.next().map(|s| s.to_string_lossy().into_owned());
        } else if file.is_none() {
            file = Some(PathBuf::from(arg));
        }
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Rogeha Reader")
            .with_inner_size([1100.0, 820.0])
            .with_min_inner_size([420.0, 300.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "Rogeha Reader",
        options,
        Box::new(move |cc| Ok(Box::new(app::ReaderApp::new(cc, file, started, bench, dev)))),
    )
}
