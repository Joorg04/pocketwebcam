#!/usr/bin/env python3
"""Builds website/assets/og-image.png (1200x630) for link previews. Run: python3 build_og.py"""
import pathlib, subprocess, tempfile

HERE = pathlib.Path(__file__).parent
from og_parts import LOGO, ART, CSS

body = f"""
<div class="slide glow" style="padding:56px 64px;flex-direction:row;gap:40px;align-items:center">
  <div style="flex:1.15;display:flex;flex-direction:column;height:100%">
    <div class="brand">{LOGO}Pocket Webcam</div>
    <h1 style="font-size:60px;margin-top:36px">Use your iPhone as a <span class="y">webcam</span> on Windows and Linux.</h1>
    <p class="sub" style="margin-top:16px;font-size:28px">Free. Wireless. Every lens, up to 4K.</p>
    <div class="footer"><span class="url" style="font-size:32px">pocketwebcam.com</span><span class="pill" style="font-size:22px;padding:12px 22px"><span class="dot"></span>Coming soon</span></div>
  </div>
  <div style="flex:1;position:relative">{ART}</div>
</div>"""
out = HERE / "../website/assets/og-image.png"
with tempfile.TemporaryDirectory() as d:
    html = pathlib.Path(d) / "og.html"
    html.write_text(f'<!doctype html><meta charset="utf-8"><style>{CSS}</style>{body}')
    subprocess.run(["chromium", "--headless=new", "--disable-gpu", "--hide-scrollbars",
                    "--window-size=1200,630", f"--screenshot={out.resolve()}", html.as_uri()],
                   check=True, stderr=subprocess.DEVNULL)
print(out.resolve())
