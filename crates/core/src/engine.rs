//! Synchronous document engine. Everything here runs on the worker thread
//! (MuPDF documents are not `Send`), or directly in tests.

use std::path::{Path, PathBuf};

use mupdf::pdf::{InsertPdfOptions, InsertPosition, PdfDocument, PdfPage, PdfWriteOptions};
use mupdf::{Colorspace, Document, Matrix, TextPageFlags};

pub type Result<T> = std::result::Result<T, String>;

pub(crate) fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

enum Doc {
    Pdf(PdfDocument),
    Other(Document),
}

impl Doc {
    fn base(&self) -> &Document {
        match self {
            Doc::Pdf(d) => d,
            Doc::Other(d) => d,
        }
    }
}

/// This thread's raw MuPDF context.
///
/// The safe wrapper keeps the pointer private, so we read it out of its one-field
/// `Context` struct. The compile-time check below fails if that struct ever changes size.
pub(crate) fn raw_context() -> *mut mupdf_sys::fz_context {
    const _: () = assert!(
        std::mem::size_of::<mupdf::Context>() == std::mem::size_of::<*mut mupdf_sys::fz_context>()
    );
    // SAFETY: `mupdf::Context` is a struct with a single `*mut fz_context` field.
    unsafe { std::mem::transmute::<mupdf::Context, *mut mupdf_sys::fz_context>(mupdf::Context::get()) }
}

/// Free MuPDF's cache of decoded images and other page resources.
///
/// MuPDF keeps up to 256 MB of these to speed up re-rendering. The reader already keeps
/// the rendered pages it needs, so once rendering goes idle this cache is just memory.
/// Call on the document thread.
pub fn release_render_cache() {
    // SAFETY: valid context for this thread; shrinking the store is always allowed.
    unsafe {
        mupdf_sys::fz_shrink_store(raw_context(), 0);
    }
}

/// A rendered page: one `u32` per pixel as `0x00RRGGBB`, row by row.
/// This is exactly the format the window's frame buffer uses, so drawing a page is a row copy.
pub struct PageImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl PageImage {
    pub fn bytes(&self) -> usize {
        self.pixels.len() * 4
    }
}

/// Render an SVG picture (for example a logo) to `width` pixels wide, keeping its shape.
/// Transparent areas come out white.
pub fn render_svg(svg: &[u8], width: usize) -> Result<PageImage> {
    let doc = Document::from_bytes(svg, "svg").map_err(err)?;
    let page = doc.load_page(0).map_err(err)?;
    let bounds = page.bounds().map_err(err)?;
    let scale = width as f32 / (bounds.x1 - bounds.x0).max(1.0);
    let pix = page
        .to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), false, true)
        .map_err(err)?;
    pixmap_to_image(&pix)
}

fn pixmap_to_image(pix: &mupdf::Pixmap) -> Result<PageImage> {
    let (w, h) = (pix.width() as usize, pix.height() as usize);
    let n = pix.n() as usize;
    let stride = pix.stride() as usize;
    let samples = pix.samples();
    if n < 3 || samples.len() < stride * h {
        return Err("Unexpected pixmap format".into());
    }
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &samples[y * stride..y * stride + w * n];
        pixels.extend(
            row.chunks_exact(n)
                .map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32),
        );
    }
    Ok(PageImage { width: w, height: h, pixels })
}

/// A rendered page as bytes: red, green and blue for each pixel, row by row. This is how
/// MuPDF draws it, so making one is a plain copy, and it is a quarter smaller than a
/// [`PageImage`]. The reader keeps its pages in this form.
pub struct PageRgb {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

fn pixmap_to_rgb(pix: &mupdf::Pixmap) -> Result<PageRgb> {
    let (w, h) = (pix.width() as usize, pix.height() as usize);
    let n = pix.n() as usize;
    let stride = pix.stride() as usize;
    let samples = pix.samples();
    if n < 3 || samples.len() < stride * h {
        return Err("Unexpected pixmap format".into());
    }
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = &samples[y * stride..y * stride + w * n];
        if n == 3 {
            rgb.extend_from_slice(row);
        } else {
            row.chunks_exact(n).for_each(|p| rgb.extend_from_slice(&p[..3]));
        }
    }
    Ok(PageRgb { width: w, height: h, rgb })
}

