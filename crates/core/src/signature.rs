//! Visual signatures: a drawn, typed or photographed signature stamped onto a page.
//!
//! The signature becomes part of the page's content (not an annotation), so every PDF reader
//! shows and prints it and it cannot be dragged away or hidden as a comment could:
//!
//! * drawn signatures are stored as smooth vector strokes (sharp at any zoom, a few hundred bytes);
//! * typed signatures are set in Times Italic, one of the fonts every PDF reader has built in;
//! * pictures are stored as an image with transparency, so the paper around the ink is gone.
//!
//! This is a *visual* signature only. It does not prove who signed or that the document is
//! unchanged; that is what the digital (certificate) signature is for.

use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Colorspace, Document, Font, Image, Matrix, Pixmap};

use crate::engine::{Engine, Result, err};
use crate::ocr::winansi;

/// What a signature looks like. Coordinates inside are relative to the signature's own box:
/// (0, 0) is its top-left corner and (1, 1) its bottom-right.
#[derive(Clone, Debug, PartialEq)]
pub enum SignatureArt {
    /// Pen strokes, each a list of points.
    Strokes(Vec<Vec<(f32, f32)>>),
    /// A typed name.
    Text(String),
    /// A picture: straight-alpha RGBA, `width * height * 4` bytes.
    Image { rgba: Vec<u8>, width: usize, height: usize },
}

/// Ink colours offered for drawn and typed signatures.
pub const INK_BLACK: [u8; 3] = [0x14, 0x18, 0x1c];
pub const INK_BLUE: [u8; 3] = [0x1d, 0x43, 0x68];

const TEXT_FONT: &str = "Times-Italic";
/// Typed signatures: the letters' size as a share of the box height (room for descenders).
const TEXT_SIZE: f32 = 0.74;
const TEXT_BASELINE: f32 = 0.76;
/// Pen width as a share of the box height.
const PEN_WIDTH: f32 = 0.028;

impl SignatureArt {
    /// Width divided by height of the signature's natural box.
    pub fn aspect(&self) -> f32 {
        match self {
            // Strokes are normalised to their own bounding box by the caller, who knows its shape.
            SignatureArt::Strokes(_) => 3.0,
            SignatureArt::Text(text) => (text_width(text) * TEXT_SIZE).clamp(0.5, 30.0),
            SignatureArt::Image { width, height, .. } => *width as f32 / (*height).max(1) as f32,
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            SignatureArt::Strokes(s) => s.iter().all(|stroke| stroke.len() < 2),
            SignatureArt::Text(t) => t.trim().is_empty(),
            SignatureArt::Image { rgba, .. } => rgba.is_empty(),
        }
    }
}

/// Width of a typed signature at font size 1.
fn text_width(text: &str) -> f32 {
    let Ok(font) = Font::new(TEXT_FONT) else {
        return text.chars().count() as f32 * 0.45;
    };
    text.chars()
        .filter(|c| winansi(*c).is_some())
        .map(|c| font.encode_character(c as i32).and_then(|g| font.advance_glyph(g)).unwrap_or(0.45))
        .sum()
}

