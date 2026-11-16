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

The current description is [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). What follows is the plan it
started from.

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
8. **Visual signature** ("Sign"): draw with the mouse or a pen, type a name, or load a photo
   or scan (the paper around the ink is removed and the picture cropped). Click the page to
   place it, drag to move, drag the corner to resize, then apply. It is stamped into the page
   content: drawn signatures as smooth vector strokes, typed ones as Times Italic text,
   pictures as an image with transparency. Upright on rotated pages. Up to six signatures can
   be remembered on the computer. The on-page preview is drawn by the same code that stamps.
9. **Digital signature** (Sign window, "Certificate" tab): signs with a `.p12` / `.pfx`
   identity as a standard detached PKCS#7 / CMS signature (`adbe.pkcs7.detached`, SHA-256,
   RSA), saved incrementally so earlier signatures stay valid. Can create a self-signed
   identity (RSA 3072) as a password-protected `.p12`. On open, signatures are checked:
   intact or changed, who signed, whether content was added afterwards, self-signed or not.
   Written in Rust (RSA from RustCrypto, a small DER reader/writer of our own, `p12-keystore`),
   without MuPDF's signing API. Cross-checked both ways with pyHanko.
10. **Fill out forms**: click a text field to type in it (Tab goes to the next one), click
    checkboxes and radio buttons to switch them, click a dropdown or list to choose. Fillable
    fields are tinted. Values are stored as standard AcroForm values with regenerated
    appearances through MuPDF's form API, radio groups are kept consistent by hand, and a
    form's old XFA copy is dropped when filling starts so every reader shows the new values.
    Tested on a hand-built form and on the IRS W-9; values read back with pypdf.
11. **AI assistant**: a chat panel that starts the separate `rogeha-agent` program only when
    opened. Bring your own provider: the Claude API (default model `claude-opus-5-5`; Sonnet
    5.5, Haiku 5.5 and Fable 5.2 selectable; streamed, adaptive thinking with an effort
    setting, prompt caching, server-side refusal fallback), or any OpenAI-compatible endpoint
    including local servers (Ollama, LM Studio). The assistant reads the document through
    tools (document overview, page text, search) and answers with page citations that are
    links; it can also act: rotate or delete pages (delete asks first), add notes, highlight
    text, place the saved signature, list and fill form fields. Translation of a page is one
    of the quick actions. API keys go to the OS credential store. Verified against local
    mock servers (`crates/agent/tests`); no live provider was called during development.
12. **Black out (redact)** (Tools menu, 1.0.38): for a document sent as proof of something,
    such as a bank statement as proof of address. Boxes are marked by dragging, by a text
    wherever it appears, or once for every page; the copy is written with MuPDF's redaction
    applied, so text, picture dots and drawings under the boxes are removed (on scans too), and
    comments, links and fields over them go as well. The original file is never changed.
    Tested on generated PDFs, a made-up statement and a picture-only scan of it.

Planned, in this order:

- **Assistant, next steps**: try it against the live APIs and tune the prompt; streaming for
  OpenAI-compatible providers; images of pages for scans and figures (vision); translating a
  whole document into a new file; remembering conversations per document; showing token use
  and cost; cancelling without restarting the agent.

- **Forms, next steps**: typing a custom value into editable dropdowns, several selections
  in a list box, showing a dropdown's display text instead of its stored value, date pickers
  and field formatting hints, marking required fields that are still empty, flattening a
  filled form, importing / exporting form data. XFA-only forms stay out of scope.

10. **Translation**: offline via Bergamot / Firefox Translations models (Marian) in a helper
    process, models downloaded per language pair on demand; the AI agent is the optional
    higher-quality path.

## Known gaps in the first version

- **Memory.** Measured on a high-DPI screen (2200 x 1640 px window), steady after 8 s: 21 MB
  for an empty window, about 40-44 MB with a document. What is left with a document: 14 MB
  is the window's frame, 11 MB the page on screen (3 bytes per pixel), about 15 MB MuPDF's
  document, fonts and glyphs, and a few MB the program and the packed pages around the one on
  screen (`crates/app/src/app/pagestore.rs`). A second page on screen (two-page view, or
  scrolled between two pages) adds another 11 MB. On a normal 1080p screen frame and pages
  are about a quarter the size. What could still be done: render only the visible part of a
  page at high zoom (tiles).
- **Installed fonts are used on Windows only.** `crates/core/src/fonts.rs` answers MuPDF's
  requests from the registry's font list and the fonts Windows ships for each script (font
  files are mapped, not read, so a 20 MB Chinese font costs almost nothing). mupdf-rs's own
  `system-fonts` feature (font-kit) hangs on Windows when opening EPUBs and stays off, so on
  macOS and Linux documents that do not carry their fonts still fall back to the base-14
  fonts. Emoji are drawn in black and white.
- Deleting a comment removes it from the document, but with a normal (incremental) save its
  text can still be recovered from the file's older bytes, as in other PDF editors.
- Tabs are light: only the tab in front holds its document, so switching tabs reopens the file
  (at the remembered page and zoom) and asks about unsaved changes first. Search results and
  the assistant's conversation do not survive a tab switch.
- Printing sends each page as a picture (up to 300 dpi) and is Windows-only so far. Only the
  inked parts are sent, grey where there is no colour, which makes a job 2-6 times smaller
  than whole pictures; a page that is one big colour photo is still large. Compress, OCR and digital signing do not yet work on
  password-protected files, and dark pages also darken photos.
- Search runs over the whole document on the document thread, so rendering waits on very
  large files; above about 12 megapixels per page (very high zoom) the page is upscaled
  instead of re-rendered (needs tile rendering).
- A full rewrite after deleting pages can make some files slightly larger (object streams are
  unpacked); the compression milestone fixes this.

## Performance targets

- Window visible and first page drawn in well under 0.5 s on a normal PDF (measured: 0.12-0.4 s).
- Memory in the tens of MB for normal documents (page cache capped by bytes).
- OCR, translation and AI never touch the startup path.
