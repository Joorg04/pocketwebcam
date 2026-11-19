# Website (pocketwebcam.com)

Source: `website/`.

Static site: plain HTML + CSS, no build step, no JavaScript, no third-party requests.

| File | Page |
|---|---|
| `index.html` | `/` home |
| `download.html` | `/download` (linked from the iPhone app) |
| `privacy.html` | `/privacy` (linked from the iPhone app; required for App Review) |
| `help.html` | `/help` troubleshooting |
| `linux.html` | `/linux` Linux setup guide (SEO landing page; keep distro "tested" labels honest) |
| `windows.html` | `/windows` Windows 11 setup guide (SEO landing page; HowTo + FAQPage JSON-LD; says "preview" until Windows is tested) |
| `get.html` | `/get` phone-app picker the desktop app's QR code is meant to point to (iPhone card + Android "coming soon"); `noindex`, not in the sitemap |
| `vs/camo.html`, `vs/epoccam.html`, `vs/ivcam.html` | `/vs/…` comparison pages. Competitor facts only from the competitor's own site, sources and check date in an HTML comment at the top. Re-check before editing |
| `donate.html` | `/donate` Ko-fi donations and the Windows microphone plan (linked from every footer and `/help#mic`). Ko-fi page: `ko-fi.com/pocketwebcam` (ours, claimed 2026-10-21). |
| `404.html` | not-found page |
| `_headers` | security headers for Cloudflare Pages |
| `assets/` | stylesheet and logo |

Cloudflare serves `download.html` at `/download` (and `vs/camo.html` at `/vs/camo`) automatically.

## Preview locally

```bash
cd website && python3 -m http.server 8765
# or, with clean URLs like the live site:  python3 tools/preview_site.py
```

## Hosting (Cloudflare Workers static assets, free)

Live since 2026-10-13 as the Worker **`pocketwebcam`**:
- `https://pocketwebcam.com`: Worker custom domain
- `https://www.pocketwebcam.com`: Worker route `www.pocketwebcam.com/*`, with a proxied CNAME `www` → `pocketwebcam.com`
- The Worker's `workers.dev` address: test address (dashboard → Workers → `pocketwebcam`)
- "Always Use HTTPS" is on (SSL/TLS → Edge Certificates)

**To update the site** from the repo root (wrangler is signed in on the Linux machine;
`wrangler.jsonc` there holds the asset settings):

```sh
npx wrangler@4 versions upload --message "<what changed>"   # prints a version ID
npx wrangler@4 versions deploy <version-id>@100% -y
```

Upload + deploy (not `wrangler deploy`) only touches the site's files, never the custom domain,
the www route or the workers.dev setting. Earlier versions stay available for rollback
(`npx wrangler@4 deployments list`, then `versions deploy <old-id>@100%`). The dashboard upload
(Workers & Pages → `pocketwebcam` → Deployments) still works too.

**The `www` CNAME record (`www` → `pocketwebcam.com`, proxied/orange) must stay.** It looks redundant,
but the Worker route for www only works when that proxied record exists.

**DNS warning:** never touch the MX, TXT, `_dmarc`, `*._domainkey`, `mail`, `r.mail` or `img.mail`
records; they run the email (Cloudflare Email Routing + Brevo).

## Downloads (Cloudflare R2, free tier)

Files too big for the site (Workers' 25 MB limit) live in the R2 bucket **`pocketwebcam-downloads`**
(Standard storage class), served at `https://downloads.pocketwebcam.com` (R2 custom domain, added
2026-10-21). Linked from `download.html`.

- `windows/PocketWebcam-Setup-<version>.exe`: the Windows installer (CI artifact `PocketWebcam-Setup`).

**Stay inside the free tier, so nothing gets charged:** 10 GB storage, 1 million Class A operations
(uploads, lists) and 10 million Class B operations (reads) a month; egress is free.
- Keep one or two installer versions; delete old ones when a new one is linked.
- Upload with `Cache-Control: public, max-age=31536000, immutable` and a new file name per version,
  so Cloudflare's cache serves repeat downloads and they don't count as R2 reads. Never overwrite a
  published file name.
- Leave the bucket's public `r2.dev` URL **off** (it isn't cached and is rate-limited).
- Never switch the bucket to Infrequent Access (every read costs money there).
- Usage: dashboard → R2 object storage → `pocketwebcam-downloads` → Metrics.

Upload a new version (PowerShell blocks `npx`, so use `npx.cmd`; sign in once with `npx.cmd wrangler login`):

```powershell
npx.cmd wrangler r2 object put pocketwebcam-downloads/windows/PocketWebcam-Setup-0.1.0.exe --remote --file PocketWebcam-Setup-0.1.0.exe --content-type application/vnd.microsoft.portable-executable --cache-control "public, max-age=31536000, immutable" --content-disposition 'attachment; filename="PocketWebcam-Setup-0.1.0.exe"'
```

Then update the link and size in `download.html` and delete the oldest version:
`npx.cmd wrangler r2 object delete pocketwebcam-downloads/windows/<old file> --remote`.

## Caching: version changed assets

`_headers` lets browsers cache `/assets/*` for 24 hours. When you change `style.css` or a logo,
bump a version in the references in every page (`style.css` is at `?v=2` since 2026-11-04; next change → `?v=3`, `/assets/favicon.svg?v=2`),
or returning visitors keep seeing the old file for up to a day.

## At launch, update

- **iPhone link:** `get.html` has one marked block (`IPHONE APP LINK`): change its URL and label to
  the public TestFlight link when there is one, then to the App Store URL on launch day.
- **App Store badges:** `index.html` (hero) and `download.html` (iPhone card) each have a commented
  `APP STORE BADGE` block. Save Apple's official badge unchanged as `assets/app-store-badge.svg`,
  replace `APP_STORE_URL` with the same App Store URL, uncomment, and remove the button it replaces.
  Check: `grep -rn APP_STORE_URL website/` must print nothing afterwards.
- Download buttons → App Store link and Linux packages (`download.html`, `index.html`)
- "Coming soon" wording (`index.html` hero, `download.html`)
- `sitemap.xml` dates

## SEO (2026-10-15)

Done per `Pocket Webcam SEO Strategy.pdf`: keyword H1 ("Use your iPhone as a webcam on Windows and Linux"),
"Continuity Camera for Windows and Linux" platform heading, `og:image` + `twitter:card` on every page
(`assets/og-image.png`, 1200×630, rebuild with `python3 brand/build_og.py`), `SoftwareApplication` and
`FAQPage` JSON-LD on the homepage (FAQ text must match the visible FAQ), decorative hero art hidden from
assistive tech and snippets, sitemap lastmod updated.
Not done yet: Search Console + Bing verification, `/linux` page, signup form, AlternativeTo listing.
If `og-image.png` changes, bump `?v=` in the og:image URLs.

`/linux` added 2026-10-16: HowTo + FAQPage JSON-LD (FAQ text must match the visible FAQ), linked from the homepage Linux card and the download page, and in the sitemap. Only Arch is marked tested; update when other distros are verified.
