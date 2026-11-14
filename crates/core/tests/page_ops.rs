//! End-to-end tests of the engine on PDFs generated with MuPDF itself.

use std::path::{Path, PathBuf};

use mupdf::Size;
use mupdf::pdf::PdfDocument;
use rogeha_core::{FieldKind, Identity, SignOptions, SignatureStatus};
use rogeha_core::{AnnotKind, Engine, INK_BLUE, MarkupKind, Preset, SignatureArt, compress_pdf, load_signature_image, ocr_pdf};

const A: (f32, f32) = (595.0, 842.0);
const B: (f32, f32) = (300.0, 400.0);
const C: (f32, f32) = (500.0, 200.0);

/// A three-page PDF whose pages can be told apart by size.
fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("rogeha-reader-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}-{}.pdf", std::process::id()));
    let mut doc = PdfDocument::new();
    for (w, h) in [A, B, C] {
        doc.new_page(Size::new(w, h)).unwrap();
    }
    doc.save(path.to_str().unwrap()).unwrap();
    path
}

fn sizes(engine: &Engine) -> Vec<(i32, i32)> {
    engine
        .page_sizes()
        .iter()
        .map(|&(w, h)| (w.round() as i32, h.round() as i32))
        .collect()
}

fn px(s: (f32, f32)) -> (i32, i32) {
    (s.0 as i32, s.1 as i32)
}

fn rotation_on_disk(path: &Path, page: i32) -> i32 {
    let doc = PdfDocument::open(path.to_str().unwrap()).unwrap();
    doc.load_pdf_page(page).unwrap().rotation().unwrap()
}

