# Build sources

An app's source decides what Tond builds or pulls on each deploy. This guide covers every source type, how Tond builds code without a Dockerfile, and how to check what it will do before you deploy. Deploying from a Git repository is covered in [Deploy from Git](/guides/deploy-from-git).

| Source type | What you give Tond | Where to set it up |
| --- | --- | --- |
| `git` | A repository URL and branch | Dashboard, CLI, API |
| `tarball` | A directory uploaded with `tond apps push` | CLI, API |
| `image` | A prebuilt image reference | Dashboard, CLI, API |
| `dockerfile` | A Dockerfile pasted into Tond | Dashboard, API |
| `compose` | A `docker-compose.yml` pasted into Tond or read from a Git repository (admins only) | Dashboard, API, [templates](/guides/templates) |

## Building code: TBuild or Dockerfile

Git and tarball sources are built on the server. Each app has a build method:

- **TBuild** (the default) scans the code and generates a multi-stage Dockerfile for it. It recognises Node.js, Python, Go, Rust and static HTML projects.
- **Dockerfile** builds the `Dockerfile` at the build root. The deploy fails if there is none.

The build root is the repository root, or the **Base Directory** you set on the wizard's **Build** step (useful for monorepos). Turn on **Static Build?** when the build output is a static site that should be served as files.

TBuild writes its generated Dockerfile over any `Dockerfile` at the build root, so choose **Dockerfile** if you maintain your own. After a TBuild deploy, open the deployment in the app's **Deployments** tab to see the **Generated Dockerfile**.

From the CLI, pass `--build-method tbuild` or `--build-method dockerfile` to `tond apps create`.

### Check detection before you deploy

`tond scanner explain` runs the same detection on a local checkout and prints the candidates, the chosen stack, the builder and runtime images, the run command and the reasons:

```sh
tond scanner explain .
tond scanner explain . --base-dir apps/web
tond scanner explain . --static-build
```

It needs no server connection, so you can run it on your own machine if you have the `tond` binary there.

## Upload a directory (tarball)

Use this when the code isn't in a Git repository Tond can reach, or when you want to deploy exactly what is on your disk.

1. Create the app once:

   ```sh
   tond apps create site --source-type tarball --port 8080
   ```

2. From the project directory, upload and deploy:

   ```sh
   cd ~/code/site
   tond apps push site
   ```

`tond apps push` packs the current directory into a gzipped tar and uploads it. It leaves out `.git`, `node_modules`, `target`, `.svelte-kit`, `.next`, `__pycache__`, `.venv`, `dist`, `build` and `.DS_Store` at the top level, plus every path listed in a `.tondignore` file (one path per line, relative to the directory, `#` for comments). If you deploy prebuilt output from `dist` or `build`, build it on the server instead or rename the directory.

The server extracts the archive in a private directory and refuses absolute paths, `..`, links that point outside the upload, device files and more than 100,000 entries. Setuid bits are stripped. The unpacked size is capped by **Settings → Upload limit (MiB)**, 1 GiB by default.

`tond apps push` returns once the build is queued. Follow it in the dashboard or with `tond apps deployments site`.

Pushing to an app switches its source type to `tarball`. A later `tond apps deploy site` or dashboard redeploy runs the last uploaded build again; it does not fetch anything new.

To push from a CI pipeline, see [CI with a deployer token](/guides/ci).

## Deploy a prebuilt image

1. Open **Apps → New app**, choose **Docker**, then **Docker Image**.
2. Enter the image, for example `nginx:1.27` or `registry.example.com/team/api:2.3.1`.
3. Set the port the container listens on in the **Network** step.

From the CLI:

```sh
tond apps create api --source-type image --source ghcr.io/you/api:2.3.1 --port 8080
tond apps deploy api
```

Each deploy pulls the image and pins the pulled bytes under the deployment's own tag, so a rollback runs exactly the image that was deployed, even if the tag has moved since. To ship a new version, change the tag (**Settings → Git → Source**) or push a new image under the same tag and redeploy.

### Private registries

Store the registry login once and attach it to apps:

1. Open **Integrations → Registry Credentials** and add a credential: **Name**, **Registry Server** (for example `ghcr.io`), **Username** and **Password / Token**. The password is encrypted at rest and never shown again.
2. In the new-app wizard, pick it under **Registry Credentials** on the **Environment** step. For an existing app, use **Settings → Git → Registry Credential**.

Use a read-only token from the registry where it offers one.

## Paste a Dockerfile

