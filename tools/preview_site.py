#!/usr/bin/env python3
"""Local preview of website/ that serves /privacy as privacy.html, like Cloudflare Pages.

Usage: python3 tools/preview_site.py [port]   then open http://127.0.0.1:8765
"""
import http.server
import os
import sys

SITE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "website")


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=SITE, **kwargs)

    def send_head(self):
        path = self.path.split("?", 1)[0].split("#", 1)[0]
        local = self.translate_path(path)
        if not os.path.exists(local) and os.path.exists(local + ".html"):
            self.path = path + ".html"
        elif not os.path.exists(local):
            self.path = "/404.html"
        return super().send_head()


port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
print(f"Preview: http://127.0.0.1:{port}  (Ctrl+C to stop)")
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
