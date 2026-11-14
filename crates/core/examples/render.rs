//! Render one page to a PPM picture: `render <document> <page, from 1> <scale> <out.ppm>`.
//! A developer tool for looking at what the engine draws without the window around it.

use std::io::Write;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        return Err("usage: render <document> <page> <scale> <out.ppm>".into());
    }
    let engine = rogeha_core::Engine::open(std::path::Path::new(&args[1]))?;
    let page: usize = args[2].parse().map_err(|_| "page must be a number")?;
    let scale: f32 = args[3].parse().map_err(|_| "scale must be a number")?;
    let image = engine.render(page.saturating_sub(1), scale)?;
    let mut out = Vec::with_capacity(image.pixels.len() * 3 + 32);
    write!(out, "P6\n{} {}\n255\n", image.width, image.height).map_err(|e| e.to_string())?;
    for pixel in &image.pixels {
        out.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8]);
    }
    std::fs::write(&args[4], out).map_err(|e| e.to_string())
}
