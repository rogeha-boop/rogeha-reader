//! OCR: make scanned PDFs searchable and selectable.
//!
//! Each page without text is rendered and read by Tesseract (compiled into MuPDF, so it works
//! offline). The recognised words are then written onto the **original** page as an invisible
//! text layer (PDF text render mode 3), each word placed and stretched to sit exactly over
//! its picture. The scan itself is not re-encoded or changed, pages that already have text are
//! left alone, and the result goes to a new file.
//!
//! Language data (`eng.traineddata`, ...) is read from a folder the caller provides.
//!
//! Limits of this version: the text layer uses the Windows-1252 character set (Western
//! European languages); characters outside it are left out of the layer. Lines that are not
//! horizontal are skipped.
//!
//! IMPORTANT: like compression, run this in a helper process. The OCR device is driven
//! through raw FFI, and MuPDF ends the process on errors it cannot recover from (missing
//! language data, a damaged page). `rogeha-reader --ocr` is that helper.

use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;

use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use mupdf::{Font, TextPageFlags};
use mupdf_sys as sys;

use crate::engine::{Result, err, raw_context};

/// Resolution pages are read at. 300 dpi is what Tesseract is trained for.
const OCR_DPI: f32 = 300.0;
/// A page with at least this many letters already has a text layer and is skipped.
const HAS_TEXT_CHARS: usize = 20;
/// Name of the font resource added to pages for the invisible text.
const FONT_KEY: &str = "RRocr";

#[derive(Clone, Copy, Debug, Default)]
pub struct OcrStats {
    pub pages: usize,
    /// Pages that were read and got a text layer.
    pub pages_recognised: usize,
    /// Pages left alone because they already had text.
    pub pages_with_text: usize,
    pub words: usize,
}

/// One recognised word, in page points with the origin at the top left.
struct Word {
    text: String,
    x: f32,
    baseline: f32,
    width: f32,
    size: f32,
}

unsafe fn take_error(e: *mut sys::mupdf_error_t) -> Option<String> {
    if e.is_null() {
        return None;
    }
    // SAFETY: a non-null error from the wrapper owns a NUL-terminated message.
    unsafe {
        let m = (*e).message;
        let text = if m.is_null() {
            "unknown MuPDF error".to_owned()
        } else {
            CStr::from_ptr(m).to_string_lossy().into_owned()
        };
        sys::mupdf_drop_error(e);
        Some(text)
    }
}

/// Run Tesseract over one page and collect its words.
///
/// SAFETY: `ctx` and `doc` must be valid; may end the process on MuPDF errors (see module docs).
unsafe fn recognise_page(
    ctx: *mut sys::fz_context,
    doc: *mut sys::fz_document,
    page_no: i32,
    language: &CStr,
    data_dir: &CStr,
) -> Vec<Word> {
    unsafe {
        let scale = OCR_DPI / 72.0;
        let page = sys::fz_load_page(ctx, doc, page_no);
        let b = sys::fz_bound_page(ctx, page);
        let ctm = sys::fz_matrix { a: scale, b: 0.0, c: 0.0, d: scale, e: 0.0, f: 0.0 };
        let mediabox = sys::fz_rect { x0: b.x0 * scale, y0: b.y0 * scale, x1: b.x1 * scale, y1: b.y1 * scale };

        let stext = sys::fz_new_stext_page(ctx, mediabox);
        let options: sys::fz_stext_options = std::mem::zeroed();
        let text_device = sys::fz_new_stext_device(ctx, stext, &options);
        let ocr_device = sys::fz_new_ocr_device(
            ctx,
            text_device,
            ctm,
            mediabox,
            1,
            language.as_ptr(),
            data_dir.as_ptr(),
            None,
            ptr::null_mut(),
        );
        sys::fz_run_page(ctx, page, ocr_device, ctm, ptr::null_mut());
        sys::fz_close_device(ctx, ocr_device);
        sys::fz_drop_device(ctx, ocr_device);
        sys::fz_close_device(ctx, text_device);
        sys::fz_drop_device(ctx, text_device);

        let mut words = Vec::new();
        let mut block = (*stext).first_block;
        while !block.is_null() {
            if (*block).type_ == sys::FZ_STEXT_BLOCK_TEXT as i32 {
                let mut line = (*block).u.t.first_line;
                while !line.is_null() {
                    // Only horizontal lines (this version).
                    if (*line).dir.y.abs() < 0.2 && (*line).dir.x > 0.0 {
                        let line_start = words.len();
                        let mut current: Option<Word> = None;
                        let mut ch = (*line).first_char;
                        while !ch.is_null() {
                            let c = char::from_u32((*ch).c as u32).unwrap_or(' ');
                            let q = (*ch).quad;
                            let (left, right) = (q.ul.x.min(q.ll.x) / scale, q.ur.x.max(q.lr.x) / scale);
                            if c.is_whitespace() {
                                words.extend(current.take());
                            } else {
                                let word = current.get_or_insert_with(|| Word {
                                    text: String::new(),
                                    x: left,
                                    // The bottom of the word's box for now; see below.
                                    baseline: q.ul.y.max(q.ll.y).max(q.ur.y).max(q.lr.y) / scale,
                                    width: 0.0,
                                    size: (*ch).size / scale,
                                });
                                word.text.push(c);
                                word.width = (right - word.x).max(word.width);
                            }
                            ch = (*ch).next;
                        }
                        words.extend(current.take());
                        // Tesseract reports a tight box per word, not a baseline. Words with
                        // descenders (g, p, y) reach lower than the rest, so the line's
                        // baseline is taken as the median of the box bottoms.
                        let mut bottoms: Vec<f32> = words[line_start..].iter().map(|w| w.baseline).collect();
                        if !bottoms.is_empty() {
                            bottoms.sort_by(f32::total_cmp);
                            let baseline = bottoms[bottoms.len() / 2];
                            for word in &mut words[line_start..] {
                                word.baseline = baseline;
                            }
                        }
                    }
                    line = (*line).next;
                }
            }
            block = (*block).next;
        }
        sys::fz_drop_stext_page(ctx, stext);
        sys::fz_drop_page(ctx, page);
        words
    }
}

