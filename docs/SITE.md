# tond.io: building and deploying the website

Contributor notes, not part of the published docs. tond.io is two static Astro
builds in this directory, merged into one folder that a GitHub Actions workflow
(`.github/workflows/site.yml`) deploys to GitHub Pages from `main`.

| Path | Source |
| --- | --- |
| `/` and `/404.html` | the landing page: `landing/` (pages, layout, styles, screenshots), built with `astro.landing.config.mjs` |
| `/docs/…` | the docs: `content/`, built with `astro.config.mjs` (`base: '/docs'`, own link check, Pagefind, llms files) |
| `/install`, `/update` | `release/web/install.sh` and `release/web/update.sh`, byte-for-byte, no extension |
| `/llms.txt` | a copy of `/docs/llms.txt` |
| `/sitemap.xml`, `/CNAME` | written by `scripts/assemble-site.mjs` |

Both builds share `node_modules`, the tokens (`src/styles/tokens.css`, the
dashboard's palette), the logo and the icons. The landing page loads nothing
from other servers and sets no cookies; its only script is
`landing/scripts/landing.ts` (theme toggle, copy button).

## Commands

```sh
just site          # landing page dev server, http://localhost:4322
just docs          # docs dev server, http://localhost:4321/docs
just site-build    # everything -> docs/dist-site (also part of `just check`)
just site-preview  # serve docs/dist-site at http://localhost:4400
just docs-screenshots landing   # retake the landing page's dashboard shots and og.png
```

`just site-build` builds the docs, builds the landing page, assembles
`docs/dist-site`, checks every internal link of the merged site and runs
`scripts/site.test.mjs` (among others: `/install` and `/update` must equal the
committed scripts byte-for-byte).

### Why release/web/install.sh

`curl -fsSL https://tond.io/install | sudo bash` serves
`release/web/install.sh`, a small POSIX script that resolves the latest release,
verifies its checksum and minisign signature, and runs the `install.sh` inside
that release bundle with the user's arguments. Serving `release/install.sh`
directly would run the repository's installer against whatever release is
latest, and in download mode it copies the bundle into the current directory.
`/update` is `release/web/update.sh` for the same reason (the upgrading guide
links it).

GitHub Pages serves extensionless files as `application/octet-stream`. That is
fine: `curl` doesn't care about the content type and pipes the bytes to `bash`.
Browsers download the file instead of showing it.

## Deploying

`.github/workflows/site.yml` builds exactly what `just site-build` builds
(`pnpm build`, then `pnpm build:site` in `docs/`, with node and pnpm only: the
pages generated from Rust, `content/reference/cli.md`, `src/data/exit-codes.json`
and `openapi.json`, are committed) and deploys `docs/dist-site` with
`actions/upload-pages-artifact` and `actions/deploy-pages`.

It runs only on a push to `main` that changes `docs/**`, `release/web/install.sh`,
`release/web/update.sh` or the workflow itself, and by hand from the Actions tab
(`workflow_dispatch`). No pull-request or scheduled runs. A run that fails the
link check or the site tests deploys nothing.

## GitHub Pages settings (once)

In the repository: **Settings → Pages**

1. **Build and deployment → Source**: **GitHub Actions**.
2. **Custom domain**: `tond.io`, then **Save**. With an Actions deploy the
   domain comes from this setting, not from the `CNAME` file in the build.
3. **Enforce HTTPS**: tick it once GitHub has issued the certificate for
   tond.io (it can take a while after DNS is in place).

The first run creates the `github-pages` environment. If you add protection
rules to it, allow the `main` branch.

Recommended: verify `tond.io` at the account level (**your profile → Settings →
Pages → Add a domain**, a TXT record) so no other repository can claim the
domain.

The repository is public, so Pages works on any plan.

## DNS records for tond.io

At the DNS provider for tond.io:

| Name | Type | Value |
| --- | --- | --- |
| `tond.io` (apex, `@`) | `A` | `185.199.108.153` |
| `tond.io` | `A` | `185.199.109.153` |
| `tond.io` | `A` | `185.199.110.153` |
| `tond.io` | `A` | `185.199.111.153` |
| `tond.io` | `AAAA` | `2606:50c0:8000::153` |
| `tond.io` | `AAAA` | `2606:50c0:8001::153` |
| `tond.io` | `AAAA` | `2606:50c0:8002::153` |
| `tond.io` | `AAAA` | `2606:50c0:8003::153` |
| `www` | `CNAME` | `tahayvr.github.io` |

These are GitHub's documented Pages addresses at the time of writing; check
"Managing a custom domain for your GitHub Pages site" in GitHub's docs before
changing DNS. With both the apex and `www` set up, GitHub redirects
`www.tond.io` to `tond.io`. Remove any other `A`/`AAAA`/`ALIAS` records on the
apex, and don't use wildcard records (`*.tond.io`) pointing at GitHub.

Check with `dig +short tond.io A`, `dig +short tond.io AAAA` and
`dig +short www.tond.io CNAME`, then `curl -fsSL https://tond.io/install | head` (it must match
`release/web/install.sh`).
