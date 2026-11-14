"""Independent check that Rogeha Reader's comments are standard PDF annotations.

Reads a PDF written by the `comments_are_saved_as_standard_annotations` test with pypdf
(a different PDF library than the one the reader uses) and checks every entry other
readers rely on.

    set ROGEHA_KEEP_ANNOTATED=annotated.pdf
    cargo test --release -p rogeha-core comments_are_saved
    python crates/core/tests/check_annotations.py annotated.pdf
"""
import re
import sys

from pypdf import PdfReader

annots = [a.get_object() for a in PdfReader(sys.argv[1]).pages[0].get("/Annots", [])]
# Sticky notes come with a standard /Popup annotation (their text window); check the link.
popups = [a for a in annots if a["/Subtype"] == "/Popup"]
annots = [a for a in annots if a["/Subtype"] != "/Popup"]
assert len(popups) == 1 and popups[0]["/Parent"].get_object()["/Subtype"] == "/Text"
assert [a["/Subtype"] for a in annots] == ["/Highlight", "/Underline", "/Text"], annots
date = re.compile(r"^D:\d{14}Z$")
for a in annots:
    kind = a["/Subtype"]
    assert a["/T"] == "Ada", kind                      # author
    assert date.match(a["/M"]), (kind, a.get("/M"))    # modified
    assert date.match(a["/CreationDate"]), kind
    assert a["/NM"], kind                              # unique name
    assert len(a["/C"]) == 3, kind                     # RGB colour
    assert len(a["/Rect"]) == 4, kind
    assert "/N" in a["/AP"], kind                      # appearance stream
    assert int(a["/F"]) & 4, kind                      # print flag
    if kind != "/Text":
        assert len(a["/QuadPoints"]) % 8 == 0 and a["/QuadPoints"], kind
assert annots[0]["/Contents"] == "Check this sentence"
assert annots[2]["/Contents"] == "Edited note \u00e9\u4e2d"
assert annots[2]["/Name"] == "/Comment"
print("OK:", len(annots), "standard annotations with author, dates, colour, appearance")
