# Rogeha Reader — plan

A free, open-source PDF + EPUB reader for Windows (macOS/Linux later), written in Rust on MuPDF.
Goal: SumatraPDF-class speed (near-instant open, tens of MB of RAM) plus editing, signing, OCR,
translation and an optional AI agent that never slows down normal reading.

## License (important)

MuPDF is **AGPL-3.0** (or a paid commercial license from Artifex). Rogeha Reader links MuPDF
statically, so the whole app is **AGPL-3.0-or-later**. That's fine for a free, open-source app:
anyone who gets a copy (or uses a hosted version) must be able to get the full source.
You can't ship a closed-source version without buying a MuPDF commercial license.

## Architecture

```
rogeha-reader (workspace)
├── crates/core    rogeha-core: MuPDF wrapper, no GUI
│     doc worker thread (owns the fz_document; MuPDF documents are !Send)
│     page cache (LRU of rendered pixmaps, keyed by page/zoom/rotation)
│     page ops: rotate (saved into /Rotate), delete, move, save / save-as
│     text: search, page text (for selection, translation, AI context)
├── crates/app     rogeha-reader.exe: egui GUI, drawn on the CPU (winit + softbuffer)
│     continuous vertical scroll, lazy page requests for visible pages only
│     zoom, fit-width, go-to-page, thumbnails sidebar (later), keyboard shortcuts
│     page organizer actions (rotate/delete/move), dirty tracking, save prompts
└── crates/agent   rogeha-agent.exe: AI agent, a separate process
      spawned only when the user opens the AI panel; JSON lines over stdin/stdout
      providers: Anthropic Claude API (default), OpenAI-compatible, local (Ollama / llama.cpp server)
      tools it can ask the reader to run: get_page_text, search, rotate_pages, delete_pages, ...
```

Why these choices:

- **mupdf crate (mupdf-rs 0.8, MuPDF 1.27.2)**: safe wrapper with page rotation, delete/move/insert
  pages, image insertion, signature widgets, text pages and search. It builds MuPDF from source with
  MSBuild on Windows, so no prebuilt DLLs are needed. Missing pieces (PKCS#7 signing callbacks,
  OCR device) go through the raw `mupdf-sys` FFI that comes with it.
- **egui, drawn on the CPU**: native, immediate-mode, one small exe, no web view, and no GPU.
  The first version used eframe with OpenGL; the graphics driver alone cost 60-100 MB of memory
  and ~0.2 s of startup. Now winit opens the window, a software rasteriser draws the UI, and
  softbuffer hands the finished frame to the OS. MuPDF renders pages on the CPU anyway, and
  their pixels are copied straight into the frame, so each rendered page is in memory once.
  Alternatives considered: iced (heavier, async-first), Slint (good, but a DSL and a
  GPL/royalty licensing split), native Win32 (fastest, but not portable).
- **AI agent as a separate process**: none of the HTTP/TLS/JSON code is linked into the reader,
  so it has zero cost at startup and none while reading. The process starts only when you open the AI panel.

## Building MuPDF on Windows

- Toolchain: Rust stable (MSVC target) + Visual Studio 2022 Build Tools (cl, MSBuild). `mupdf-sys`
  runs MSBuild on MuPDF's own `platform/win32/mupdf.sln` (target `libmupdf`) and links it statically.
- `bindgen` needs `libclang.dll`. This machine has no LLVM, so we use the `libclang` Python wheel
  (`pip install --user libclang`) and point `LIBCLANG_PATH` at it in `.cargo/config.toml`.
  Installing LLVM (`winget install LLVM.LLVM`) also works.
- The first build compiles MuPDF and its third-party libraries (freetype, harfbuzz, jbig2, openjpeg,
  tesseract, leptonica, ...) and takes several minutes. Later builds are incremental.

## Milestones

Done in the first version:

1. **Viewer**: open PDF/EPUB/CBZ/XPS/FB2/MOBI, render in a background thread, texture cache,
   scroll, zoom, fit width/page, page navigation, search, keyboard shortcuts, drag-and-drop.
2. **Page ops**: rotate pages (saved into the file), delete, move, save / save as.
   Saves are incremental when possible (fast, keeps existing digital signatures valid); a full
   rewrite is used after deleting pages so the removed content is really gone.
3. **Combine PDFs** (first cut): add the pages of other PDFs at the end or after the current page.
4. **Comments** (first cut), stored only as standard PDF annotations so other readers can
   read, edit and reply to them: select text to **highlight / underline / strike out**, add
   **sticky notes**, edit or delete any comment (including ones made in other readers), a
   Comments panel listing them all, text selection and copy. Each annotation gets the entries
   other readers rely on: author (`T`), text (`Contents`), dates (`M`, `CreationDate`), unique
   name (`NM`), colour, `QuadPoints`/`Rect`, an appearance stream, and a `Popup` for notes.
   Checked with an independent PDF library (`crates/core/tests/check_annotations.py`).
