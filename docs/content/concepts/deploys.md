# Deploys and rollback

A deploy turns an app's source into a new running container and moves traffic to it without dropping requests. Every deploy is recorded as a deployment with its commit, trigger, phases, log and, on failure, a likely cause. This page explains how a deploy runs, how the build queue shares the server, and how to go back to an earlier version.

## Starting a deploy

Deploys start from:

| Trigger | Recorded as |
| --- | --- |
| **Deploy** in the dashboard | `manual`, with your email |
| `tond apps deploy <app>` on the server | `cli` |
| An API token, e.g. from CI | `api`, with the token name |
| A Git push webhook, when auto-deploy is on | `webhook` |
| `tond apps push <app>` (upload) | `upload` |
| **Deploy** on a project | `manual`, one deployment per member app |
| A rollback | `rollback` |

```sh
tond apps deploy web              # queue it and follow the progress
tond apps deploy web --no-wait    # return as soon as it is queued
```

The API answers `202 Accepted` with the deployment ID and its place in the queue. The deploy itself runs in the background.

## The build queue

Every production deploy goes through one queue:

- **One deploy per app at a time.** Deploys of the same app never overlap.
- **At most one waiting deploy per app.** A newer request replaces the one that is waiting. The replaced deployment is marked `superseded`. A burst of pushes builds only the first and the last.
- **A global limit on builds.** At most `TOND_BUILD_CONCURRENCY` image builds run at once across all apps and previews. The default is `1`. Only the build holds a slot: health checks and cutover don't, so one app's slow health check doesn't block another app's build.

To allow more builds at once, set `TOND_BUILD_CONCURRENCY` in tond-server's environment. See [Configuration](/reference/configuration).

Builds run in the systemd slice `tondbuild.slice`. The installer caps it at 60% of RAM (`MemoryMax=60%`) and gives it a lower CPU and I/O weight than running apps. A runaway `npm install` is killed inside its build instead of pushing the whole server into an out-of-memory state.

If Tond is stopped or restarted, deploys that were waiting stay queued and resume at the next start, in their original order. Uploads are the exception: their files are gone after a restart, so you need to upload again. A deploy that was running when Tond stopped is marked failed.

## How a deploy runs

A deployment moves through up to five phases. The dashboard shows them live on the app's **Deployments** tab.

<figure class="diagram">
<svg viewBox="0 0 900 250" role="img" aria-labelledby="deploy-title deploy-desc" xmlns="http://www.w3.org/2000/svg">
<title id="deploy-title">How a deploy runs</title>
<desc id="deploy-desc">A deploy starts from a push, the dashboard, the CLI or the API and waits in the build queue (one per app, a global number of build slots). It is built into its own image, started next to the running version on a new local port, and health-checked directly. When it is healthy, tond-proxy switches traffic and acknowledges, then the old container drains and stops. If the build or the health check fails, the new container is removed and the old version keeps serving.</desc>
<defs><marker id="deploy-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="var(--muted-foreground)"/></marker></defs>
<g font-size="12" fill="var(--foreground)">
<rect x="10" y="30" width="140" height="74" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="80" y="56" text-anchor="middle" font-weight="600">Trigger</text>
<text x="80" y="75" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">push, CLI,</text>
<text x="80" y="90" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">dashboard, API</text>
<rect x="175" y="30" width="140" height="74" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="245" y="56" text-anchor="middle" font-weight="600">Queue</text>
<text x="245" y="75" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">one per app,</text>
<text x="245" y="90" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">build slots</text>
<rect x="340" y="30" width="150" height="74" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="415" y="56" text-anchor="middle" font-weight="600">Build</text>
<text x="415" y="75" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">its own image,</text>
<text x="415" y="90" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">tondbuild.slice</text>
<rect x="515" y="30" width="160" height="74" rx="6" fill="var(--card)" stroke="var(--input)"/>
<text x="595" y="56" text-anchor="middle" font-weight="600">Start and check</text>
<text x="595" y="75" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">next to the old one,</text>
<text x="595" y="90" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">health gate</text>
<rect x="700" y="30" width="190" height="74" rx="6" fill="var(--card)" stroke="var(--foreground)" stroke-width="1.5"/>
<text x="795" y="56" text-anchor="middle" font-weight="600">Switch</text>
<text x="795" y="75" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">proxy moves traffic,</text>
<text x="795" y="90" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">old one drains, stops</text>
<g stroke="var(--muted-foreground)" stroke-width="1.4" fill="none" marker-end="url(#deploy-arrow)">
<path d="M150 67 H171"/><path d="M315 67 H336"/><path d="M490 67 H511"/><path d="M675 67 H696"/>
<path d="M415 104 V156"/><path d="M595 104 V156"/>
</g>
<rect x="330" y="160" width="360" height="60" rx="6" fill="var(--card)" stroke="var(--destructive)"/>
<text x="510" y="185" text-anchor="middle" font-weight="600" fill="var(--destructive)">Failed</text>
<text x="510" y="204" text-anchor="middle" fill="var(--muted-foreground)" font-size="11">new container removed, old version keeps serving</text>
<text x="795" y="128" text-anchor="middle" fill="var(--success)" font-size="11">succeeded</text>
</g>
</svg>
<figcaption>Traffic moves only after the new version is healthy and the proxy has confirmed the switch.</figcaption>
</figure>

