#!/usr/bin/env python3
"""Logo, illustration and CSS shared by the brand graphics (used by build_og.py)."""
import pathlib
import subprocess

HERE = pathlib.Path(__file__).parent
LOGO = (HERE / "logo.svg").resolve().read_text()

# Laptop + phone + Wi-Fi illustration (same as the website hero).
ART = """
<svg viewBox="0 44 520 270" xmlns="http://www.w3.org/2000/svg">
  <rect x="20" y="60" width="330" height="214" rx="14" fill="#1c1c1f" stroke="#33333a" stroke-width="2"/>
  <rect x="34" y="74" width="302" height="186" rx="6" fill="#24231e"/>
  <circle cx="185" cy="148" r="34" fill="#3a382f"/>
  <path d="M115 260c6-46 38-70 70-70s64 24 70 70z" fill="#3a382f"/>
  <rect x="46" y="86" width="64" height="20" rx="10" fill="#000" fill-opacity="0.45"/>
  <circle cx="58" cy="96" r="4" fill="#30d158"/>
  <text x="68" y="100" font-family="Noto Sans" font-size="10" font-weight="800" fill="#f5f5f7">LIVE 4K</text>
  <path d="M0 274h370l-16 20H16z" fill="#2a2a2e"/>
  <g fill="none" stroke="#ffd60a" stroke-width="5" stroke-linecap="round" transform="translate(372 150)">
    <path d="M8 34a16 16 0 0 1 22 0"/><path d="M0 24a28 28 0 0 1 38 0"/><path d="M-8 14a40 40 0 0 1 54 0"/>
  </g>
  <rect x="420" y="96" width="86" height="176" rx="18" fill="#0d0d0f" stroke="#3a3a40" stroke-width="2"/>
  <rect x="429" y="106" width="68" height="156" rx="12" fill="#18181b"/>
  <circle cx="446" cy="124" r="11" fill="#000" stroke="#3a3a40" stroke-width="2"/><circle cx="446" cy="124" r="5" fill="#2a2a50"/>
  <circle cx="470" cy="124" r="11" fill="#000" stroke="#3a3a40" stroke-width="2"/><circle cx="470" cy="124" r="5" fill="#2a2a50"/>
  <circle cx="458" cy="146" r="11" fill="#000" stroke="#3a3a40" stroke-width="2"/><circle cx="458" cy="146" r="5" fill="#2a2a50"/>
</svg>"""

CSS = """
* { box-sizing: border-box; margin: 0; }
html, body { width: 100%; height: 100%; }
body {
  font-family: "Noto Sans", sans-serif; color: #f5f5f7; background: #0b0b0c;
  -webkit-font-smoothing: antialiased; overflow: hidden;
}
.slide { position: relative; width: 100%; height: 100%; padding: 84px; display: flex; flex-direction: column; }
.glow::before {
  content: ""; position: absolute; width: 1100px; height: 1100px; right: -420px; top: -460px;
  background: radial-gradient(closest-side, rgba(255,214,10,.22), transparent 70%);
}
.brand { display: flex; align-items: center; gap: 18px; font-size: 34px; font-weight: 800; position: relative; }
.brand svg { width: 64px; height: 64px; border-radius: 16px; }
h1 { font-weight: 900; letter-spacing: -0.035em; line-height: 1.02; position: relative; }
.y { color: #ffd60a; }
.sub { color: #a1a1a8; font-size: 38px; line-height: 1.35; font-weight: 500; }
.pill {
  display: inline-flex; align-items: center; gap: 14px; padding: 16px 28px; border-radius: 999px;
  background: rgba(255,214,10,.12); border: 2px solid rgba(255,214,10,.35); color: #ffd60a;
  font-size: 30px; font-weight: 800;
}
.dot { width: 14px; height: 14px; border-radius: 50%; background: #ffd60a; box-shadow: 0 0 16px #ffd60a; }
.footer { margin-top: auto; display: flex; justify-content: space-between; align-items: center; position: relative; }
.swipe { color: #a1a1a8; font-size: 30px; font-weight: 700; }
.page { color: #6e6e75; font-size: 26px; font-weight: 700; }
.chips { display: flex; gap: 26px; align-items: center; }
.chip {
  display: grid; place-items: center; border-radius: 50%; background: #000; border: 3px solid #2a2a2e;
  width: 170px; height: 170px; font-size: 52px; font-weight: 900;
}
.chip.on { width: 210px; height: 210px; color: #ffd60a; border-color: #ffd60a; font-size: 64px; }
.stats { display: flex; gap: 22px; }
.stat {
  flex: 1; background: #18181b; border: 2px solid #2a2a2e; border-radius: 32px; padding: 34px 36px;
}
.stat b { display: block; font-size: 72px; font-weight: 900; letter-spacing: -0.03em; color: #ffd60a; }
.stat span { font-size: 28px; color: #a1a1a8; font-weight: 600; }
.yellow { background: #ffd60a; color: #1a1500; }
.yellow .sub { color: #3d3410; }
.yellow .page { color: #6b5d12; }
.checks { list-style: none; padding: 0; display: flex; flex-direction: column; gap: 30px; }
.checks li { display: flex; align-items: center; gap: 28px; font-size: 58px; font-weight: 800; letter-spacing: -0.02em; }
.checks li::before {
  content: "✓"; display: grid; place-items: center; flex: none; width: 76px; height: 76px; border-radius: 50%;
  background: #1a1500; color: #ffd60a; font-size: 38px; font-weight: 900;
}
.steps { display: flex; flex-direction: column; gap: 26px; }
.step { display: flex; gap: 28px; align-items: center; background: #18181b; border: 2px solid #2a2a2e; border-radius: 30px; padding: 30px 34px; }
.num { flex: none; display: grid; place-items: center; width: 70px; height: 70px; border-radius: 50%; background: #ffd60a; color: #1a1500; font-size: 36px; font-weight: 900; }
.step b { display: block; font-size: 38px; font-weight: 800; }
.step span { font-size: 28px; color: #a1a1a8; font-weight: 500; }
.url { font-size: 44px; font-weight: 900; color: #ffd60a; letter-spacing: -0.01em; }
"""

