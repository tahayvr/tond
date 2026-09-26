# Tond

Tond is a fast, lightweight, self-hosted platform for running apps and databases on one cheap VPS: deploy from Git, an image or a tarball, get HTTPS on a domain, previews, managed databases, snapshots to S3, metrics, uptime checks and alerts, from a dashboard, a CLI or an HTTP API.

```sh
curl -fsSL https://tond.io/install | sudo bash
```

Runs on Debian 12/13 and Ubuntu 24.04/26.04 LTS, x86_64 or ARM64.

**Documentation: [tond.io/docs](https://tond.io/docs)**: [Quickstart](https://tond.io/docs/quickstart) · [Concepts](https://tond.io/docs/concepts/apps) · [CLI](https://tond.io/docs/reference/cli) · [HTTP API](https://tond.io/docs/reference/api/) · [Upgrading](https://tond.io/docs/guides/upgrading) · [Security](https://tond.io/docs/security/threat-model) · [llms.txt](https://tond.io/docs/llms.txt)

## Development

The docs live in [`docs/`](docs/) (`just docs` for a live preview). [`AGENTS.md`](AGENTS.md) describes the repository and its conventions; `just check` and `just test` run the checks and tests.
