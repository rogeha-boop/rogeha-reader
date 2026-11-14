//! Render a square SVG to raw RGBA bytes (used to make the window icon).
//! Usage: svg2rgba <in.svg> <size> <out.rgba>
//! Pixels outside the icon's rounded tile are made transparent.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let size: usize = a[2].parse().unwrap();
    let img = rogeha_core::render_svg(&std::fs::read(&a[1]).unwrap(), size).unwrap();
    let r = size as f32 * 28.0 / 128.0; // the tile's corner radius
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let p = img.pixels[y * img.width + x];
            // Coverage of the rounded square at this pixel (simple anti-aliasing).
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = (r - fx).max(fx - (size as f32 - r)).max(0.0);
            let dy = (r - fy).max(fy - (size as f32 - r)).max(0.0);
            let alpha = ((r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            out.extend([(p >> 16) as u8, (p >> 8) as u8, p as u8, alpha]);
        }
    }
    std::fs::write(&a[3], out).unwrap();
}
