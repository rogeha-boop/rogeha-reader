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
//! The resolution settings only matter for pictures. Many PDFs are big for another reason:
//! they carry data no reader shows. A file made for a print shop can hold a 1.5 MB colour
//! profile of the printing press and a copy of its metadata on every picture; design programs
//! leave their own working data in the file. That is taken out as well (see [`strip_extras`]),
//! and [`CompressStats::picture_bytes`] tells the reader how much of the file was pictures, so
//! it can say why the settings made little difference when they did.
//!
//! Embedded fonts are trimmed to the letters the document uses ([`trim_fonts`]): a file that
//! carries two whole fonts for a few lines of text carries a quarter of a megabyte for nothing.
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
    /// Keeps up to 144 dpi, JPEG quality 60, and drops the print colour profile. Fine print
    /// may soften.
    Small,
    /// Keeps up to 96 dpi, JPEG quality 55: screen resolution. For sending by e-mail. Also the
    /// only setting that still does something for a file whose pictures were already reduced
    /// to 150 dpi, which is what scanner apps and other compressors produce.
    Tiny,
}

impl Preset {
    /// From gentlest to strongest.
    pub const ALL: [Preset; 4] = [Preset::HighQuality, Preset::Balanced, Preset::Small, Preset::Tiny];

    pub fn id(self) -> &'static str {
        match self {
            Preset::HighQuality => "high",
            Preset::Balanced => "balanced",
            Preset::Small => "small",
            Preset::Tiny => "tiny",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::HighQuality => "High quality",
            Preset::Balanced => "Balanced",
            Preset::Small => "Small file",
            Preset::Tiny => "Smallest file",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Preset::HighQuality => "Up to 300 dpi. For printing and archiving.",
            Preset::Balanced => "Up to 200 dpi. Looks the same on screen. Recommended.",
            Preset::Small => "Up to 144 dpi. Very fine print may soften, and print colours may shift a shade.",
            Preset::Tiny => "Up to 96 dpi. For e-mail: readable on screen, soft when zoomed in or printed.",
        }
    }

    /// Is the colour profile of the printing press (the PDF's "output intent") kept?
    /// Without it, colours given in printing inks come out a shade different (measured on a
    /// brand red: 227, 0, 14 became 237, 28, 36). Only the smallest preset accepts that.
    fn keeps_print_profile(self) -> bool {
        matches!(self, Preset::HighQuality | Preset::Balanced)
    }

    /// The resolution above which pictures are scaled down, in dots per inch.
    pub fn dpi(self) -> u32 {
        self.settings().0 as u32
    }

    /// (target dpi, JPEG quality)
    fn settings(self) -> (i32, &'static CStr) {
        match self {
            Preset::HighQuality => (300, c"85"),
            Preset::Balanced => (200, c"75"),
            Preset::Small => (144, c"60"),
            Preset::Tiny => (96, c"55"),
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
    /// How many bytes of the original file were pictures. The resolution settings change
    /// only these.
    pub picture_bytes: u64,
    /// The resolution the pictures were stored at in the original, in dots per inch: the
    /// value that half of all picture dots lie above. 0 when there are no pictures. A setting
    /// whose limit is above this has little to reduce.
    pub picture_dpi: u32,
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

/// What a raw pass does to the document before writing it out tidily.
#[derive(Clone, Copy, PartialEq)]
enum Work {
    /// Nothing more.
    Tidy,
    /// Re-encode the pictures.
    Images,
    /// Trim the embedded fonts to the glyphs in use.
    Fonts,
}

/// One raw MuPDF pass: open `input`, do `work`, and write a tidy `output`. Returns the page
/// count.
fn raw_pass(input: &Path, output: &Path, preset: Preset, work: Work) -> Result<usize> {
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

        match work {
            Work::Images => sys::pdf_rewrite_images(ctx, pdf, &mut o),
            Work::Fonts => sys::pdf_subset_fonts(ctx, pdf, 0, ptr::null()),
            Work::Tidy => {}
        }

        let mut w = sys::pdf_default_write_options;
        w.do_garbage = 4; // drop unused objects and merge identical ones
        w.do_compress = 1;
        w.do_compress_images = 1;
        w.do_compress_fonts = 1;
        w.do_use_objstms = 1;
        w.compression_effort = 100; // pack text and drawings as tightly as zlib can
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

/// Compress `input` into `output` (which must be a different file), without trimming fonts.
pub fn compress_pdf(input: &Path, output: &Path, preset: Preset) -> Result<CompressStats> {
    compress_pdf_with(input, output, preset, None)
}

/// Compress `input` into `output` (which must be a different file).
///
/// `font_helper` is a program that trims fonts when started as
/// `<program> --subset-fonts <in.pdf> <out.pdf>` (see [`subset_fonts`]); the reader passes
/// itself. Without one, fonts are left as they are. It is asked for, never guessed: starting
/// "this program" blindly would start whatever happens to be running this code.
pub fn compress_pdf_with(input: &Path, output: &Path, preset: Preset, font_helper: Option<&Path>) -> Result<CompressStats> {
    if input == output {
        return Err("Choose a different file name for the compressed copy".into());
    }
    let bytes_before = std::fs::metadata(input).map_err(|e| e.to_string())?.len();
    let temp = |tag: &str| {
        let mut name = output.as_os_str().to_owned();
        name.push(tag);
        std::path::PathBuf::from(name)
    };
    let temps = [temp(".rr-1"), temp(".rr-2"), temp(".rr-3"), temp(".rr-5")];
    let candidate = temp(".rr-4");
    let cleanup = || {
        for file in temps.iter().chain([&candidate]) {
            let _ = std::fs::remove_file(file);
        }
    };
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX);

    let run = || -> Result<(Outcome, u64, u32)> {
        let picture_bytes = picture_bytes(input)?;
        let picture_dpi = picture_dpi(input)?;
        let mut best = build(input, output, Some(preset), preset, &temps, font_helper)?;
        // Re-encoding a picture has side costs that MuPDF's own "only when smaller" check does
        // not see: the page's drawing instructions are written anew, and a picture in printing
        // inks gets a colour profile of its own (139 KB in one print-shop file, to save 19 KB of
        // picture). Where pictures are a small part of the file such costs can outweigh the
        // saving, so there the gentler settings and "pictures left alone" are tried as well and
        // the smallest result is kept. A stronger setting then never gives a larger file. (Where
        // pictures are most of the file this cannot happen, and trying would only cost time.)
        if picture_bytes * 4 < bytes_before {
            let gentler = Preset::ALL.into_iter().take_while(|p| *p != preset).map(Some);
            for images in gentler.chain([None]) {
                let outcome = build(input, &candidate, images, preset, &temps, font_helper)?;
                if size(&candidate) < size(output) {
                    std::fs::copy(&candidate, output).map_err(|e| e.to_string())?;
                    best = outcome;
                }
            }
        }
        Ok((best, picture_bytes, picture_dpi))
    };
    let result = run();
    cleanup();
    let (outcome, picture_bytes, picture_dpi) = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(output);
            return Err(e);
        }
    };
    let bytes_after = std::fs::metadata(output).map_err(|e| e.to_string())?.len();
    Ok(CompressStats {
        bytes_before,
        bytes_after,
        pages: outcome.pages,
        pages_restored: outcome.pages_restored,
        images_untouched: outcome.images_untouched,
        picture_bytes,
        picture_dpi,
    })
}

