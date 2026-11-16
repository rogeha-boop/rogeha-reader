# Rogeha Reader: Microsoft Store submission

Everything Partner Center asks for, ready to copy. Product: **Rogeha Reader**
(`RogehaStudio.RogehaReader`), Store ID `9PH8X1J7HB1P`.

**Published** in the Microsoft Store on 15 November 2026 with version 1.0.36:
https://apps.microsoft.com/detail/9PH8X1J7HB1P. A later version is a new submission of the same product in
Partner Center, with a package from `tools/package.ps1` (its version is always higher than the last one's).

## Before the first submission

1. **Reserve the name.** Partner Center > Apps and games > New product > MSIX or PWA app > "Rogeha Reader".
2. **Check the identity.** Product management > Product identity must show

   | | |
   | --- | --- |
   | Package/Identity/Name | `RogehaStudio.RogehaReader` |
   | Package/Identity/Publisher | `CN=961E1179-772D-4F2C-9BC0-390262D24B36` |
   | Package/Properties/PublisherDisplayName | `Rogeha Studio` |

   The publisher is the studio's and is the same as Rogeha Capture's. If the name differs, change it in
   `tools/package.ps1` (the `$NAME$` line) and build again; the Store refuses a package whose identity does not
   match.
3. **Publish the website pages** (rogehastudio-website, `projects/rogeha-reader/`): the listing's website and
   privacy policy addresses point there, and the app's Feature overview button opens the page.
4. **Build the package** from a clean checkout of `main`:

   ```powershell
   ./tools/package.ps1           # artifacts\RogehaReader_<major>.<minor>.<build>.0_x64.msix, unsigned: the Store signs it
   ./tools/package.ps1 -Sign     # "Rogeha Reader (test)", signed with the test certificate, to try it first
   ```

## Releases

| Version | Built from | Status |
| --- | --- | --- |
| 1.0.36.0 | `c58d9a1` | First release: passed certification and published on 15 November 2026 |
| 1.0.40.0 | `e781031` | Black out (redact). Built on 16 November 2026 |
| 1.1.x.0 | `main` | The same reader as 1.0.40 under the version number 1.1 (asked for on 16 November 2026). Not built yet |

## Checks before uploading

1. Install the test package (`-Sign`) and start "Rogeha Reader (test)" from the Start menu.
2. Right-click a PDF > Open with: "Rogeha Reader (test)" is offered. Open it; the page shows.
3. Add a highlight, save, close, open again: the highlight is there (the package may write beside the file).
4. Tools > Recognise text (OCR): the language data downloads and a scan becomes searchable.
5. Open the Assistant, enter a key in its Settings, ask a question: an answer with page numbers comes back.
6. The Feature overview button (four squares, beside About) opens the website page in the browser.
7. Tools > Make Rogeha Reader the default PDF viewer opens Settings > Default apps on the app's entry.
8. Print one page to a real printer.
9. Tools > Black out (redact): drag a box on a copy of a statement, save the copy, open it: the boxed text cannot be found with Find.
10. Remove the test app again (Settings > Apps > Installed apps).

Checked so far, on 14 November 2026 with the test package 1.0.26.0: it installs, starts (19 MB with an empty
window) and Windows offers it for .pdf and .epub. Checks 2 to 8 have not been done by hand.

## Package

| | |
| --- | --- |
| File | `artifacts/RogehaReader_<major>.<minor>.<build>.0_x64.msix` (about 11 MB, unsigned; the Store signs it) |
| Identity | `RogehaStudio.RogehaReader`, `CN=961E1179-772D-4F2C-9BC0-390262D24B36`, Rogeha Studio |
| Version | `<major>.<minor>.<number of commits>.0`, now 1.1 (every later package must be higher, which the commit count gives) |
| Contents | `rogeha-reader.exe` (the reader), `rogeha-agent.exe` (the assistant, started on request), the tile pictures, the licence |
| Device family | Windows 10 version 1809 or later, desktop, x64 |
| Depends on | `Microsoft.VCLibs.140.00.UWPDesktop` (the Visual C++ runtime; the Store installs it with the app) |
| Capabilities | `runFullTrust` (restricted, justification below), `internetClient` |
| File types | .pdf, .epub, .fb2, .mobi, .cbz, .xps, .oxps |

## Properties

| Field | Value |
| --- | --- |
| Category | Productivity |
| Subcategory | (none) |
| Privacy policy URL | `https://rogehastudio.com/projects/rogeha-reader/privacy.html` |
| Website | `https://rogehastudio.com/projects/rogeha-reader/` |
| Support contact info | `support@rogehastudio.com` |
| Does this product access, collect or transmit personal information? | Yes (the optional assistant sends document text to the AI service the user chooses; the privacy policy says so) |
| Display mode | (leave unticked) |
| Product declarations | "Customers can install this app to alternate drives or removable storage": yes. "Windows can include this app's data in automatic backups to OneDrive": yes |
| System requirements | Minimum: none. Recommended: none (keyboard and mouse are the default) |

## Pricing and availability

| Field | Value |
| --- | --- |
| Markets | All markets |
| Audience | Public |
| Discoverability | Make this product available and discoverable in the Store |
| Schedule | Release as soon as possible |
| Base price | Free |
| Free trial | (not applicable) |

## Age ratings (IARC questionnaire)

Category: **Utility, Productivity, Communication or Other**. Answer **No** to every content question
(violence, sexuality, language, controlled substances, gambling, user interaction, sharing location, digital
purchases). The app shows the user's own documents and has no content of its own. Expected rating: 3+ /
Everyone.

## Store listing: English (United States)

### Product name

```
Rogeha Reader
```

### Short description (max 270 characters)

```
A fast, free reader for PDF and EPUB files. Comment, highlight, sign, fill in forms, black out private details, reorder pages, compress scans and make them searchable. An optional AI assistant answers questions about your document, with your own key. Works offline.
```

### Description (max 10,000 characters)

```
Rogeha Reader opens PDFs and e-books quickly and stays small in memory. The first page of an ordinary PDF shows in a fraction of a second, and the program does the things people otherwise need a heavy application for.

READ
• PDF and EPUB, and also comic archives (CBZ), XPS, FictionBook (FB2) and MOBI.
• Tabs for several documents. Recent files reopen at the page, zoom and layout you left.
• The document's outline in a side panel, clickable links with Back and Forward, and Find across the whole document.
• One page or two pages side by side, with the cover standing alone like in a printed book. Full screen with F11.
• Dark pages for reading at night: the paper turns dark, colours stay recognisable.
• Documents that do not carry their fonts get the fonts installed in Windows. Chinese, Japanese, Korean, Arabic, Hebrew, Indic scripts, Thai, symbols and emoji are shown.
• Password-protected PDFs: the password is asked for and never stored.

MARK UP
• Highlight, underline and strike through text; add notes; reply to comments.
• A Comments panel lists everything with its author and page. Each author gets a colour.
• Everything is stored as standard PDF annotations, so other PDF programs show it too.
• Undo and redo for comments, signatures, form entries and page changes.

SIGN AND FILL IN
• Fill in PDF forms: text fields, checkboxes, radio buttons and dropdown lists.
• Sign by hand: draw your signature, type it, or use a photo of it. Signatures are kept for next time.
• Sign digitally with a certificate file (PKCS#12, .p12 or .pfx), and check the signatures in documents you receive.

WORK WITH PAGES
• Thumbnails of all pages. Reorder by dragging, rotate, delete, and add the pages of other PDFs.
• Compress scanned PDFs, with a preview of how much smaller the file becomes.
• Black out private details for good, for example the payments on a bank statement you send as proof of address. Drag a box, mark a text wherever it appears, or the same box on every page. The text and picture under each box are taken out of the copy you save, not just covered, and the original file stays as it is.
• Recognise text (OCR) in scans, so they can be searched and copied. The language data is downloaded once, on request.
• Print through the Windows print dialog; comments, form entries and signatures are printed as shown.

ASK (OPTIONAL)
• An assistant answers questions about the document, summarises it, and translates a selection or a page. Answers name the pages they come from; click a page number to go there.
• It can carry out edits you ask for, such as rotating all landscape pages. Ctrl+Z takes them back.
• The assistant works with your own key for Claude (Anthropic) or with any service that speaks the OpenAI protocol, including one on your own computer. It is a separate small program that only starts when you open its panel.

PRIVATE BY DESIGN
Rogeha Reader has no account, no advertising and no usage statistics. It goes online in three cases only, each started by you: the assistant (the text of the pages a question needs is sent to the service you chose), the one-time download of language data for text recognition, and the Feature overview button, which opens the studio's website in your browser. Everything else works offline, and your files stay on your computer.

SMALL AND FAST
The reader is written in Rust on the MuPDF engine. There is no browser engine inside. On a high-resolution screen it uses about 20 MB with an empty window and about 45 MB with a document open.

FREE AND OPEN SOURCE
Rogeha Reader is free software under the GNU Affero General Public License, version 3 or later. The source code is at https://github.com/rogeha-boop/rogeha-reader.

Made by Rogeha Studio, Switzerland.
```

### What's new in this version (max 1,500 characters)

```
New: Black out (redact), in the Tools menu. Send a bank statement or a bill as proof of address without showing what you spent: drag a box over what nobody should see, mark a name or number wherever it appears, or use the same box on every page. The copy you save has the text and picture under each box taken out, not just covered, so it cannot be copied, searched or uncovered. It works on scans too. The original file is not changed.

Also: the About window no longer repeats the Feature overview link; the button beside it opens the page.
```

### Product features (up to 20, max 200 characters each)

```
1.  Opens PDF, EPUB, CBZ, XPS, FB2 and MOBI files quickly: the first page of an ordinary PDF shows in a fraction of a second
2.  Tabs, recent files that reopen where you left them, the document's outline, clickable links with Back and Forward
3.  One page or two pages side by side, with the cover standing alone like in a printed book; full screen
4.  Dark pages for reading at night, in a window that follows the light or dark theme of Windows
5.  Highlight, underline, strike through and add notes; a Comments panel with authors and replies
6.  Fill in PDF forms: text fields, checkboxes, radio buttons and dropdown lists
7.  Sign by hand (drawn, typed or from a photo) and sign digitally with a certificate file
8.  Check the digital signatures in documents you receive
9.  Reorder, rotate and delete pages, and combine PDFs
10. Compress scanned PDFs, with a preview of the saving
11. Recognise text (OCR) in scans so they can be searched and copied
12. Undo and redo for every change to the document
13. Print through the Windows print dialog, with comments and signatures as shown
14. Uses the fonts installed in Windows, including Chinese, Japanese, Korean, Arabic and Indic scripts
15. Optional AI assistant with your own key: questions, summaries and translation, with page citations
16. Opens password-protected PDFs; the password is never stored
17. No account, no advertising, no usage statistics; reading works offline
18. Small: about 45 MB of memory with a document open, no browser engine inside
19. Free and open source (AGPL-3.0)
20. Black out private details for good: the text and picture under each box is taken out of the copy, not just covered
```

### Search terms (up to 7, max 30 characters each, 21 words in total)

```
pdf reader
epub reader
pdf viewer
sign pdf
fill pdf form
pdf comments
ocr
```

### Copyright and trademark info

```
© 2026 Rogeha Studio
```

### Additional license terms

```
Rogeha Reader is free software under the GNU Affero General Public License, version 3 or later: https://www.gnu.org/licenses/agpl-3.0.html. Source code: https://github.com/rogeha-boop/rogeha-reader. It is built on MuPDF by Artifex Software, used under the same licence.
```

### Developed by

```
Rogeha Studio
```

### Screenshots (desktop)

All in `package/store-screenshots`, 2880 × 1620 PNG (the Store wants 1366 × 768 or larger, at most 3840 ×
2160; 16:9). At least one is needed, four or more are recommended; upload them in this order. Captions are
optional (max 200 characters).

| File | Caption |
| --- | --- |
| `1-reading-outline.png` | Reading with the document's outline beside the page |
| `2-pages-two-page-view.png` | Two pages side by side, and the Pages panel for reordering, rotating and deleting |
| `3-comments.png` | Highlights, notes and underlines, listed with their authors in the Comments panel |
| `4-forms-signature.png` | A filled-in form with a signature placed on its line |
| `5-assistant.png` | The optional assistant answers a question and names the pages the answer comes from |
| `6-dark-pages.png` | Dark pages and the dark theme for reading at night |
| `7-find.png` | Find marks every hit on the page |
| `8-about.png` | About, with the links to the source code and the legal notice (taken from 1.0.35) |

Screenshot 8 is optional: it shows the version number of the build it was taken from, so retake it from the
build you submit (or leave it out).

They show the reader with its own short tour (`package/store-shot-samples/rogeha-reader-tour.pdf`), so no
one else's document appears in the Store. How they are made is under "Retaking the screenshots" below.

**Screenshot 5 shows a scripted answer.** It was taken without an API key: a small stand-in for the AI
service (`package/store-shot-samples/shot_assistant.py`) plays one fixed answer, written by hand from what
the tour says on the pages it cites. The window, the question and the citations are the real program. Before
submitting, either retake it with a real key (open the tour, ask the same question) or leave it out.

### Store logos

In `package/store-logos`; Partner Center > Store listing > Store logos.

| Field | File |
| --- | --- |
| 1:1 box art (2160 × 2160) | `RogehaReader-2160.png` |
| 1:1 app tile icon (300 × 300) | `RogehaReader-300.png` |

The 2:3 poster art and the 16:9 super hero art are optional and left empty.

## Submission options

### Restricted capabilities: why does the product need `runFullTrust`? (max 500 characters)

```
Rogeha Reader is a classic Windows desktop (Win32) application written in Rust, packaged as MSIX. It needs runFullTrust to run as a desktop app: it opens and saves the PDF and e-book files the user chooses anywhere on the computer, prints through the Windows print dialog, and starts its own helper programs from the package (the optional assistant, and text recognition and compression of scans). It uses no other restricted capability.
```

### Notes for certification (max 2,000 characters)

```
Rogeha Reader is a PDF and EPUB reader. No account or sign-in is needed, and there are no purchases.

To test: start the app and open any PDF with the folder button at the left of the toolbar (or drag a file onto the window, or right-click a PDF > Open with > Rogeha Reader). Everything in the toolbar and in the Tools and View menus works without a network connection.

The app goes online only in these cases, each started by the user:
1. The AI assistant (the star button in the toolbar). It is optional and needs the user's own API key for Anthropic's Claude, or the address of an OpenAI-compatible service, entered under Assistant > Settings. Without a key the panel only shows where to enter one; nothing is sent. We do not provide a key for testing: all other features can be tested without it.
2. Tools > Recognise text (OCR) downloads language data once, from the tessdata project on GitHub, when the user asks for it.
3. The Feature overview button (four squares, beside About) opens https://rogehastudio.com/projects/rogeha-reader/ in the default browser.

The package holds two programs: rogeha-reader.exe (the app) and rogeha-agent.exe (the assistant, started by the app only when the assistant panel is opened).

Rogeha Reader is free software under the GNU AGPL, version 3 or later, built on MuPDF by Artifex Software (same licence). Source code: https://github.com/rogeha-boop/rogeha-reader
```

## Retaking the screenshots

The reader takes them itself: `--size` sets the window (1440 × 810 points is 2880 × 1620 dots on a 200 %
screen; on a 100 % screen use `--size 2880x1620 --zoom 2` instead), `--script` runs the steps, `--screenshot` saves the window as a PPM picture and exits. Set
`ROGEHA_DATA_DIR` to an empty folder so your own settings and recent files are neither shown nor changed, and
work on a copy of the tour, because some steps change the document.

```bash
python package/store-shot-samples/make_tour.py          # only if the tour's text changed
export ROGEHA_DATA_DIR=/tmp/reader-shots
R=target/release/rogeha-reader.exe; T=/tmp/rogeha-reader-tour.pdf
shot() { cp package/store-shot-samples/rogeha-reader-tour.pdf $T; $R --size 1440x810 --script "$2" --screenshot "$1.ppm" $T; }
shot 1-reading-outline     "light;lightpages;outline;page=2"
shot 2-pages-two-page-view "light;lightpages;pages;view=cover;page=2"
shot 3-comments            "light;lightpages;fitpage;page=3;author=Mira;select=71,365,314,424;highlight;author=Jonas;note=Is it really nine months?;author=Mira;select=317,452,431,479;underline;comments"
shot 4-forms-signature     "light;lightpages;fitpage;page=5;fill=name|Mira Keller;fill=card|ZH-204-881;fill=email|mira@example.org;fill=arrivals|Yes;signat=285,333,Mira Keller;signapply"
python package/store-shot-samples/shot_assistant.py 8766 &
ROGEHA_ANTHROPIC_BASE_URL=http://127.0.0.1:8766 ANTHROPIC_API_KEY=any shot 5-assistant "light;lightpages;page=7;ask=How does the reader keep its memory use small?"
shot 6-dark-pages          "dark;darkpages;outline;page=3"
shot 7-find                "light;lightpages;fitpage;page=5;find=signature"
shot 8-about               "light;lightpages;fitpage;page=1;about"
```

Then turn each `.ppm` into a `.png` (for example with Pillow: `Image.open(f).save(g, optimize=True)`) and
put them in `package/store-screenshots`. The tile pictures and Store logos come from
`python package/make_assets.py`.
