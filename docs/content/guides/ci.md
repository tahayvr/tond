# CI with a deployer token

This guide deploys an app from a CI pipeline with an API token that can deploy that one app and nothing else. You need an existing app, a dashboard domain served over HTTPS, and root on the server (or an admin account in the dashboard) to create the token.

There are two ways to deploy from CI:

- **Upload the checked-out code** with `tond apps push`. The CI job's files are what gets built. Use this for tarball apps.
- **Trigger a deploy** of a Git app with `tond apps deploy` or one API call. Tond clones the app's branch itself.

## 1. Create a scoped token

On the server:

```sh
sudo tond token create --name github-actions --kind ci --preset deployer --app web --expires 90d
```

The token (`tond_pat_…`) is printed once, on stdout alone, so you can capture it with `TOKEN=$(sudo tond token create …)`. Store it as a secret in your CI system, for example a GitHub Actions secret named `TOND_TOKEN`.

The `deployer` preset grants `apps:read`, `apps:deploy` and `logs:read`. With `--app web` the token sees only that app: every other app returns `403 RESOURCE_DENIED`, and anything outside its scopes (deleting the app, reading secrets, changing settings) returns `403 SCOPE_DENIED`. Other useful options:

| Option | Effect |
| --- | --- |
| `--app <name>` | Limit to one app (repeatable) |
| `--project <name>` | Limit to the apps of a project (repeatable) |
| `--allow-ip <ip-or-cidr>` | Accept the token only from these addresses (repeatable); useful with self-hosted runners that have fixed IPs |
| `--expires 30d` | Lifetime such as `24h`, `30d`, `12w` or `never`; the default is 90 days |

To create the token in the dashboard instead, open **Profile → API tokens → New token**. Choose the **Deployer (CI)** preset under **Access**, pick the app under **Apps**, set **Expires**, and click **Create token**. The dashboard shows the token once.

A token can never do more than the user who created it can do right now. List tokens with `tond token list` and revoke one with `tond token revoke <name-or-id>`. See [Users, roles and tokens](/concepts/access).

## 2. Get the CLI in CI

The `tond` CLI ships in every release tarball, next to the server binaries. Linux on x86_64 and ARM64 is supported. Download the release that matches your server, check its checksum and install the binary:

```sh
TOND_VERSION=v0.1.0   # the version your server runs
ARCH=x86_64           # or aarch64
base="https://github.com/tahayvr/tond/releases/download/${TOND_VERSION}"
curl -fsSLO "${base}/tond-linux-${ARCH}.tar.gz"
curl -fsSLO "${base}/tond-linux-${ARCH}.tar.gz.sha256"
sha256sum -c "tond-linux-${ARCH}.tar.gz.sha256"
tar -xzf "tond-linux-${ARCH}.tar.gz"
sudo install "tond-linux-${ARCH}/tond" /usr/local/bin/tond
```

Pin the version instead of fetching the latest, so a new release never changes your pipeline unannounced. `cat /opt/tond/VERSION` on the server shows the version it runs; the release tag is that version with a `v` in front.

The CLI talks to a remote server when `TOND_SERVER` and `TOND_TOKEN` are set (or `--server` and `--token`):

```sh
export TOND_SERVER=https://tond.example.com
export TOND_TOKEN=tond_pat_…
tond apps info web
```

`TOND_SERVER` is the dashboard's address; the CLI adds `/api/v1` itself.

## 3. A GitHub Actions workflow

This workflow uploads the repository on every push to `main`:

```yaml
name: Deploy
on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install the tond CLI
        env:
          TOND_VERSION: v0.1.0
        run: |
          base="https://github.com/tahayvr/tond/releases/download/${TOND_VERSION}"
          curl -fsSLO "${base}/tond-linux-x86_64.tar.gz"
          curl -fsSLO "${base}/tond-linux-x86_64.tar.gz.sha256"
          sha256sum -c tond-linux-x86_64.tar.gz.sha256
          tar -xzf tond-linux-x86_64.tar.gz
          sudo install tond-linux-x86_64/tond /usr/local/bin/tond

      - name: Deploy
        env:
          TOND_SERVER: https://tond.example.com
          TOND_TOKEN: ${{ secrets.TOND_TOKEN }}
        run: tond apps push web
```