/// What one way of building the compressed file did.
struct Outcome {
    pages: usize,
    pages_restored: usize,
    /// The pictures had to be left alone to keep the text exact.
    images_untouched: bool,
}

/// Build one compressed copy of `input` at `output`: pictures re-encoded as `images` says
/// (`None` leaves them alone), unseen data taken out as `preset` says, fonts trimmed. `temps`
/// are four scratch files.
fn build(
    input: &Path,
    output: &Path,
    images: Option<Preset>,
    preset: Preset,
    temps: &[std::path::PathBuf; 4],
    font_helper: Option<&Path>,
) -> Result<Outcome> {
    let [step1, step2, step3, step4] = temps;
    // The last two steps of every way: trim the fonts, then the final tidy write (object
    // streams, duplicate removal).
    let finish = |stripped: &Path| -> Result<usize> {
        let trimmed = font_helper.is_some_and(|helper| trim_fonts(helper, stripped, step4));
        let source = if trimmed { step4.as_path() } else { stripped };
        raw_pass(source, output, preset, Work::Tidy)
    };
    let Some(images) = images else {
        strip_extras(input, step3, preset)?;
        let pages = finish(step3)?;
        return Ok(Outcome { pages, pages_restored: 0, images_untouched: false });
    };
    // 1. Re-encode images.
    let pages = raw_pass(input, step1, images, Work::Images)?;
    // 2. Check every page; put back the content of pages that changed beyond their images.
    match restore_changed_pages(input, step1, step2)? {
        Some(pages_restored) => {
            // 3. Take out what no reader shows.
            strip_extras(step2, step3, preset)?;
            // 4. Fonts, and the final write.
            finish(step3)?;
            Ok(Outcome { pages, pages_restored, images_untouched: false })
        }
        None => {
            // Could not get an exact match: do not risk it, keep the images as they are.
            strip_extras(input, step3, preset)?;
            finish(step3)?;
            Ok(Outcome { pages, pages_restored: 0, images_untouched: true })
        }
    }
}

