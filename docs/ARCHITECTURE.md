# Rogeha Reader: architecture

How the reader is built and why. This document describes the code as it is; keep it true with every
build (see "Keeping this document current" at the end).

Last updated for version 1.0.31 (14 November 2026).

## In one paragraph

Rogeha Reader is a Windows desktop program written in Rust. The MuPDF library opens, draws, edits and
saves documents. The window is drawn by egui on the processor and handed to Windows as a plain picture,
with no browser engine and no graphics driver. The window never touches a document: a second thread
owns MuPDF and the two exchange messages. Work that can fail badly (compressing, text recognition) and
work that needs network code (the assistant) run in processes of their own.

## The parts

```
rogeha-reader.exe  (crates/app)                       rogeha-agent.exe  (crates/agent)
┌──────────────────────────────────────────────┐      ┌───────────────────────────────┐
│ GUI thread                                   │      │ The assistant                 │
│   runner.rs    window, event loop, frame     │ JSON │   Claude or OpenAI-compatible │
│   app.rs       toolbar, panels, page layout  │◄────►│   HTTP, TLS, the model loop   │
│   app/*.rs     one file per feature          │lines │   no access to files          │
│        │ Request ▲ Event   (channels)        │      └───────────────────────────────┘
│        ▼         │                           │
│ Document thread  (crates/core, worker.rs)    │      rogeha-reader.exe --compress …
│   Engine       the only owner of MuPDF       │      rogeha-reader.exe --subset-fonts …
│   engine.rs, annotations.rs, forms.rs, …     │      rogeha-reader.exe --ocr …
└──────────────────────────────────────────────┘      (the same program as a helper process)
```

| Crate | Builds | Job |
| --- | --- | --- |
| `crates/core` (`rogeha-core`) | a library | Everything about documents, with no GUI: the engine, the worker thread, and the page operations |
| `crates/app` (`rogeha-reader`) | `rogeha-reader.exe` | The window, and the helper modes of the same program |
| `crates/agent` (`rogeha-agent`) | `rogeha-agent.exe` | The assistant, started only when its panel is opened |
| `crates/setup` (`rogeha-setup`) | `rogeha-reader-setup.exe` | The installer: one file that carries the other two |

The workspace builds the first three by default. The installer is built on request, after them, because
it embeds their program files.

## The document engine (`crates/core`)

| File | What it does |
| --- | --- |
| `engine.rs` | `Engine`: open (with a password if needed), page sizes, render a page, text, search, outline, links, rotate, delete, move, insert, save, undo and redo. Synchronous; lives on the document thread |
| `worker.rs` | The document thread. Owns the `Engine` and turns `Request`s from the window into `Event`s back |
| `annotations.rs` | Comments and markup as standard PDF annotations; text selection |
| `forms.rs` | Filling in forms (AcroForm): text fields, checkboxes, radio buttons, dropdowns |
| `signature.rs` | A drawn, typed or photographed signature stamped onto a page |
| `dsig.rs` | Digital signatures: signing with a PKCS#12 certificate, and verifying. Pure Rust (rsa, sha1, p12-keystore) |
| `compress.rs` | Making PDFs smaller (see "Compression" below) |
| `ocr.rs` | Text recognition with Tesseract, adding an invisible text layer |
| `fonts.rs` | Fonts installed in Windows, for documents that do not carry their own |

**MuPDF** comes through the `mupdf` crate (mupdf-rs 0.8, MuPDF 1.27), which builds the library from
source. Where its safe wrapper lacks something, the raw functions of `mupdf-sys` are used: the undo
journal, the image rewriter, font trimming, and making a font from mapped memory. Those places read the
raw pointer out of the wrapper's one-field structs, with a compile-time check that the struct still has
that size.

**Undo and redo** use MuPDF's own journal. Every change goes through `Engine::edit`, which opens one
journal operation around it, so one user action is one undo step, and a change that fails is rolled back.
The history starts over when the document is saved, because saving reopens the file.

**Fonts.** When a document names a font it does not carry, MuPDF asks the reader. `fonts.rs` answers
from the registry's list of installed fonts (by family and style) and, for Chinese, Japanese, Korean
and other scripts, symbols and emoji, with the fonts Windows ships for them. Font files are mapped into
memory, not read, so a 20 MB font costs almost nothing. Windows only; elsewhere MuPDF falls back to its
built-in fonts.