#[test]
fn opens_and_renders() {
    let path = fixture("render");
    let engine = Engine::open(&path).unwrap();
    assert!(engine.is_pdf());
    assert!(!engine.is_dirty());
    assert_eq!(sizes(&engine), vec![px(A), px(B), px(C)]);

    let img = engine.render(1, 2.0).unwrap();
    assert_eq!((img.width, img.height), (600, 800));
    assert_eq!(img.pixels.len(), 600 * 800);
    // A blank page renders white.
    assert!(img.pixels.iter().all(|&p| p == 0x00FF_FFFF));
    drop(engine);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn rotation_is_saved_into_the_file() {
    let path = fixture("rotate");
    let mut engine = Engine::open(&path).unwrap();
    engine.rotate_pages(&[0], 90).unwrap();
    assert!(engine.is_dirty());
    // Rotated page reports swapped width/height straight away.
    assert_eq!(sizes(&engine)[0], (842, 595));
    // Nothing is written until save.
    assert_eq!(rotation_on_disk(&path, 0), 0);

    let saved = engine.save(None).unwrap();
    assert_eq!(saved, path);
    assert!(!engine.is_dirty());
    assert_eq!(rotation_on_disk(&path, 0), 90);
    assert_eq!(rotation_on_disk(&path, 1), 0);

    // The engine keeps working on the reopened file, and rotations accumulate.
    engine.rotate_pages(&[0], -180).unwrap();
    engine.rotate_pages(&[2], -90).unwrap();
    engine.save(None).unwrap();
    assert_eq!(rotation_on_disk(&path, 0), 270);
    assert_eq!(rotation_on_disk(&path, 2), 270);
    let reopened = Engine::open(&path).unwrap();
    assert_eq!(sizes(&reopened), vec![(842, 595), px(B), (200, 500)]);
    drop((engine, reopened));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn delete_and_move_pages() {
    let path = fixture("organize");
    let mut engine = Engine::open(&path).unwrap();

    engine.move_page(0, 2).unwrap();
    assert_eq!(sizes(&engine), vec![px(B), px(C), px(A)]);
    engine.move_page(2, 1).unwrap();
    assert_eq!(sizes(&engine), vec![px(B), px(A), px(C)]);

    engine.delete_pages(&[0]).unwrap();
    assert_eq!(sizes(&engine), vec![px(A), px(C)]);
    assert!(engine.delete_pages(&[0, 1]).is_err(), "must not delete every page");
    assert!(engine.delete_pages(&[7]).is_err());

    engine.save(None).unwrap();
    let reopened = Engine::open(&path).unwrap();
    assert_eq!(sizes(&reopened), vec![px(A), px(C)]);
    drop((engine, reopened));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn save_as_leaves_the_original_alone() {
    let path = fixture("saveas");
    let copy = path.with_extension("copy.pdf");
    let mut engine = Engine::open(&path).unwrap();
    engine.rotate_pages(&[1], 90).unwrap();
    engine.save(Some(&copy)).unwrap();

    assert_eq!(engine.path(), copy.as_path());
    assert_eq!(rotation_on_disk(&copy, 1), 90);
    assert_eq!(rotation_on_disk(&path, 1), 0);
    drop(engine);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(copy).unwrap();
}

#[test]
fn bad_input_is_an_error_not_a_crash() {
    let path = fixture("bad");
    let mut engine = Engine::open(&path).unwrap();
    assert!(engine.rotate_pages(&[0], 45).is_err());
    assert!(engine.rotate_pages(&[99], 90).is_err());
    assert!(engine.move_page(0, 3).is_err());
    assert!(!engine.is_dirty());
    assert!(Engine::open(Path::new("does-not-exist.pdf")).is_err());
    drop(engine);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn merge_pdfs() {
    let path = fixture("merge-a");
    let other = fixture("merge-b");
    let mut engine = Engine::open(&path).unwrap();

    // Append, then insert the same file again before page 2.
    assert_eq!(engine.insert_pdf(&other, None).unwrap(), 3);
    assert_eq!(sizes(&engine), vec![px(A), px(B), px(C), px(A), px(B), px(C)]);
    engine.insert_pdf(&other, Some(1)).unwrap();
    assert_eq!(engine.page_count(), 9);
    assert_eq!(sizes(&engine)[..5], [px(A), px(A), px(B), px(C), px(B)]);

    // The merged file must not depend on the source still existing.
    std::fs::remove_file(&other).unwrap();
    engine.save(None).unwrap();
    let reopened = Engine::open(&path).unwrap();
    assert_eq!(reopened.page_count(), 9);
    assert!(reopened.render(8, 1.0).is_ok());
    drop((engine, reopened));
    std::fs::remove_file(path).unwrap();
}

/// A one-page PDF with a line of real text, so there is something to select.
fn text_fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("rogeha-reader-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}-{}.pdf", std::process::id()));
    let mut doc = PdfDocument::new();
    let page = doc.new_page(Size::A4).unwrap();
    let font = doc
        .add_object(&doc.new_object_from_str("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>").unwrap())
        .unwrap();
    let resources = doc
        .new_object_from_str(&format!("<< /Font << /F1 {} 0 R >> >>", font.as_indirect().unwrap()))
        .unwrap();
    let content = mupdf::Buffer::from_bytes(b"BT /F1 24 Tf 72 700 Td (Hello annotated world) Tj ET").unwrap();
    let stream = doc.add_stream(&content, None, false).unwrap();
    let mut obj = page.object();
    obj.dict_put("Resources", resources).unwrap();
    obj.dict_put("Contents", stream).unwrap();
    drop(page);
    doc.save(path.to_str().unwrap()).unwrap();
    path
}

#[test]
fn comments_are_saved_as_standard_annotations() {
    let path = text_fixture("comments");
    let mut engine = Engine::open(&path).unwrap();
    assert!(engine.all_annotations().is_empty());

    // The text baseline is at y = 842 - 700 = 142 in top-left page coordinates.
    let sel = engine.select_text(0, (70.0, 135.0), (400.0, 135.0)).unwrap();
    assert_eq!(sel.text, "Hello annotated world");
    assert!(!sel.quads.is_empty());
    let word = engine.select_text(0, (70.0, 135.0), (125.0, 135.0)).unwrap();
    assert_eq!(word.text, "Hello");

    let highlight = engine
        .add_markup(0, MarkupKind::Highlight, &sel.quads, "Ada", "Check this sentence")
        .unwrap();
    engine.add_markup(0, MarkupKind::Underline, &word.quads, "Ada", "").unwrap();
    let strike = engine.add_markup(0, MarkupKind::StrikeOut, &word.quads, "Ada", "").unwrap();
    let note = engine.add_note(0, (300.0, 300.0), "A sticky note", "Ada").unwrap();
    assert!(engine.is_dirty());
    assert!(engine.add_markup(0, MarkupKind::Highlight, &[], "Ada", "").is_err());

    engine.set_annotation_contents(0, note, "Edited note \u{e9}\u{4e2d}").unwrap();
    engine.delete_annotation(0, strike).unwrap();
    assert!(engine.delete_annotation(0, strike).is_err(), "already deleted");

    // The highlight is drawn: the page is no longer pure white where the text is.
    let img = engine.render(0, 1.0).unwrap();
    let px = img.pixels[130 * img.width + 200];
    assert_ne!(px, 0x00FF_FFFF, "highlight should colour the text background");

    engine.save(None).unwrap();
    let reopened = Engine::open(&path).unwrap();
    let list = reopened.all_annotations();
    let kinds: Vec<_> = list.iter().map(|a| a.kind).collect();
    assert_eq!(kinds, vec![AnnotKind::Highlight, AnnotKind::Underline, AnnotKind::Note]);
    assert_eq!(list[0].contents, "Check this sentence");
    assert_eq!(list[0].author, "Ada");
    assert!(!list[0].quads.is_empty());
    assert_eq!(list[2].contents, "Edited note \u{e9}\u{4e2d}");
    let _ = highlight;

    // Leave the file for the independent check when asked (see tests/check_annotations.py).
    drop((engine, reopened));
    match std::env::var_os("ROGEHA_KEEP_ANNOTATED") {
        Some(dest) => {
            std::fs::copy(&path, dest).unwrap();
        }
        None => {}
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn comments_need_a_pdf() {
    let path = fixture("comments-bad");
    let mut engine = Engine::open(&path).unwrap();
    assert!(engine.add_note(9, (10.0, 10.0), "x", "me").is_err());
    assert!(engine.set_annotation_contents(0, 12345, "x").is_err());
    assert!(!engine.is_dirty());
    drop(engine);
    std::fs::remove_file(path).unwrap();
}

/// A "scan": one page that is a large, smooth, slightly noisy image stored without loss at
/// about 400 dpi, with a line of real text on top.
fn scan_fixture(name: &str) -> PathBuf {
    use mupdf::pdf::{InsertImageOptions, PageImageSource};
    let path = text_fixture(name);
    let mut doc = PdfDocument::open(path.to_str().unwrap()).unwrap();
    let (w, h) = (1600usize, 1200usize);
    let mut pix = mupdf::Pixmap::new_with_w_h(&mupdf::Colorspace::device_rgb(), w as i32, h as i32, false).unwrap();
    let stride = pix.stride() as usize;
    let mut seed = 12345u32;
    let data = pix.samples_mut();
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = (seed >> 29) as usize; // 0..7
            let i = y * stride + x * 3;
            data[i] = (60 + x * 150 / w + noise) as u8;
            data[i + 1] = (200 - y * 120 / h + noise) as u8;
            data[i + 2] = (120 + (x + y) * 60 / (w + h) + noise) as u8;
        }
    }
    let mut page = doc.load_pdf_page(0).unwrap();
    // 1600 px across 288 pt (4 inches) = 400 dpi.
    page.insert_image(&mut doc, mupdf::Rect::new(72.0, 200.0, 360.0, 416.0), PageImageSource::Pixmap(&pix), InsertImageOptions::default())
        .unwrap();
    drop(page);
    let scan = path.with_extension("scan.pdf");
    doc.save(scan.to_str().unwrap()).unwrap();
    drop(doc);
    std::fs::remove_file(path).unwrap();
    scan
}

#[test]
fn compressing_a_scan_shrinks_it_and_keeps_it_readable() {
    let scan = scan_fixture("compress");
    let out = scan.with_extension("small.pdf");
    let stats = compress_pdf(&scan, &out, Preset::Balanced).unwrap();
    assert_eq!(stats.pages, 1);
    assert!(!stats.images_untouched);
    assert!(
        stats.bytes_after * 3 < stats.bytes_before,
        "expected at least 3x smaller, got {} -> {}",
        stats.bytes_before,
        stats.bytes_after
    );
    assert!(stats.saved_percent() > 66.0);

    // Same page, same text, and the picture still looks the same at screen resolution.
    let (a, b) = (Engine::open(&scan).unwrap(), Engine::open(&out).unwrap());
    assert_eq!(sizes(&a), sizes(&b));
    let text = |e: &Engine| e.select_text(0, (70.0, 135.0), (400.0, 135.0)).unwrap().text;
    assert_eq!(text(&b), "Hello annotated world");
    assert_eq!(text(&a), text(&b));
    let (ia, ib) = (a.render(0, 1.5).unwrap(), b.render(0, 1.5).unwrap());
    let mut sum = 0f64;
    for (p, q) in ia.pixels.iter().zip(&ib.pixels) {
        for shift in [16, 8, 0] {
            let d = ((p >> shift) & 255) as f64 - ((q >> shift) & 255) as f64;
            sum += d * d;
        }
    }
    let psnr = 10.0 * (255.0f64 * 255.0 / (sum / (ia.pixels.len() * 3) as f64).max(1e-9)).log10();
    assert!(psnr > 38.0, "picture changed too much: {psnr:.1} dB");

    // The original is untouched, no temporary files are left, and bad requests are refused.
    assert_eq!(std::fs::metadata(&scan).unwrap().len(), stats.bytes_before);
    assert!(!out.with_extension("pdf.rr-1").exists() && !out.with_extension("pdf.rr-2").exists());
    assert!(compress_pdf(&scan, &scan, Preset::Balanced).is_err());
    drop((a, b));
    std::fs::remove_file(scan).unwrap();
    std::fs::remove_file(out).unwrap();
}

#[test]
fn ocr_refuses_bad_requests_before_touching_anything() {
    let path = text_fixture("ocr-args");
    let out = path.with_extension("ocr.pdf");
    let empty_dir = std::env::temp_dir().join("rogeha-reader-tests");
    // No language data in that folder: a clear error, and no output file.
    let e = ocr_pdf(&path, &out, "eng", &empty_dir, |_, _| {}).unwrap_err();
    assert!(e.contains("not installed"), "{e}");
    assert!(ocr_pdf(&path, &path, "eng", &empty_dir, |_, _| {}).is_err());
    assert!(!out.exists());
    std::fs::remove_file(path).unwrap();
}

/// Full OCR run. Needs Tesseract language data, so it only runs when ROGEHA_TESSDATA points
/// at a folder with eng.traineddata:
///     ROGEHA_TESSDATA=/path/to/tessdata cargo test --release -p rogeha-core ocr
#[test]
fn ocr_adds_a_text_layer_that_matches_the_picture() {
    let Some(data) = std::env::var_os("ROGEHA_TESSDATA") else {
        eprintln!("skipped: ROGEHA_TESSDATA is not set");
        return;
    };
    // A page that is only a picture of text: render the text fixture and wrap the image in a PDF.
    use mupdf::pdf::{InsertImageOptions, PageImageSource};
    let source = text_fixture("ocr-src");
    let rendered = Engine::open(&source).unwrap().render(0, 300.0 / 72.0).unwrap();
    let mut pix = mupdf::Pixmap::new_with_w_h(&mupdf::Colorspace::device_rgb(), rendered.width as i32, rendered.height as i32, false).unwrap();
    let stride = pix.stride() as usize;
    for (i, p) in rendered.pixels.iter().enumerate() {
        let (x, y) = (i % rendered.width, i / rendered.width);
        let o = y * stride + x * 3;
        pix.samples_mut()[o..o + 3].copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    let mut doc = PdfDocument::new();
    let mut page = doc.new_page(Size::A4).unwrap();
    page.insert_image(&mut doc, mupdf::Rect::new(0.0, 0.0, 595.0, 842.0), PageImageSource::Pixmap(&pix), InsertImageOptions::default()).unwrap();
    drop(page);
    let scan = source.with_extension("scan.pdf");
    doc.save(scan.to_str().unwrap()).unwrap();
    drop(doc);

    let before = Engine::open(&scan).unwrap();
    assert_eq!(before.page_text(0).unwrap().trim(), "", "the scan must start without text");
    let out = scan.with_extension("ocr.pdf");
    let mut calls = 0;
    let stats = ocr_pdf(&scan, &out, "eng", data.as_ref(), |_, _| calls += 1).unwrap();
    assert_eq!((stats.pages, stats.pages_recognised, stats.pages_with_text, calls), (1, 1, 0, 1));

    let after = Engine::open(&out).unwrap();
    assert_eq!(after.page_text(0).unwrap().split_whitespace().collect::<Vec<_>>(), ["Hello", "annotated", "world"]);
    // The words sit where the picture shows them: selecting along the printed line finds them,
    // and a search hit covers the printed word (baseline at y = 142, see text_fixture).
    assert_eq!(after.select_text(0, (70.0, 135.0), (400.0, 135.0)).unwrap().text, "Hello annotated world");
    let hit = after.search_page(0, "annotated").unwrap()[0];
    let (top, bottom) = (hit[0].1.min(hit[1].1), hit[2].1.max(hit[3].1));
    assert!(top < 135.0 && bottom > 135.0 && top > 110.0 && bottom < 160.0, "hit spans {top}..{bottom}");
    assert!((hit[0].0 - 139.0).abs() < 6.0, "hit starts at x = {}", hit[0].0);
    // The page still looks exactly the same.
    assert!(before.render(0, 1.0).unwrap().pixels == after.render(0, 1.0).unwrap().pixels);
    // Running OCR again finds the text layer and leaves the page alone.
    let again = out.with_extension("again.pdf");
    let stats = ocr_pdf(&out, &again, "eng", data.as_ref(), |_, _| {}).unwrap();
    assert_eq!((stats.pages_recognised, stats.pages_with_text), (0, 1));

    drop((before, after));
    for f in [source, scan, out, again] {
        std::fs::remove_file(f).unwrap();
    }
}

/// Share of pixels in a page rectangle (given in points) that are not white.
fn ink_share(engine: &Engine, page: usize, rect: (f32, f32, f32, f32)) -> f64 {
    let img = engine.render(page, 2.0).unwrap();
    let (x0, y0, x1, y1) = ((rect.0 * 2.0) as usize, (rect.1 * 2.0) as usize, (rect.2 * 2.0) as usize, (rect.3 * 2.0) as usize);
    let mut inked = 0;
    for y in y0..y1.min(img.height) {
        for x in x0..x1.min(img.width) {
            inked += (img.pixels[y * img.width + x] != 0x00FF_FFFF) as usize;
        }
    }
    inked as f64 / ((x1 - x0) * (y1 - y0)).max(1) as f64
}

fn zigzag() -> SignatureArt {
    SignatureArt::Strokes(vec![
        vec![(0.0, 0.8), (0.2, 0.1), (0.4, 0.9), (0.6, 0.1), (0.8, 0.9), (1.0, 0.2)],
        vec![(0.1, 0.5), (0.9, 0.5)],
    ])
}

#[test]
fn drawn_and_typed_signatures_are_stamped_into_the_page() {
    let path = text_fixture("sign");
    let mut engine = Engine::open(&path).unwrap();
    let drawn = (300.0, 500.0, 480.0, 560.0);
    let typed = (80.0, 600.0, 330.0, 650.0);
    assert_eq!(ink_share(&engine, 0, drawn), 0.0);

    engine.stamp_signature(0, drawn, &zigzag(), INK_BLUE).unwrap();
    engine.stamp_signature(0, typed, &SignatureArt::Text("Ada Lovelace (\u{e9})".into()), INK_BLUE).unwrap();
    assert!(engine.is_dirty());
    // Bad requests are refused and leave the page alone.
    assert!(engine.stamp_signature(0, drawn, &SignatureArt::Text("  ".into()), INK_BLUE).is_err());
    assert!(engine.stamp_signature(0, (10.0, 10.0, 10.0, 10.0), &zigzag(), INK_BLUE).is_err());
    assert!(engine.stamp_signature(5, drawn, &zigzag(), INK_BLUE).is_err());

    engine.save(None).unwrap();
    let reopened = Engine::open(&path).unwrap();
    // The ink is there, inside its box and nowhere around it, and it is the chosen colour.
    assert!(ink_share(&reopened, 0, drawn) > 0.03);
    assert!(ink_share(&reopened, 0, typed) > 0.03);
    assert_eq!(ink_share(&reopened, 0, (300.0, 430.0, 480.0, 490.0)), 0.0);
    assert_eq!(ink_share(&reopened, 0, (490.0, 500.0, 560.0, 560.0)), 0.0);
    let img = reopened.render(0, 2.0).unwrap();
    let mid_line = img.pixels[(530 * 2) * img.width + 390 * 2]; // on the horizontal stroke
    assert_eq!(mid_line, 0x001D_4368, "stroke colour");
    // The typed name is real text, and the page's own text is untouched.
    let text = reopened.page_text(0).unwrap();
    assert!(text.contains("Ada Lovelace (\u{e9})"), "{text}");
    assert!(text.contains("Hello annotated world"));
    // It is page content, not a comment.
    assert!(reopened.all_annotations().is_empty());
    drop((engine, reopened));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn signatures_come_out_upright_on_rotated_pages() {
    let path = text_fixture("sign-rot");
    let mut engine = Engine::open(&path).unwrap();
    engine.rotate_pages(&[0], 90).unwrap();
    assert_eq!(sizes(&engine), vec![(842, 595)]);
    // A box near the bottom right of the page as displayed (landscape).
    let rect = (600.0, 450.0, 800.0, 520.0);
    engine.stamp_signature(0, rect, &zigzag(), INK_BLUE).unwrap();
    assert!(ink_share(&engine, 0, rect) > 0.03);
    assert_eq!(ink_share(&engine, 0, (600.0, 370.0, 800.0, 440.0)), 0.0);
    // Upright: the horizontal stroke is still horizontal, at half the box height.
    let img = engine.render(0, 2.0).unwrap();
    let y = (485.0 * 2.0) as usize;
    let on_line = (640..760).filter(|x| img.pixels[y * img.width + x * 2] != 0x00FF_FFFF).count();
    assert!(on_line > 100, "horizontal stroke covers {on_line} of 120 samples");
    drop(engine);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn picture_signatures_keep_their_transparency() {
    let path = text_fixture("sign-img");
    let mut engine = Engine::open(&path).unwrap();
    // A red block with a fully transparent border.
    let (w, h) = (60usize, 30usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 8..22 {
        for x in 10..50 {
            rgba[(y * w + x) * 4..][..4].copy_from_slice(&[200, 30, 30, 255]);
        }
    }
    let art = SignatureArt::Image { rgba, width: w, height: h };
    assert_eq!(art.aspect(), 2.0);
    // Placed over the page's text line: the transparent border must not cover it.
    let rect = (60.0, 110.0, 180.0, 170.0);
    let before = engine.render(0, 2.0).unwrap();
    engine.stamp_signature(0, rect, &art, INK_BLUE).unwrap();
    let after = engine.render(0, 2.0).unwrap();
    let px = |img: &rogeha_core::PageImage, x: f32, y: f32| img.pixels[(y * 2.0) as usize * img.width + (x * 2.0) as usize];
    assert_eq!(px(&after, 120.0, 140.0), 0x00C8_1E1E, "centre shows the picture");
    for (x, y) in [(64.0, 114.0), (176.0, 166.0), (70.0, 122.0)] {
        assert_eq!(px(&after, x, y), px(&before, x, y), "border at {x},{y} must be see-through");
    }

    // Loading a "photo" (here: the fixture page itself): white paper becomes transparent
    // and the picture is cropped to the ink.
    let photo = load_signature_image(&std::fs::read(&path).unwrap(), "pdf", true).unwrap();
    let SignatureArt::Image { rgba, width, height } = &photo else { panic!() };
    assert!(*width > 100 && *height < *width / 2, "cropped to the inked area: {width}x{height}");
    assert!(rgba.chunks_exact(4).any(|p| p[3] == 0) && rgba.chunks_exact(4).any(|p| p[3] == 255));
    assert!(load_signature_image(b"not a picture", "png", true).is_err());
    // The preview is drawn by the same code: transparent ground, ink where the strokes are.
    let (preview, pw, ph) = rogeha_core::render_signature(&zigzag(), 3.0, INK_BLUE, 300).unwrap();
    assert_eq!((pw, ph), (300, 100));
    assert_eq!(preview[3], 0, "corner is transparent");
    let mid = &preview[(50 * pw + 150) * 4..][..4]; // on the horizontal stroke
    assert_eq!(mid, [0x1d, 0x43, 0x68, 255]);
    drop(engine);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn digital_signatures_sign_and_verify() {
    let path = text_fixture("dsig");
    let mut engine = Engine::open(&path).unwrap();
    assert!(engine.signatures().is_empty());

    // A new self-signed identity, written to and read back from a .p12 file.
    let p12 = Identity::create_self_signed("Ada Lovelace", "Analytical Engines", "test-pass", 3).unwrap();
    assert!(Identity::from_pkcs12(&p12, "wrong").is_err(), "wrong password must be refused");
    assert!(Identity::from_pkcs12(b"not a certificate", "x").is_err());
    assert!(Identity::create_self_signed(" ", "", "p", 1).is_err());
    let identity = Identity::from_pkcs12(&p12, "test-pass").unwrap();
    assert_eq!(identity.name(), "Ada Lovelace");

    let before = engine.render(0, 1.0).unwrap().pixels;
    let options = SignOptions { reason: "I approve this document".into(), location: "Z\u{fc}rich".into() };
    engine.sign_document(None, &identity, &options).unwrap();
    assert!(!engine.is_dirty());
    assert!(engine.render(0, 1.0).unwrap().pixels == before, "an invisible signature changes nothing on the page");

    let sigs = engine.signatures();
    assert_eq!(sigs.len(), 1);
    let s = &sigs[0];
    assert_eq!(s.status, SignatureStatus::Intact);
    assert_eq!((s.signer.as_str(), s.reason.as_str(), s.location.as_str()), ("Ada Lovelace", "I approve this document", "Z\u{fc}rich"));
    assert!(s.covers_whole_file && s.self_signed && s.certificate_valid_at_signing);
    assert!(s.signed_at.is_some());
    drop(engine);

    // Keep copies for the independent check (see tests/check_signature.py).
    if let Some(dir) = std::env::var_os("ROGEHA_KEEP_SIGNED") {
        let dir = PathBuf::from(dir);
        std::fs::copy(&path, dir.join("signed.pdf")).unwrap();
        std::fs::write(dir.join("identity.p12"), &p12).unwrap();
    }

    // Tampering with a signed byte is detected.
    let good = std::fs::read(&path).unwrap();
    let mut bad = good.clone();
    let at = bad.windows(5).position(|w| w == b"Hello").unwrap();
    bad[at] = b'J';
    std::fs::write(&path, &bad).unwrap();
    let tampered = Engine::open(&path).unwrap().signatures();
    assert!(matches!(&tampered[0].status, SignatureStatus::Broken(why) if why.contains("changed")), "{:?}", tampered[0].status);
    std::fs::write(&path, &good).unwrap();

    // Adding a comment afterwards keeps the signature intact, but it no longer covers the
    // whole file; signing again adds a second signature and keeps the first one valid.
    let mut engine = Engine::open(&path).unwrap();
    engine.add_note(0, (300.0, 300.0), "Looks good", "Bob").unwrap();
    engine.save(None).unwrap();
    let sigs = engine.signatures();
    assert_eq!(sigs[0].status, SignatureStatus::Intact);
    assert!(!sigs[0].covers_whole_file);

    engine.sign_document(None, &identity, &SignOptions::default()).unwrap();
    let sigs = engine.signatures();
    assert_eq!(sigs.len(), 2);
    assert!(sigs.iter().all(|s| s.status == SignatureStatus::Intact), "{sigs:?}");
    assert_eq!(sigs.iter().filter(|s| s.covers_whole_file).count(), 1);
    assert_eq!(engine.all_annotations().len(), 1, "the comment is still there");
    drop(engine);
    std::fs::remove_file(path).unwrap();
}

/// A one-page form: a text field, a checkbox, a group of two radio buttons and a dropdown.
fn form_fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("rogeha-reader-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}-{}.pdf", std::process::id()));
    let mut doc = PdfDocument::new();
    let page = doc.new_page(Size::A4).unwrap();
    let page_ref = doc.find_page(0).unwrap().as_indirect().unwrap();
    let add = |doc: &mut PdfDocument, src: &str| {
        let obj = doc.new_object_from_str(src).unwrap();
        doc.add_object(&obj).unwrap().as_indirect().unwrap()
    };
    // Appearances for the buttons: a filled square when on, nothing when off.
    let look = |doc: &mut PdfDocument, content: &[u8]| {
        let dict = doc.new_object_from_str("<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>").unwrap();
        doc.add_stream(&mupdf::Buffer::from_bytes(content).unwrap(), Some(&dict), false).unwrap().as_indirect().unwrap()
    };
    let (on, off) = (look(&mut doc, b"0 0 0.6 rg 2 2 10 10 re f"), look(&mut doc, b""));
    let font = add(&mut doc, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
    let widget = format!("/Type /Annot /Subtype /Widget /F 4 /P {page_ref} 0 R");

    let text = add(&mut doc, &format!("<< {widget} /FT /Tx /T (name) /TU (Your full name) /Ff 2 /Rect [72 700 300 720] /DA (/Helv 11 Tf 0 g) >>"));
    let check = add(&mut doc, &format!("<< {widget} /FT /Btn /T (agree) /Rect [72 660 86 674] /V /Off /AS /Off /AP << /N << /Yes {on} 0 R /Off {off} 0 R >> >> >>"));
    let group = add(&mut doc, "<< /FT /Btn /Ff 49152 /T (colour) /V /Off >>");
    let radio = |doc: &mut PdfDocument, state: &str, x: i32| {
        add(doc, &format!("<< {widget} /Parent {group} 0 R /Rect [{x} 620 {} 634] /AS /Off /AP << /N << /{state} {on} 0 R /Off {off} 0 R >> >> >>", x + 14))
    };
    let (red, blue) = (radio(&mut doc, "red", 72), radio(&mut doc, "blue", 100));
    doc.xref_object(group).unwrap().unwrap().dict_put("Kids", doc.new_object_from_str(&format!("[{red} 0 R {blue} 0 R]")).unwrap()).unwrap();
    let combo = add(&mut doc, &format!("<< {widget} /FT /Ch /Ff 131072 /T (country) /Rect [72 580 300 600] /DA (/Helv 11 Tf 0 g) /Opt [(Switzerland) (Germany) [(AT) (Austria)]] >>"));
    let locked = add(&mut doc, &format!("<< {widget} /FT /Tx /T (office) /Ff 1 /Rect [72 540 300 560] /DA (/Helv 11 Tf 0 g) /V (do not fill) >>"));

    let mut page_obj = page.object();
    page_obj
        .dict_put("Annots", doc.new_object_from_str(&format!("[{text} 0 R {check} 0 R {red} 0 R {blue} 0 R {combo} 0 R {locked} 0 R]")).unwrap())
        .unwrap();
    let form = doc
        .new_object_from_str(&format!(
            "<< /Fields [{text} 0 R {check} 0 R {group} 0 R {combo} 0 R {locked} 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv {font} 0 R >> >> >>"
        ))
        .unwrap();
    doc.catalog().unwrap().dict_put("AcroForm", form).unwrap();
    drop(page);
    doc.save(path.to_str().unwrap()).unwrap();
    path
}

#[test]
fn forms_can_be_filled_and_saved() {
    let path = form_fixture("form");
    let mut engine = Engine::open(&path).unwrap();
    let fields = engine.form_fields();
    let find = |fields: &[rogeha_core::FormField], name: &str| fields.iter().filter(|f| f.name == name).cloned().collect::<Vec<_>>();
    let kinds: Vec<_> = fields.iter().map(|f| (f.name.as_str(), f.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            ("name", FieldKind::Text { multiline: false, password: false }),
            ("agree", FieldKind::CheckBox),
            ("colour", FieldKind::Radio),
            ("colour", FieldKind::Radio),
            ("country", FieldKind::Dropdown),
            ("office", FieldKind::Text { multiline: false, password: false }),
        ]
    );
    let name = &find(&fields, "name")[0];
    assert_eq!((name.label.as_str(), name.required, name.read_only), ("Your full name", true, false));
    // The box is reported in display coordinates (top-left origin): 842 - 720 = 122.
    assert_eq!((name.rect.0.round(), name.rect.1.round(), name.rect.3.round()), (72.0, 122.0, 142.0));
    let country = &find(&fields, "country")[0];
    assert_eq!(country.options, vec![("Switzerland".into(), "Switzerland".into()), ("Germany".into(), "Germany".into()), ("AT".into(), "Austria".into())]);
    let (agree, radios, office) = (&find(&fields, "agree")[0], find(&fields, "colour"), &find(&fields, "office")[0]);
    assert!(!agree.checked && office.read_only);

    engine.set_field_value(0, name.xref, "Ada L\u{f6}velace").unwrap();
    engine.set_field_checked(0, agree.xref, true).unwrap();
    engine.set_field_checked(0, radios[0].xref, true).unwrap();
    engine.set_field_checked(0, radios[1].xref, true).unwrap(); // choosing blue un-chooses red
    engine.set_field_value(0, country.xref, "AT").unwrap();
    assert!(engine.is_dirty());
    // What cannot be done is refused.
    assert!(engine.set_field_value(0, country.xref, "Atlantis").is_err());
    assert!(engine.set_field_value(0, office.xref, "x").is_err(), "read-only field");
    assert!(engine.set_field_checked(0, radios[1].xref, false).is_err());
    assert!(engine.set_field_value(0, agree.xref, "text").is_err());
    assert!(engine.set_field_value(0, 99999, "x").is_err());

    // The typed text is drawn on the page.
    let img = engine.render(0, 2.0).unwrap();
    let inked = (150..580).filter(|x| img.pixels[(132 * 2) * img.width + x] != 0x00FF_FFFF).count();
    assert!(inked > 20, "text field shows its value ({inked} dark pixels)");

    engine.save(None).unwrap();
    let reopened = Engine::open(&path).unwrap();
    let after = reopened.form_fields();
    assert_eq!(find(&after, "name")[0].value, "Ada L\u{f6}velace");
    assert!(find(&after, "agree")[0].checked);
    let colours: Vec<bool> = find(&after, "colour").iter().map(|f| f.checked).collect();
    assert_eq!(colours, vec![false, true]);
    assert_eq!(find(&after, "country")[0].value, "AT");
    assert_eq!(find(&after, "office")[0].value, "do not fill");

    // Unticking works too, and a file without a form reports no fields.
    let mut engine2 = reopened;
    engine2.set_field_checked(0, find(&after, "agree")[0].xref, false).unwrap();
    assert!(!find(&engine2.form_fields(), "agree")[0].checked);
    drop((engine, engine2));
    if let Some(dest) = std::env::var_os("ROGEHA_KEEP_FORM") {
        std::fs::copy(&path, dest).unwrap();
    }
    std::fs::remove_file(path).unwrap();
    let plain = text_fixture("no-form");
    assert!(Engine::open(&plain).unwrap().form_fields().is_empty());
    std::fs::remove_file(plain).unwrap();
}
