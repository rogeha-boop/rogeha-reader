//! End-to-end tests of the engine on PDFs generated with MuPDF itself.

use std::path::{Path, PathBuf};

use mupdf::Size;
use mupdf::pdf::PdfDocument;
use rogeha_core::{AnnotKind, Engine, MarkupKind};

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