5. **Compress scanned PDFs** (Pages menu): re-encodes page images with MuPDF's image rewriter
   in a helper process and writes a new, tidied file. Three presets (300 / 200 / 144 dpi, JPEG
   85 / 75 / 60); black-and-white images stay lossless (CCITT G4); an image is replaced only
   if the result is smaller. Safety net: every page's text and vector content is fingerprinted
   before and after, and pages that changed beyond their images get their original content
   back (MuPDF's rewriter damages text on some PDFs). Measured: 8.1 MB colour scan at 300 dpi
   to 0.96 MB on Balanced with no visible difference at 200% zoom.
6. **Rogeha Studio design system**: colour tokens for light and dark, IBM Plex type, radii,
   an app icon drawn to the icon rules (four strata, white open book, the pip), and an About
   window with the Studio cover banner on `canvas`.
7. **OCR** (Pages menu, "Recognize text"): Tesseract inside MuPDF reads pages that have no
   text, in a helper process with page-by-page progress. The words are written onto the
   original page as an invisible text layer (render mode 3), each stretched to sit over its
   picture, so the scan looks identical but can be searched, selected, copied and commented.
   Pages that already have text are skipped. Language data (8 languages offered) is downloaded
   once on request into the app data folder; after that it works offline.
   Measured on 300 dpi test scans: 96-98% of words exact, about 4 s per page.

Planned, in this order:

- **OCR, next steps**: text outside Windows-1252 (Cyrillic, Greek, CJK need an embedded
  Unicode font for the layer), rotated and vertical text, automatic language detection,
  OCR of just the current page for quick searching, deskewing crooked scans first.
- **Design system, next steps**: an .ico in the exe (Explorer and taskbar pinning), the
  toolbar as icons from the system's icon set, the depth-ramp for progress indicators.
- **Compression, next steps**: JBIG2 for black-and-white scans, mixed raster content (text
  layer in black-and-white over a low-resolution colour background) for the biggest savings
  on text scans, and re-encoding images inside pages that currently have to be restored.
- **Comments, next steps**: reply threads (`IRT`), free-text boxes, freehand ink and shapes,
  colour choice, moving notes, remembering your author name between sessions.

5. **Page organizer**: thumbnail grid with drag-to-reorder, multi-select, rotate/delete,
   merge from several files, extract/split pages.
7. **Visual signature**: draw / type / load an image; place and resize on the page; stamp it
   into a saved copy (`PdfPage::insert_image`).
8. **Digital signature**: PKCS#7/CMS from .p12/PFX (or a new self-signed identity); signing through
   MuPDF's `pdf_sign_signature` with a Rust-implemented `pdf_pkcs7_signer` (RustCrypto `cms`,
   `pkcs12`, `rsa`, `x509-cert`); verifying signatures on open with a Rust verifier.
10. **Translation**: offline via Bergamot / Firefox Translations models (Marian) in a helper
    process, models downloaded per language pair on demand; the AI agent is the optional
    higher-quality path.
11. **AI agent**: `rogeha-agent.exe` is already spawned lazily (stub). To do: summarize/explain/ask
    with page citations, translate, multi-step actions through reader tools. The model is a
    setting: current Claude IDs are `claude-fable-5-2`, `claude-opus-5-5`, `claude-sonnet-5-5`
    and `claude-haiku-5-5` (re-check the list when this is built). Also OpenAI-compatible
    endpoints and local models. API keys go in the OS credential store, not a plain-text file.

## Known gaps in the first version

- **Memory is much better but still above SumatraPDF.** Measured on a high-DPI screen
  (2200 x 1640 px window): 47 MB for an empty window, 75-140 MB with a document. What is left
  is mostly pixels: the window frame is held twice (2 x 14 MB) and three rendered pages are kept
  (about 15 MB each at this resolution). On a normal 1080p screen these are about a quarter
  the size. Next steps: keep pages as 3 bytes per pixel or only the visible tiles, drop the
  rasteriser's second frame copy, and render only the visible part at high zoom.
- **System fonts are off.** mupdf-rs's `system-fonts` feature (font-kit) hangs on Windows when
  opening EPUBs, so it is disabled. PDFs that don't embed their fonts, and EPUB text in scripts
  outside the built-in fonts (CJK, Arabic, emoji...), fall back to the base-14 fonts or show
  missing glyphs. Fix: a small own font loader that reads `C:\Windows\Fonts` (mupdf-rs has a
  `FontLoader` hook), or bundle Noto/Droid fallback fonts.
- Deleting a comment removes it from the document, but with a normal (incremental) save its
  text can still be recovered from the file's older bytes, as in other PDF editors.
- Password-protected files are refused; no outline/bookmarks sidebar,
  thumbnails, links, printing, or session restore yet.
- Search runs over the whole document on the document thread, so rendering waits on very
  large files; above about 12 megapixels per page (very high zoom) the page is upscaled
  instead of re-rendered (needs tile rendering).
- A full rewrite after deleting pages can make some files slightly larger (object streams are
  unpacked); the compression milestone fixes this.

## Performance targets

- Window visible and first page drawn in well under 0.5 s on a normal PDF (measured: 0.12-0.4 s).
- Memory in the tens of MB for normal documents (page cache capped by bytes).
- OCR, translation and AI never touch the startup path.
