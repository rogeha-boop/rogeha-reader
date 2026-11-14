"""Makes rogeha-reader-tour.pdf, the document the Store and website screenshots show.

The screenshots must not show anyone else's documents, so the reader shows its own short tour:
real text about what it does, an outline, links, a table, a chart, and a form to fill in.

    python package/store-shot-samples/make_tour.py

Needs reportlab (pip install reportlab). Fonts: IBM Plex from crates/app/assets/fonts.
"""
import os

from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import BaseDocTemplate, Flowable, Frame, PageBreak, PageTemplate, Paragraph, Spacer, Table, TableStyle

HERE = os.path.dirname(os.path.abspath(__file__))
FONTS = os.path.join(HERE, "..", "..", "crates", "app", "assets", "fonts")
pdfmetrics.registerFont(TTFont("Plex", os.path.join(FONTS, "IBMPlexSans-Regular.ttf")))
pdfmetrics.registerFont(TTFont("Plex-Semi", os.path.join(FONTS, "IBMPlexSans-SemiBold.ttf")))
pdfmetrics.registerFont(TTFont("Plex-Mono", os.path.join(FONTS, "IBMPlexMono-Regular.ttf")))
pdfmetrics.registerFontFamily("Plex", normal="Plex", bold="Plex-Semi", italic="Plex", boldItalic="Plex-Semi")

# The Rogeha Studio design system's colours.
INK = colors.HexColor("#14181c")
STEEL = colors.HexColor("#3d6f94")
MUTED = colors.HexColor("#5b6670")
RULE = colors.HexColor("#d5dade")
PAPER = colors.HexColor("#f4f6f7")
PROBE = colors.HexColor("#e2632a")

body = ParagraphStyle("body", fontName="Plex", fontSize=10.5, leading=16, textColor=INK, spaceAfter=7)
lead = ParagraphStyle("lead", parent=body, fontSize=12.5, leading=19, textColor=MUTED, spaceAfter=12)
h1 = ParagraphStyle("h1", fontName="Plex-Semi", fontSize=22, leading=27, textColor=INK, spaceAfter=10)
h2 = ParagraphStyle("h2", fontName="Plex-Semi", fontSize=13.5, leading=18, textColor=INK, spaceBefore=12, spaceAfter=5)
eyebrow = ParagraphStyle("eyebrow", fontName="Plex-Mono", fontSize=8.5, leading=12, textColor=STEEL, spaceAfter=4)
small = ParagraphStyle("small", parent=body, fontSize=8.5, leading=12, textColor=MUTED)
bullet = ParagraphStyle("bullet", parent=body, leftIndent=14, bulletIndent=2, spaceAfter=3)
title = ParagraphStyle("title", fontName="Plex-Semi", fontSize=40, leading=44, textColor=INK, spaceAfter=10)
cell = ParagraphStyle("cell", parent=body, fontSize=9.5, leading=13, spaceAfter=0)
cellb = ParagraphStyle("cellb", parent=cell, fontName="Plex-Semi")


class Heading(Paragraph):
    """A heading that also becomes an entry in the PDF's outline."""

    def __init__(self, text, style, level, key):
        super().__init__(f'<a name="{key}"/>{text}', style)
        self.level, self.key, self.plain = level, key, text

    def draw(self):
        super().draw()
        self.canv.bookmarkPage(self.key)
        self.canv.addOutlineEntry(self.plain, self.key, self.level, closed=False)


class Bars(Flowable):
    """A small bar chart: label, value, bar."""

    def __init__(self, rows, unit, width=150 * mm, colour=STEEL):
        super().__init__()
        self.rows, self.unit, self.width, self.colour = rows, unit, width, colour
        self.height = len(rows) * 9 * mm + 2 * mm

    def draw(self):
        c = self.canv
        top = max(v for _, v, _ in self.rows)
        for i, (label, value, strong) in enumerate(self.rows):
            y = self.height - (i + 1) * 9 * mm
            c.setFont("Plex", 9)
            c.setFillColor(INK)
            c.drawString(0, y + 2 * mm, label)
            c.setFillColor(self.colour if strong else RULE)
            c.rect(52 * mm, y + 0.5 * mm, (self.width - 75 * mm) * value / top, 5.5 * mm, stroke=0, fill=1)
            c.setFillColor(INK)
            c.setFont("Plex-Mono", 9)
            c.drawString(52 * mm + (self.width - 75 * mm) * value / top + 2 * mm, y + 2 * mm, f"{value:g} {self.unit}")


