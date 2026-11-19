#!/usr/bin/env python3
"""Builds receiver/packaging/windows/pocketwebcam.ico (installer, Start menu, uninstall list)
from profile-1024.png, with rounded corners like other Windows app icons.
Needs Pillow. Run: python3 build_ico.py"""
import pathlib

from PIL import Image, ImageDraw

HERE = pathlib.Path(__file__).parent
OUT = HERE / "../receiver/packaging/windows/pocketwebcam.ico"

src = Image.open(HERE / "profile-1024.png").convert("RGBA")
mask = Image.new("L", src.size, 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, *src.size), radius=src.width // 5, fill=255)
src.putalpha(mask)
src.save(OUT, sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
print(f"wrote {OUT.resolve()}")
