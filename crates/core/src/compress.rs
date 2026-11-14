//! Make scanned PDFs smaller without visibly hurting them.
//!
//! Scans are big because every page is a large image, usually stored at a higher resolution
//! and quality than any screen or printer needs. This re-encodes the images with MuPDF's image
//! rewriter and then writes a tidy file (duplicate objects merged, everything compressed):
//!
//! * colour and grey images above a resolution limit are scaled down (bicubic) and stored as JPEG;
//! * black-and-white images stay lossless (CCITT G4 fax coding) and are never blurred;
//! * an image is only replaced when the new version is actually smaller;
//! * text, vector graphics, fonts, comments and bookmarks are left as they are.
//!
//! The result always goes to a **new file**; the original is never touched.
//!
//! Safety net: MuPDF's rewriter re-writes each page's drawing instructions, and on some PDFs
//! that damages text. So every page is fingerprinted before and after (every glyph, its font
//! and position, and every vector shape; image pixels excluded). Where the fingerprint
//! changed, the page's original content is put back. If a page still differs after that, the
//! images are left alone and the file is only tidied losslessly.
//!
//! IMPORTANT: run this in a helper process, not in the reader itself. The image rewriter is
//! called through raw FFI, and MuPDF ends the process if it hits an error it cannot recover
//! from (for example a badly damaged image). `rogeha-reader --compress` is that helper.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::ptr;

use mupdf_sys as sys;

use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use mupdf::{Colorspace, Device, Matrix, NativeDevice};

use crate::engine::{Result, err, raw_context};

