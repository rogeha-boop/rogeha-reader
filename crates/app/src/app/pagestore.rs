//! How rendered pages are kept in memory.
//!
//! Rendered pages are most of what the reader holds: a page at the size of a high-resolution
//! window is close to four million dots. Two things keep that small:
//!
//! * A page is stored as three bytes per dot (red, green, blue), the way MuPDF draws it. The
//!   window wants four, and gets them while the page is copied to the screen.
//! * Only pages that are on screen are kept as plain dots. The others (the page read ahead,
//!   pages just scrolled past) are kept packed. A page of text packs to a small fraction of
//!   its size and unpacks in a few milliseconds when it scrolls into view.

use std::sync::Arc;

use rogeha_core::PageRgb;

/// A rendered page as plain dots: red, green and blue bytes, row by row.
pub struct PagePixels {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

impl PagePixels {
    /// The bytes of row `y`, three per dot.
    #[inline]
    pub fn row(&self, y: usize) -> &[u8] {
        &self.rgb[y * self.width * 3..(y + 1) * self.width * 3]
    }
}

/// One dot of a row as the window wants it: `0x00RRGGBB`.
#[inline]
pub fn dot(row: &[u8], x: usize) -> u32 {
    (row[x * 3] as u32) << 16 | (row[x * 3 + 1] as u32) << 8 | row[x * 3 + 2] as u32
}

/// A rendered page in the cache: ready to draw, or packed.
pub enum Stored {
    /// On screen. `packed` is the packed form from before, kept when it is small so the page
    /// can go back to packed without packing it again.
    Shown { pixels: Arc<PagePixels>, packed: Option<Vec<u8>> },
    Packed { width: usize, height: usize, data: Vec<u8> },
}

impl Stored {
    pub fn new(page: PageRgb) -> Self {
        let pixels = PagePixels { width: page.width, height: page.height, rgb: page.rgb };
        Stored::Shown { pixels: Arc::new(pixels), packed: None }
    }

    /// Memory this page takes.
    pub fn bytes(&self) -> usize {
        match self {
            Stored::Shown { pixels, packed } => pixels.rgb.len() + packed.as_ref().map_or(0, Vec::len),
            Stored::Packed { data, .. } => data.len(),
        }
    }

    /// The page as plain dots, unpacking it first if needed.
    pub fn show(&mut self) -> Option<Arc<PagePixels>> {
        if let Stored::Packed { width, height, data } = self {
            let rgb = lz4_flex::block::decompress(data, *width * *height * 3).ok()?;
            let pixels = Arc::new(PagePixels { width: *width, height: *height, rgb });
            let data = std::mem::take(data);
            // Keeping both forms is only worth it when the packed one is small.
            let packed = (data.len() * 4 < pixels.rgb.len()).then_some(data);
            *self = Stored::Shown { pixels, packed };
        }
        match self {
            Stored::Shown { pixels, .. } => Some(pixels.clone()),
            Stored::Packed { .. } => None,
        }
    }

    /// Pack the page: it is not on screen.
    pub fn pack(&mut self) {
        if let Stored::Shown { pixels, packed } = self {
            let data = packed.take().unwrap_or_else(|| lz4_flex::block::compress(&pixels.rgb));
            *self = Stored::Packed { width: pixels.width, height: pixels.height, data };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> PageRgb {
        // Mostly white with a few dots, like a page of text.
        let mut rgb = vec![0xFF; 200 * 100 * 3];
        for i in (0..rgb.len()).step_by(37) {
            rgb[i] = i as u8;
        }
        PageRgb { width: 200, height: 100, rgb }
    }

    #[test]
    fn a_dot_is_read_as_red_green_blue() {
        let row = [0x11, 0x22, 0x33, 0xAA, 0xBB, 0xCC];
        assert_eq!(dot(&row, 0), 0x0011_2233);
        assert_eq!(dot(&row, 1), 0x00AA_BBCC);
        let Stored::Shown { pixels, .. } = Stored::new(PageRgb { width: 1, height: 2, rgb: row.to_vec() }) else {
            panic!("new pages are shown")
        };
        assert_eq!(dot(pixels.row(1), 0), 0x00AA_BBCC);
    }

    #[test]
    fn packing_and_unpacking_gives_the_same_page_back() {
        let mut stored = Stored::new(page());
        let before = stored.show().unwrap().rgb.clone();
        stored.pack();
        assert!(matches!(stored, Stored::Packed { .. }));
        assert!(stored.bytes() * 4 < before.len(), "a mostly white page packs well");
        let after = stored.show().unwrap();
        assert_eq!(after.rgb, before);
        // The small packed form was kept, so packing again costs nothing and changes nothing.
        let kept = stored.bytes() - before.len();
        stored.pack();
        assert_eq!(stored.bytes(), kept);
        assert_eq!(stored.show().unwrap().rgb, before);
    }
}
