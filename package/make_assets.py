"""Makes the package's tile pictures and the Store logos from the app icon.

    cargo build --release -p rogeha-core --example svg2rgba
    python package/make_assets.py

Needs Pillow. The same layout as Rogeha Capture's package: the icon fills the small 44-dot
logo, sits at two thirds of the height on the transparent tiles, and at two thirds on the
studio's ink colour for the Store logos.
"""
import os
import subprocess
import tempfile

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SVG = os.path.join(ROOT, "crates", "app", "assets", "rogeha-reader-icon.svg")
TOOL = os.path.join(ROOT, "target", "release", "examples", "svg2rgba.exe")
INK = (0x14, 0x18, 0x1C, 255)


def icon(size):
    with tempfile.TemporaryDirectory() as tmp:
        raw = os.path.join(tmp, "icon.rgba")
        subprocess.run([TOOL, SVG, str(size), raw], check=True)
        return Image.frombytes("RGBA", (size, size), open(raw, "rb").read())


def tile(width, height, icon_size, ground=(0, 0, 0, 0)):
    picture = Image.new("RGBA", (width, height), ground)
    mark = icon(icon_size)
    picture.paste(mark, ((width - icon_size) // 2, (height - icon_size) // 2), mark)
    return picture


def save(picture, *path):
    out = os.path.join(ROOT, "package", *path)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    picture.save(out, optimize=True)
    print(out, picture.size)


save(tile(44, 44, 44), "Assets", "Square44x44Logo.png")
save(tile(50, 50, 50), "Assets", "StoreLogo.png")
save(tile(150, 150, 100), "Assets", "Square150x150Logo.png")
save(tile(310, 150, 100), "Assets", "Wide310x150Logo.png")
# File icon for PDFs and e-books that open with the reader.
save(tile(44, 44, 44), "Assets", "Document44x44Logo.png")
save(tile(300, 300, 200, INK), "store-logos", "RogehaReader-300.png")
save(tile(2160, 2160, 1440, INK), "store-logos", "RogehaReader-2160.png")
