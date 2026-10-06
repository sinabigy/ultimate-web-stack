# Project website

`index.html` is the project's landing page: one self-contained static file with inline CSS, a few
lines of inline JavaScript (the copy button, which is optional), and inline SVG. It has no build
step, no external scripts, fonts or images, and no tracking.

## Preview locally

Open the file directly:

```sh
open site/index.html          # macOS
xdg-open site/index.html      # Linux
```

Or serve it (closer to how it will be hosted):

```sh
python3 -m http.server -d site 8000
# then visit http://localhost:8000
```

The copy button uses the Clipboard API, which needs a secure context (`https://` or
`localhost`). From `file://` it falls back to selecting the text.

## Repository links and naming

Links point at `sinabigy/ultimate-web-stack`, documentation at `blob/main/`. To rename the
project, find-and-replace `Ultimate Web Stack` (title, header, hero, footer) and the repository
slug. There is no sponsorship link until a funding account exists: add one next to the footer's
"Star on GitHub" button when it does.

## Keeping the numbers honest

Every number on the page is copied from `docs/FINAL_REPORT.md`, `docs/FINAL_ACCEPTANCE.md`,
`docs/benchmarks/SUMMARY.md`, `CHANGELOG.md`, `README.md` or
`docs/BLUEPRINT_CAPABILITY_MATRIX.md`. When those documents change (a new release, new
benchmarks), update the page from them. Do not add numbers that are not in them. Keep the
"What is not yet proven" section in step with the documents' Known limitations.

## Deploying (not done yet; a maintainer decision)

**Recommended: GitHub Pages via the included workflow** (`.github/workflows/pages.yml`).
- It runs on the same account, at no cost, with HTTPS and no extra service or credentials.
- It deploys only when run by hand, and refuses to deploy if placeholders reappear.
- The page's canonical and Open Graph URLs already point at
  `https://sinabigy.github.io/ultimate-web-stack/`.

```sh
# 1. enable Pages with GitHub Actions as the source (once)
gh api -X POST repos/sinabigy/ultimate-web-stack/pages -f build_type=workflow
# 2. deploy
gh workflow run pages.yml -R sinabigy/ultimate-web-stack
gh run watch -R sinabigy/ultimate-web-stack $(gh run list -R sinabigy/ultimate-web-stack --workflow pages.yml --limit 1 --json databaseId --jq '.[0].databaseId')
# 3. verify, then set the repository homepage
curl -sI https://sinabigy.github.io/ultimate-web-stack/ | head -1
gh repo edit sinabigy/ultimate-web-stack --homepage https://sinabigy.github.io/ultimate-web-stack/
```

For a custom domain later: Settings → Pages → Custom domain (plus a DNS `CNAME` to
`sinabigy.github.io`). Then update the `canonical`, `og:url` and `og:image` URLs in
`index.html`.

### Cloudflare Pages

1. **Workers & Pages → Create → Pages → Connect to Git**, and pick the repository.
2. Build settings: framework preset **None**, build command empty, build output directory
   `site`.
3. Deploy. Every push to the production branch redeploys; other branches get preview URLs.
4. Alternatively, without Git integration: `npx wrangler pages deploy site --project-name <name>`.

### Any static host

Upload `index.html`. Recommended response headers (set them in the host's configuration):

```
Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'
X-Content-Type-Options: nosniff
Referrer-Policy: strict-origin-when-cross-origin
```

## Checks done on the current version

- HTML well-formedness (all tags balanced, unique ids, every in-page anchor resolves).
- No horizontal page scroll at 375 px; rendered at 375 px and 1280 px in light and dark color
  schemes with no console errors.
- Every number on the page matched against the six source documents.