/// Error text when a document cannot be opened without a password.
pub const NEEDS_PASSWORD: &str = "This document needs a password";
/// Error text when the given password does not open the document.
pub const WRONG_PASSWORD: &str = "That password does not open this document";

/// One line of the document's outline (table of contents), flattened: `level` 0 is a
/// top entry, 1 sits under the entry before it, and so on.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    /// The page it leads to (0-based), if it leads to one.
    pub page: Option<usize>,
    pub level: usize,
}

/// Where a link on a page leads.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    /// Another page of this document (0-based).
    Page(usize),
    /// A web or mail address, as written in the document.
    Url(String),
}

/// A clickable area on a page, in page points: (x0, y0, x1, y1).
#[derive(Clone, Debug, PartialEq)]
pub struct PageLink {
    pub rect: (f32, f32, f32, f32),
    pub target: LinkTarget,
}

/// A search hit as a quad in page points: [ul, ur, ll, lr] as (x, y).
pub type HitQuad = [(f32, f32); 4];

pub struct Engine {
    doc: Doc,
    path: PathBuf,
    /// Page sizes in points (1/72 inch), with page rotation applied.
    sizes: Vec<(f32, f32)>,
    dirty: bool,
    /// Set when content was removed: the next save rewrites the whole file so the removed
    /// pages are really gone, instead of appending an update to the old bytes.
    removed_content: bool,
    /// The password this document was opened with, kept to reopen the file after a save.
    password: Option<String>,
    /// MuPDF's undo journal is on (it is switched on by the first [`Engine::edit`]).
    journal: bool,
}