/// Windows-1252 byte for a character, if it has one (the encoding of the text-layer font).
fn winansi(c: char) -> Option<u8> {
    Some(match c {
        ' '..='~' => c as u8,
        '\u{a0}'..='\u{ff}' => c as u32 as u8,
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8a,
        '‹' => 0x8b,
        'Œ' => 0x8c,
        'Ž' => 0x8e,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9a,
        '›' => 0x9b,
        'œ' => 0x9c,
        'ž' => 0x9e,
        'Ÿ' => 0x9f,
        // Ligatures Tesseract sometimes returns: spell them out below instead.
        _ => return None,
    })
}

/// The invisible text layer for one page, as a PDF content stream.
/// `to_pdf` maps top-left page points to the page's own coordinate system.
fn text_layer(words: &[Word], font: &Font, to_pdf: &mupdf::Matrix) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut written = 0;
    out.extend_from_slice(
        format!(
            "q\n{} {} {} {} {} {} cm\nBT\n3 Tr\n/{FONT_KEY} 1 Tf\n",
            to_pdf.a, to_pdf.b, to_pdf.c, to_pdf.d, to_pdf.e, to_pdf.f
        )
        .as_bytes(),
    );
    for word in words {
        // Spell out ligatures, then keep what the font's encoding can hold.
        let text = word.text.replace('ﬁ', "fi").replace('ﬂ', "fl").replace('ﬀ', "ff");
        let mut bytes = Vec::new();
        let mut natural = 0.0f32; // width at font size 1
        for c in text.chars() {
            let Some(b) = winansi(c) else { continue };
            bytes.push(b);
            natural += font
                .encode_character(c as i32)
                .and_then(|gid| font.advance_glyph(gid))
                .unwrap_or(0.5);
        }
        if bytes.is_empty() || natural <= 0.0 || word.size <= 0.0 || word.width <= 0.0 {
            continue;
        }
        // Stretch the word horizontally so it covers exactly its picture; the y axis points
        // down in this space, so the vertical scale is negative to keep glyphs upright.
        let sx = word.width / natural;
        out.extend_from_slice(format!("{sx:.4} 0 0 {:.4} {:.3} {:.3} Tm (", -word.size, word.x, word.baseline).as_bytes());
        for b in bytes {
            if matches!(b, b'(' | b')' | b'\\') {
                out.push(b'\\');
            }
            out.push(b);
        }
        out.extend_from_slice(b") Tj\n");
        written += 1;
    }
    out.extend_from_slice(b"ET\nQ\n");
    (out, written)
}