Choose **Docker → Dockerfile** in the wizard and paste the Dockerfile. Tond builds it in an empty directory: there is no repository, so `COPY` and `ADD` have nothing local to copy. This fits images assembled from public base images and downloads. For your own code, use a Git or tarball source with the **Dockerfile** build method.

## Docker Compose apps

A Docker Compose app runs several services together from one compose file: for example a web app, its database and a cache. Tond builds or pulls every service's image, starts the set with `docker compose`, and routes domains to the services you choose. Only admins can create compose apps or edit their files (`settings:write`), because a compose file is a full container specification.

### Create a compose app

1. Open **Apps → New app**, choose **Docker**, then **Docker Compose**.
2. Choose where the file comes from:
   - **Paste a file**: paste your `docker-compose.yml`. Tond checks it as you type, lists every problem at once and shows the services it found.
   - **From a Git repository**: enter the **Repository URL**, the **Branch** and the **Compose file path** (`docker-compose.yml` by default, relative to the repository root). Tond clones the branch on every deploy and reads the file from it, so the file is checked at deploy time.
3. Pick the **Web service**: the service that gets the app's port and domain. If you leave the choice to Tond, it takes the service that declares the app's port under `ports:` or `expose:`, else the first service that declares any port.
4. On the **Network** step, set the port the web service listens on inside its container.

A service only has to declare its port, for example `expose: ["80"]`: Tond publishes it. If no service declares a port, the app runs with internal services only and has no domain until you publish one on the **Services** tab.

You can also create a compose app with the API (`POST /api/v1/apps` with `source_type: "compose"` and either `compose_content`, or a `source_url` with `compose_path`), or from a [template](/guides/templates).

### Services

The app's **Services** tab lists every service with its state (and health, when the service defines a `healthcheck`), its image or **Built from source**, the pinned image it runs, its ports, CPU and memory use, and its domains. For each service you can:

- read its recent **Logs**, with the app's secrets masked;
- **Restart**, **Stop** or **Start** it on its own (stopping asks first);
- open **Domains & port** to publish a container port and route domains to it.

With the CLI:

```sh
tond apps services shop
tond apps service shop worker restart
tond apps service shop worker logs
```

Stop, start and restart on the app itself act on every service. The app's combined log tags each line with its service, for example `[cache]`. The in-browser terminal opens in the web service.

A service that leaves the compose file leaves the list and takes its domains with it. Services in a `profiles:` group, or scaled to zero replicas, aren't started.

### Service domains

Each compose service can have its own domains. In **Domains & port**, choose the container port Tond publishes for the service (one the file declares, or another) and add domains, up to 20 per service. Choose **Internal only** to publish nothing: the service is then reachable only from the app's other services, by its service name.

- Domains you add to a service that already publishes that port are routed at once, and get a certificate like any app domain.
- A new or changed published port is bound on the next deploy. The service shows **(after deploy)** until then, and the tab offers **Deploy now**.

