//! Printing. Each page is drawn by MuPDF at the printer's resolution (up to 300 dpi) and
//! sent to the printer as a picture, so what comes out is what the reader shows: comments,
//! filled form fields and signatures included.
//!
//! To keep print jobs small only the parts of the picture that have ink are sent; white paper
//! is not sent at all. The page is cut into pieces along its white space, the way one would
//! cut it with scissors: between columns, between lines, between words. Cuts never go through
//! ink (except in pictures taller than a strip), so no seam can show in letters. A piece
//! without colour is sent as grey, with 16 grey levels where it is text or line art (half a
//! byte per dot) and 256 where it is a photo; only coloured pieces take three bytes per dot.

use rogeha_core::Request;

use super::ReaderApp;

impl ReaderApp {
    pub(super) fn print(&mut self) {
        let Some(doc) = &self.doc else { return };
        if self.print_busy {
            self.info("Still printing the previous job");
            return;
        }
        let (name, count) = (document_name(doc), doc.page_sizes.len());
        #[cfg(windows)]
        if let Some(job) = win::ask(count) {
            self.print_busy = true;
            self.info("Printing…");
            self.worker.send(Request::WithEngine(Box::new(move |engine| {
                win::run(engine, job.hdc, job.from, job.to, &name, None)
            })));
        }
        #[cfg(not(windows))]
        {
            let _ = (name, count, Request::Close);
            self.error("Printing is not available on this system yet");
        }
    }

    /// Scripted check: print everything to a file through "Microsoft Print to PDF".
    pub(super) fn dev_print(&mut self, output: &str) {
        let Some(doc) = &self.doc else { return };
        let (name, count) = (document_name(doc), doc.page_sizes.len());
        #[cfg(windows)]
        match win::open_printer("Microsoft Print to PDF") {
            Some(hdc) => {
                self.print_busy = true;
                let output = std::path::PathBuf::from(output);
                self.worker.send(Request::WithEngine(Box::new(move |engine| {
                    win::run(engine, hdc, 0, count.saturating_sub(1), &name, Some(&output))
                })));
            }
            None => self.error("Printer not found"),
        }
        #[cfg(not(windows))]
        let _ = (name, count, output);
    }
}

fn document_name(doc: &rogeha_core::DocInfo) -> String {
    doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Document".into())
}

/// How many bits a piece of the page picture needs per pixel.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Depth {
    /// Black, white and the soft edges of letters: 16 grey levels.
    Gray4,
    /// Grey photos and shadings: 256 grey levels.
    Gray8,
    /// Anything with colour.
    Colour,
}

/// A piece of the page picture that has ink and is sent to the printer.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq)]
struct Piece {
    /// Columns and rows of the page picture: first, and the one after the last.
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    depth: Depth,
}

/// White space at least this wide (in dots, at up to 300 per inch) separates columns.
#[cfg_attr(not(windows), allow(dead_code))]
const COLUMN_GAP: usize = 24;
/// White space at least this wide separates words, where the depth may change.
#[cfg_attr(not(windows), allow(dead_code))]
const WORD_GAP: usize = 4;
/// Pieces are at most this tall: some printer drivers refuse very large bitmaps.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_ROWS: usize = 256;

#[cfg_attr(not(windows), allow(dead_code))]
const WHITE: u32 = 0x00FF_FFFF;

/// The stretches of `true` in `flags`, as (first, one after last), where stretches closer
/// together than `gap` count as one.
#[cfg_attr(not(windows), allow(dead_code))]
fn runs(flags: &[bool], gap: usize) -> Vec<(usize, usize)> {
    let mut found: Vec<(usize, usize)> = Vec::new();
    for (at, _) in flags.iter().enumerate().filter(|(_, flag)| **flag) {
        match found.last_mut() {
            Some(last) if at < last.1 + gap => last.1 = at + 1,
            _ => found.push((at, at + 1)),
        }
    }
    found
}

/// Cut the page picture into the pieces that have ink.
#[cfg_attr(not(windows), allow(dead_code))]
fn pieces(image: &rogeha_core::PageImage) -> Vec<Piece> {
    let mut found = Vec::new();
    cut(image, (0, image.width, 0, image.height), &mut found);
    found
}