/// Recognise the text of `input` and write a searchable copy to `output`.
///
/// `language` is a Tesseract language code such as `eng` or `deu`, or several joined with `+`.
/// `data_dir` must contain `<language>.traineddata` for each of them.
/// `progress(done, total)` is called after every page.
pub fn ocr_pdf(
    input: &Path,
    output: &Path,
    language: &str,
    data_dir: &Path,
    mut progress: impl FnMut(usize, usize),
) -> Result<OcrStats> {
    if input == output {
        return Err("Choose a different file name for the searchable copy".into());
    }
    for lang in language.split('+') {
        if !data_dir.join(format!("{lang}.traineddata")).is_file() {
            return Err(format!("Language data for \"{lang}\" is not installed"));
        }
    }
    let utf8 = |p: &Path| p.to_str().map(str::to_owned).ok_or("File path is not valid Unicode");
    let c_in = CString::new(utf8(input)?).map_err(err)?;
    let c_lang = CString::new(language).map_err(err)?;
    let c_dir = CString::new(utf8(data_dir)?).map_err(err)?;

    // The copy that gets the text layer (safe API).
    let mut doc = PdfDocument::open(utf8(input)?.as_str()).map_err(|_| "Only PDF files can be made searchable".to_owned())?;
    let pages = doc.page_count().map_err(err)?.max(0);
    let font = Font::new("Helvetica").map_err(err)?;
    let font_obj = doc
        .add_object(
            &doc.new_object_from_str("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
                .map_err(err)?,
        )
        .map_err(err)?;
    let font_ref = font_obj.as_indirect().map_err(err)?;

    // The same file again, for the OCR device (raw API).
    let ctx = raw_context();
    // SAFETY: plain MuPDF calls on this thread's context; the raw document is dropped below.
    let raw = unsafe {
        let mut e = ptr::null_mut();
        let raw = sys::mupdf_open_document(ctx, c_in.as_ptr(), &mut e);
        if let Some(message) = take_error(e) {
            return Err(message);
        }
        raw
    };

    let mut stats = OcrStats { pages: pages as usize, ..Default::default() };
    for page_no in 0..pages {
        let has_text = doc
            .load_page(page_no)
            .and_then(|p| p.to_text_page(TextPageFlags::empty()))
            .and_then(|t| t.to_text())
            .is_ok_and(|t| t.chars().filter(|c| c.is_alphanumeric()).count() >= HAS_TEXT_CHARS);
        // A page we already gave a text layer (even a very short one) is never done twice.
        let has_layer = doc.find_page(page_no).is_ok_and(|p| {
            matches!(p.get_dict_inheritable("Resources"), Ok(Some(r))
                if matches!(r.get_dict("Font"), Ok(Some(f)) if matches!(f.get_dict(FONT_KEY), Ok(Some(_)))))
        });
        if has_text || has_layer {
            stats.pages_with_text += 1;
        } else {
            // SAFETY: valid context and document; see module docs for the failure mode.
            let words = unsafe { recognise_page(ctx, raw, page_no, &c_lang, &c_dir) };
            if !words.is_empty() {
                let mut page = doc.load_pdf_page(page_no).map_err(err)?;
                let to_pdf = page.ctm().map_err(err)?.invert().ok_or("Page has an unusable coordinate system")?;
                let (content, written) = text_layer(&words, &font, &to_pdf);
                if written > 0 {
                    // Register the font on the page, then add the layer on top of the content.
                    let mut page_obj = page.object();
                    let mut resources = match page_obj.get_dict_inheritable("Resources").map_err(err)? {
                        Some(r) => r,
                        None => {
                            let r = doc.new_dict().map_err(err)?;
                            page_obj.dict_put("Resources", r).map_err(err)?;
                            page_obj.get_dict("Resources").map_err(err)?.ok_or("Could not add page resources")?
                        }
                    };
                    let mut fonts = match resources.get_dict("Font").map_err(err)? {
                        Some(f) => f,
                        None => {
                            let f = doc.new_dict().map_err(err)?;
                            resources.dict_put("Font", f).map_err(err)?;
                            resources.get_dict("Font").map_err(err)?.ok_or("Could not add page fonts")?
                        }
                    };
                    fonts.dict_put(FONT_KEY, doc.new_indirect(font_ref, 0).map_err(err)?).map_err(err)?;
                    // Isolate the existing content's graphics state, then append ours.
                    page.wrap_contents(&mut doc).map_err(err)?;
                    page.insert_contents(&mut doc, &content, true).map_err(err)?;
                    stats.pages_recognised += 1;
                    stats.words += written;
                }
            }
        }
        progress(page_no as usize + 1, pages as usize);
    }
    // SAFETY: `raw` came from mupdf_open_document above and is not used again.
    unsafe { sys::fz_drop_document(ctx, raw) };

    let mut options = PdfWriteOptions::default();
    options.set_garbage_level(1).set_compress(true);
    if let Err(e) = doc.save_with_options(utf8(output)?.as_str(), options) {
        let _ = std::fs::remove_file(output);
        return Err(err(e));
    }
    Ok(stats)
}