| Phase | What happens |
| --- | --- |
| Clone | Git apps: clone the branch. Image apps: pull the image. |
| Build | Build the image with TBuild or the Dockerfile. Skipped for image apps. |
| Start | Start the new container next to the old one, on a new host port |
| Health | Wait until the new container is ready |
| Release | Switch traffic to the new container, then stop the old one |

In detail:

1. **Image.** Every deployment runs its own image, tagged `tond-<app>:<first 12 characters of the deployment ID>`. Pulled images are re-tagged this way too, so a later rollback runs exactly the bytes you deployed, even if `nginx:latest` has moved on since.
2. **Start.** On a redeploy the new container starts as `tond-<app>-next` while `tond-<app>` keeps serving. Each container gets a fresh host port on `127.0.0.1`.
3. **Health gate.** Tond probes the new container directly, not through the proxy. How depends on the [app kind](/concepts/apps):
   - web app with a health check path: the path must answer 2xx within the drain timeout;
   - web app without one: any HTTP answer on the port, within at least 60 seconds;
   - TCP service: the port must accept connections;
   - background worker: the container must stay up for `ready_after_secs`.

   If the container isn't ready in time, or exits, Tond removes it and the deploy fails. **The previous version keeps running.**
4. **Cutover.** Tond records the new port, which bumps the routing version. It then waits until tond-proxy confirms it routes to the new container, usually within milliseconds.
5. **Drain.** Tond sends the old container `SIGTERM` and gives it the drain timeout to finish in-flight requests before it is removed. Then the new container is renamed to `tond-<app>`.

The **drain timeout** (default 30 seconds) sets both how long the health gate waits for a health check path to pass and the grace period of the old container:

```sh
tond apps drain-timeout web 60
```

Docker Compose apps skip this sequence. `docker compose up -d` recreates only the services that changed, and Tond then routes to the service that publishes the app's port.

## Failures

When a deploy fails, Tond classifies the failure into a likely cause with a hint, for example "The build ran out of memory" or "The health check at /health returned HTTP 500". It shows the log lines that point to the problem. The app's status doesn't change on a failed redeploy, because the old container is still serving. Only a failed first deploy, or a failed Docker Compose deploy, leaves the app `failed`.

## Deploy logs

Each deployment keeps its own log: clone and build output, health probes and cutover messages. Secrets are removed before anything is stored. Credentials in URLs, the app's own environment values and common token formats are replaced with `[redacted]`.

- Watch a deploy live on the **Deployments** tab, or with `tond apps deploy` (it follows by default).
- List past deployments with `tond apps deployments <app>`.
- `tond apps logs <app>` shows recent deploy log lines and container output.

By default Tond keeps the logs of each app's last 30 deployments, for up to 90 days, and the last 50 deployment records per app. The logs of the latest deployment are always kept. See [Retention](/operations/retention).

## Rollback

A rollback redeploys the exact image of an earlier successful deployment, through the same queue, health gate and cutover as any deploy. It doesn't build anything, so it doesn't wait for a build slot. It uses the app's **current** environment variables, volumes and limits, not the ones from back then.

- In the dashboard, pick a deployment on the **Deployments** tab.
- With the CLI, `tond apps rollback <app>` restores the most recent successful deployment whose image differs from the one running now.

```sh
tond apps rollback web
```

A deployment can be restored only if it succeeded and its image is still on the server. After each successful deploy or rollback, Tond prunes old images. It keeps the three most recent successful deployment images plus the running one. The dashboard marks older deployments as no longer available. Docker Compose apps can't be rolled back: change the compose file and deploy again.

> [!NOTE]
> A rollback restores code, not data. To undo a change to an app's volume data, restore a snapshot (see below).

## Pre-deploy snapshots

When an app has volumes, every deploy starts a snapshot of those volumes in the background, labelled `pre-deploy`. A failed snapshot never blocks the deploy. Tond keeps 10 pre-deploy snapshots per app by default. You can restore one to put the app's volume data back as it was before that deploy. See [Snapshots and backups](/concepts/backups).

## Permissions

| Action | Scope |
| --- | --- |
| Deploy, upload | `apps:deploy` |
| See deployments | `apps:read` |
| Read deploy logs | `logs:read` |
| Roll back | `apps:rollback` |

The **Deployer (CI)** token preset holds `apps:read`, `apps:deploy` and `logs:read`. See [CI with a deployer token](/guides/ci).

## Related

- [Apps](/concepts/apps)
- [Previews](/concepts/previews)
- [Snapshots and backups](/concepts/backups)
- [CLI reference](/reference/cli)