class Form(Flowable):
    """A few form fields to fill in and a line to sign on."""

    def __init__(self):
        super().__init__()
        self.width, self.height = 150 * mm, 78 * mm

    def draw(self):
        c, form = self.canv, self.canv.acroForm
        c.setFillColor(PAPER)
        c.setStrokeColor(RULE)
        c.roundRect(0, 0, self.width, self.height, 3 * mm, stroke=1, fill=1)
        x, y = 8 * mm, self.height - 14 * mm
        look = dict(borderColor=RULE, fillColor=colors.white, textColor=INK, borderWidth=0.8, fontName="Helvetica", fontSize=10, forceBorder=True)
        for label, name, tip in [("Name", "name", "Your name"), ("Library card number", "card", "Card number"), ("E-mail", "email", "E-mail address")]:
            c.setFont("Plex", 9.5)
            c.setFillColor(MUTED)
            c.drawString(x, y + 2 * mm, label)
            # Form fields are placed in page coordinates, not the flowable's own.
            fx, fy = c.absolutePosition(x + 42 * mm, y)
            form.textfield(name=name, tooltip=tip, x=fx, y=fy, width=92 * mm, height=7 * mm, **look)
            y -= 11 * mm
        c.drawString(x, y + 2 * mm, "Send me")
        for i, (label, name) in enumerate([("new arrivals", "arrivals"), ("due-date reminders", "reminders")]):
            bx = x + 42 * mm + i * 42 * mm
            fx, fy = c.absolutePosition(bx, y + 1 * mm)
            form.checkbox(name=name, tooltip=label, x=fx, y=fy, size=4.5 * mm, borderColor=RULE, fillColor=colors.white, textColor=INK, buttonStyle="check", forceBorder=True)
            c.setFillColor(INK)
            c.drawString(bx + 7 * mm, y + 2 * mm, label)
        y -= 20 * mm
        c.setStrokeColor(MUTED)
        c.line(x + 42 * mm, y, x + 134 * mm, y)
        c.setFillColor(MUTED)
        c.drawString(x, y + 1 * mm, "Signature")


def page_frame(canvas, doc):
    canvas.saveState()
    canvas.setFont("Plex-Mono", 8)
    canvas.setFillColor(MUTED)
    if doc.page > 1:
        canvas.drawString(25 * mm, 14 * mm, "ROGEHA READER · A SHORT TOUR")
        canvas.drawRightString(A4[0] - 25 * mm, 14 * mm, str(doc.page))
        canvas.setStrokeColor(RULE)
        canvas.line(25 * mm, 19 * mm, A4[0] - 25 * mm, 19 * mm)
    else:
        # The cover: a band of ink with the probe mark, like the studio's banner.
        canvas.setFillColor(INK)
        canvas.rect(0, A4[1] - 95 * mm, A4[0], 95 * mm, stroke=0, fill=1)
        canvas.setStrokeColor(colors.HexColor("#2b333a"))
        for i in range(1, 9):
            canvas.line(i * 25 * mm, A4[1] - 95 * mm, i * 25 * mm, A4[1])
        canvas.setFillColor(PROBE)
        canvas.circle(A4[0] - 40 * mm, A4[1] - 45 * mm, 3 * mm, stroke=0, fill=1)
        canvas.setFillColor(colors.white)
        canvas.setFont("Plex-Semi", 40)
        canvas.drawString(25 * mm, A4[1] - 55 * mm, "Rogeha Reader")
        canvas.setFont("Plex", 15)
        canvas.setFillColor(colors.HexColor("#c4ccd2"))
        canvas.drawString(25 * mm, A4[1] - 68 * mm, "A short tour of a small, fast PDF and e-book reader")
    canvas.restoreState()