impl Engine {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, None)
    }

    /// Open a document, with the password for a protected one. Without the right password
    /// the error is [`NEEDS_PASSWORD`] or [`WRONG_PASSWORD`].
    pub fn open_with(path: &Path, password: Option<&str>) -> Result<Self> {
        let path_str = path.to_str().ok_or("File path is not valid Unicode")?;
        crate::fonts::install();
        let mut doc = Document::open(path_str).map_err(err)?;
        let mut used = None;
        if doc.needs_password().map_err(err)? {
            match password {
                None => return Err(NEEDS_PASSWORD.into()),
                Some(pw) => {
                    if !doc.authenticate(pw).map_err(err)? {
                        return Err(WRONG_PASSWORD.into());
                    }
                    used = Some(pw.to_owned());
                }
            }
        }
        let doc = if doc.is_pdf() {
            Doc::Pdf(PdfDocument::try_from(doc).map_err(err)?)
        } else {
            Doc::Other(doc)
        };
        let mut engine = Self {
            doc,
            path: path.to_path_buf(),
            sizes: Vec::new(),
            dirty: false,
            removed_content: false,
            password: used,
            journal: false,
        };
        engine.refresh_sizes()?;
        Ok(engine)
    }

    fn refresh_sizes(&mut self) -> Result<()> {
        let doc = self.doc.base();
        let count = doc.page_count().map_err(err)?;
        let mut sizes = Vec::with_capacity(count.max(0) as usize);
        for i in 0..count {
            let size = match doc.load_page(i).and_then(|p| p.bounds()) {
                Ok(b) => ((b.x1 - b.x0).max(1.0), (b.y1 - b.y0).max(1.0)),
                // A broken page shouldn't stop the whole document from opening.
                Err(_) => sizes.last().copied().unwrap_or((595.0, 842.0)),
            };
            sizes.push(size);
        }
        self.sizes = sizes;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_pdf(&self) -> bool {
        matches!(self.doc, Doc::Pdf(_))
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    pub fn page_sizes(&self) -> &[(f32, f32)] {
        &self.sizes
    }

    pub fn title(&self) -> String {
        self.doc
            .base()
            .metadata(mupdf::MetadataName::Title)
            .unwrap_or_default()
    }

    /// Render a page at `scale` (1.0 = 72 dpi).
    /// The raw MuPDF handle of an open PDF, for the few calls the safe wrapper lacks.
    fn raw_pdf(&self) -> Option<*mut mupdf_sys::pdf_document> {
        let Doc::Pdf(doc) = &self.doc else { return None };
        const _: () = assert!(std::mem::size_of::<Document>() == std::mem::size_of::<*mut mupdf_sys::fz_document>());
        let base: &Document = doc;
        // SAFETY: `mupdf::Document` is a struct with a single `*mut fz_document` field (checked
        // above), and the document is alive for as long as `self` is borrowed.
        let pdf = unsafe {
            let fz = *(base as *const Document as *const *mut mupdf_sys::fz_document);
            mupdf_sys::pdf_document_from_fz_document(raw_context(), fz)
        };
        (!pdf.is_null()).then_some(pdf)
    }

    /// Make a change that can be undone: everything `change` does to the document becomes
    /// one undo step. If it fails, whatever it had already changed is rolled back.
    pub fn edit<T>(&mut self, change: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let Some(pdf) = self.raw_pdf() else { return change(self) };
        // SAFETY: valid context and document; an operation is always closed below.
        unsafe {
            if !self.journal {
                mupdf_sys::pdf_enable_journal(raw_context(), pdf);
                self.journal = true;
            }
            mupdf_sys::pdf_begin_operation(raw_context(), pdf, c"Edit".as_ptr());
        }
        let result = change(self);
        // The change may have replaced the document (it must not, but stay safe).
        if let Some(pdf) = self.raw_pdf() {
            // SAFETY: closes the operation opened above.
            unsafe {
                if result.is_ok() {
                    mupdf_sys::pdf_end_operation(raw_context(), pdf);
                } else {
                    mupdf_sys::pdf_abandon_operation(raw_context(), pdf);
                }
            }
        }
        if result.is_err() {
            let _ = self.refresh_sizes();
        }
        result
    }

    pub fn can_undo(&self) -> bool {
        // SAFETY: valid context and document.
        self.journal && self.raw_pdf().is_some_and(|pdf| unsafe { mupdf_sys::pdf_can_undo(raw_context(), pdf) != 0 })
    }

    pub fn can_redo(&self) -> bool {
        // SAFETY: valid context and document.
        self.journal && self.raw_pdf().is_some_and(|pdf| unsafe { mupdf_sys::pdf_can_redo(raw_context(), pdf) != 0 })
    }

    /// Take back the last change made through [`Engine::edit`].
    pub fn undo(&mut self) -> Result<()> {
        self.step_history(false)
    }

    /// Make the last undone change again.
    pub fn redo(&mut self) -> Result<()> {
        self.step_history(true)
    }

    fn step_history(&mut self, forward: bool) -> Result<()> {
        let possible = if forward { self.can_redo() } else { self.can_undo() };
        let Some(pdf) = self.raw_pdf().filter(|_| possible) else {
            return Err(if forward { "Nothing to redo" } else { "Nothing to undo" }.into());
        };
        // SAFETY: valid context and document; we checked that the step exists, which is the
        // only reason these calls fail.
        let position = unsafe {
            if forward {
                mupdf_sys::pdf_redo(raw_context(), pdf);
            } else {
                mupdf_sys::pdf_undo(raw_context(), pdf);
            }
            let mut steps = 0;
            mupdf_sys::pdf_undoredo_state(raw_context(), pdf, &mut steps)
        };
        // Back at the start of the history means back at what is in the file.
        self.dirty = position > 0;
        self.refresh_sizes()
    }

    /// The outline (bookmarks, table of contents) as a flat list. Empty when there is none.
    pub fn outline(&self) -> Vec<OutlineItem> {
        fn walk(items: &[mupdf::Outline], level: usize, count: usize, out: &mut Vec<OutlineItem>) {
            for item in items {
                let page = item.dest.map(|d| d.loc.page_number as usize).filter(|p| *p < count);
                let title = item.title.split_whitespace().collect::<Vec<_>>().join(" ");
                out.push(OutlineItem { title, page, level });
                // A broken file can nest without end; stop at a sensible depth and size.
                if level < 12 && out.len() < 20_000 {
                    walk(&item.down, level + 1, count, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Ok(items) = self.doc.base().outlines() {
            walk(&items, 0, self.sizes.len(), &mut out);
        }
        out
    }

    /// The links on a page.
    pub fn links(&self, page: usize) -> Result<Vec<PageLink>> {
        let loaded = self.doc.base().load_page(page as i32).map_err(err)?;
        let mut out = Vec::new();
        for link in loaded.links().map_err(err)? {
            let target = match link.dest {
                Some(dest) if (dest.loc.page_number as usize) < self.sizes.len() => LinkTarget::Page(dest.loc.page_number as usize),
                Some(_) => continue,
                None if link.uri.is_empty() => continue,
                None => LinkTarget::Url(link.uri),
            };
            let b = link.bounds;
            out.push(PageLink { rect: (b.x0, b.y0, b.x1, b.y1), target });
        }
        Ok(out)
    }

    pub fn render(&self, page: usize, scale: f32) -> Result<PageImage> {
        pixmap_to_image(&self.pixmap(page, scale)?)
    }

    /// Render a page at `scale` as red-green-blue bytes.
    pub fn render_rgb(&self, page: usize, scale: f32) -> Result<PageRgb> {
        pixmap_to_rgb(&self.pixmap(page, scale)?)
    }

    fn pixmap(&self, page: usize, scale: f32) -> Result<mupdf::Pixmap> {
        let p = self.doc.base().load_page(page as i32).map_err(err)?;
        let ctm = Matrix::new_scale(scale, scale);
        p.to_pixmap(&ctm, &Colorspace::device_rgb(), false, true).map_err(err)
    }

    pub fn page_text(&self, page: usize) -> Result<String> {
        let p = self.doc.base().load_page(page as i32).map_err(err)?;
        let tp = p.to_text_page(TextPageFlags::empty()).map_err(err)?;
        tp.to_text().map_err(err)
    }

    /// The text of pages `from..=to` (0-based), each introduced by `=== Page N ===`.
    /// Stops before `max_chars` is exceeded; returns the text and the last page included.
    pub fn pages_text(&self, from: usize, to: usize, max_chars: usize) -> Result<(String, usize)> {
        if from > to || to >= self.page_count() {
            return Err(format!("Pages {}-{} are not in this document (it has {} pages)", from + 1, to + 1, self.page_count()));
        }
        let mut out = String::new();
        let mut last = from;
        for page in from..=to {
            let text = self.page_text(page).unwrap_or_default();
            let text = text.trim();
            let body = if text.is_empty() { "(no text on this page; it may be a scan or a picture)" } else { text };
            if page > from && out.len() + body.len() > max_chars {
                break;
            }
            out.push_str(&format!("=== Page {} ===\n{body}\n\n", page + 1));
            last = page;
        }
        Ok((out, last))
    }

    pub fn search_page(&self, page: usize, needle: &str) -> Result<Vec<HitQuad>> {
        let p = self.doc.base().load_page(page as i32).map_err(err)?;
        let tp = p.to_text_page(TextPageFlags::empty()).map_err(err)?;
        let quads = tp.search(needle).map_err(err)?;
        Ok(quads
            .iter()
            .map(|q| {
                [
                    (q.ul.x, q.ul.y),
                    (q.ur.x, q.ur.y),
                    (q.ll.x, q.ll.y),
                    (q.lr.x, q.lr.y),
                ]
            })
            .collect())
    }

    pub(crate) fn base(&self) -> &Document {
        self.doc.base()
    }

    pub(crate) fn pdf(&self) -> Option<&PdfDocument> {
        match &self.doc {
            Doc::Pdf(d) => Some(d),
            Doc::Other(_) => None,
        }
    }

    /// Let go of the opened file (needed on Windows before replacing it).
    pub(crate) fn release_file(&mut self) {
        self.doc = Doc::Pdf(PdfDocument::new());
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn pdf_mut(&mut self) -> Result<&mut PdfDocument> {
        match &mut self.doc {
            Doc::Pdf(d) => Ok(d),
            Doc::Other(_) => Err("Page editing only works on PDF files".into()),
        }
    }

    fn check_pages(&self, pages: &[usize]) -> Result<()> {
        match pages.iter().find(|&&p| p >= self.sizes.len()) {
            Some(p) => Err(format!("Page {} does not exist", p + 1)),
            None => Ok(()),
        }
    }

    /// Rotate pages by `delta` degrees (a multiple of 90, may be negative).
    /// This changes the page's /Rotate entry, so it is saved with the file.
    pub fn rotate_pages(&mut self, pages: &[usize], delta: i32) -> Result<()> {
        if delta % 90 != 0 {
            return Err("Rotation must be a multiple of 90 degrees".into());
        }
        self.check_pages(pages)?;
        let doc = self.pdf_mut()?;
        for &page in pages {
            let mut p: PdfPage = doc.load_pdf_page(page as i32).map_err(err)?;
            let current = p.rotation().map_err(err)?;
            p.set_rotation((current + delta).rem_euclid(360)).map_err(err)?;
        }
        self.dirty = true;
        self.refresh_sizes()
    }

    pub fn delete_pages(&mut self, pages: &[usize]) -> Result<()> {
        self.check_pages(pages)?;
        let mut unique: Vec<usize> = pages.to_vec();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() >= self.sizes.len() {
            return Err("Can't delete every page of a document".into());
        }
        let doc = self.pdf_mut()?;
        doc.delete_pages(unique).map_err(err)?;
        self.dirty = true;
        self.removed_content = true;
        self.refresh_sizes()
    }

    /// Move the page at `from` so that it ends up at index `to`.
    pub fn move_page(&mut self, from: usize, to: usize) -> Result<()> {
        self.check_pages(&[from, to])?;
        let doc = self.pdf_mut()?;
        doc.move_page(from, to).map_err(err)?;
        self.dirty = true;
        self.refresh_sizes()
    }

    /// Move several pages at once: they are taken out and put back together, keeping their
    /// order, in front of the page that is at index `before` now (`before == page_count()`
    /// puts them at the end). Dragging a selection in the Pages panel does this.
    pub fn reorder_pages(&mut self, pages: &[usize], before: usize) -> Result<()> {
        let count = self.sizes.len();
        self.check_pages(pages)?;
        if before > count {
            return Err(format!("Page {} does not exist", before + 1));
        }
        let mut moving: Vec<usize> = pages.to_vec();
        moving.sort_unstable();
        moving.dedup();
        // The wanted order, as indices into the current order.
        let staying = |p: &usize| !moving.contains(p);
        let mut order: Vec<usize> = (0..before).filter(staying).collect();
        order.extend(&moving);
        order.extend((before..count).filter(staying));
        if order.iter().copied().eq(0..count) {
            return Ok(()); // nothing actually moves
        }
        // Get there one page at a time, tracking where each original page currently is.
        let doc = self.pdf_mut()?;
        let mut current: Vec<usize> = (0..count).collect();
        for (target, wanted) in order.iter().enumerate() {
            let at = current.iter().position(|p| p == wanted).expect("every page is in the list");
            if at != target {
                doc.move_page(at, target).map_err(err)?;
                let page = current.remove(at);
                current.insert(target, page);
            }
        }
        self.dirty = true;
        self.refresh_sizes()
    }

    /// Merge: copy every page of another PDF into this one. `before = None` appends at the
    /// end, `Some(i)` inserts before page `i`. Returns the number of pages added.
    pub fn insert_pdf(&mut self, other: &Path, before: Option<usize>) -> Result<usize> {
        if let Some(i) = before {
            if i > self.sizes.len() {
                return Err(format!("Page {} does not exist", i + 1));
            }
        }
        let other_str = other.to_str().ok_or("File path is not valid Unicode")?;
        let source = Document::open(other_str).map_err(err)?;
        if !source.is_pdf() {
            return Err("Only PDF files can be merged (for now)".into());
        }
        if source.needs_password().map_err(err)? {
            return Err("That PDF is password protected".into());
        }
        let source = PdfDocument::try_from(source).map_err(err)?;
        let added = source.page_count().map_err(err)?.max(0) as usize;
        let at_end = before.is_none_or(|i| i >= self.sizes.len());
        let options = InsertPdfOptions {
            target: match before {
                Some(i) if !at_end => InsertPosition::Before(i),
                _ => InsertPosition::Append,
            },
            ..Default::default()
        };
        let doc = self.pdf_mut()?;
        doc.insert_pdf(&source, options).map_err(err)?;
        self.dirty = true;
        self.refresh_sizes()?;
        Ok(added)
    }

    /// Save changes. `target = None` overwrites the opened file.
    ///
    /// MuPDF reads the open file lazily, so we never write over it directly: the new file
    /// is written next to the target, then swapped in, and the document is reopened.
    pub fn save(&mut self, target: Option<&Path>) -> Result<PathBuf> {
        let target = target.unwrap_or(&self.path).to_path_buf();
        let Doc::Pdf(doc) = &self.doc else {
            return Err("Only PDF files can be saved".into());
        };
        let mut tmp = target.clone().into_os_string();
        tmp.push(".rr-saving");
        let tmp = PathBuf::from(tmp);
        let tmp_str = tmp.to_str().ok_or("File path is not valid Unicode")?;

        // Two ways to save:
        // * Incremental: copy the original bytes and append only what changed. Fast, keeps the
        //   file size, and keeps existing digital signatures valid.
        // * Full rewrite: needed when pages were deleted (so their content is really removed)
        //   or when the file can't be updated incrementally (e.g. it was repaired on open).
        let incremental = !self.removed_content && doc.can_be_saved_incrementally();
        let mut opts = PdfWriteOptions::default();
        let written = if incremental {
            opts.set_incremental(true);
            std::fs::copy(&self.path, &tmp)
                .map_err(err)
                .and_then(|_| doc.save_with_options(tmp_str, opts).map_err(err))
        } else {
            // Drop unreferenced objects and keep streams compressed.
            opts.set_garbage_level(3).set_compress(true);
            doc.save_with_options(tmp_str, opts).map_err(err)
        };
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }

        // Release the old file handle before replacing the file (required on Windows).
        self.doc = Doc::Pdf(PdfDocument::new());
        let swap = std::fs::rename(&tmp, &target);
        let reopen = if swap.is_ok() { &target } else { &self.path };
        let password = self.password.clone();
        let reopened = Engine::open_with(reopen, password.as_deref());
        if let Err(e) = swap {
            let _ = std::fs::remove_file(&tmp);
            if let Ok(old) = reopened {
                // Keep the unsaved state visible to the caller: edits are lost from memory,
                // but the original file is untouched.
                *self = old;
            }
            return Err(format!("Could not write {}: {e}", target.display()));
        }
        *self = reopened?;
        Ok(target)
    }
}
