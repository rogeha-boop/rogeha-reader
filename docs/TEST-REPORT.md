# Rogeha Reader: test report

What was tested for this build, and what was not. The automated part is written by
`python tools/test_report.py`, which runs the tests; run it for every build. The part on scripted runs
and checks by hand is kept by people: change it when such a check is done or a known gap closes.

| | |
| --- | --- |
| Version tested | 1.1.42 (commit `21fd130`, plus changes not yet committed) |
| Date | 16 November 2026 |
| Command | `cargo test --release` |
| Result | **All 54 automated tests passed** |

The version above is the build the tests ran on. Committing this report adds one to the build number.

## Automated tests

| Group | Tests | Passed | Failed |
| --- | --- | --- | --- |
| Assistant | 2 | 2 | 0 |
| Engine, small parts | 7 | 7 | 0 |
| Engine, on real documents | 27 | 27 | 0 |
| Reader | 18 | 18 | 0 |
| **All** | **54** | **54** | **0** |

### Assistant

The assistant's protocol and settings (crates/agent, unit tests).

| Test | Result |
| --- | --- |
| Definitions are well formed | passed |
| Inputs are checked | passed |

### Engine, small parts

Dates, signature encoding, font names (crates/core, unit tests).

| Test | Result |
| --- | --- |
| Pdf dates | passed |
| Der round trips | passed |
| Times | passed |
| Pdf font names are taken apart | passed |
| Registry entries are split into faces | passed |
| The closest installed face is tried first | passed |
| The standard fonts stay built in | passed |

### Engine, on real documents

Opens, edits, saves and reopens PDFs made for the test (crates/core/tests/page_ops.rs).

| Test | Result |
| --- | --- |
| A protected pdf opens only with its password and stays protected | passed |
| An archive copy keeps its colour profile | passed |
| Bad input is an error not a crash | passed |
| Changes can be undone and redone | passed |
| Comments are saved as standard annotations | passed |
| Comments need a pdf | passed |
| Compressing a scan shrinks it and keeps it readable | passed |
| Compressing takes out what no reader shows | passed |
| Delete and move pages | passed |
| Digital signatures sign and verify | passed |
| Drawn and typed signatures are stamped into the page | passed |
| Each person gets their own comment colour | passed |
| Forms can be filled and saved | passed |
| Merge pdfs | passed |
| Ocr adds a text layer that matches the picture | passed |
| Ocr refuses bad requests before touching anything | passed |
| Opens and renders | passed |
| Outline and links lead to pages | passed |
| Picture signatures keep their transparency | passed |
| Reading pages and highlighting by text | passed |
| Redaction takes the content out instead of covering it | passed |
| Rotation is saved into the file | passed |
| Save as leaves the original alone | passed |
| Several pages move together | passed |
| Signatures come out upright on rotated pages | passed |
| The strongest setting reduces pictures the others leave | passed |
| Trimming fonts leaves every page looking the same | passed |

### Reader

Page store, printing, dark pages, the assistant's citations, page layout (crates/app, unit tests).

| Test | Result |
| --- | --- |
| Citations become links | passed |
| A dot is read as red green blue | passed |
| Packing and unpacking gives the same page back | passed |
| A quarter turn moves the top left corner to the top right | passed |
| Blank paper is not sent | passed |
| Colour and photos get more bits | passed |
| Only the part with ink is sent | passed |
| Rows are packed and padded to four bytes | passed |
| Stretches closer than the gap count as one | passed |
| White rows and wide white gaps cut the page | passed |
| A text hit is marked a little larger than its letters | passed |
| The copy is named after the original | passed |
| A place survives being written and read | passed |
| Dark pages flip lightness and keep hue | passed |
| Boxes stay on the page and keep their shape | passed |
| Saved signatures survive a restart | passed |
| Spreads pair pages the way a magazine does | passed |
| Blending keeps white white and hits the endpoints | passed |

<!-- by hand: start -->
## Checked by scripted runs

