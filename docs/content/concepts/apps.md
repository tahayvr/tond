# Apps

An app is something Tond builds, runs and routes traffic to: a web site, an API, a TCP service or a background worker. Most apps run as one Docker container on your server. A Docker Compose app runs several containers, its services, from one compose file. This page explains what an app is made of, how Tond decides it is ready, and what happens over its lifetime.

## What an app is

Every app has:

- a **name**: letters, digits and hyphens, unique on the server. Tond uses it for the container (`tond-<app>`), the image (`tond-<app>:<tag>`), the Docker network alias and the automatic hostname.
- a **source**: where the code or image comes from (see [Sources](#sources)).
- a **kind**: how Tond decides the app is ready and whether it serves traffic (see [App kinds](#app-kinds)).
- a **container port**: the port the app listens on inside its container. The default is `3000`. Tond also passes it to the container as the `PORT` environment variable.
- optional settings: environment variables, a domain, health checks, volumes, resource limits and preview settings.

You create apps from **Apps → New app** in the dashboard, from a [template](/guides/templates), or with the CLI:

```sh
tond apps create web --source https://github.com/acme/web.git --branch main --port 8080
```

An app does nothing until its first deploy. See [Deploys and rollback](/concepts/deploys).

## App kinds

The kind is called *readiness* in the API and CLI. You pick it when you create the app and can change it later in the app's settings. A change takes effect on the next deploy.

| Kind (dashboard) | Readiness | Ready when | Domain and route |
| --- | --- | --- | --- |
| Web app | `http` | The health check path answers 2xx, or, without one, the port answers any HTTP response | Yes |
| TCP service | `tcp` | The container port accepts TCP connections | Yes |
| Background worker | `none` | The container stays up for `ready_after_secs` without exiting or restarting | No |

Web apps are the default.

A background worker serves no traffic. It gets no domain, no route and no health check path. It waits `ready_after_secs` seconds before Tond treats it as ready: the default is 10, and you can set 1 to 600. If you switch an existing app to a worker, Tond removes its domain and health check path. Docker Compose apps can't be background workers.

## Sources

| Source | Where it comes from | Build |
| --- | --- | --- |
| Git (`git`) | A repository and branch. Public repos work directly. Private repos use a GitHub App, a GitLab token or an SSH deploy key. | TBuild or Dockerfile |
| Docker image (`image`) | An image reference from any registry, optionally with a registry credential | Pulled, not built |
| Dockerfile (`dockerfile`) | A Dockerfile you paste into Tond, without Git | `docker build` |
| Docker Compose (`compose`) | A compose file you paste into Tond, or one read from a repository | Each service built or pulled, then `docker compose up` |
| Upload (`tarball`) | An archive of a directory, sent with `tond apps push` | TBuild or Dockerfile |

For Git and uploaded sources you choose a **build method**:

- **TBuild** (the default) scans the code, generates a Dockerfile and builds it. The generated Dockerfile is saved with the deployment so you can read it.
- **Dockerfile** builds the `Dockerfile` in the repository root, or in the base directory if you set one.

A **base directory** makes a subdirectory the build root, which is useful for monorepos. It must be a relative path inside the checkout.

To deploy the current directory as an upload:

```sh
tond apps push web
```

Uploads are limited to 1 GiB by default. An admin can change the limit in **Settings**.

Compose apps and apps with volumes need an admin, because both reach the host. Tond checks every compose file before it stores or deploys it. It refuses host-level options such as `privileged`, host binds outside the app's volume directory and the Docker socket. It publishes ports on `127.0.0.1` only and adds the same hardening as any other container.

A compose app's container port is the port of its **web service**, the service that gets the app's domain. Other services can get domains of their own or stay internal, and you can restart, stop, start and read the logs of each one on the app's **Services** tab. Apps created from a [template](/guides/templates) are compose apps. See [Docker Compose apps](/guides/sources#docker-compose-apps).

For step-by-step setup, see [Deploy from Git](/guides/deploy-from-git) and [Build sources](/guides/sources).

## Health checks

Two separate checks watch a web app:

1. **At deploy time**, Tond probes the new container directly before it moves traffic (see [App kinds](#app-kinds)). If the container isn't ready in time, the deploy fails and the old container keeps serving.
2. **While it runs**, if you set a health check path, Tond sends `GET <path>` to the app on a schedule. Each request has a 5 second timeout. A 2xx answer resets the failure count. When the count reaches the threshold, Tond marks the app `unhealthy`, restarts its container and sets it back to `running`.

| Setting | Default |
| --- | --- |
| Health check path | None (runtime checks off) |
| Interval | 30 seconds |
| Failure threshold | 3 consecutive failures |

Configure them in the app's **Settings → Health**, or with the CLI:

```sh
tond apps health web --path /health --interval 30 --threshold 3
tond apps health web --disable
```

For a Docker Compose app, the deploy-time check probes every published service by the app's kind and health check path, and also waits for services with a compose `healthcheck` to be healthy. Runtime health checks don't run for compose apps: the service states and compose health are shown on the **Services** tab.

Health checks only test the app from the server itself. To test the public URL, including DNS and TLS, use [uptime checks](/concepts/observability).

## Lifecycle

| Action | What it does | Scope |
| --- | --- | --- |
| Deploy | Builds or pulls the source and replaces the running container without downtime | `apps:deploy` |
| Stop | Stops the container. The app shows `stopped` and is no longer routed. | `apps:control` |
| Start | Starts the stopped container again | `apps:control` |
| Restart | Stops and starts the same container. It doesn't rebuild or pick up new settings. | `apps:control` |
| Delete | Removes the container, the app's own network and the app record | `apps:delete` |

```sh
tond apps stop web
tond apps start web
tond apps restart web
tond apps delete web
```

Stop, start and restart act on the existing container. Changes to environment variables, the port, volumes, readiness and resource limits only apply when a new container is created, so deploy again to apply them. On a Docker Compose app they act on every service; the **Services** tab and `tond apps service` act on one.

Delete is in the app's **Settings → Danger zone**. Deleting an app doesn't delete its volume data on disk (see [Volumes](/concepts/volumes)). Deleting a compose app also removes its named Docker volumes and its images.

Containers use Docker's `unless-stopped` restart policy, so Docker restarts a crashed container on its own. Tond records restarts, crashes and out-of-memory kills as [events](/concepts/observability).

## Resource limits

| Limit | Default | How to set it |
| --- | --- | --- |
| Memory | No explicit limit. A default cap of 75% of the server's RAM (at least 256 MiB) applies. | **Settings → Resources**, or `--memory-mb` on `tond apps create` |
| CPU | No limit: the app may use every core | **Settings → Resources**, or `--cpus` on `tond apps create` (e.g. `0.5`) |

The default memory cap stops one runaway container from taking down Tond, the proxy or SSH. A container that goes over its memory limit is killed by the kernel. Tond records this as an OOM event, and you can alert on it.

Every container Tond starts also gets:

- `no-new-privileges`, and Docker's default capabilities minus `NET_RAW`, `MKNOD`, `AUDIT_WRITE` and `SETFCAP`.
- a limit of 4096 processes and threads.
- rotated logs: three files of 10 MiB each.
- a port published on `127.0.0.1` only. The proxy reaches the app there; the internet can't reach it directly.

## Status

| Status | Meaning |
| --- | --- |
| `pending` | Created, never deployed |
| `building` | A deploy is in progress and nothing is serving yet: a first deploy, or a deploy of a stopped or failed app |
| `running` | The container is up and routed (unless it is a worker). It stays `running` while a new version deploys. |
| `stopped` | Stopped by you |
| `failed` | A deploy failed and there is no working container to fall back on |
| `unhealthy` | Runtime health checks failed and Tond is restarting the container, or a compose deploy failed after some services were already replaced |

A running app keeps its status during a deploy and stays routed: the old container serves every request until the new one passes its health gate and traffic moves over. The dashboard shows **Deploying a new version** next to the status, with a link to the live deploy. The app's status changes only at the end, if at all.

When a deploy fails, the app falls back to what it was doing before:

- A running or unhealthy app keeps its status, because the old container is still serving. The exception is a compose app whose failed deploy had already recreated some services: it becomes `unhealthy`, and its domains stay routed.
- A stopped app stays `stopped`.
- An app with nothing to fall back on, such as a failed first deploy, becomes `failed`.

## Related

- [Deploys and rollback](/concepts/deploys)
- [Domains and certificates](/concepts/domains)
- [Environment and secrets](/concepts/secrets)
- [Networks and database links](/concepts/networks)
- [CLI reference](/reference/cli)