/// How hard to squeeze. All presets keep black-and-white pages lossless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Keeps up to 300 dpi, JPEG quality 85. For archiving and printing.
    HighQuality,
    /// Keeps up to 200 dpi, JPEG quality 75. Looks the same on screen; the default.
    Balanced,
    /// Keeps up to 144 dpi, JPEG quality 60. Smallest; fine print may soften.
    Small,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::HighQuality, Preset::Balanced, Preset::Small];

    pub fn id(self) -> &'static str {
        match self {
            Preset::HighQuality => "high",
            Preset::Balanced => "balanced",
            Preset::Small => "small",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::HighQuality => "High quality",
            Preset::Balanced => "Balanced",
            Preset::Small => "Smallest file",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Preset::HighQuality => "Up to 300 dpi. For printing and archiving.",
            Preset::Balanced => "Up to 200 dpi. Looks the same on screen. Recommended.",
            Preset::Small => "Up to 144 dpi. Smallest; very fine print may soften.",
        }
    }

    /// (target dpi, JPEG quality)
    fn settings(self) -> (i32, &'static CStr) {
        match self {
            Preset::HighQuality => (300, c"85"),
            Preset::Balanced => (200, c"75"),
            Preset::Small => (144, c"60"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CompressStats {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub pages: usize,
    /// Pages whose original content was put back because re-encoding changed more than images.
    pub pages_restored: usize,
    /// True when the images were left alone entirely (only lossless tidying was done).
    pub images_untouched: bool,
}

impl CompressStats {
    /// Percentage saved, 0..100.
    pub fn saved_percent(&self) -> f64 {
        if self.bytes_before == 0 {
            return 0.0;
        }
        (1.0 - self.bytes_after as f64 / self.bytes_before as f64).max(0.0) * 100.0
    }
}

/// Turn a wrapper error pointer into a Rust error and free it.
unsafe fn take_error(e: *mut sys::mupdf_error_t) -> Option<String> {
    if e.is_null() {
        return None;
    }
    // SAFETY: a non-null error from the wrapper owns a NUL-terminated message.
    let message = unsafe {
        let m = (*e).message;
        let text = if m.is_null() {
            "unknown MuPDF error".to_owned()
        } else {
            CStr::from_ptr(m).to_string_lossy().into_owned()
        };
        sys::mupdf_drop_error(e);
        text
    };
    Some(message)
}

/// One raw MuPDF pass: open `input`, optionally re-encode its images, and write a tidy
/// `output`. Returns the page count.
fn raw_pass(input: &Path, output: &Path, preset: Preset, rewrite_images: bool) -> Result<usize> {
    let c_in = CString::new(input.to_str().ok_or("File path is not valid Unicode")?)
        .map_err(|e| e.to_string())?;
    let c_out = CString::new(output.to_str().ok_or("File path is not valid Unicode")?)
        .map_err(|e| e.to_string())?;
    let (dpi, quality) = preset.settings();
    let quality = quality.as_ptr() as *mut c_char;
    // Only touch images that are clearly above the target, so nothing is resampled for a
    // few percent (resampling always costs a little sharpness).
    let threshold = dpi + dpi / 8;

    let ctx = raw_context();
    // SAFETY: plain MuPDF calls on this thread's context. The document is dropped on every
    // path. `pdf_rewrite_images` may end the process on unrecoverable errors (see module docs).
    unsafe {
        let mut e = ptr::null_mut();
        let doc = sys::mupdf_open_document(ctx, c_in.as_ptr(), &mut e);
        if let Some(message) = take_error(e) {
            return Err(message);
        }
        let pdf = sys::pdf_document_from_fz_document(ctx, doc);
        if pdf.is_null() {
            sys::fz_drop_document(ctx, doc);
            return Err("Only PDF files can be compressed".into());
        }
        if sys::fz_needs_password(ctx, doc) != 0 {
            sys::fz_drop_document(ctx, doc);
            return Err("This PDF is password protected".into());
        }
        let mut e = ptr::null_mut();
        let pages = sys::mupdf_document_page_count(ctx, doc, &mut e).max(0) as usize;
        if let Some(message) = take_error(e) {
            sys::fz_drop_document(ctx, doc);
            return Err(message);
        }

        let mut o: sys::pdf_image_rewriter_options = std::mem::zeroed();
        let bicubic = sys::FZ_SUBSAMPLE_BICUBIC as i32;
        let jpeg = sys::FZ_RECOMPRESS_JPEG as i32;
        // Colour: photos and colour scans, whether stored lossy (JPEG) or lossless (Flate).
        o.color_lossy_image_subsample_method = bicubic;
        o.color_lossy_image_subsample_threshold = threshold;
        o.color_lossy_image_subsample_to = dpi;
        o.color_lossy_image_recompress_method = jpeg;
        o.color_lossy_image_recompress_quality = quality;
        o.color_lossless_image_subsample_method = bicubic;
        o.color_lossless_image_subsample_threshold = threshold;
        o.color_lossless_image_subsample_to = dpi;
        o.color_lossless_image_recompress_method = jpeg;
        o.color_lossless_image_recompress_quality = quality;
        // Greyscale: same treatment.
        o.gray_lossy_image_subsample_method = bicubic;
        o.gray_lossy_image_subsample_threshold = threshold;
        o.gray_lossy_image_subsample_to = dpi;
        o.gray_lossy_image_recompress_method = jpeg;
        o.gray_lossy_image_recompress_quality = quality;
        o.gray_lossless_image_subsample_method = bicubic;
        o.gray_lossless_image_subsample_threshold = threshold;
        o.gray_lossless_image_subsample_to = dpi;
        o.gray_lossless_image_recompress_method = jpeg;
        o.gray_lossless_image_recompress_quality = quality;
        // Black and white (1 bit): text scans. Keep every pixel, store with fax (G4) coding.
        o.bitonal_image_subsample_threshold = 0;
        o.bitonal_image_recompress_method = sys::FZ_RECOMPRESS_FAX as i32;
        // Never make an image bigger.
        o.recompress_when = sys::FZ_RECOMPRESS_WHEN_SMALLER as i32;

        if rewrite_images {
            sys::pdf_rewrite_images(ctx, pdf, &mut o);
        }

        let mut w = sys::pdf_default_write_options;
        w.do_garbage = 4; // drop unused objects and merge identical ones
        w.do_compress = 1;
        w.do_compress_images = 1;
        w.do_compress_fonts = 1;
        w.do_use_objstms = 1;
        let mut e = ptr::null_mut();
        sys::mupdf_pdf_save_document(ctx, pdf, c_out.as_ptr(), w, &mut e);
        let failed = take_error(e);
        sys::fz_drop_document(ctx, doc);
        if let Some(message) = failed {
            let _ = std::fs::remove_file(output);
            return Err(message);
        }
        Ok(pages)
    }
}

/// Compress `input` into `output` (which must be a different file).
pub fn compress_pdf(input: &Path, output: &Path, preset: Preset) -> Result<CompressStats> {
    if input == output {
        return Err("Choose a different file name for the compressed copy".into());
    }
    let bytes_before = std::fs::metadata(input).map_err(|e| e.to_string())?.len();
    let temp = |tag: &str| {
        let mut name = output.as_os_str().to_owned();
        name.push(tag);
        std::path::PathBuf::from(name)
    };
    let (step1, step2) = (temp(".rr-1"), temp(".rr-2"));
    let cleanup = || {
        let _ = std::fs::remove_file(&step1);
        let _ = std::fs::remove_file(&step2);
    };

    let run = || -> Result<(usize, usize, bool)> {
        // 1. Re-encode images.
        let pages = raw_pass(input, &step1, preset, true)?;
        // 2. Check every page; put back the content of pages that changed beyond their images.
        match restore_changed_pages(input, &step1, &step2)? {
            Some(restored) => {
                // 3. Final tidy write (object streams, duplicate removal).
                raw_pass(&step2, output, preset, false)?;
                Ok((pages, restored, false))
            }
            None => {
                // Could not get an exact match: do not risk it, keep the images as they are.
                raw_pass(input, output, preset, false)?;
                Ok((pages, 0, true))
            }
        }
    };
    let result = run();
    cleanup();
    let (pages, pages_restored, images_untouched) = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(output);
            return Err(e);
        }
    };
    let bytes_after = std::fs::metadata(output).map_err(|e| e.to_string())?.len();
    Ok(CompressStats { bytes_before, bytes_after, pages, pages_restored, images_untouched })
}

/// Compare each page of `compressed` with `original`. Pages whose text or vector content
/// changed get their original `/Contents` and `/Resources` back. Writes the result to `fixed`.
/// Returns the number of restored pages, or `None` if some page could not be made to match.
fn restore_changed_pages(original: &Path, compressed: &Path, fixed: &Path) -> Result<Option<usize>> {
    let path = |p: &Path| p.to_str().map(str::to_owned).ok_or("File path is not valid Unicode");
    let orig = PdfDocument::open(path(original)?.as_str()).map_err(err)?;
    let comp = PdfDocument::open(path(compressed)?.as_str()).map_err(err)?;
    let pages = orig.page_count().map_err(err)?;
    if comp.page_count().map_err(err)? != pages {
        return Ok(None);
    }
    let mut graft = comp.new_graft_map().map_err(err)?;
    let mut restored = 0;
    for page in 0..pages {
        let before = fingerprint(&orig, page)?;
        if fingerprint(&comp, page)? == before {
            continue;
        }
        let source = orig.find_page(page).map_err(err)?;
        let mut target = comp.find_page(page).map_err(err)?;
        for key in ["Contents", "Resources"] {
            match source.get_dict_inheritable(key).map_err(err)? {
                Some(value) => {
                    let copy = graft.graft_object(&value).map_err(err)?;
                    target.dict_put(key, copy).map_err(err)?;
                }
                None => target.dict_delete(key).map_err(err)?,
            }
        }
        if fingerprint(&comp, page)? != before {
            return Ok(None);
        }
        restored += 1;
    }
    let mut options = PdfWriteOptions::default();
    options.set_garbage_level(3).set_compress(true);
    comp.save_with_options(path(fixed)?.as_str(), options).map_err(err)?;
    Ok(Some(restored))
}

/// Everything a page draws except image pixels, as a list of hashes (one per drawing call).
fn fingerprint(doc: &PdfDocument, page: i32) -> Result<Vec<u64>> {
    let ops = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let device = Device::from_native(Fingerprint { ops: ops.clone() }).map_err(err)?;
    let p = doc.load_page(page).map_err(err)?;
    p.run(&device, &Matrix::IDENTITY).map_err(err)?;
    drop(device);
    Ok(ops.take())
}

struct Fingerprint {
    ops: std::rc::Rc<std::cell::RefCell<Vec<u64>>>,
}

/// FNV-1a, fed with small integers.
struct Hash(u64);

impl Hash {
    fn new(tag: u8) -> Self {
        let mut h = Hash(0xcbf2_9ce4_8422_2325);
        h.int(tag as i64);
        h
    }
    fn int(&mut self, v: i64) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    /// Positions are compared to 1/20 of a point, far below anything visible but tolerant
    /// of numbers being written out with different rounding.
    fn pos(&mut self, v: f32) {
        self.int((v * 20.0).round() as i64);
    }
    fn matrix(&mut self, m: &Matrix) {
        for v in [m.a, m.b, m.c, m.d] {
            self.int((v * 1000.0).round() as i64);
        }
        self.pos(m.e);
        self.pos(m.f);
    }
    fn color(&mut self, color: &[f32], alpha: f32) {
        for c in color {
            self.int((c * 255.0).round() as i64);
        }
        self.int((alpha * 255.0).round() as i64);
    }
    fn text(&mut self, text: &mupdf::Text) {
        for span in text.spans() {
            for b in span.font().name().bytes() {
                self.int(b as i64);
            }
            self.matrix(&span.trm());
            for item in span.items() {
                self.int(item.gid() as i64);
                self.int(item.ucs() as i64);
                self.pos(item.x());
                self.pos(item.y());
            }
        }
    }
    fn path(&mut self, path: &mupdf::Path) {
        struct Walk<'a>(&'a mut Hash);
        impl mupdf::PathWalker for Walk<'_> {
            fn move_to(&mut self, x: f32, y: f32) {
                self.0.int(1);
                self.0.pos(x);
                self.0.pos(y);
            }
            fn line_to(&mut self, x: f32, y: f32) {
                self.0.int(2);
                self.0.pos(x);
                self.0.pos(y);
            }
            fn curve_to(&mut self, cx1: f32, cy1: f32, cx2: f32, cy2: f32, ex: f32, ey: f32) {
                self.0.int(3);
                for v in [cx1, cy1, cx2, cy2, ex, ey] {
                    self.0.pos(v);
                }
            }
            fn close(&mut self) {
                self.0.int(4);
            }
        }
        let _ = path.walk(Walk(self));
    }
}