The reader can drive itself (`--script`, `--screenshot`, `--bench`; see `docs/ARCHITECTURE.md`, "Checking
it"). These runs were looked at by eye or compared by a script. They are not repeated for every build:
each line says which build it was done on.

| What | How | Build | Result |
| --- | --- | --- | --- |
| The eight Store screenshots: outline, two-page view with the Pages panel, comments, a filled form with a signature, the assistant, dark pages, Find, About | Script steps, picture looked at | 1.0.27 | As expected |
| Installed fonts: a PDF without embedded fonts (Latin, Chinese, Japanese, Korean) and an EPUB in twelve scripts with symbols and emoji | Rendered with and without the fix, looked at | 1.0.24 | All scripts shown; emoji in black and white |
| Printing: three documents through "Microsoft Print to PDF" | Output rendered at full and reduced size, compared with the old output | 1.0.24 | Pages look right; jobs 2 to 6 times smaller |
| The window picture after the memory changes | Screenshot compared dot for dot with the build before | 1.0.25 | Identical in light mode; 8 dots differ by one step with dark pages |
| Compression: five PDFs from daily use at every setting | Sizes compared; results opened and rendered | 1.0.29 | A stronger setting never gives a larger file |
| Compression: a nine-page file of certificates already at 150 dpi | All four settings; one page looked at, at 96 dpi | 1.0.30 | 22 to 24% smaller, 45% at 96 dpi; text readable, softer when zoomed |
| The Microsoft Store package | Signed test package installed, started, removed | 1.0.26 | Installs and starts; Windows offers it for PDF and EPUB at once |
| The website page and privacy policy | Opened in a browser from a local copy; light theme at phone width by the website session | 1.0.27 | No sideways scrolling, all pictures load |
| The About box without its Feature overview link (the toolbar button stays) | Script step `about`, picture looked at | 1.0.35 | Only "Source code" and "Legal notice" left in that row |
| Store screenshot 8 retaken on a 100 % screen with `--size 2880x1620 --zoom 2` | Compared by eye with the old one, taken at 200 % | 1.0.36 | Same size (2880 x 1620) and layout; only the link and the version differ |
| Black out: a made-up two-page bank statement; the payments marked with one box on every page and the account number by text, then saved as a copy | Script steps `redact`, `redactevery`, `redactbox`, `redactfind`, `redactsave`; both pictures looked at; the copy's text read with PyMuPDF | 1.0.38 | Payments, amounts and account number gone from the copy's text on both pages; name, address and dates kept; no annotations left; original unchanged |
| Black out on a scan without text: the same statement as one JPEG picture per page, payments and account number boxed | Script steps; the pictures inside the copy read with PyMuPDF and compared with the original's | 1.0.38 | In the copy the boxed parts of each picture are blank, the address in the same picture is kept, and a black box is drawn over them |
| Version number 1.1 | `rogeha-reader.exe --version` from the build | 1.1.43 | Shows 1.1 with the build number (1.1.42 plus the uncommitted change before the commit) |

## Measurements

On a laptop with a 2880 x 1800 screen at 200%, window 2200 x 1640 dots.

| What | Value | Build |
| --- | --- | --- |
| Time to the first page (14-page paper, datasheet, colour flyer) | about 110, 155, 240 ms | 1.0.25 |
| Memory, empty window | 21 MB | 1.0.25 |
| Memory, one document open, one page on screen | 39 to 44 MB | 1.0.25 |
| Time to draw a frame | about 6 ms (slowest in a ten-page run: 24 ms) | 1.0.25 |
| Print job, 14-page paper | 74 MB of picture data (460 MB as whole pictures) | 1.0.24 |
| Store package | 11 MB | 1.0.30 |

## Not checked

Things no test covers and nobody has tried by hand. Each is a risk until someone does.

- Printing on a real printer, and the Windows print dialog (only "Microsoft Print to PDF", by script).
- The assistant and translation against a live AI service (only against a local stand-in).
- Working with the mouse: selecting text, clicking links, dragging thumbnails, drawing a signature.
- Ctrl+Tab between tabs.
- The Black out window with the mouse (dragging boxes, clicking one away), and on a real bank statement or a real scan.
- Scrolling by hand since pages are kept packed (a stutter when a page scrolls into view would show here).
- The Compress window by hand, including its new messages.
- Text recognition (OCR) from the Store package.
- The installer's final message, the reader's icon in Explorer and the Start menu, and the Default apps page.
- "Make default viewer" from the Store package.
- The installer on a freshly installed Windows without the Visual C++ runtime.
- macOS and Linux: never built.

## Known problems in this build

- Blacking out works on the pages only: bookmarks, the document's title and properties, and attached files
  are left as they are.
- Windows shows a freshly installed reader in its "Select an app" list only after it has been picked once
  through "Choose an app on your PC" (the installer; the Store package does not have this).
- The installer is not code-signed, so SmartScreen may warn.
- In a compressed file, the transparency mask of a reduced picture stays at its full size.
<!-- by hand: end -->
