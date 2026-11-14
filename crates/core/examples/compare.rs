//! Compare two versions of a document page by page (e.g. before/after compression).
//! Usage: compare <a.pdf> <b.pdf> [dpi]
//! Prints the PSNR of each page rendered at `dpi` (default 110, about what a screen shows).
//! Above ~35 dB differences are hard to see; above ~40 dB they are practically invisible.
use rogeha_core::Engine;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let (a, b) = (args.next().expect("a"), args.next().expect("b"));
    let dpi: f32 = args.next().and_then(|d| d.to_string_lossy().parse().ok()).unwrap_or(110.0);
    let (a, b) = (Engine::open(a.as_ref()).unwrap(), Engine::open(b.as_ref()).unwrap());
    assert_eq!(a.page_count(), b.page_count(), "page counts differ");
    let mut worst = f64::MAX;
    for page in 0..a.page_count() {
        let (ia, ib) = (a.render(page, dpi / 72.0).unwrap(), b.render(page, dpi / 72.0).unwrap());
        assert_eq!((ia.width, ia.height), (ib.width, ib.height), "page {} size differs", page + 1);
        let mut sum = 0f64;
        for (p, q) in ia.pixels.iter().zip(&ib.pixels) {
            for shift in [16, 8, 0] {
                let d = ((p >> shift) & 255) as f64 - ((q >> shift) & 255) as f64;
                sum += d * d;
            }
        }
        let mse = sum / (ia.pixels.len() * 3) as f64;
        let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
        worst = worst.min(psnr);
        println!("page {:>3}: {psnr:5.1} dB", page + 1);
    }
    println!("worst page: {worst:.1} dB at {dpi} dpi");
}