Tond binds the published port on `127.0.0.1` only, on a host port it allocates and keeps across deploys. On the first deploy, the web service gets the automatic `<app>.<apps domain>` hostname if the app has no domain yet. See [Domains and certificates](/concepts/domains#compose-service-domains) for the rules service domains share with other app domains.

With the API, use `PUT /api/v1/apps/{id}/services/{service}` with `published_port` and `domains`.

### Edit the compose file

Open **Settings → Compose file** to edit the pasted file, or to switch between a pasted file and a Git repository. **Save** stores the change for the next deploy, and **Save and deploy** also starts one. Tond keeps your file exactly as you wrote it. The API equivalent is `PUT /api/v1/apps/{id}/compose`.

### Variables in the compose file

`${VAR}` in the compose file is filled from the app's own environment variables when it deploys, so secrets can stay out of the file: set `DB_PASSWORD` under **Settings → Environment** and write `POSTGRES_PASSWORD: ${DB_PASSWORD}`. `${VAR:-default}` uses the default when the variable is unset or empty. Only the variables the file references are passed to `docker compose`: never Tond's own environment, and never names that change how Docker runs, such as `DOCKER_*`, `COMPOSE_*`, `LD_*`, `PATH` or `TOND_*`. Substitution isn't allowed in security-relevant fields such as volumes and ports (see [below](#what-the-compose-policy-rejects)).

The app's environment variables don't reach the containers on their own: list what each service needs under its `environment:`.

### How a compose app deploys

Compose apps go through the same [build queue](/concepts/deploys#the-build-queue) as other apps, in the phases Clone (repository-based apps only), Build, Start, Health and Release:

1. **Build.** Tond checks and hardens the file, then produces each service's image. It builds services that have a `build:` section, in the same memory-capped build slice as other builds, and pulls the others, with the app's registry credential if it has one. Each image is pinned as `tond-<app>/<service>:<image ID prefix>`.
2. **Start.** Tond writes its hardened copy of the file with the pinned images and published ports, and runs `docker compose up -d --remove-orphans`. Compose recreates only the services whose image or configuration changed. An unchanged service keeps its image tag, so its container keeps running.
3. **Health.** Tond waits until services with a `healthcheck` are healthy and every service runs (a one-shot service may exit with code 0). It then probes each published service like a single-container app, with the app's health check path.
4. **Release.** Tond records the deployment's compose file and the image of every service, and waits until tond-proxy routes the service domains.

The deployment page lists each service with its image and status. Service domains stay routed throughout the deploy, and unchanged services keep answering. A service that compose recreates is replaced in place, not started next to the old one, so it can be unavailable for a moment. See [Deploys and rollback](/concepts/deploys#docker-compose-apps) for rollback and failed compose deploys.

### What the compose policy rejects

The dashboard checks the file as you type and lists every problem at once. The server checks it again on create, on every save and on every deploy. It rejects:

- `privileged`, `devices`, `device_cgroup_rules`, `gpus` and device reservations
- host namespaces: `pid`, `ipc`, `uts`, `userns_mode` or `cgroup` set to `host`, and `network_mode: host`, `bridge`, `default` or `container:…`
- `cap_add` beyond Docker's default capability set, and `security_opt` values that disable confinement (for example `seccomp:unconfined`, `apparmor:unconfined`, `label:disable`)
- `cgroup_parent`, `volumes_from`, `include`, `extends.file` and `env_file` (put variables under `environment:`)
- bind mounts outside the app's volume directory, or of the whole directory
- named volumes that are `external`, have a fixed `name` or use a driver other than `local`
- published ports bound to an explicit IP such as `0.0.0.0:80:80`
- external, fixed-name or non-bridge networks
- `secrets` and `configs` that read host files, and build contexts or build options that read from the host
- `${…}` variable substitution in security-relevant fields such as volumes and ports

Before running `docker compose`, Tond rewrites its own copy of the file (the stored file stays as you wrote it):

- relative bind mounts such as `./data` point into the app's volume directory
- published ports bind to `127.0.0.1`, so they are reachable only through Tond's proxy
- every service gets `no-new-privileges`, a trimmed capability set, a process limit, a default memory limit and rotated logs

Bind mounts and named volumes are both included in [snapshots](/concepts/backups). Deleting a compose app removes its containers, its named volumes and its images.

### Backing up a database service

Volumes are copied while the services run, which is fine for uploads but not for a database's files. Give a database service a `tond.backup.dump` label: a command Tond runs inside the service before each snapshot copies the volumes, whose output is saved in the snapshot. An optional `tond.backup.restore` label loads that dump back after a restore, reading it on standard input:

```yaml
services:
  db:
    image: postgres:17-alpine
    environment:
      POSTGRES_USER: app
      POSTGRES_DB: app
      POSTGRES_PASSWORD: ${DB_PASSWORD}
    volumes:
      - ./postgres:/var/lib/postgresql/data
    labels:
      tond.backup.dump: pg_dump -U "$$POSTGRES_USER" -d "$$POSTGRES_DB" --clean --if-exists
      tond.backup.restore: psql -U "$$POSTGRES_USER" -d "$$POSTGRES_DB" -q -v ON_ERROR_STOP=1
```

`$$` keeps compose from substituting the variable, so the shell in the container reads it from the service's environment. Each label is one line, and a restore needs a dump. See [Templates](/guides/templates#backup-hooks) for MySQL and Redis examples and [Snapshots and backups](/concepts/backups) for when the hooks run.

### Limits

- Compose apps have no previews, and can't be deployed from an upload (`tond apps push`).
- They can't be linked to a managed database, because they run on their own compose network. Run the database as a service in the file instead.
- They can't be background workers: publish a web service port, or leave every service internal.

## Next steps

- [Apps](/concepts/apps) describes readiness, health checks and the app lifecycle.
- [Deploys and rollback](/concepts/deploys) explains the build queue.
- [Templates](/guides/templates) start an app from a ready-made definition.