/// Cut one rectangle (x0, x1, y0, y1) of the page: into bands at white rows, or else into
/// columns at wide white gaps, and each of those again, until nothing more can be cut.
#[cfg_attr(not(windows), allow(dead_code))]
fn cut(image: &rogeha_core::PageImage, (x0, x1, y0, y1): (usize, usize, usize, usize), found: &mut Vec<Piece>) {
    let mut rows = vec![false; y1 - y0];
    let mut columns = vec![false; x1 - x0];
    for y in y0..y1 {
        let row = &image.pixels[y * image.width + x0..y * image.width + x1];
        for (x, p) in row.iter().enumerate() {
            if p & WHITE != WHITE {
                rows[y - y0] = true;
                columns[x] = true;
            }
        }
    }
    let bands = runs(&rows, 1);
    let blocks = runs(&columns, COLUMN_GAP);
    let (Some(band), Some(block)) = (bands.first(), blocks.first()) else { return };
    if bands.len() > 1 {
        for (top, bottom) in bands {
            cut(image, (x0, x1, y0 + top, y0 + bottom), found);
        }
    } else if blocks.len() > 1 {
        for (left, right) in blocks {
            cut(image, (x0 + left, x0 + right, y0, y1), found);
        }
    } else {
        // One block of ink. Send it in strips of at most MAX_ROWS.
        let (left, right, top, bottom) = (x0 + block.0, x0 + block.1, y0 + band.0, y0 + band.1);
        for strip in (top..bottom).step_by(MAX_ROWS) {
            words(image, (left, right, strip, (strip + MAX_ROWS).min(bottom)), found);
        }
    }
}

/// Send one block of ink, split between words where the depth changes: a blue link in a
/// line of black text should not make the whole line cost three bytes per dot.
#[cfg_attr(not(windows), allow(dead_code))]
fn words(image: &rogeha_core::PageImage, (x0, x1, y0, y1): (usize, usize, usize, usize), found: &mut Vec<Piece>) {
    // Per column: any ink, any colour, and how many dots are neither dark nor light.
    let mut ink = vec![false; x1 - x0];
    let mut colour = vec![false; x1 - x0];
    let mut soft = vec![0u32; x1 - x0];
    for y in y0..y1 {
        let row = &image.pixels[y * image.width + x0..y * image.width + x1];
        for (x, p) in row.iter().enumerate() {
            if p & WHITE != WHITE {
                ink[x] = true;
                // Grey means red, green and blue are equal.
                colour[x] |= (p >> 8) & 0xFFFF != p & 0xFFFF;
                let level = p & 0xFF;
                soft[x] += u32::from(level > 40 && level < 215);
            }
        }
    }
    let first = found.len();
    for (left, right) in runs(&ink, WORD_GAP) {
        // Text and line art are nearly all black and white; a photo is mostly in between.
        let depth = if colour[left..right].contains(&true) {
            Depth::Colour
        } else if soft[left..right].iter().sum::<u32>() as usize * 4 > (right - left) * (y1 - y0) {
            Depth::Gray8
        } else {
            Depth::Gray4
        };
        let joins = found.len() > first;
        match found.last_mut() {
            Some(before) if joins && before.depth == depth => before.x1 = x0 + right,
            _ => found.push(Piece { x0: x0 + left, x1: x0 + right, y0, y1, depth }),
        }
    }
}

/// Pack a piece the way Windows wants bitmap rows: half a byte or one byte of grey, or
/// blue-green-red, per pixel, and each row padded to a multiple of four bytes.
#[cfg_attr(not(windows), allow(dead_code))]
fn pack(image: &rogeha_core::PageImage, piece: &Piece, out: &mut Vec<u8>) {
    out.clear();
    let pixels = piece.x1 - piece.x0;
    let stride = match piece.depth {
        Depth::Gray4 => (pixels.div_ceil(2) + 3) & !3,
        Depth::Gray8 => (pixels + 3) & !3,
        Depth::Colour => (pixels * 3 + 3) & !3,
    };
    for y in piece.y0..piece.y1 {
        let start = out.len();
        let row = &image.pixels[y * image.width + piece.x0..y * image.width + piece.x1];
        match piece.depth {
            Depth::Gray4 => {
                // 256 levels to 16, rounded; two pixels per byte, the left one in the high half.
                let level = |p: &u32| (((p & 0xFF) * 15 + 127) / 255) as u8;
                for pair in row.chunks(2) {
                    out.push(level(&pair[0]) << 4 | pair.get(1).map_or(0, level));
                }
            }
            Depth::Gray8 => out.extend(row.iter().map(|p| *p as u8)),
            Depth::Colour => {
                for p in row {
                    out.extend_from_slice(&[*p as u8, (*p >> 8) as u8, (*p >> 16) as u8]);
                }
            }
        }
        out.resize(start + stride, 0);
    }
}