def bullets(items):
    return [Paragraph(text, bullet, bulletText="–") for text in items]


def table(rows, widths):
    data = [[Paragraph(c, cellb if r == 0 else cell) for c in row] for r, row in enumerate(rows)]
    t = Table(data, colWidths=widths, hAlign="LEFT")
    t.setStyle(TableStyle([
        ("LINEBELOW", (0, 0), (-1, 0), 0.8, INK),
        ("LINEBELOW", (0, 1), (-1, -1), 0.4, RULE),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("TOPPADDING", (0, 0), (-1, -1), 4),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 4),
        ("LEFTPADDING", (0, 0), (-1, -1), 0),
    ]))
    return t


def build(path):
    doc = BaseDocTemplate(path, pagesize=A4, leftMargin=25 * mm, rightMargin=25 * mm, topMargin=24 * mm, bottomMargin=26 * mm,
                          title="Rogeha Reader: a short tour", author="Rogeha Studio", subject="What Rogeha Reader does and how it is built")
    frame = Frame(doc.leftMargin, doc.bottomMargin, doc.width, doc.height, id="main", leftPadding=0, rightPadding=0, topPadding=0, bottomPadding=0)
    doc.addPageTemplates([PageTemplate(id="page", frames=[frame], onPage=page_frame)])
    s = []

    # 1. Cover
    s += [Spacer(1, 92 * mm),
          Paragraph("WHAT THIS IS", eyebrow),
          Paragraph("Rogeha Reader opens PDFs and e-books quickly, stays small in memory, and still does the things "
                    "people otherwise need a heavy program for: comments, signatures, forms, page editing, text "
                    "recognition and an assistant that answers questions about the document.", lead),
          Paragraph('This tour is itself a PDF with an outline, <a href="#reading" color="#3d6f94">links between its pages</a>, '
                    'a table, a chart and a small form, so every feature has something to show. Open the outline on '
                    'the left to move around.', body),
          Spacer(1, 6 * mm),
          table([["Chapter", "What it shows"],
                 ['<a href="#reading" color="#3d6f94">1 Reading</a>', "Tabs, outline, links, two-page view, dark pages"],
                 ['<a href="#marking" color="#3d6f94">2 Marking up</a>', "Highlights, notes, comments, undo"],
                 ['<a href="#pages" color="#3d6f94">3 Pages</a>', "Reorder, rotate, delete, combine, compress, recognise text"],
                 ['<a href="#forms" color="#3d6f94">4 Forms and signatures</a>', "Fill in, sign by hand, sign with a certificate"],
                 ['<a href="#assistant" color="#3d6f94">5 The assistant</a>', "Questions, summaries, translation, page citations"],
                 ['<a href="#hood" color="#3d6f94">6 Under the hood</a>', "How it stays fast and small"]],
                [52 * mm, 108 * mm]),
          Spacer(1, 10 * mm),
          Paragraph("Free and open source under the AGPL-3.0 licence. Made by Rogeha Studio, Switzerland. "
                    '<a href="https://rogehastudio.com/projects/rogeha-reader/" color="#3d6f94">rogehastudio.com</a>', small),
          PageBreak()]

    # 2. Reading
    s += [Paragraph("CHAPTER 1", eyebrow), Heading("Reading", h1, 0, "reading"),
          Paragraph("A reader is judged in its first second. Rogeha Reader shows the first page of an ordinary PDF "
                    "in a tenth to four tenths of a second, and scrolling never waits for a page to be drawn: pages "
                    "are rendered on a second thread, just ahead of where you are.", lead),
          Heading("Formats", h2, 1, "formats"),
          Paragraph("PDF and EPUB are the main ones. Comic archives (CBZ), XPS, FictionBook (FB2) and MOBI open too, "
                    "because the MuPDF engine underneath reads them.", body),
          Heading("Finding your way", h2, 1, "finding"),
          *bullets(["<b>Tabs</b> for several documents, and <b>recent files</b> that reopen at the page, zoom and "
                    "layout you left.",
                    "The document's <b>outline</b> in a side panel, and <b>links</b> you can click, with Back and "
                    "Forward like in a browser.",
                    "<b>Find</b> across the whole document, with every hit marked on the page.",
                    "<b>Thumbnails</b> of all pages in the Pages panel."]),
          Heading("Layouts", h2, 1, "layouts"),
          Paragraph("One page per row, or two pages side by side. For books and magazines the first page can stand "
                    "alone as the cover, so that left and right pages face each other as they do in print. F11 "
                    "fills the screen.", body),
          Heading("Reading at night", h2, 1, "night"),
          Paragraph("<b>Dark pages</b> turn the paper dark and the text light while keeping colours recognisable: "
                    "a red stays red. The window itself follows the light or dark theme of Windows.", body),
          Heading("Fonts and scripts", h2, 1, "fonts"),
          Paragraph("A PDF that names a font without carrying it gets the real font from Windows. Text in Chinese, "
                    "Japanese, Korean, Arabic, Hebrew, Hindi, Thai and other scripts, and symbols and emoji, are "
                    "drawn with the fonts Windows has for them.", body),
          PageBreak()]

    # 3. Marking up
    s += [Paragraph("CHAPTER 2", eyebrow), Heading("Marking up", h1, 0, "marking"),
          Paragraph("Select text with the mouse and a small bar appears: highlight, underline, strike through, add "
                    "a note, copy, or translate. What you add is stored as standard PDF annotations, so every other "
                    "PDF program shows it too.", lead),
          Heading("Comments", h2, 1, "comments"),
          Paragraph("The Comments panel lists every note and highlight in the document with its author and page. "
                    "Click one to jump to it; reply, edit or delete it there. Each author gets a colour of their "
                    "own, so a review by several people stays readable.", body),
          Heading("Undo and redo", h2, 1, "undo"),
          Paragraph("Ctrl+Z takes back the last change and Ctrl+Y brings it back: comments, signatures, form "
                    "entries and page changes alike. The history starts over when the document is saved.", body),
          Heading("A paragraph to try it on", h2, 1, "try"),
          Paragraph("The alpine marmot spends up to nine months of the year asleep. Before winter it lines its "
                    "burrow with dried grass, seals the entrance with earth and stones, and lets its body "
                    "temperature fall to a few degrees above freezing. A family sleeps together, and the young "
                    "survive more often when older animals lie beside them and keep them warm.", body),
          Paragraph("In spring the animals lose no time. They have about four months to raise young and to eat "
                    "enough to double their weight before the first snow. A sentinel keeps watch from a rock "
                    "while the others feed, and one sharp whistle sends the whole colony underground.", body),
          PageBreak()]

    # 4. Pages
    s += [Paragraph("CHAPTER 3", eyebrow), Heading("Pages", h1, 0, "pages"),
          Paragraph("The Pages panel shows every page as a thumbnail. Drag pages to reorder them, select several "
                    "and rotate or delete them, or add the pages of another PDF.", lead),
          table([["Tool", "What it does"],
                 ["Reorder", "Drag one page or a selection to a new place"],
                 ["Rotate", "Turn single pages or the whole document, left or right"],
                 ["Delete", "Remove the selected pages"],
                 ["Combine", "Add another PDF's pages at the end or after the current page"],
                 ["Compress", "Shrink scanned PDFs by re-encoding their pictures, with a preview of the saving"],
                 ["Recognise text (OCR)", "Add an invisible text layer to scans, so they can be searched and copied"],
                 ["Print", "Through the Windows print dialog, with comments and signatures as shown"]],
                [48 * mm, 112 * mm]),
          Heading("Smaller print jobs", h2, 1, "printing"),
          Paragraph("A page is sent to the printer as a picture, so what comes out is exactly what the reader "
                    "shows. Only the inked parts are sent, in grey where there is no colour. Measured on three "
                    "documents, the data handed to Windows:", body),
          Bars([("Paper, 14 pages: before", 460, False), ("Paper, 14 pages: now", 74, True),
                ("Datasheet, 4 pages: before", 135, False), ("Datasheet, 4 pages: now", 35, True),
                ("Colour flyer: before", 65, False), ("Colour flyer: now", 34, True)], "MB"),
          PageBreak()]

    # 5. Forms and signatures
    s += [Paragraph("CHAPTER 4", eyebrow), Heading("Forms and signatures", h1, 0, "forms"),
          Paragraph("Click a field and type. Checkboxes, radio buttons and dropdown lists work as well, and what "
                    "you fill in is saved in the PDF itself.", lead),
          Form(), Spacer(1, 5 * mm),
          Heading("Signing by hand", h2, 1, "signing"),
          Paragraph("Draw your signature with the mouse or a pen, type it, or use a photo of it on paper; the "
                    "reader removes the paper's background. Then click where it belongs on the page. Signatures "
                    "you have made are kept for next time.", body),
          Heading("Signing with a certificate", h2, 1, "certificate"),
          Paragraph("A digital signature proves who signed and that nothing changed afterwards. Rogeha Reader "
                    "signs with a certificate file (PKCS#12, .p12 or .pfx) and checks the signatures in documents "
                    "you receive. The password of the certificate is asked for each time and never stored.", body),
          PageBreak()]

    # 6. Assistant
    s += [Paragraph("CHAPTER 5", eyebrow), Heading("The assistant", h1, 0, "assistant"),
          Paragraph("Ask a question about the document and get an answer that says which pages it comes from. "
                    "Click a page number in the answer and the reader goes there.", lead),
          *bullets(["<b>Summaries</b> of the whole document or of a range of pages.",
                    "<b>Questions</b> in your own words, answered from the document's text.",
                    "<b>Translation</b> of a selection or of the current page, into the language you choose.",
                    "<b>Edits on request</b>: \"rotate the landscape pages\" or \"delete the blank pages\" are "
                    "carried out as ordinary changes that Ctrl+Z takes back."]),
          Heading("Yours to switch on", h2, 1, "optional"),
          Paragraph("The assistant is a separate small program that is only started when you open its panel, so "
                    "it costs nothing while you just read. It works with your own key for Claude, or with any "
                    "service that speaks the OpenAI protocol, including one running on your own computer. Without "
                    "a key, the reader never goes online by itself.", body),
          Heading("What is sent", h2, 1, "sent"),
          Paragraph("Only the text of the pages a question needs, and only to the service you chose. The file "
                    "itself stays on your computer.", body),
          PageBreak()]

    # 7. Under the hood
    s += [Paragraph("CHAPTER 6", eyebrow), Heading("Under the hood", h1, 0, "hood"),
          Paragraph("Rogeha Reader is written in Rust on top of MuPDF. There is no browser engine inside and no "
                    "graphics driver is loaded: the window is drawn on the processor.", lead),
          Heading("Memory", h2, 1, "memory"),
          Paragraph("Measured on a high-resolution screen (window 2200 by 1640 dots) with a 14-page paper open:", body),
          Bars([("Before", 98, False), ("Now", 44, True)], "MB", colour=PROBE),
          Paragraph("Three things brought it down: pages that are not on screen are kept packed, pages are stored "
                    "with three bytes per dot instead of four, and the window picture is held once, not twice.", body),
          Heading("The parts", h2, 1, "parts"),
          table([["Part", "Job"],
                 ["Document engine", "MuPDF, on its own thread: opens, renders, edits and saves"],
                 ["Window", "egui, drawn on the processor and handed to Windows as a plain picture"],
                 ["Assistant", "A separate program, started only on request"],
                 ["Helpers", "Compression and text recognition run in a helper process, so a broken file cannot take the window down"]],
                [42 * mm, 118 * mm]),
          Heading("Open source", h2, 1, "source"),
          Paragraph('The code is public under the AGPL-3.0 licence: '
                    '<a href="https://github.com/rogeha-boop/rogeha-reader" color="#3d6f94">github.com/rogeha-boop/rogeha-reader</a>.', body)]

    doc.build(s)


if __name__ == "__main__":
    out = os.path.join(HERE, "rogeha-reader-tour.pdf")
    build(out)
    print("made", out, os.path.getsize(out), "bytes")