/// Load a picture of a signature (PNG, JPEG, ...; `extension` is the file's extension).
///
/// With `remove_paper`, light areas become transparent so only the ink remains, and the
/// picture is cropped to the ink. Returns straight-alpha RGBA and its size.
pub fn load_signature_image(bytes: &[u8], extension: &str, remove_paper: bool) -> Result<SignatureArt> {
    const MAX_SIDE: f32 = 1400.0;
    let doc = Document::from_bytes(bytes, &extension.to_ascii_lowercase())
        .map_err(|_| "This picture could not be read. Use a PNG or JPEG file.".to_owned())?;
    let page = doc.load_page(0).map_err(err)?;
    let b = page.bounds().map_err(err)?;
    let scale = (MAX_SIDE / (b.x1 - b.x0).max(b.y1 - b.y0).max(1.0)).min(1.0);
    let pix = page
        .to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), false, false)
        .map_err(err)?;
    let (w, h, n, stride) = (pix.width() as usize, pix.height() as usize, pix.n() as usize, pix.stride() as usize);
    let samples = pix.samples();
    if n < 3 || w == 0 || h == 0 {
        return Err("This picture could not be read.".into());
    }

    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for p in samples[y * stride..y * stride + w * n].chunks_exact(n) {
            let alpha = if remove_paper {
                // Paper is light, ink is dark: fade from opaque (dark) to clear (light).
                let lum = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
                const INK: u32 = 120; // this dark or darker: fully kept
                const PAPER: u32 = 205; // this light or lighter: fully removed
                (PAPER.saturating_sub(lum) * 255 / (PAPER - INK)).min(255) as u8
            } else {
                255
            };
            rgba.extend_from_slice(&[p[0], p[1], p[2], alpha]);
        }
    }
    if !remove_paper {
        return Ok(SignatureArt::Image { rgba, width: w, height: h });
    }

    // Crop to the ink (with a little margin).
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if rgba[(y * w + x) * 4 + 3] > 40 {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 || y0 > y1 {
        return Err("No signature was found in this picture: it looks blank.".into());
    }
    let pad = 4;
    let (x0, y0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
    let (x1, y1) = ((x1 + pad + 1).min(w), (y1 + pad + 1).min(h));
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut cropped = Vec::with_capacity(cw * ch * 4);
    for y in y0..y1 {
        cropped.extend_from_slice(&rgba[(y * w + x0) * 4..(y * w + x1) * 4]);
    }
    Ok(SignatureArt::Image { rgba: cropped, width: cw, height: ch })
}

/// A smooth path through a stroke's points, as PDF path operators.
fn stroke_path(points: &[(f32, f32)], out: &mut String) {
    use std::fmt::Write;
    let _ = writeln!(out, "{:.2} {:.2} m", points[0].0, points[0].1);
    if points.len() == 2 {
        let _ = writeln!(out, "{:.2} {:.2} l", points[1].0, points[1].1);
    } else {
        // Curve from midpoint to midpoint with the points themselves as control points
        // (quadratic Béziers written as cubics), which rounds off the corners of the polyline.
        let mid = |a: (f32, f32), b: (f32, f32)| ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
        let mut from = points[0];
        for i in 1..points.len() - 1 {
            let (control, to) = (points[i], mid(points[i], points[i + 1]));
            let c1 = (from.0 + (control.0 - from.0) * 2.0 / 3.0, from.1 + (control.1 - from.1) * 2.0 / 3.0);
            let c2 = (to.0 + (control.0 - to.0) * 2.0 / 3.0, to.1 + (control.1 - to.1) * 2.0 / 3.0);
            let _ = writeln!(out, "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c", c1.0, c1.1, c2.0, c2.1, to.0, to.1);
            from = to;
        }
        let last = points[points.len() - 1];
        let _ = writeln!(out, "{:.2} {:.2} l", last.0, last.1);
    }
    out.push_str("S\n");
}

/// Put `value` into the page's `/Resources/<kind>/<name>` (creating the dictionaries).
fn add_page_resource(doc: &mut PdfDocument, page: i32, kind: &str, name: &str, value: PdfObject) -> Result<()> {
    let mut page_obj = doc.find_page(page).map_err(err)?;
    let mut resources = match page_obj.get_dict_inheritable("Resources").map_err(err)? {
        Some(r) => r,
        None => {
            page_obj.dict_put("Resources", doc.new_dict().map_err(err)?).map_err(err)?;
            page_obj.get_dict("Resources").map_err(err)?.ok_or("Could not add page resources")?
        }
    };
    let mut group = match resources.get_dict(kind).map_err(err)? {
        Some(g) => g,
        None => {
            resources.dict_put(kind, doc.new_dict().map_err(err)?).map_err(err)?;
            resources.get_dict(kind).map_err(err)?.ok_or("Could not add page resources")?
        }
    };
    group.dict_put(name, value).map_err(err)
}

/// Draw a signature into a page of `doc`. See [`Engine::stamp_signature`].
fn stamp(doc: &mut PdfDocument, page: i32, rect: (f32, f32, f32, f32), art: &SignatureArt, ink: [u8; 3]) -> Result<()> {
    use std::fmt::Write;
    let (x0, y0, x1, y1) = rect;
    let (w, h) = (x1 - x0, y1 - y0);
    if !(w > 1.0 && h > 1.0) || art.is_empty() {
        return Err("There is no signature to place".into());
    }
    let color = format!("{:.3} {:.3} {:.3}", ink[0] as f32 / 255.0, ink[1] as f32 / 255.0, ink[2] as f32 / 255.0);

    // Everything is drawn in "display" coordinates (origin top left, y down); this matrix
    // takes them to the page's own coordinate system, whatever its rotation or origin.
    let to_pdf = {
        let p = doc.load_pdf_page(page).map_err(err)?;
        p.ctm().map_err(err)?.invert().ok_or("Page has an unusable coordinate system")?
    };
    let mut content = format!("q\n{} {} {} {} {} {} cm\n", to_pdf.a, to_pdf.b, to_pdf.c, to_pdf.d, to_pdf.e, to_pdf.f);

    match art {
        SignatureArt::Strokes(strokes) => {
            let pen = (h * PEN_WIDTH).clamp(0.6, 3.0);
            let _ = writeln!(content, "{color} RG\n{pen:.2} w\n1 J\n1 j");
            for stroke in strokes.iter().filter(|s| s.len() >= 2) {
                let points: Vec<(f32, f32)> = stroke.iter().map(|(x, y)| (x0 + x * w, y0 + y * h)).collect();
                stroke_path(&points, &mut content);
            }
        }
        SignatureArt::Text(text) => {
            let name = "RRsig";
            let font = doc
                .add_object(
                    &doc.new_object_from_str(&format!(
                        "<< /Type /Font /Subtype /Type1 /BaseFont /{TEXT_FONT} /Encoding /WinAnsiEncoding >>"
                    ))
                    .map_err(err)?,
                )
                .map_err(err)?;
            add_page_resource(doc, page, "Font", name, font)?;
            // As large as fits the box in both directions.
            let natural = text_width(text).max(0.01);
            let size = (h * TEXT_SIZE).min(w / natural);
            let baseline = y0 + h * TEXT_BASELINE;
            let _ = write!(content, "BT\n{color} rg\n/{name} 1 Tf\n{size:.3} 0 0 {:.3} {x0:.3} {baseline:.3} Tm (", -size);
            for b in text.chars().filter_map(winansi) {
                if matches!(b, b'(' | b')' | b'\\') {
                    content.push('\\');
                }
                // Bytes above 127 are written as octal escapes so the stream stays ASCII.
                if b < 128 {
                    content.push(b as char);
                } else {
                    let _ = write!(content, "\\{b:03o}");
                }
            }
            content.push_str(") Tj\nET\n");
        }
        SignatureArt::Image { rgba, width, height } => {
            if rgba.len() != width * height * 4 {
                return Err("The signature picture is damaged".into());
            }
            // MuPDF wants premultiplied alpha; the PDF then stores colour and a soft mask.
            let mut pix = Pixmap::new_with_w_h(&Colorspace::device_rgb(), *width as i32, *height as i32, true).map_err(err)?;
            let stride = pix.stride() as usize;
            let samples = pix.samples_mut();
            for y in 0..*height {
                for x in 0..*width {
                    let p = &rgba[(y * width + x) * 4..][..4];
                    let a = p[3] as u32;
                    let o = y * stride + x * 4;
                    for c in 0..3 {
                        samples[o + c] = (p[c] as u32 * a / 255) as u8;
                    }
                    samples[o + 3] = p[3];
                }
            }
            let image = Image::from_pixmap(&pix).map_err(err)?;
            let object = doc.add_image(&image).map_err(err)?;
            let name = format!("RRsig{}", object.as_indirect().map_err(err)?);
            add_page_resource(doc, page, "XObject", &name, object)?;
            // Images are drawn in a unit square with y up; flip it into the box.
            let _ = writeln!(content, "{w:.3} 0 0 {:.3} {x0:.3} {y1:.3} cm\n/{name} Do", -h);
        }
    }
    content.push_str("Q\n");

    let mut p = doc.load_pdf_page(page).map_err(err)?;
    // Isolate the existing content's graphics state, then draw on top of it.
    p.wrap_contents(doc).map_err(err)?;
    p.insert_contents(doc, content.as_bytes(), true).map_err(err)?;
    Ok(())
}

/// A picture of the signature exactly as it will be stamped (same code path), for previews:
/// premultiplied RGBA on a transparent background, `width_px` wide, shaped by `aspect`.
pub fn render_signature(art: &SignatureArt, aspect: f32, ink: [u8; 3], width_px: usize) -> Result<(Vec<u8>, usize, usize)> {
    let (w, h) = (300.0, 300.0 / aspect.clamp(0.2, 30.0));
    let mut doc = PdfDocument::new();
    doc.new_page(mupdf::Size::new(w, h)).map_err(err)?;
    stamp(&mut doc, 0, (0.0, 0.0, w, h), art, ink)?;
    let scale = width_px.max(1) as f32 / w;
    let pix = doc
        .load_page(0)
        .map_err(err)?
        .to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), true, false)
        .map_err(err)?;
    let (pw, ph, stride) = (pix.width() as usize, pix.height() as usize, pix.stride() as usize);
    if pix.n() != 4 {
        return Err("Unexpected pixmap format".into());
    }
    let mut rgba = Vec::with_capacity(pw * ph * 4);
    for y in 0..ph {
        rgba.extend_from_slice(&pix.samples()[y * stride..y * stride + pw * 4]);
    }
    Ok((rgba, pw, ph))
}

impl Engine {
    /// Stamp a signature onto a page.
    ///
    /// `rect` is where it goes, as (x0, y0, x1, y1) in page points with the origin at the top
    /// left of the page *as displayed* (so it comes out upright on rotated pages too).
    /// `ink` colours drawn and typed signatures; pictures keep their own colours.
    pub fn stamp_signature(&mut self, page: usize, rect: (f32, f32, f32, f32), art: &SignatureArt, ink: [u8; 3]) -> Result<()> {
        if page >= self.page_count() {
            return Err(format!("Page {} does not exist", page + 1));
        }
        stamp(self.pdf_mut()?, page as i32, rect, art, ink)?;
        self.mark_dirty();
        Ok(())
    }
}
