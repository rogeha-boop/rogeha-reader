//! Redaction: blacking out parts of pages for good.
//!
//! A black box drawn on top of a page hides nothing: the text under it can still be selected,
//! copied and read by any program. Redaction takes the content out instead. Under each marked
//! area MuPDF removes the text (visible text and the invisible text layer of a scan), the
//! picture dots (they are replaced in the picture itself, the rest of the picture stays) and the
//! drawings, and paints a black box where it was. Comments, links and form fields that overlap
//! an area are deleted too, and page thumbnails, which would still show the old page, are
//! dropped. The next save rewrites the whole file, so the removed content is not left in the
//! old bytes of the file either.

use mupdf::pdf::{PdfAnnotationType, PdfRedactImageMethod, PdfRedactLineArtMethod, PdfRedactOptions, PdfRedactTextMethod};
use mupdf::Rect;

use crate::engine::{Engine, Result, err};

/// An area to black out, in page points with the origin at the top left (as everywhere in
/// the engine), as (x0, y0, x1, y1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RedactArea {
    pub page: usize,
    pub rect: (f32, f32, f32, f32),
}

/// What a redaction did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RedactReport {
    pub areas: usize,
    pub pages: usize,
    /// Comments, links and form fields that overlapped an area and were deleted.
    pub annotations_removed: usize,
}

const OPTIONS: PdfRedactOptions = PdfRedactOptions {
    black_boxes: true,
    // Only the dots under the area are blacked out; the rest of a picture (a logo, a scan) stays.
    image_method: PdfRedactImageMethod::Pixels,
    // Lines and shapes inside an area go; a table's frame running through it stays.
    line_art: PdfRedactLineArtMethod::RemoveIfCovered,
    text: PdfRedactTextMethod::Remove,
};

fn normalised((x0, y0, x1, y1): (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
}

fn overlaps(a: (f32, f32, f32, f32), b: &Rect) -> bool {
    a.0 < b.x1 && b.x0 < a.2 && a.1 < b.y1 && b.y0 < a.3
}

impl Engine {
    /// Black out the given areas for good (see the module notes). Areas smaller than half a
    /// point are ignored. Works on PDF files only; one undo step.
    pub fn redact(&mut self, areas: &[RedactArea]) -> Result<RedactReport> {
        let areas: Vec<RedactArea> = areas
            .iter()
            .map(|a| RedactArea { page: a.page, rect: normalised(a.rect) })
            .filter(|a| a.rect.2 - a.rect.0 >= 0.5 && a.rect.3 - a.rect.1 >= 0.5)
            .collect();
        if areas.is_empty() {
            return Err("Mark an area to black out first".into());
        }
        if let Some(a) = areas.iter().find(|a| a.page >= self.page_count()) {
            return Err(format!("Page {} does not exist", a.page + 1));
        }
        self.pdf_mut()?;
        let mut pages: Vec<usize> = areas.iter().map(|a| a.page).collect();
        pages.sort_unstable();
        pages.dedup();

        let mut report = RedactReport { areas: areas.len(), pages: pages.len(), annotations_removed: 0 };
        for &page in &pages {
            let here: Vec<(f32, f32, f32, f32)> = areas.iter().filter(|a| a.page == page).map(|a| a.rect).collect();
            let mut p = self.pdf_mut()?.load_pdf_page(page as i32).map_err(err)?;

            // Whatever sits on top of the page in an area goes first: a comment there could
            // quote what is being hidden, a link could point at it, a field could hold it.
            let covered: Vec<_> = p
                .annotations()
                .filter(|a| a.r#type().is_ok_and(|t| t != PdfAnnotationType::Redact))
                .filter(|a| a.rect().or_else(|_| a.bounds()).is_ok_and(|r| here.iter().any(|&h| overlaps(h, &r))))
                .collect();
            report.annotations_removed += covered.len();
            for annot in covered {
                p.delete_annotation(annot).map_err(err)?;
            }
            // Links are stored as annotations too, but MuPDF keeps them apart: take them out of
            // the page's /Annots list by hand. Their /Rect is in PDF space (origin bottom left).
            let ctm = p.ctm().map_err(err)?;
            if let Some(mut list) = p.object().get_dict("Annots").map_err(err)?.and_then(|a| a.resolve().ok().flatten()) {
                for i in (0..list.len().map_err(err)? as i32).rev() {
                    let Some(entry) = list.get_array(i).map_err(err)?.and_then(|e| e.resolve().ok().flatten()) else { continue };
                    let is_link = entry.get_dict("Subtype").ok().flatten().and_then(|s| s.as_name().ok()).is_some_and(|n| n == b"Link");
                    let Some(r) = entry.get_dict("Rect").ok().flatten().filter(|_| is_link) else { continue };
                    let n = |k: i32| r.get_array(k).ok().flatten().and_then(|v| v.as_float().ok()).unwrap_or(0.0);
                    let (a, b, c, d) = (n(0), n(1), n(2), n(3));
                    let rect = Rect::new(a.min(c), b.min(d), a.max(c), b.max(d)).transform(&ctm);
                    if here.iter().any(|&h| overlaps(h, &rect)) {
                        list.array_delete(i).map_err(err)?;
                        report.annotations_removed += 1;
                    }
                }
            }

            for &(x0, y0, x1, y1) in &here {
                let mut annot = p.create_annotation(PdfAnnotationType::Redact).map_err(err)?;
                annot.set_rect(Rect::new(x0, y0, x1, y1)).map_err(err)?;
            }
            p.apply_redactions_with_options(OPTIONS).map_err(err)?;

            // A thumbnail picture stored with the page would still show what was there.
            let mut obj = p.object();
            if obj.get_dict("Thumb").ok().flatten().is_some() {
                obj.dict_delete("Thumb").map_err(err)?;
            }
            let _ = p.update();
        }
        self.mark_removed_content();
        Ok(report)
    }
}
