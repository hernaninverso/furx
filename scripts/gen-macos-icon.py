#!/usr/bin/env python3
"""Genera el master del ícono macOS con la forma squircle de Apple (esquinas redondeadas + padding
transparente) a partir del arte full-bleed de marca. macOS NO redondea el ícono del Dock solo: hay que
hornear el squircle en el .icns. iOS es al revés (enmascara solo), así que su ícono queda full-bleed y NO
se toca acá. Salida: src-tauri/icons/source/furx-icon-macos-1024.png (RGBA con alpha)."""
from PIL import Image, ImageDraw
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "src-tauri/icons/source/furx-icon-1024.png"   # arte full-bleed (ink + F coral), 2048px
OUT = ROOT / "src-tauri/icons/source/furx-icon-macos-1024.png"

CANVAS = 1024          # lienzo final
ART = 824              # área de arte (grilla macOS Big Sur: 824 sobre 1024, ~100px de margen)
MARGIN = (CANVAS - ART) // 2
SS = 4                 # supersample para bordes suaves
RADIUS = round(ART * 0.2237)  # radio del squircle (proporción iOS/macOS, ~22.4%)

art = Image.open(SRC).convert("RGBA").resize((ART * SS, ART * SS), Image.LANCZOS)
mask = Image.new("L", (ART * SS, ART * SS), 0)
ImageDraw.Draw(mask).rounded_rectangle(
    [0, 0, ART * SS - 1, ART * SS - 1], radius=RADIUS * SS, fill=255
)
art.putalpha(mask)
art = art.resize((ART, ART), Image.LANCZOS)

canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
canvas.paste(art, (MARGIN, MARGIN), art)
canvas.save(OUT)
print(f"OK -> {OUT} ({CANVAS}x{CANVAS}, art {ART}, radius {RADIUS}, margin {MARGIN})")
