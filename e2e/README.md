# End-to-end tests

`smoke.sh` installs nothing itself: it drives a Tond that was just installed with
`release/install.sh`, exactly as a user would get it — systemd services, Docker,
tond-proxy routing and ACME, the CLI over its Unix socket, and the API over TCP.
`run.sh` wraps it: install a bundle, set up ACME, run the smoke test.

What the smoke test covers:

- services up, API health
- auth boundaries: loopback TCP and proxied requests without credentials get 401,
  the CLI socket is `0600 root` and refuses non-root users, API keys over TCP
- data directory permissions: `tond.db` shared only with the `tond-proxy` group, TLS/ACME
  keys owner-only, nothing world-readable
- service sandboxing: tond-proxy runs as `tond-proxy` with only `CAP_NET_BIND_SERVICE`,
  cannot read `tond.env` or reach Docker; systemd hardening on both units
- previews (with `--preview-domain`): served only under the preview domain, the dashboard
  and API are 404 there, same-site preview domains are rejected, previews are refused
  without a preview domain
- auth hardening: root API key and setup token only in `0600 root` files (never in
  the journal), token-gated single-use setup, `__Host-` Secure/SameSite=Strict session
  cookie, CSRF header + Origin checks, cross-origin WebSocket refusal, security
  headers/HSTS, 2FA re-auth, host terminal off by default (socket-only switch,
  step-up, single-use tickets, audit log), rate limiter not bypassable with a
  spoofed `X-Forwarded-For`, failed-login backoff. It creates the admin account
  `E2E_ADMIN_EMAIL` / `E2E_ADMIN_PASSWORD` through the setup token.
- an image app (pull, run, route through the proxy, HTTP→HTTPS redirect)
- a pushed app (`tond apps push` → TBuild → `docker build` → run → route)
- a real certificate issued over ACME HTTP-01 and verified against the CA
- a managed Redis database, a cron job run, a full snapshot
- apps survive restarting `tond-server` and `tond-proxy`

## Where it runs

| Where | Command | ACME | When |
| --- | --- | --- | --- |
| Local OrbStack VM | `just vm-e2e` / `just arch=arm64 vm-e2e` | Pebble | while developing (the default) |
| GitHub Actions runners | `ci.yml` → `e2e` job (x86_64 + aarch64 matrix) | Pebble (local test CA) | manual only (Actions tab), to save minutes |
| Throwaway Hetzner server | `just e2e-hcloud` / `e2e-hetzner.yml` (arch input) | Let's Encrypt staging, real DNS via `<ip>.sslip.io` (previews on `<ip>.nip.io`) | manual, before releases |

All three install the bundle built by `release/package.sh` (`just package`, or
`just arch=arm64 package` for `dist/tond-linux-aarch64`), the same packaging the
release workflow publishes for both architectures. The local and CI runs use
`tond.test` for apps (dashboard `dash.tond.test`) and `preview.test` as the preview
domain — a different site, as required.

## Local VM (OrbStack)

```bash
brew install --cask orbstack   # once, then open OrbStack
just vm-create                 # x86_64 Ubuntu 24.04 machine "tond-vm"
just vm-install                # build + install Tond
just server=root@tond-vm@orb deploy   # fast redeploys, same as the old tond-dev flow
just vm-e2e                    # full smoke test (resets the VM's ACME state)
```

ARM64 runs natively on Apple Silicon (no Rosetta), so it is the faster loop there:

```bash
just arch=arm64 vm-create      # arm64 Ubuntu 24.04 machine "tond-vm-arm64"
just arch=arm64 vm-e2e         # build dist/tond-linux-aarch64 + smoke test
just arch=arm64 vm-destroy
```

Cross-compiling for aarch64 needs the Rust target once:
`rustup target add aarch64-unknown-linux-gnu` (cargo-zigbuild provides the linker).

## Throwaway Hetzner server

Needs the `hcloud` CLI with an active context (or `HCLOUD_TOKEN`).

```bash
just e2e-hcloud                 # cx23 in nbg1, deleted afterwards (~€0.01 per run)
just arch=arm64 e2e-hcloud      # ARM64: cax11
just e2e-hcloud --keep          # keep the server for debugging (prints SSH + delete commands)
just e2e-hcloud --acme pebble   # same flow as CI, on Hetzner
just e2e-hcloud-gc              # delete leftovers from killed runs (older than 3 h)
```

For the manual `e2e-hetzner.yml` workflow, add a read/write Hetzner Cloud API token as the
`HCLOUD_TOKEN` repository secret.
