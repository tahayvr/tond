# Releasing Tond

## Version scheme

- [SemVer](https://semver.org/). Tags are `v<version>`. The bundle's `VERSION`
  file and `tond --version` show `<version>` without the `v`.
- One version for everything, set in `[workspace.package]` of `Cargo.toml`.
  That covers every crate, `tond --version`, tond-server's `current_version`
  and the update check. `dashboard/package.json` carries the same version.
  `scripts/check-version.sh` checks that they agree and that `CHANGELOG.md` has
  a section for the version. The release workflow refuses a tag that doesn't
  match.
- `0.x` while on the launch track. **0.2.0** is the first public release
  (Phases 0–3). A minor bump may break things; say so in the changelog.
  **1.0.0** comes once the launch gate passes.
- A version with a pre-release part (`0.3.0-rc.1`) is published as a GitHub
  pre-release. `<base>/latest`, `install.sh`, `update.sh` and the dashboard's
  update check all skip pre-releases. Install one explicitly with
  `TOND_VERSION=0.3.0-rc.1`.

## Cutting a release

```sh
just release-prep 0.2.0   # bump Cargo.toml/Cargo.lock, dashboard, docs example; start CHANGELOG.md; just check; just test
# edit CHANGELOG.md, commit "chore(release): v0.2.0", merge to main
# hosted e2e on the release commit (e2e/HOSTED.md): just e2e-hcloud --acme letsencrypt, both arches
just release 0.2.0        # checks, then PRINTS the git push / tag / push-tag commands; runs nothing
```

Running the printed commands pushes the tag. That starts
`.github/workflows/release.yml`:

1. **build**: checks the version (`scripts/check-version.sh $TAG`), then
   builds `tond-linux-x86_64` and `tond-linux-aarch64` natively and packages
   them (`release/package.sh`: tarball and `.sha256`).
2. **sign**: runs in the `release` environment, which holds
   `MINISIGN_SECRET_KEY`; add required reviewers there so a person approves
   each release. `release/sign.sh` checks the checksums, signs each tarball
   and each `.sha256` file, and verifies every signature with the committed
   `release/minisign.pub` (key id `F788AF428DA4C372`). It then attests SLSA
   build provenance for both tarballs. This job runs no code from the
   project's dependencies.
3. **release**: publishes the GitHub Release. The notes are the tag's
   section of `CHANGELOG.md`. The assets are, per architecture,
   `tond-linux-<arch>.tar.gz`, `.tar.gz.sha256`, `.tar.gz.minisig` and
   `.tar.gz.sha256.minisig`, plus `minisign.pub`.

Afterwards:

```sh
gh attestation verify tond-linux-x86_64.tar.gz --repo tahayvr/tond
minisign -Vm tond-linux-x86_64.tar.gz -p release/minisign.pub
scripts/release-smoke.sh --host root@<fresh server> --from 0.2.0             # first release: install only
scripts/release-smoke.sh --host root@<fresh server> --from 0.2.0 --to 0.2.1  # later: install, update, roll back
```

If `release/web/*.sh` changed, publish them to tond.io (`/install`,
`/update`) after the release is out. The tond.io copies verify with the key
embedded in them.

## Where clients download from

Releases live in this repository's public GitHub releases. Every downloader
uses the same layout, `<base>/download/<tag>/<asset>`, and finds the latest
release at `<base>/latest` (GitHub redirects that to `…/releases/tag/<tag>`;
a plain file server serves a file containing the tag). The downloaders are
`install.sh`, `update.sh`, `release/web/{install,update}.sh` (the shared
"release signatures" block) and `tond update --direct` (`tond_common::release`).

The base defaults to `https://github.com/tahayvr/tond/releases` and is
configurable in one way everywhere:

- `TOND_RELEASE_BASE=<url>` (env), or `--release-base <url>` for `install.sh`
  and `update.sh`.
- `install.sh` records a non-default base in `/opt/tond/RELEASE_BASE`, so
  `update.sh`, `tond update` and tond-server's update check (which reads
  `<base>/releases.json` for a non-GitHub base) keep using it.

Tests use this to point everything at a local file server
(`release/tests/signing-test.sh`, `scripts/release-smoke.sh --release-base`).

The tests that keep the pieces consistent are in
`crates/tond-common/src/release.rs`. They check that the four scripts share
the release block byte for byte, that no script builds download URLs outside
it, that `release.yml` publishes exactly the assets the clients fetch for both
architectures, and that the embedded keys match `release/minisign.pub` (in
`tond-cli`'s `signature.rs`).

## Testing the signing locally

```sh
just release-signing-test --fake      # stand-in bundles, seconds
just package-all && just release-signing-test   # the real bundles of both architectures
```

The test generates a **throwaway** minisign key in a temp directory and never
touches the real key. It signs with `release/sign.sh` exactly as the workflow
does and serves the release layout with `python3 -m http.server`. It then
downloads and verifies each architecture with each script's release block,
under bash and sh. It also checks that a tampered tarball, a signature from
another key, a missing signature and a malformed tag are all refused.