/// The helper behind [`trim_fonts`]: trim every embedded font of `input` to the glyphs the
/// document uses and write the result to `output`. Run it in a process of its own
/// (`rogeha-reader --subset-fonts <in> <out>`): MuPDF ends the process when a font is too
/// damaged to trim, and that must not end the compression.
pub fn subset_fonts(input: &Path, output: &Path) -> Result<()> {
    raw_pass(input, output, Preset::Balanced, Work::Fonts).map(|_| ())
}

/// Trim the embedded fonts of `source` into `target`. Returns whether `target` is to be used.
///
/// The result is only taken when it is safe and worth it:
///
/// * not for a form: a field that is filled in later needs letters nobody has typed yet;
/// * the trimming runs in a process of its own (`helper`), so a font MuPDF cannot handle
///   costs the saving, not the whole compression;
/// * every page must come out exactly as before, dot for dot;
/// * and the file must be smaller.
fn trim_fonts(helper: &Path, source: &Path, target: &Path) -> bool {
    let _ = std::fs::remove_file(target);
    let is_form = || {
        let doc = PdfDocument::open(source.to_str()?).ok()?;
        doc.has_acro_form().ok()
    };
    if is_form() != Some(false) {
        return false;
    }
    let mut helper = std::process::Command::new(helper);
    helper.arg("--subset-fonts").arg(source).arg(target);
    helper.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        helper.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    let worked = helper.status().is_ok_and(|status| status.success());
    let keep = worked && matches!((size(target), size(source)), (Some(new), Some(old)) if new < old) && same_look(source, target) == Ok(true);
    if !keep {
        let _ = std::fs::remove_file(target);
    }
    keep
}

