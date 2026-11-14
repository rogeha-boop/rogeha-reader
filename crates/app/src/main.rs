// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent_host;
mod app;
mod runner;

use std::path::PathBuf;
use std::time::Instant;

fn main() {
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

    let viewport = egui::ViewportBuilder::default()
        .with_title("Rogeha Reader")
        .with_inner_size([1100.0, 820.0])
        .with_min_inner_size([420.0, 300.0])
        .with_drag_and_drop(true);

    let result = runner::run(viewport, move |ctx| {
        app::ReaderApp::new(ctx, file, started, bench, dev)
    });
    if let Err(message) = result {
        rfd::MessageDialog::new()
            .set_title("Rogeha Reader")
            .set_description(format!("Rogeha Reader could not start:
{message}"))
            .show();
        std::process::exit(1);
    }
}