/// Turn a picture a quarter turn clockwise.
#[cfg_attr(not(windows), allow(dead_code))]
fn quarter_turn(image: &rogeha_core::PageImage) -> rogeha_core::PageImage {
    let (w, h) = (image.width, image.height);
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..w {
        for x in 0..h {
            pixels.push(image.pixels[(h - 1 - x) * w + y]);
        }
    }
    rogeha_core::PageImage { width: h, height: w, pixels }
}

#[cfg(windows)]
mod win {
    use std::path::Path;

    use rogeha_core::Engine;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Graphics::Gdi::{
        BITMAPINFO, BITMAPINFOHEADER, COLORONCOLOR, CreateDCW, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps, HDC, HORZRES,
        LOGPIXELSX, RGBQUAD, SRCCOPY, SetStretchBltMode, StretchDIBits, VERTRES,
    };
    use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        PD_NOSELECTION, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW, PrintDlgW,
    };

    /// A printer chosen in the print dialog. The device context is a plain number here so it
    /// can be handed to the document thread, which does the printing and then frees it.
    pub struct Job {
        pub hdc: usize,
        /// First and last page to print, 0-based.
        pub from: usize,
        pub to: usize,
    }

    /// A bitmap header followed by the greys that grey pixels point into (16 or 256 of them).
    #[repr(C)]
    struct GrayHeader {
        header: BITMAPINFOHEADER,
        grays: [RGBQUAD; 256],
    }

    fn gray_header(levels: usize) -> GrayHeader {
        // SAFETY: both parts are plain numbers, for which all zeroes is a valid value.
        let mut gray: GrayHeader = unsafe { std::mem::zeroed() };
        gray.header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        gray.header.biPlanes = 1;
        gray.header.biClrUsed = levels as u32;
        for (step, entry) in gray.grays.iter_mut().take(levels).enumerate() {
            let level = (step * 255 / (levels - 1)) as u8;
            *entry = RGBQUAD { rgbBlue: level, rgbGreen: level, rgbRed: level, rgbReserved: 0 };
        }
        gray
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Show the system print dialog. `None` when the user cancels.
    pub fn ask(page_count: usize) -> Option<Job> {
        if page_count == 0 {
            return None;
        }
        let last = page_count.min(u16::MAX as usize) as u16;
        // SAFETY: the structure is zeroed, sized, and only read back after the call.
        unsafe {
            let mut dialog: PRINTDLGW = std::mem::zeroed();
            dialog.lStructSize = std::mem::size_of::<PRINTDLGW>() as u32;
            dialog.Flags = PD_RETURNDC | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE;
            dialog.nFromPage = 1;
            dialog.nToPage = last;
            dialog.nMinPage = 1;
            dialog.nMaxPage = last;
            dialog.nCopies = 1;
            if PrintDlgW(&mut dialog) == 0 {
                return None;
            }
            if !dialog.hDevMode.is_null() {
                GlobalFree(dialog.hDevMode);
            }
            if !dialog.hDevNames.is_null() {
                GlobalFree(dialog.hDevNames);
            }
            if dialog.hDC.is_null() {
                return None;
            }
            let (from, to) = if dialog.Flags & PD_PAGENUMS != 0 {
                let from = (dialog.nFromPage.max(1) - 1) as usize;
                let to = (dialog.nToPage.max(1) - 1) as usize;
                (from.min(to), from.max(to).min(page_count - 1))
            } else {
                (0, page_count - 1)
            };
            Some(Job { hdc: dialog.hDC as usize, from, to })
        }
    }

    /// Open a printer by name, with its default settings and no dialog.
    pub fn open_printer(name: &str) -> Option<usize> {
        let name = wide(name);
        // SAFETY: `name` is a NUL-terminated wide string that outlives the call.
        let hdc = unsafe { CreateDCW(std::ptr::null(), name.as_ptr(), std::ptr::null(), std::ptr::null()) };
        (!hdc.is_null()).then_some(hdc as usize)
    }

    /// Print pages `from..=to`. Runs on the document thread; frees the printer when done.
    pub fn run(engine: &Engine, hdc: usize, from: usize, to: usize, name: &str, output: Option<&Path>) -> Result<String, String> {
        let hdc = hdc as HDC;
        let result = print_pages(engine, hdc, from, to, name, output);
        // SAFETY: the device context came from the print dialog or CreateDCW and is ours to free.
        unsafe {
            if result.is_err() {
                AbortDoc(hdc);
            }
            DeleteDC(hdc);
        }
        result
    }

    fn print_pages(engine: &Engine, hdc: HDC, from: usize, to: usize, name: &str, output: Option<&Path>) -> Result<String, String> {
        // SAFETY: plain GDI calls on a valid printer device context; every pointer passed
        // (names, pixels, bitmap header) outlives the call it is passed to.
        unsafe {
            let (paper_w, paper_h) = (GetDeviceCaps(hdc, HORZRES as i32), GetDeviceCaps(hdc, VERTRES as i32));
            let dpi = GetDeviceCaps(hdc, LOGPIXELSX as i32);
            if paper_w <= 0 || paper_h <= 0 || dpi <= 0 {
                return Err("The printer did not report its paper size".into());
            }
            let doc_name = wide(name);
            let out_name = output.map(|p| wide(&p.to_string_lossy()));
            let info = DOCINFOW {
                cbSize: std::mem::size_of::<DOCINFOW>() as i32,
                lpszDocName: doc_name.as_ptr(),
                lpszOutput: out_name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                lpszDatatype: std::ptr::null(),
                fwType: 0,
            };
            if StartDocW(hdc, &info) <= 0 {
                return Err("The printer did not accept the job".into());
            }
            // The picture never has more dots than the printer, so it is only ever enlarged;
            // plain enlarging keeps every dot as it was drawn and needs no smoothing between pieces.
            SetStretchBltMode(hdc, COLORONCOLOR);
            let sizes = engine.page_sizes();
            let mut printed = 0;
            let (mut full_bytes, mut sent_bytes, mut sent_pieces) = (0usize, 0usize, 0usize);
            let mut packed = Vec::new();
            let (mut gray4, mut gray8, mut colour) = (gray_header(16), gray_header(256), gray_header(2));
            colour.header.biClrUsed = 0;
            for page in from..=to.min(sizes.len().saturating_sub(1)) {
                let (w_pt, h_pt) = sizes[page];
                // Turn a landscape page on portrait paper (and the other way round).
                let turn = (w_pt > h_pt) != (paper_w > paper_h);
                let (sheet_w, sheet_h) = if turn { (h_pt, w_pt) } else { (w_pt, h_pt) };
                // Printer dots per point: real size, or smaller when the page does not fit.
                let k = (paper_w as f32 / sheet_w).min(paper_h as f32 / sheet_h).min(dpi as f32 / 72.0);
                let (dest_w, dest_h) = ((sheet_w * k).round() as i32, (sheet_h * k).round() as i32);
                let (dest_x, dest_y) = ((paper_w - dest_w) / 2, (paper_h - dest_h) / 2);
                let scale = k.min(300.0 / 72.0).min((40.0e6 / (w_pt * h_pt)).sqrt());
                let mut image = engine.render(page, scale).map_err(|e| format!("Page {}: {e}", page + 1))?;
                if turn {
                    image = super::quarter_turn(&image);
                }
                if StartPage(hdc) <= 0 {
                    return Err("The printer stopped the job".into());
                }
                // Send only the pieces of the picture that have ink.
                let (w, h) = (image.width, image.height);
                full_bytes += w * h * 4;
                let across = |x: usize| dest_x + (x as i64 * dest_w as i64 / w as i64) as i32;
                let down = |y: usize| dest_y + (y as i64 * dest_h as i64 / h as i64) as i32;
                for piece in super::pieces(&image) {
                    super::pack(&image, &piece, &mut packed);
                    sent_bytes += packed.len();
                    sent_pieces += 1;
                    let (bits, header) = match piece.depth {
                        super::Depth::Gray4 => (4, &mut gray4),
                        super::Depth::Gray8 => (8, &mut gray8),
                        super::Depth::Colour => (24, &mut colour),
                    };
                    header.header.biWidth = (piece.x1 - piece.x0) as i32;
                    header.header.biHeight = -((piece.y1 - piece.y0) as i32); // top-down
                    header.header.biBitCount = bits;
                    StretchDIBits(
                        hdc,
                        across(piece.x0),
                        down(piece.y0),
                        across(piece.x1) - across(piece.x0),
                        down(piece.y1) - down(piece.y0),
                        0,
                        0,
                        (piece.x1 - piece.x0) as i32,
                        (piece.y1 - piece.y0) as i32,
                        packed.as_ptr().cast(),
                        (&raw const *header).cast::<BITMAPINFO>(),
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                }
                if EndPage(hdc) <= 0 {
                    return Err("The printer stopped the job".into());
                }
                printed += 1;
            }
            if EndDoc(hdc) <= 0 {
                return Err("The printer did not finish the job".into());
            }
            // For the scripted check: how much smaller the job is than the full pictures.
            if std::env::var_os("ROGEHA_PRINT_STATS").is_some() {
                eprintln!("print: {printed} pages, {sent_bytes} bytes sent in {sent_pieces} pieces, {full_bytes} bytes as full pictures");
            }
            Ok(match printed {
                1 => "Sent 1 page to the printer".to_owned(),
                n => format!("Sent {n} pages to the printer"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Depth, Piece, pack, pieces, runs};
    use rogeha_core::PageImage;

    const W: u32 = 0x00FF_FFFF;

    /// A white page with the given dots: (x, y, colour).
    fn page(width: usize, height: usize, dots: &[(usize, usize, u32)]) -> PageImage {
        let mut pixels = vec![W; width * height];
        for (x, y, colour) in dots {
            pixels[y * width + x] = *colour;
        }
        PageImage { width, height, pixels }
    }

    #[test]
    fn blank_paper_is_not_sent() {
        assert_eq!(pieces(&page(40, 30, &[])), []);
    }

    #[test]
    fn stretches_closer_than_the_gap_count_as_one() {
        let flags = [false, true, false, false, true, true, false];
        assert_eq!(runs(&flags, 1), [(1, 2), (4, 6)]);
        assert_eq!(runs(&flags, 3), [(1, 6)]);
    }

    #[test]
    fn only_the_part_with_ink_is_sent() {
        let image = page(40, 30, &[(5, 10, 0), (7, 11, 0)]);
        assert_eq!(pieces(&image), [Piece { x0: 5, x1: 8, y0: 10, y1: 12, depth: Depth::Gray4 }]);
    }

    #[test]
    fn white_rows_and_wide_white_gaps_cut_the_page() {
        // Two lines; the second has two columns far apart.
        let image = page(100, 30, &[(5, 2, 0), (9, 2, 0), (5, 20, 0), (90, 20, 0)]);
        let found = pieces(&image);
        let boxes: Vec<_> = found.iter().map(|p| (p.x0, p.x1, p.y0, p.y1)).collect();
        assert_eq!(boxes, [(5, 10, 2, 3), (5, 6, 20, 21), (90, 91, 20, 21)]);
    }

    #[test]
    fn colour_and_photos_get_more_bits() {
        // A black word, then a red word after a word gap: two pieces of different depth.
        let found = pieces(&page(40, 3, &[(1, 1, 0), (2, 1, 0), (10, 1, 0xFF0000)]));
        assert_eq!(found.iter().map(|p| p.depth).collect::<Vec<_>>(), [Depth::Gray4, Depth::Colour]);
        // Mostly middle greys: a photo.
        let found = pieces(&page(4, 1, &[(0, 0, 0x808080), (1, 0, 0x707070), (2, 0, 0x909090), (3, 0, 0)]));
        assert_eq!(found[0].depth, Depth::Gray8);
    }

    #[test]
    fn rows_are_packed_and_padded_to_four_bytes() {
        let image = PageImage { width: 4, height: 2, pixels: vec![W, 0x000000, 0x808080, W, W, 0x112233, W, W] };
        let mut out = Vec::new();
        pack(&image, &Piece { x0: 1, x1: 3, y0: 0, y1: 2, depth: Depth::Gray8 }, &mut out);
        assert_eq!(out, [0x00, 0x80, 0, 0, 0x33, 0xFF, 0, 0]);
        // Blue, green, red, then one byte of padding; only the second row.
        pack(&image, &Piece { x0: 1, x1: 2, y0: 1, y1: 2, depth: Depth::Colour }, &mut out);
        assert_eq!(out, [0x33, 0x22, 0x11, 0]);
        // 16 grey levels: black is 0, middle grey 8, white 15; two pixels per byte.
        pack(&image, &Piece { x0: 0, x1: 3, y0: 0, y1: 1, depth: Depth::Gray4 }, &mut out);
        assert_eq!(out, [0xF0, 0x80, 0, 0]);
    }

    #[test]
    fn a_quarter_turn_moves_the_top_left_corner_to_the_top_right() {
        // 3 wide, 2 high:  1 2 3 / 4 5 6
        let image = rogeha_core::PageImage { width: 3, height: 2, pixels: vec![1, 2, 3, 4, 5, 6] };
        let turned = super::quarter_turn(&image);
        assert_eq!((turned.width, turned.height), (2, 3));
        assert_eq!(turned.pixels, vec![4, 1, 5, 2, 6, 3]);
    }
}