/// Do two documents show exactly the same on every page? Rendered at 72 dpi and compared dot
/// for dot.
fn same_look(a: &Path, b: &Path) -> Result<bool> {
    let (a, b) = (crate::engine::Engine::open(a)?, crate::engine::Engine::open(b)?);
    if a.page_count() != b.page_count() {
        return Ok(false);
    }
    for page in 0..a.page_count() {
        let (one, other) = (a.render(page, 1.0)?, b.render(page, 1.0)?);
        if one.width != other.width || one.pixels != other.pixels {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The resolution the document's pictures are stored at: every picture's dots per inch as it
/// is placed on its page, and of those the value that half of all picture dots lie above.
fn picture_dpi(input: &Path) -> Result<u32> {
    let doc = PdfDocument::open(input.to_str().ok_or("File path is not valid Unicode")?).map_err(err)?;
    let found = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    for page in 0..doc.page_count().map_err(err)? {
        let device = Device::from_native(Resolutions { found: found.clone() }).map_err(err)?;
        // A page that cannot be drawn has no pictures to count.
        if let Ok(p) = doc.load_page(page) {
            let _ = p.run(&device, &Matrix::IDENTITY);
        }
    }
    let mut found: Vec<(u32, u64)> = found.take();
    found.sort_unstable();
    let half = found.iter().map(|(_, dots)| dots).sum::<u64>() / 2;
    let mut below = 0;
    for (dpi, dots) in found {
        below += dots;
        if below >= half {
            return Ok(dpi);
        }
    }
    Ok(0)
}

/// Collects (dots per inch, number of dots) for every picture a page draws.
struct Resolutions {
    found: std::rc::Rc<std::cell::RefCell<Vec<(u32, u64)>>>,
}

impl Resolutions {
    fn note(&mut self, image: &mupdf::Image, ctm: Matrix) {
        // A picture is drawn into the unit square, which `ctm` maps onto the page (in points).
        let width = (ctm.a * ctm.a + ctm.b * ctm.b).sqrt();
        let height = (ctm.c * ctm.c + ctm.d * ctm.d).sqrt();
        if width > 0.5 && height > 0.5 {
            let dpi = (image.width() as f32 * 72.0 / width).max(image.height() as f32 * 72.0 / height);
            self.found.borrow_mut().push((dpi.round() as u32, image.width() as u64 * image.height() as u64));
        }
    }
}

impl NativeDevice for Resolutions {
    fn fill_image(&mut self, image: &mupdf::Image, ctm: Matrix, _: f32, _: mupdf::ColorParams) {
        self.note(image, ctm);
    }
}

/// Every object of the document that is a dictionary (streams included): its number, the
/// reference to it (a stream's data is read through the reference) and the dictionary itself.
fn dictionaries(doc: &PdfDocument) -> Result<Vec<(i32, mupdf::pdf::PdfObject, mupdf::pdf::PdfObject)>> {
    let count = doc.count_objects().map_err(err)? as i32;
    let mut found = Vec::new();
    for number in 1..count {
        // A number that is free or cannot be read is simply not a dictionary.
        let Ok(reference) = doc.new_indirect(number, 0) else { continue };
        let Ok(Some(object)) = reference.resolve() else { continue };
        if object.is_dict().unwrap_or(false) {
            found.push((number, reference, object));
        }
    }
    Ok(found)
}

fn name_is(object: &mupdf::pdf::PdfObject, key: &str, name: &str) -> bool {
    matches!(object.get_dict(key), Ok(Some(value)) if value.as_name().is_ok_and(|n| n == name.as_bytes()))
}

/// How many bytes of the file are pictures, as they are stored.
fn picture_bytes(input: &Path) -> Result<u64> {
    let doc = PdfDocument::open(input.to_str().ok_or("File path is not valid Unicode")?).map_err(err)?;
    let mut total = 0;
    for (_, reference, object) in dictionaries(&doc)? {
        if name_is(&object, "Subtype", "Image") {
            total += reference.read_raw_stream().map(|bytes| bytes.len() as u64).unwrap_or(0);
        }
    }
    Ok(total)
}

/// Take out data that no reader shows, and write the result to `target`:
///
/// * `/Metadata` on anything but the document itself: design programs attach a copy of a
///   picture's or a page's metadata to each of them (the document's own stays);
/// * `/PieceInfo`: the private working data of the program that made the file;
/// * `/Thumb`: page thumbnails, which readers draw themselves;
/// * `/OutputIntents`: the colour profile of the printing press, often the largest object in a
///   file made for a print shop. Only the smallest preset takes it out, because colours shift
///   a shade without it, and never from PDF/A files, which are archive copies and must carry it.
///
/// Whatever these pointed at is dropped when the file is written, unless something that is
/// shown uses it too.
fn strip_extras(source: &Path, target: &Path, preset: Preset) -> Result<()> {
    let path = |p: &Path| p.to_str().map(str::to_owned).ok_or("File path is not valid Unicode");
    let doc = PdfDocument::open(path(source)?.as_str()).map_err(err)?;
    let mut catalog = doc.catalog().map_err(err)?;
    let catalog_number = doc.trailer().ok().and_then(|t| t.get_dict("Root").ok().flatten()).and_then(|r| r.as_indirect().ok());
    for (number, _, mut object) in dictionaries(&doc)? {
        let mut unseen = vec!["PieceInfo", "Thumb"];
        if Some(number) != catalog_number {
            unseen.push("Metadata");
        }
        for key in unseen {
            if matches!(object.get_dict(key), Ok(Some(_))) {
                object.dict_delete(key).map_err(err)?;
            }
        }
    }
    if !preset.keeps_print_profile() && !is_pdf_a(&catalog) && matches!(catalog.get_dict("OutputIntents"), Ok(Some(_))) {
        catalog.dict_delete("OutputIntents").map_err(err)?;
    }
    let mut options = PdfWriteOptions::default();
    options.set_garbage_level(3).set_compress(true);
    doc.save_with_options(path(target)?.as_str(), options).map_err(err)
}

/// Does the document say it is a PDF/A archive copy? (Its metadata then names the PDF/A part.)
fn is_pdf_a(catalog: &mupdf::pdf::PdfObject) -> bool {
    let Ok(Some(metadata)) = catalog.get_dict("Metadata") else { return false };
    metadata.read_stream().is_ok_and(|xml| xml.windows(7).any(|w| w == b"pdfaid:"))
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
