# Rogeha Reader

A fast, free, open-source PDF and EPUB reader for Windows, written in Rust on the
[MuPDF](https://mupdf.com) engine. Native GUI (egui) drawn on the CPU: no Electron, no web view,
no GPU driver.

**Status: early version.** Viewing, page editing, comments, forms, compression, OCR, signing and the
AI assistant work; offline translation is planned (see [PLAN.md](PLAN.md)).

## What works today

- Opens **PDF, EPUB, CBZ, XPS, FB2, MOBI** (everything MuPDF reads)
- Continuous scrolling; pages render in a background thread, only the visible ones plus a little read-ahead
- Zoom (Ctrl + wheel, Ctrl +/-), fit width, fit page, go to page
- **Rotate pages and save the rotation into the PDF**, delete pages, move pages, save / save as
- **Combine PDFs**: add the pages of other PDF files at the end or after the current page (Pages menu)
- **Comments that other PDF readers understand**: select text to highlight, underline or strike it out,
  add sticky notes, edit or delete comments, and see them all in the Comments panel. They are saved as
  standard PDF annotations, so Acrobat, Edge, Foxit, Preview and others show and edit them too
- **Fill out forms**: click a field to type, tick or choose; values are saved the standard way so other
  readers show them
- **Sign**: draw, type or load a picture of your signature, place and resize it on the page, and stamp it
  in
- **Digital signatures**: sign with a `.p12` / `.pfx` certificate (or create a self-signed one), and see
  for every signed PDF whether it is intact and who signed it. Standard PKCS#7 signatures that other
  readers validate. The signer's certificate is not yet checked against trusted authorities
- **OCR** (Pages menu): makes scanned PDFs searchable and selectable by adding an invisible text layer;
  the pages look exactly the same. Language data is downloaded once per language, then it works offline
- **Compress scanned PDFs** (Pages menu) into a smaller copy with no visible quality loss; text and
  black-and-white pages are kept exactly, and the original file is never changed
- Select and copy text (drag, then Copy or Ctrl+C)
- Text search with highlighted matches
- Drag and drop, open from the command line (`rogeha-reader file.pdf`)
- **AI assistant** (the "AI" button): ask questions and get answers with links to the pages they come from,
  summarize, explain or translate a page, and have it act on the document ("rotate all landscape pages",
  "highlight every mention of the deadline"). You bring your own provider: a Claude API key, or any
  OpenAI-compatible service, or a local model. It runs as a separate program that starts only when you open
  the panel, so reading stays fast

## Look

The app follows the Rogeha Studio design system: its colour tokens in light and dark (it follows
the Windows setting), IBM Plex Sans and Mono, and the Studio cover banner in the About window.
The tokens live in `crates/app/src/theme.rs`; brand assets and fonts are in `crates/app/assets`
(IBM Plex is under the SIL Open Font License, see `assets/fonts/OFL-ibm-plex.txt`).
The Rogeha Studio name, banner and app icon are the studio's brand and are not covered by the AGPL.

## Keyboard

| Keys | Action |
| --- | --- |
| Ctrl+O / Ctrl+S / Ctrl+Shift+S | Open / save / save as |
| Ctrl + wheel, Ctrl + `+` / `-` | Zoom |
| Ctrl+0 / Ctrl+1 | Fit width / 100% |
| PgUp, PgDn, Space, arrows, Home, End | Move around |
| R / Shift+R | Rotate the current page right / left |
| Ctrl+F, Enter, F3 / Shift+F3 | Find, next / previous match |

## The assistant and your data

The assistant only sees what it reads: the text of the pages it asks for, your question, and any text you
have selected. That text is sent to the provider you choose in its settings (Anthropic, another service, or
a model on your own computer). Nothing is sent unless you use the assistant. Your API key is stored in the
Windows Credential Manager. Changes the assistant makes are not written to the file until you save, and
deleting pages always asks you first.

## Building (Windows)

You need:

1. **Rust** (stable, MSVC toolchain): <https://rustup.rs>
2. **Visual Studio 2022 Build Tools** with the "Desktop development with C++" workload (MSBuild + cl)
3. **libclang** for bindgen. Either install LLVM (`winget install LLVM.LLVM`) or
   `pip install --user libclang`, then make sure `LIBCLANG_PATH` points at the folder with
   `libclang.dll` (see `.cargo/config.toml`).

```
cargo build --release
cargo test --release
python crates/agent/tests/check_agent.py target/release/rogeha-agent.exe   # assistant, against a local mock
```

The first build compiles MuPDF and its bundled libraries from source and takes several minutes.
The result is `target/release/rogeha-reader.exe` (plus `rogeha-agent.exe`, the AI helper).

`rogeha-reader.exe file.pdf --bench` prints time-to-first-page and memory use, then exits.

## License

Rogeha Reader is licensed under the **GNU Affero General Public License v3.0 or later**
(see [LICENSE](LICENSE)), because it links MuPDF, which is AGPL-3.0. If you distribute a modified
version, or run one as a network service, you must offer its complete source code under the same license.
