# Build sources

An app's source decides what Tond builds or pulls on each deploy. This guide covers every source type, how Tond builds code without a Dockerfile, and how to check what it will do before you deploy. Deploying from a Git repository is covered in [Deploy from Git](/guides/deploy-from-git).

| Source type | What you give Tond | Where to set it up |
| --- | --- | --- |
| `git` | A repository URL and branch | Dashboard, CLI, API |
| `tarball` | A directory uploaded with `tond apps push` | CLI, API |
| `image` | A prebuilt image reference | Dashboard, CLI, API |
| `dockerfile` | A Dockerfile pasted into Tond | Dashboard, API |
| `compose` | A `docker-compose.yml` pasted into Tond (admins only) | Dashboard, API |

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

## Deploy a Docker Compose file

Choose **Docker → Docker Compose** and paste a `docker-compose.yml`. Only admins can create compose apps, because a compose file is a full container specification.

Tond routes HTTP to one port: a service must publish the app's port under `ports:`, for example `"3000"` or `"8080:3000"` for container port 3000. Set the same port in the wizard. The deploy fails if no service publishes it.

### What the compose policy rejects

The dashboard checks the file as you type and lists every problem at once. The server checks it again on create and on every deploy. It rejects:

- `privileged`, `devices`, `device_cgroup_rules`, `gpus` and device reservations
- host namespaces: `pid`, `ipc`, `uts`, `userns_mode` or `cgroup` set to `host`, and `network_mode: host`, `bridge`, `default` or `container:…`
- `cap_add` beyond Docker's default capability set, and `security_opt` values that disable confinement (for example `seccomp:unconfined`, `apparmor:unconfined`, `label:disable`)
- `cgroup_parent`, `volumes_from`, `include`, `extends.file` and `env_file` (put variables under `environment:`)
- bind mounts outside the app's volume directory, or of the whole directory
- published ports bound to an explicit IP such as `0.0.0.0:80:80`
- external, fixed-name or non-bridge networks
- `secrets` and `configs` that read host files, and build contexts or build options that read from the host
- `${…}` variable substitution in security-relevant fields such as volumes and ports

Before running `docker compose`, Tond rewrites its own copy of the file (the stored file stays as you wrote it):

- relative bind mounts such as `./data` point into the app's volume directory
- published ports bind to `127.0.0.1`, so they are reachable only through Tond's proxy
- every service gets `no-new-privileges`, a trimmed capability set, a process limit, a default memory limit and rotated logs

Compose apps have limits the other types don't: they can't be rolled back (update the file and deploy again), they have no previews, and they can't be linked to a managed database because they run on their own compose network.

## Next steps

- [Apps](/concepts/apps) describes readiness, health checks and the app lifecycle.
- [Deploys and rollback](/concepts/deploys) explains the build queue.
- [Templates](/guides/templates) start an app from a ready-made definition.
