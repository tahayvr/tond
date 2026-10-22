# Hosted end-to-end run (Hetzner)

The full e2e suite on real, throwaway Hetzner Cloud servers: one x86_64 (CX23)
and one ARM64 (CAX11). It uses public DNS (`<ip>.sslip.io`), a real ACME CA
(Let's Encrypt staging or production), real DNS-01 through Cloudflare, and the
resource budgets (`e2e/budget.sh`, which are only a gate on real hardware).
Run it from your machine before a release; it uses no GitHub Actions minutes.

Each server runs `e2e/run.sh --test both`. That installs the bundle exactly as
users get it (`install.sh`), runs `budget.sh` on the fresh install, then runs
`smoke.sh` on the same server. The smoke test covers the whole API, the CLI,
routing, ACME HTTP-01, a DNS-01 wildcard for `*.<server>.<zone>` via Tond's
Cloudflare provider, and self-update and rollback.

## What you need (once)

- `hcloud` with an active context whose token is **Read & Write**, from a
  Hetzner project used only for e2e (Console → project → Security → API
  tokens). The project needs quota for two servers. CAX (ARM) is only
  available in some locations; `nbg1` (the default) has both types.
- `jq`.
- For DNS-01: `CLOUDFLARE_API_TOKEN`, a Cloudflare token with **Zone → DNS →
  Edit** restricted to the test zone, and that zone passed as `--dns-zone`
  (e.g. `tond-e2e.dev`). Use a **dedicated domain**: the token can edit every
  record in it. Records are named `tond-e2e-<date>-…` and deleted after each run.

## Start a run

```sh
just e2e-hcloud --test both --dns-zone tond-e2e.dev --artifacts /tmp/e2e-x86
just arch=arm64 e2e-hcloud --test both --dns-zone tond-e2e.dev --artifacts /tmp/e2e-arm
```

`--test` is `both` (budget on the fresh install, then smoke), `smoke` or
`budget`. `--acme` picks `letsencrypt-staging` (default), `letsencrypt` or
`pebble`. Leave out `--dns-zone` to skip DNS-01.

## Results

The `--artifacts` directory gets:

- `run.log`: the full console output (install, budget, smoke)
- `budget.json`: every budget measurement with its limit
- `journal-tond-server.log`, `journal-tond-proxy.log`, `journal-tond-update.log`, `journal-docker.log`
- `update.log` and `update-status.json`: the self-update and rollback runs
- `summary.txt`: the exit status, OS, kernel, Docker version and memory

## Cleanup guarantees

Servers cost money until they are deleted, so there are two layers:

1. `e2e/hcloud.sh` deletes its server, its SSH key and its DNS records in an
   `EXIT` trap. The trap also runs on failure, Ctrl-C and SIGTERM, and retries
   the delete.
2. Each run starts with `hcloud.sh --gc`, which deletes e2e servers, keys and
   DNS records older than 3 hours.

To check by hand: `hcloud server list -l tond-e2e=true`. To delete
everything left over: `e2e/hcloud.sh --gc --max-age-hours 0 --dns-zone <zone>`.

## Cost per run

These are estimates. Check current prices.

- **Hetzner**: Cloud servers are billed per started hour. A run keeps each
  server for 35–60 minutes (install ~3 min, budget ~15 min, smoke ~20–30 min).
  CX23 costs about €0.006/h and CAX11 about €0.007/h, plus about €0.001/h for
  each primary IPv4. That is **about €0.02 per run** for both architectures,
  and well under €1 a month even when run daily.
- **Let's Encrypt**: free. Production has rate limits: 50 certificates per
  registered domain per week, and 5 duplicate certificates per week.
  `sslip.io` and `nip.io` are on the Public Suffix List, so each server IP
  counts as its own domain. The test zone gets one wildcard certificate per
  run and architecture. Use staging (the default) for routine runs, and
  production only before a release.
- **Cloudflare**: free plan.

## When it fails

- Read `run.log` from the failed step. The smoke test prints diagnostics (both
  journals, containers, apps) when it fails.
- If a budget failed, compare `budget.json` across architectures. The run
  still continues with the smoke test, and the job fails at the end.
- To reproduce interactively: `just e2e-hcloud --keep ...` keeps the server
  and prints the SSH command and how to delete it.
