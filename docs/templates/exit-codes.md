# Exit codes

The `tond` CLI, the installer and the update script exit with codes a script or CI job can act on. `0` always means success.

## tond CLI

Each exit code stands for a class of failure, and the API [error codes](/reference/error-codes) map onto them, so a script can tell "not found" from "not allowed" from "the server is restarting" without reading messages. `tond help exit-codes` prints this table on any machine (`--json` for a machine-readable copy).

<!-- table:exit-codes -->

### Codes the CLI reports itself

Some failures never reach the server. The CLI gives them codes of its own:

<!-- table:local-codes -->

### Reading errors in scripts

Errors go to stderr as `Error: …`, followed by `Hint: …` when there is advice. With `--json`, stdout holds only the command's JSON result, and a failure prints exactly one JSON line on stderr:

```json
{"error":{"code":"APP_NOT_FOUND","message":"App not found: web","hint":null,"field":null},"exit_code":3}
```

`code` is `null` when the server sent none (for example a proxy's HTML error page).

```sh
tond apps deploy web --json > deploy.json
case $? in
  0) echo "deployed" ;;
  6) echo "the build or the health check failed" >&2; exit 1 ;;
  8) echo "tond-server is unavailable; retry later" >&2; exit 75 ;;
  *) echo "deploy did not start" >&2; exit 1 ;;
esac
```

Destructive commands ask for confirmation in a terminal. Without a terminal (CI, cron) they refuse with exit `2` unless you pass `--yes`.

### SIGPIPE

When the reading end of a pipe closes (`tond apps logs web | head`), the CLI ends quietly on SIGPIPE, which shells report as `141`. With `set -o pipefail` a pipeline can therefore report 141; treat it as success.

## install.sh

| Code | Meaning |
| --- | --- |
| `0` | Installed and running. `--help` also exits `0`. |
| `1` | A fatal error stopped the installation; the reason is printed in red. |
| `130` | Interrupted (Ctrl+C, SIGINT or SIGTERM). |
| other | The script runs with `set -euo pipefail`: an unexpected failing command ends it with that command's code. |

Exit `1` covers, among others:

- A distribution outside the [support matrix](/support-matrix) without `--allow-unsupported-os`, a non-Linux system or an unsupported CPU architecture (checked before anything is downloaded or asked).
- An unknown flag, or an invalid `--dashboard-domain`, `--apps-domain` or `--preview-domain`.
- Not running as root, or a bundle built for another architecture.
- `git` older than 2.31, or no Docker packages for this system in Docker's apt repository.
- A release download whose checksum or signature doesn't verify (a missing signature is refused unless `TOND_ALLOW_UNSIGNED=1`).
- `tond-server` or `tond-proxy` not running at the end, or a domain rejected by `tond-server` (`Installation completed with errors.`).

A failed installation can be run again. It keeps the existing encryption key and data.

## update.sh

| Code | Meaning |
| --- | --- |
| `0` | Updated, rolled back as asked, or nothing to do (already on that version). |
| `1` | The update didn't happen, or it failed and the previous version is running again. The last line and `/opt/tond/data/update/status.json` say which. |
| `2` | Bad options (for example `--rollback` with `--version`). |
| `3` | The update failed **and** putting the previous version back failed too: Tond may be down. The message lists the files to copy back by hand. |
| `130` | Interrupted. |

`tond update` exits non-zero whenever the run didn't succeed: `1` for a failed or rolled-back update, `8` when it can't reach tond-server to start one. See [Upgrading Tond](/guides/upgrading).
