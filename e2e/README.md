# End-to-end tests

`smoke.sh` installs nothing itself: it drives a Tond that was just installed with
`release/install.sh`, exactly as a user would get it — systemd services, Docker,
tond-proxy routing and ACME, the CLI over its Unix socket, and the API over TCP.
`run.sh` wraps it: install a bundle, set up ACME, run the smoke test.

What the smoke test covers:

- services up, API health
- auth boundaries: loopback TCP and proxied requests without credentials get 401,
  the CLI socket is `0600 root` and refuses non-root users, API keys over TCP
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
| Local OrbStack VM | `just vm-e2e` | Pebble | while developing (the default) |
| GitHub Actions runner | `ci.yml` → `e2e` job | Pebble (local test CA) | manual only (Actions tab), to save minutes |
| Throwaway Hetzner server | `just e2e-hcloud` / `e2e-hetzner.yml` | Let's Encrypt staging, real DNS via `<ip>.sslip.io` | manual, before releases |

All three install the bundle built by `release/package.sh` (`just package`), the
same packaging the release workflow publishes.

## Local VM (OrbStack)

```bash
brew install --cask orbstack   # once, then open OrbStack
just vm-create                 # x86_64 Ubuntu 24.04 machine "tond-vm"
just vm-install                # build + install Tond
just server=root@tond-vm@orb deploy   # fast redeploys, same as the old tond-dev flow
just vm-e2e                    # full smoke test (resets the VM's ACME state)
```

## Throwaway Hetzner server

Needs the `hcloud` CLI with an active context (or `HCLOUD_TOKEN`).

```bash
just e2e-hcloud                 # cx23 in nbg1, deleted afterwards (~€0.01 per run)
just e2e-hcloud --keep          # keep the server for debugging (prints SSH + delete commands)
just e2e-hcloud --acme pebble   # same flow as CI, on Hetzner
just e2e-hcloud-gc              # delete leftovers from killed runs (older than 3 h)
```

For the manual `e2e-hetzner.yml` workflow, add a read/write Hetzner Cloud API token as the
`HCLOUD_TOKEN` repository secret.
