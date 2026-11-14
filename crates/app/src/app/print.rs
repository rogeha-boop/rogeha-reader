//! Printing. Each page is drawn by MuPDF at the printer's resolution (up to 300 dpi) and
//! sent to the printer as a picture, so what comes out is what the reader shows: comments,
//! filled form fields and signatures included.
//!
//! The system print dialog chooses the printer, the page range and the copies. Pages keep
//! their real size and are only made smaller when they do not fit the paper; a landscape
//! page on portrait paper is turned.

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
        BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps, HALFTONE, HDC, HORZRES,
        LOGPIXELSX, SRCCOPY, SetStretchBltMode, StretchDIBits, VERTRES,
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
            SetStretchBltMode(hdc, HALFTONE);
            let sizes = engine.page_sizes();
            let mut printed = 0;
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
                // Send the picture in strips: some printer drivers refuse very large bitmaps.
                let (w, h) = (image.width, image.height);
                let mut row = 0;
                while row < h {
                    let rows = (h - row).min(256);
                    let y0 = dest_y + (row as i64 * dest_h as i64 / h as i64) as i32;
                    let y1 = dest_y + ((row + rows) as i64 * dest_h as i64 / h as i64) as i32;
                    let mut header: BITMAPINFO = std::mem::zeroed();
                    header.bmiHeader = BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w as i32,
                        biHeight: -(rows as i32), // top-down
                        biPlanes: 1,
                        biBitCount: 32,
                        ..std::mem::zeroed()
                    };
                    StretchDIBits(
                        hdc,
                        dest_x,
                        y0,
                        dest_w,
                        y1 - y0,
                        0,
                        0,
                        w as i32,
                        rows as i32,
                        image.pixels[row * w..].as_ptr().cast(),
                        &header,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                    row += rows;
                }
                if EndPage(hdc) <= 0 {
                    return Err("The printer stopped the job".into());
                }
                printed += 1;
            }
            if EndDoc(hdc) <= 0 {
                return Err("The printer did not finish the job".into());
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
    #[test]
    fn a_quarter_turn_moves_the_top_left_corner_to_the_top_right() {
        // 3 wide, 2 high:  1 2 3 / 4 5 6
        let image = rogeha_core::PageImage { width: 3, height: 2, pixels: vec![1, 2, 3, 4, 5, 6] };
        let turned = super::quarter_turn(&image);
        assert_eq!((turned.width, turned.height), (2, 3));
        assert_eq!(turned.pixels, vec![4, 1, 5, 2, 6, 3]);
    }
}
