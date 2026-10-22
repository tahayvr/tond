# Release signing

Every Tond release tarball (`tond-linux-<arch>.tar.gz`) and its checksum file
are signed with [minisign](https://jedisct1.github.io/minisign/), and each
tarball carries a [SLSA build provenance](https://slsa.dev/) attestation made
by GitHub Actions (`actions/attest-build-provenance`, keyless via GitHub OIDC).

The public key is [`release/minisign.pub`](minisign.pub). It is also embedded
in `install.sh`, `update.sh`, the tond.io web installers and the `tond` CLI
(a test keeps the copies identical).

## What checks the signature

`install.sh` (download mode), `update.sh`, `curl -fsSL https://tond.io/install`,
`curl -fsSL https://tond.io/update` and `tond update` all:

1. pin the release (resolve "latest" to a tag first),
2. verify the SHA-256 checksum,
3. verify `tond-linux-<arch>.tar.gz.minisig` with the embedded public key
   (the shell scripts need only `openssl`; the CLI verifies in Rust),

before running anything from the tarball. A signature that does not verify is
always fatal. A **missing** signature is also refused ("fail closed"), except
for releases published before signing existed (v0.1.0) or when you set
`TOND_ALLOW_UNSIGNED=1` on purpose.

## Verify a release by hand

```sh
# Signature (minisign 0.10+)
minisign -Vm tond-linux-x86_64.tar.gz -p minisign.pub

# Build provenance: built by this repository's release workflow from the tag
gh attestation verify tond-linux-x86_64.tar.gz --repo tahayvr/tond
```

## Maintainers: setting up the key (once)

```sh
minisign -G -W -p release/minisign.pub -s tond-release.key   # -W: no password
```

1. Commit `release/minisign.pub`, and copy its second line (the `RW…` key)
   into the `TOND_RELEASE_PUBKEY="…"` line of `release/install.sh`,
   `release/update.sh`, `release/web/install.sh` and `release/web/update.sh`
   (`cargo test -p tond-cli signature` fails until they match).
2. Store the content of `tond-release.key` as the `MINISIGN_SECRET_KEY` secret
   of the `release` environment (Settings → Environments; add required
   reviewers there), then delete the local file or keep it offline.
3. Push to `main`: the Site workflow (`.github/workflows/site.yml`) republishes
   the web installers at tond.io/install and tond.io/update.

The release workflow refuses to publish when the secret is missing, when
`release/minisign.pub` has no key, or when a signature does not verify with
the committed key.

Rotating the key: generate a new pair, update all copies, and release. Clients
older than the rotation verify with the old key, so sign one transition
release with the old key that ships the new one.