`tond apps push` packs the working directory (leaving out `.git`, `node_modules`, `dist`, `build` and the other defaults, plus anything in `.tondignore`), uploads it, and follows the build like `tond apps deploy` does (below). See [Build sources](/guides/sources) for what is excluded and the size limit.

For a Git app, replace the last step's command with:

```sh
tond apps deploy web
```

`tond apps deploy` queues a deploy of the app's branch and follows it, printing each phase and the build log. It exits `0` when the new version is live and non-zero otherwise, so the CI job fails with it: `6` when the build or the health check failed, `5` when a newer deploy superseded it or it was cancelled, `8` when the server was unreachable or restarting, `4` when the token isn't allowed. Add `--no-wait` to return as soon as the deploy is queued (both `deploy` and `push`). See [Exit codes](/reference/exit-codes).

A few flags make pipelines sturdier:

- `--json` prints the result as one JSON document on stdout (and an error as one JSON line on stderr), for later steps to read.
- `--idempotency-key "$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"` makes a retried job return the first job's deploy instead of starting another one.
- Commands that delete or roll back ask for confirmation in a terminal and refuse without one; add `--yes` when a pipeline really should do that.

## 4. Or call the API with curl

Every CLI command is an HTTP call with the token as a bearer token. To deploy a Git app:

```sh
curl -fsS -X POST "https://tond.example.com/api/v1/apps/web/deploy" \
  -H "Authorization: Bearer $TOND_TOKEN"
```

To upload a tarball you built yourself:

```sh
tar -czf site.tar.gz -C ./site .
curl -fsS -X POST "https://tond.example.com/api/v1/apps/web/upload" \
  -H "Authorization: Bearer $TOND_TOKEN" \
  -F "file=@site.tar.gz"
```

Both answer `202 Accepted` with the new deployment's ID in `data.deployment_id`. To wait for the result, poll the deployment until its `status` is final:

```sh
dep=$(curl -fsS -X POST "https://tond.example.com/api/v1/apps/web/deploy" \
  -H "Authorization: Bearer $TOND_TOKEN" | jq -r '.data.deployment_id')

while :; do
  status=$(curl -fsS "https://tond.example.com/api/v1/apps/web/deployments/${dep}" \
    -H "Authorization: Bearer $TOND_TOKEN" | jq -r '.data.status')
  case "$status" in
    succeeded) echo "deployed"; break ;;
    failed|superseded|cancelled) echo "deploy ${status}" >&2; exit 1 ;;
  esac
  sleep 5
done
```

A deploy moves through `queued` and `running` before it ends as `succeeded`, `failed`, `superseded` (a newer deploy replaced it in the queue) or `cancelled`. The response also carries the log lines. Errors come back as JSON with a stable `code`; see [Error codes](/reference/error-codes) and the [HTTP API](/reference/api/).

## Alternative: the generic webhook

If you'd rather not hand CI an API token, a Git app can be deployed through its generic webhook, authenticated with the app's webhook secret instead. Open the app's **Settings → Git → Webhooks → CI / Generic**, copy the URL and store the secret in CI, and turn on **Auto-deploy**:

```sh
curl -fsS -X POST "$TOND_WEBHOOK_URL" \
  -H "Authorization: Bearer $TOND_WEBHOOK_SECRET" \
  -H "X-Tond-Delivery: $GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"
```

Send a unique `X-Tond-Delivery` per run: Tond accepts each delivery ID once. The webhook can only trigger a deploy and doesn't report the result. See [Deploy from Git](/guides/deploy-from-git).

## Keep it tight

- One token per pipeline and app, with an expiry. Rotate by creating a new token, updating the CI secret, then revoking the old one.
- Every deploy records the token in the [audit log](/concepts/audit-log), so you can tell CI deploys from manual ones.
- Never give CI a token with admin scopes. The [Security hardening checklist](/guides/hardening) has more.
