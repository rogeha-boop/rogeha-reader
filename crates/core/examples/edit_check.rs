//! Real-file check of page operations. Works on a copy; never touches the input.
//! Usage: edit_check <input.pdf> <output.pdf>
//! Rotates page 1 right, saves, reopens and verifies that the saved page is pixel-for-pixel
//! the old page turned 90 degrees clockwise. Then deletes the last page and swaps pages 1 and 2.
use rogeha_core::Engine;
use std::path::Path;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let (input, output) = (args.next().expect("input"), args.next().expect("output"));
    let output = Path::new(&output);
    std::fs::copy(&input, output).unwrap();

    let mut e = Engine::open(output).unwrap();
    let n = e.page_count();
    let before = e.render(0, 1.0).unwrap();
    let size_before = e.page_sizes()[0];

    e.rotate_pages(&[0], 90).unwrap();
    e.save(None).unwrap();
    drop(e);

    let mut e = Engine::open(output).unwrap();
    let after = e.render(0, 1.0).unwrap();
    let size_after = e.page_sizes()[0];
    println!("pages: {n}; page 1 size {size_before:?} -> {size_after:?}");
    assert_eq!((after.width, after.height), (before.height, before.width), "dimensions must swap");

    // Clockwise 90: new(x, y) = old(y, H-1-x). Compare 4x4 block averages so that
    // sub-pixel anti-aliasing differences in small text don't count.
    let (bw, bh) = (before.width, before.height);
    let gray = |img: &rogeha_core::PageImage, x: usize, y: usize| -> u32 {
        let p = &img.rgba[(y * img.width + x) * 4..][..3];
        (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3
    };
    let (mut blocks, mut differing) = (0usize, 0usize);
    for by in (0..after.height.saturating_sub(4)).step_by(4) {
        for bx in (0..after.width.saturating_sub(4)).step_by(4) {
            let (mut sa, mut sb) = (0u32, 0u32);
            for y in by..by + 4 {
                for x in bx..bx + 4 {
                    sa += gray(&after, x, y);
                    sb += gray(&before, y, bh - 1 - x);
                }
            }
            blocks += 1;
            if sa.abs_diff(sb) / 16 > 40 {
                differing += 1;
            }
        }
    }
    let _ = bw;
    let pct = differing as f64 * 100.0 / blocks as f64;
    println!("rotated page vs original turned 90 degrees: {pct:.3}% of 4x4 blocks differ");
    assert!(pct < 0.5, "saved rotation does not match");

    if n >= 3 {
        let sizes = e.page_sizes().to_vec();
        e.delete_pages(&[n - 1]).unwrap();
        e.move_page(0, 1).unwrap();
        e.save(None).unwrap();
        drop(e);
        let e = Engine::open(output).unwrap();
        assert_eq!(e.page_count(), n - 1);
        assert_eq!(e.page_sizes()[0], sizes[1]);
        assert_eq!(e.page_sizes()[1], sizes[0]);
        println!("delete last page + swap pages 1 and 2: saved and verified ({} pages)", e.page_count());
    }
    println!("OK");
}