## The two threads

MuPDF documents must stay on one thread, and drawing a page can take a noticeable time. So:

- **The document thread** (`worker.rs`) owns the `Engine`. Nothing else calls MuPDF.
- **The GUI thread** sends a `Request` (`Open`, `Want` these pages, `Rotate`, `AddNote`, `Save`, …) and
  gets `Event`s back (`Doc`, `Page`, `Rendered` for thumbnails, `SearchResults`, `Saved`, `Error`, …).

The window asks for the pages on screen plus one ahead. A page arrives as red-green-blue bytes, exactly
as MuPDF drew it, and the window is repainted. A slow page delays that page, never the scroll. Requests
carry a generation number, so answers for a document that has since changed or closed are dropped.

For work that needs the engine but has no message of its own (printing), the window sends a closure with
`Request::WithEngine`.

## The window (`crates/app`)

**Drawn on the processor.** `runner.rs` runs the event loop (winit), lets egui lay out the UI, and draws
the frame in two layers into the window's own buffer (softbuffer):

1. the pages, copied straight from the page store into the frame;
2. the UI on top, by a software rasteriser for egui.

No graphics driver is loaded: on Windows a driver alone adds 60 to 100 MB to a program. The rasteriser's
cache is switched off, because it is a second picture of the whole window and drawing the reader's small
UI afresh takes the same time (about 6 ms a frame). Frames are drawn only when something changes.

**Where memory goes** (`app/pagestore.rs`). Rendered pages are most of what the reader holds.

- A page is kept as three bytes per dot and widened to the window's four while it is copied to the screen.
- Only pages on screen are kept as plain dots. The page read ahead and pages just passed are kept packed
  (LZ4) and unpacked when they scroll into view.

**One file per feature** under `app/`:

| File | Feature |
| --- | --- |
| `session.rs` | Tabs, recent files with the place in each, the password prompt, dark pages, translation |
| `navigate.rs` | Outline panel, clickable links, Back and Forward |
| `pages_panel.rs` | Thumbnails; reorder, rotate, delete, add PDFs |
| `comments.rs` | Selecting text, markup, notes, the Comments panel |
| `forms.rs` | Typing into fields on the page |
| `sign.rs`, `dsign.rs` | Signing by hand; signing with a certificate and checking signatures |
| `print.rs` | Printing (see below) |
| `compress.rs`, `ocr.rs` | The Compress and Recognise text windows; both start a helper process |
| `assistant.rs` | The assistant panel; runs the tools the assistant asks for |
| `about.rs` | About, and the address of the Feature overview page |

`icons.rs` draws the toolbar icons as vectors, `theme.rs` holds the Rogeha Studio design system's
colours and type, and `install.rs` registers the reader with Windows (see "Installing").

**Tabs are light.** Only the tab in front holds its document. Switching tabs reopens the file at the
remembered page and zoom.

## Printing

A page goes to the printer as a picture, drawn by MuPDF at up to 300 dpi, so what comes out is what the
reader shows, comments and signatures included. To keep print jobs small, `print.rs` cuts the picture
along its white space (between columns, lines and words) and sends only the pieces with ink: 16 grey
levels for text, 256 for grey photos, colour only where there is colour. Cuts never run through ink,
so no seam can show in letters. Windows only (GDI).

## Compression

`compress.rs` builds a smaller copy in steps, each writing a new temporary file:

1. Re-encode pictures above the setting's resolution (MuPDF's image rewriter). Black-and-white pictures
   stay exact.
2. Compare every page with the original, by everything it draws except picture dots. A page whose text
   or drawings changed gets its original content back.
3. Take out what no reader shows: metadata copies on pictures and pages, a design program's private
   data, thumbnails, and at the two smallest settings the print shop's colour profile (never from PDF/A).
4. Trim embedded fonts to the glyphs in use, in a helper process, and keep the result only if every page
   then renders exactly as before. Never in forms.
5. Write the final file tidily.

Where pictures are a small part of the file, the gentler settings and "pictures left alone" are tried
as well and the smallest result is kept, so a stronger setting never gives a larger file. The settings
are 300, 200, 144 and 96 dpi.

## Helper processes

Three jobs run in a process of their own, which is the reader started again with a different first
argument:

| Argument | Job | Why not in the window's process |
| --- | --- | --- |
| `--compress <in> <out> <setting>` | Compression | MuPDF ends the process on some damaged files |
| `--subset-fonts <in> <out>` | Font trimming, started by the compressor | The same; a font it cannot handle then costs the saving, not the compression |
| `--ocr <in> <out> <language> <data folder>` | Text recognition | The same, and it is slow |

A helper prints one line of JSON with its result. If it ends without one, the window reports that the
file could not be processed, and keeps running.

## The assistant (`crates/agent`)

A separate program, so that no network code is loaded while reading. The reader starts it when the
assistant panel is opened and talks to it in JSON, one object per line, over the program's input and
output.

- **Providers:** Anthropic's Claude (default), or any OpenAI-compatible service, which can be one on the
  user's own computer.
- **The assistant has no access to files.** It sees the document only through tools, and the *reader*
  runs every tool: `get_document_info`, `get_pages`, `search`, `go_to_page`, `rotate_pages`,
  `delete_pages`, `add_note`, `highlight_text`, `place_signature`, `get_form_fields`, `fill_form_field`.
  Changes made this way are ordinary edits that undo takes back.
- **The key** is kept in the Windows credential store, never in a file.

## What is stored, and where

| What | Where |
| --- | --- |
| Recent files, open tabs, dark pages, translation language | `state.json` in the reader's data folder |
| Saved signatures | `signatures.json` and picture files in the same folder |
| Text recognition language data | `tessdata` in the same folder, downloaded on request |
| Assistant settings (provider, model) | `assistant.json` in the same folder |
| The assistant's key | Windows Credential Manager |

`ROGEHA_DATA_DIR` points the reader at another data folder; the scripted checks use it so the user's own
settings are neither shown nor changed.

## When the reader goes online

Only in three cases, each started by the user: the assistant (the text of the pages a question needs goes
to the chosen service), the one-time download of language data for text recognition, and the Feature
overview button, which opens the reader's page on rogehastudio.com in the browser.

## Installing

Two ways, from the same program files:

- **The Microsoft Store package** (`tools/package.ps1`, `package/AppxManifest.xml`): an MSIX with the
  reader and the assistant. The manifest declares the file types and the Visual C++ runtime dependency.
  From a package, the reader writes nothing to the registry.
- **The installer** (`crates/setup`): copies the two programs into
  `%LOCALAPPDATA%\Programs\Rogeha Reader` for the current user and has the reader register itself
  (`install.rs`): "Open with" for its file types, an entry under Installed apps, a Start menu shortcut.

Windows does not let a program make itself the default viewer; both ways open the Default apps page.

## Versions

`MAJOR.MINOR.BUILD`. Major and minor come from `Cargo.toml`. The build number is the number of commits
in the repository, worked out by `crates/app/build.rs`, so every committed change has a higher number.
About shows it with the commit's short id.

## Checking it

- **Automated tests:** `cargo test --release`. `docs/TEST-REPORT.md` has the current results.
- **Scripted runs:** the reader can drive itself. `--script "page=3;find=text;…"` runs steps,
  `--screenshot file.ppm` saves the window and exits, `--size 1440x810` sets the window, `--bench`
  prints time to first page and memory. The Store screenshots are made this way.
- **Developer tools** in `crates/core/examples`: `render` (a page to a picture), `svg2rgba`, and others.
- **Measuring:** `ROGEHA_FRAME_STATS=1` prints each frame's drawing time; `ROGEHA_PRINT_STATS=1` prints
  how much data a print job sent.

## Known limits

- Windows only so far. The engine and the window are portable; printing, installed fonts and
  installation are written for Windows.
- Emoji are drawn in black and white.
- Printing sends pictures, not text and lines, so a page that is one large colour photo is a large job.
- Tabs reopen their file when switched to; search results and the assistant's conversation do not
  survive a tab switch.
- The undo history starts over on save.
- The installer (not the Store package) needs the Visual C++ runtime to be present on the computer.

More detail on these, with numbers, is in `PLAN.md` under "Known gaps".

## Keeping this document current

With every build that changes how the reader is put together (a new part, a new helper, a different way
of storing or drawing something, a new place it goes online), change the section that describes it and
the version in the line at the top. A build that only fixes behaviour inside one part needs no change
here beyond that line. `docs/TEST-REPORT.md` is regenerated for every build.