impl Fingerprint {
    fn push(&mut self, h: Hash) {
        self.ops.borrow_mut().push(h.0);
    }
}

impl NativeDevice for Fingerprint {
    fn fill_path(&mut self, path: &mupdf::Path, even_odd: bool, ctm: Matrix, _: &Colorspace, color: &[f32], alpha: f32, _: mupdf::ColorParams) {
        let mut h = Hash::new(1);
        h.int(even_odd as i64);
        h.matrix(&ctm);
        h.color(color, alpha);
        h.path(path);
        self.push(h);
    }
    fn stroke_path(&mut self, path: &mupdf::Path, _: &mupdf::StrokeState, ctm: Matrix, _: &Colorspace, color: &[f32], alpha: f32, _: mupdf::ColorParams) {
        let mut h = Hash::new(2);
        h.matrix(&ctm);
        h.color(color, alpha);
        h.path(path);
        self.push(h);
    }
    fn clip_path(&mut self, path: &mupdf::Path, even_odd: bool, ctm: Matrix, _: mupdf::Rect) {
        let mut h = Hash::new(3);
        h.int(even_odd as i64);
        h.matrix(&ctm);
        h.path(path);
        self.push(h);
    }
    fn clip_stroke_path(&mut self, path: &mupdf::Path, _: &mupdf::StrokeState, ctm: Matrix, _: mupdf::Rect) {
        let mut h = Hash::new(4);
        h.matrix(&ctm);
        h.path(path);
        self.push(h);
    }
    fn fill_text(&mut self, text: &mupdf::Text, ctm: Matrix, _: &Colorspace, color: &[f32], alpha: f32, _: mupdf::ColorParams) {
        let mut h = Hash::new(5);
        h.matrix(&ctm);
        h.color(color, alpha);
        h.text(text);
        self.push(h);
    }
    fn stroke_text(&mut self, text: &mupdf::Text, _: &mupdf::StrokeState, ctm: Matrix, _: &Colorspace, color: &[f32], alpha: f32, _: mupdf::ColorParams) {
        let mut h = Hash::new(6);
        h.matrix(&ctm);
        h.color(color, alpha);
        h.text(text);
        self.push(h);
    }
    fn clip_text(&mut self, text: &mupdf::Text, ctm: Matrix, _: mupdf::Rect) {
        let mut h = Hash::new(7);
        h.matrix(&ctm);
        h.text(text);
        self.push(h);
    }
    fn clip_stroke_text(&mut self, text: &mupdf::Text, _: &mupdf::StrokeState, ctm: Matrix, _: mupdf::Rect) {
        let mut h = Hash::new(8);
        h.matrix(&ctm);
        h.text(text);
        self.push(h);
    }
    fn ignore_text(&mut self, text: &mupdf::Text, ctm: Matrix) {
        let mut h = Hash::new(9);
        h.matrix(&ctm);
        h.text(text);
        self.push(h);
    }
    fn fill_shade(&mut self, _: &mupdf::Shade, ctm: Matrix, alpha: f32, _: mupdf::ColorParams) {
        let mut h = Hash::new(10);
        h.matrix(&ctm);
        h.color(&[], alpha);
        self.push(h);
    }
    // Images: where they go matters, their pixels (which compression changes) do not.
    fn fill_image(&mut self, _: &mupdf::Image, ctm: Matrix, alpha: f32, _: mupdf::ColorParams) {
        let mut h = Hash::new(11);
        h.matrix(&ctm);
        h.color(&[], alpha);
        self.push(h);
    }
    fn fill_image_mask(&mut self, _: &mupdf::Image, ctm: Matrix, _: &Colorspace, color: &[f32], alpha: f32, _: mupdf::ColorParams) {
        let mut h = Hash::new(12);
        h.matrix(&ctm);
        h.color(color, alpha);
        self.push(h);
    }
    fn clip_image_mask(&mut self, _: &mupdf::Image, ctm: Matrix, _: mupdf::Rect) {
        let mut h = Hash::new(13);
        h.matrix(&ctm);
        self.push(h);
    }
    fn pop_clip(&mut self) {
        self.push(Hash::new(14));
    }
    fn end_group(&mut self) {
        self.push(Hash::new(15));
    }
}
