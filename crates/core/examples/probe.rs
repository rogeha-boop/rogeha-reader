//! Command-line probe: open a document, print page count and timings.
//! Usage: cargo run --release -p rogeha-core --example probe -- file.pdf
use std::time::Instant;

fn main() {
    let path = std::env::args_os().nth(1).expect("usage: probe <file>");
    let t = Instant::now();
    let engine = match rogeha_core::Engine::open(path.as_ref()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("open failed after {:?}: {e}", t.elapsed());
            std::process::exit(1);
        }
    };
    println!("open: {:?}, pages: {}, pdf: {}", t.elapsed(), engine.page_count(), engine.is_pdf());
    let t = Instant::now();
    match engine.render(0, 2.0) {
        Ok(img) => println!("render p1 @2x: {:?} ({}x{})", t.elapsed(), img.width, img.height),
        Err(e) => println!("render failed: {e}"),
    }
    let text = engine.page_text(0).unwrap_or_default();
    println!("text p1: {:?}", text.chars().take(80).collect::<String>());
}
