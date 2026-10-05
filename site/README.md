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

## Before publishing: replace the placeholders

| placeholder | where | replace with |
|---|---|---|
| `Ultimate Web Stack` | page title, header, hero, footer | the final project name |
| `OWNER/REPO` | every GitHub link | the real `owner/repository` |
| `sponsors/OWNER` | "Support development" link | the real GitHub Sponsors account, or remove the link |
| `blob/main/` | documentation links | the default branch, if it is not `main` |

A single find-and-replace per row is enough:

```sh
sed -i '' 's#OWNER/REPO#acme/web-stack#g; s#sponsors/OWNER#sponsors/acme#g' site/index.html   # macOS sed
```

## Keeping the numbers honest

Every number on the page is copied from `docs/FINAL_REPORT.md`, `docs/FINAL_ACCEPTANCE.md`,
`docs/benchmarks/SUMMARY.md`, `CHANGELOG.md`, `README.md` or
`docs/BLUEPRINT_CAPABILITY_MATRIX.md`. When those documents change (a new release, new
benchmarks), update the page from them. Do not add numbers that are not in them. Keep the
"What is not yet proven" section in step with the documents' Known limitations.

## Deploying later (not done yet)

Nothing has been deployed, no domain is registered, and no GitHub resources were created. When
you are ready, either option serves the `site/` directory as-is.

### GitHub Pages

1. Push the repository to GitHub.
2. Either:
   - **Settings → Pages → Build and deployment → Source: GitHub Actions**, and add a workflow
     that uploads `site/` with `actions/upload-pages-artifact` and publishes it with
     `actions/deploy-pages`; or
   - publish from a branch: Pages can only serve the repository root or `/docs` from a branch,
     so for this layout the Actions route is simpler.
3. The page is then served at `https://OWNER.github.io/REPO/`. All links on the page are
   absolute or in-page anchors, so it works under that sub-path.
4. Optional custom domain: **Settings → Pages → Custom domain**, plus a `CNAME` DNS record
   pointing at `OWNER.github.io`.

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
