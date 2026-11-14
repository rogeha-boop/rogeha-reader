// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent_host;
mod app;
mod runner;
mod theme;

use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let started = Instant::now();

    // Helper mode: `rogeha-reader --compress <in.pdf> <out.pdf> <preset>`.
    // The reader starts itself this way so a damaged file can only stop the helper.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--compress") {
        std::process::exit(compress_helper(&args[2..]));
    }
    // Helper mode: `rogeha-reader --ocr <in.pdf> <out.pdf> <language> <language data folder>`.
    if args.get(1).is_some_and(|a| a == "--ocr") {
        std::process::exit(ocr_helper(&args[2..]));
    }

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
        .with_drag_and_drop(true)
        // 64 x 64 RGBA, made from assets/rogeha-reader-icon.svg with
        // `cargo run --release -p rogeha-core --example svg2rgba`.
        .with_icon(egui::IconData {
            rgba: include_bytes!("../assets/icon-64.rgba").to_vec(),
            width: 64,
            height: 64,
        });

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

/// Runs one compression and reports the result as a JSON line on stdout.
fn compress_helper(args: &[std::ffi::OsString]) -> i32 {
    let result = match args {
        [input, output, preset] => {
            match rogeha_core::Preset::from_id(&preset.to_string_lossy()) {
                Some(preset) => rogeha_core::compress_pdf(input.as_ref(), output.as_ref(), preset),
                None => Err("Unknown preset (use high, balanced or small)".to_owned()),
            }
        }
        _ => Err("Usage: rogeha-reader --compress <in.pdf> <out.pdf> <high|balanced|small>".to_owned()),
    };
    match result {
        Ok(stats) => {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "before": stats.bytes_before,
                    "after": stats.bytes_after,
                    "pages": stats.pages,
                    "pages_restored": stats.pages_restored,
                    "images_untouched": stats.images_untouched,
                })
            );
            0
        }
        Err(message) => {
            println!("{}", serde_json::json!({ "ok": false, "error": message }));
            1
        }
    }
}

/// Runs OCR on one file. Prints a JSON line per page for progress, then the result.
fn ocr_helper(args: &[std::ffi::OsString]) -> i32 {
    use std::io::Write;
    let [input, output, language, data_dir] = args else {
        println!("{}", serde_json::json!({ "ok": false, "error": "Usage: rogeha-reader --ocr <in.pdf> <out.pdf> <language> <data folder>" }));
        return 1;
    };
    let result = rogeha_core::ocr_pdf(
        input.as_ref(),
        output.as_ref(),
        &language.to_string_lossy(),
        data_dir.as_ref(),
        |done, total| {
            println!("{}", serde_json::json!({ "page": done, "of": total }));
            let _ = std::io::stdout().flush();
        },
    );
    match result {
        Ok(stats) => {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "pages": stats.pages,
                    "recognised": stats.pages_recognised,
                    "with_text": stats.pages_with_text,
                    "words": stats.words,
                })
            );
            0
        }
        Err(message) => {
            println!("{}", serde_json::json!({ "ok": false, "error": message }));
            1
        }
    }
}
