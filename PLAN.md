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
├── crates/app     rogeha-reader.exe: egui/eframe GUI (glow/OpenGL backend)
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
- **egui/eframe + glow**: native, immediate-mode, one small exe, no web view. Glow (OpenGL) starts
  faster and uses less memory than wgpu. Alternatives considered: iced (heavier, async-first),
  Slint (good, but a DSL and a GPL/royalty licensing split), native Win32 (fastest, but not portable).
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

1. **Viewer**: open PDF/EPUB/CBZ/XPS/FB2/MOBI, render in a background thread, page cache,
   scroll, zoom, page navigation, keyboard shortcuts, drag-and-drop, open from command line.
2. **Page ops**: rotate pages (saved into the file), delete, move/reorder, save / save as.
3. **Visual signature**: draw / type / load an image; place and resize on the page; stamp it
   into a saved copy (`PdfPage::insert_image` / an image XObject in the page content).
4. **Digital signature**: PKCS#7/CMS from .p12/PFX (or a new self-signed identity); signing through
   MuPDF's `pdf_sign_signature` with a Rust-implemented `pdf_pkcs7_signer` (RustCrypto `cms`,
   `p12`/`pkcs12`, `rsa`, `x509-cert`); verifying signatures on open (`pdf_check_digest` /
   `pdf_check_certificate` with a Rust verifier).
5. **OCR**: MuPDF's built-in Tesseract (`fz_new_ocr_device`), which is already compiled in.
   Language data (`eng.traineddata`, ...) is downloaded once on demand into the app data folder,
   so everything works offline after that. Text becomes searchable/selectable; optionally saved
   as an invisible text layer.
6. **Translation**: offline via Bergamot / Firefox Translations models (Marian) in a helper process,
   models downloaded per language pair on demand; the AI agent is the higher-quality optional path.
7. **AI agent**: lazily spawned `rogeha-agent.exe`; summarize/explain/ask with page citations,
   translate, multi-step actions through reader tools. The model is a setting: current Claude
   IDs are `claude-fable-5-2`, `claude-opus-5-5`, `claude-sonnet-5-5` and `claude-haiku-5-5`
   (re-check the list when this milestone is built). API keys go in the OS credential store,
   not in a plain-text file.

## Performance targets

- Window visible and first page drawn in well under 0.5 s on a normal PDF.
- Memory in the tens of MB for normal documents (page cache capped by bytes).
- OCR, translation and AI never touch the startup path.
