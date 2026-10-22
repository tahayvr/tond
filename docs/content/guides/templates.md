# Templates

A template is a ready-made app: a Docker Compose file with pinned images, plus a short manifest that says which service gets the domain, which values to ask you for, which secrets to generate and where the data lives. Deploying one creates a Docker Compose app and starts its first deploy in one step, with generated passwords and HTTPS. You need a running Tond server and an admin role, because every template is a compose app.

![The template catalog](/img/templates/templates.light.webp)

## The catalog

Tond ships 35 templates. They are part of the release: the files are built into the Tond binaries, so the catalog needs nothing on the server and matches the version you run. Tond doesn't load templates from anywhere else.

| Category | Templates |
| --- | --- |
| Analytics | Metabase (`metabase`), Plausible Analytics (`plausible`), Umami (`umami`) |
| Automation | n8n (`n8n`) |
| CMS | Directus (`directus`), Ghost (`ghost`) |
| Communication | Gotify (`gotify`), listmonk (`listmonk`), Mattermost (`mattermost`), ntfy (`ntfy`) |
| Developer | Appsmith (`appsmith`), code-server (`code-server`), Forgejo (`forgejo`), Gitea (`gitea`), IT Tools (`it-tools`), PocketBase (`pocketbase`) |
| Documents | Docmost (`docmost`), Paperless-ngx (`paperless-ngx`), Stirling-PDF (`stirling-pdf`), Wiki.js (`wikijs`) |
| Finance | Actual Budget (`actual`) |
| Media | Audiobookshelf (`audiobookshelf`), Immich (`immich`) |
| Monitoring | Grafana (`grafana`), Grafana + Prometheus (`prometheus-grafana`), Uptime Kuma (`uptime-kuma`) |
| Productivity | Excalidraw (`excalidraw`), FreshRSS (`freshrss`), linkding (`linkding`), Mealie (`mealie`), Memos (`memos`), Miniflux (`miniflux`), Twenty (`twenty`) |
| Security | Vaultwarden (`vaultwarden`) |
| Storage | Nextcloud (`nextcloud`) |

Each template lists its services and their images, the data it stores, a typical memory figure for all services together, its upstream version, setup notes and upgrade notes. See the full list with memory needs from the CLI:

```sh
tond templates list
tond templates show vaultwarden
```

The catalog is also served at `GET /api/v1/templates`, together with the server's free memory, its CPU architecture and the apps domain.

### Memory

Every template carries a memory estimate for light traffic. Templates that need 1 GB or less are marked **Fits on 2 GB**: they run comfortably next to Tond on a 2 GB server. When you deploy, Tond compares the estimate with the memory currently free. If the template needs more, the deploy is refused unless you choose to deploy anyway (**Deploy anyway** in the dashboard, `--allow-low-memory` in the CLI). An app short of memory may be slow, or be stopped by the kernel when memory runs out.

### CPU architecture

Most templates run on both x86_64 and ARM64 servers. A template whose images are published for one architecture only says so, and Tond refuses to deploy it on a server with the other CPU. Excalidraw and Mattermost are x86_64 only. The catalog marks them as not available on an ARM64 server, and `tond templates list` adds `(not available on ARM64)`.

## What a template deploy does

When you deploy a template, Tond:

1. checks the app name, the CPU architecture and the free memory;
2. fills in the template's inputs: values you entered, generated secrets for the ones you left empty, and fixed values such as the app's public URL;
3. creates a Docker Compose app from the template's compose file, with the inputs stored as encrypted environment variables;
4. creates the directories that hold the template's data inside the app's volume directory;
5. gives the template's public service the app's port and domain, or the automatic `<app>.<apps domain>` hostname if you set no domain;
6. queues the first deploy. For templates that pre-seed the admin account, that deploy creates it before the domain is routed (see [Admin accounts](#admin-accounts)).

If a step fails after the app was created, Tond removes the app again, so you can fix the problem and retry with the same name. Tond also refuses a name whose volume directory still holds data from an earlier app of that name, because that data wouldn't match the new passwords.

### Inputs

A template's inputs are the environment variables its compose file uses as `${KEY}`. There are three kinds:

- **Asked**: you fill them in, for example an admin email address. Required ones must be set. Email, URL and password fields are checked (a password must have at least 8 characters).
- **Generated**: left empty, Tond generates a random value: a 24-character password, 64 hex characters or 64 base64 characters. You can type your own instead.
- **Fixed**: computed by Tond and never asked, for example the app's public URL (`https://<domain>`), its hostname or its name.

Some inputs are credentials you need for the first login. Tond shows them once, right after the deploy. Save them in your password manager.

### Admin accounts

Many apps give the admin account to whoever opens them first, or ship a fixed default login. Once the app is on a public domain, anyone could get there before you. Tond handles this in one of two ways.

**Pre-seeded.** Tond creates the admin account, or replaces the default password, during the first deploy. It runs the template's setup command inside the app's container after every service is ready and before `tond-proxy` routes the domain, so nobody can reach the app first. The password is generated unless you type one, shown once after the deploy, and kept as an encrypted environment variable on the app.

| Template       | What Tond does                                                                                                             | Sign in as                       |
| -------------- | -------------------------------------------------------------------------------------------------------------------------- | -------------------------------- |
| Forgejo        | Locks the install page (`INSTALL_LOCK`) and creates the administrator with `forgejo admin user create`. Sign-up stays off. | the administrator name you chose |
| Gitea          | Locks the install page (`INSTALL_LOCK`) and creates the administrator with `gitea admin user create`. Sign-up stays off.    | the administrator name you chose |
| Mattermost     | Creates the system admin with `mmctl --local user create --system-admin`. Joining needs an invitation.                     | the username you chose           |
| Actual Budget  | Sets the server password through the server's bootstrap endpoint.                                                          | the server password              |
| Audiobookshelf | Creates the `root` user through the server's `/init` endpoint.                                                            | `root`                           |
| Umami          | Replaces the default `admin` / `umami` password in the database.                                                          | `admin`                          |
| Mealie         | Replaces the default `MyPassword` password through Mealie's API.                                                          | `changeme@example.com`           |

The deploy log shows each step. If a step still fails after its retries (about two minutes), the deploy finishes anyway and the app page shows the warning below with the reason. Then open the app right away and secure the admin account yourself.

**Warned.** Ghost, Uptime Kuma, Memos, Wiki.js, Docmost and Twenty have no documented way to create the admin from outside, so the first visitor still creates it. Their setup notes say so, and the app page shows a warning until you dismiss it:

> Open the app now and create the admin account; anyone who reaches it first can.

Open the app straight after the deploy, create the account, then click **I've created it, dismiss**. Dismissing needs the `apps:write` permission and only hides the warning. From the API, it is `POST /api/v1/apps/{id}/admin-setup/dismiss`. The app's `admin_setup` field says where it stands: `pending`, `done`, `failed`, `open` or `dismissed`.

## Deploy a template in the dashboard

1. Open **Templates** in the sidebar, or **Apps → New app** and click **Templates**.
2. Search, filter by category, or tick **Only ones that fit on 2 GB**, then pick a template. Its page shows the services, the stored data, the notes and, under **Show the Compose file**, the file Tond will deploy.
3. In the **Deploy** panel, set the **App name** (it starts as the template's ID), a **Domain** if you want one of your own, and a **Project** if you use projects.
4. Fill in the asked inputs. Open **secrets are generated for you** to see or override the generated ones.
5. Click the deploy button, for example **Deploy Vaultwarden**. The panel then shows the address, the one-time credentials (**Copy all** copies them) and the setup notes, with a link to watch the deploy.

Without the admin role the panel explains that an admin has to deploy the template. On a server with the wrong CPU it names the architectures the template supports instead of offering the button.

## Deploy a template with the CLI

Run on the server as root, or with `--server` and a token with `apps:write`, `apps:deploy` and `settings:write`:

```sh
tond apps create vault --template vaultwarden --domain vault.example.com
```

Pass inputs with `--env KEY=VALUE`, repeated. `tond templates show <id>` lists a template's inputs and whether each is required, optional or generated. The CLI prompts for each required input you didn't pass. Add `--yes` to fail instead of prompting, for scripts (`--json` never prompts either). An `--env` key the template doesn't have is refused.

The CLI sends one request to the server, prints the address and the one-time credentials, follows the first deploy and then prints the setup notes. It exits non-zero if the deploy fails.

`--template` can't be combined with `--source-type`, `--source`, `--branch` or `--build-method`, because the template decides those. `--domain` and `--allow-low-memory` apply. Resource limits, health checks and the port come from the template: change them on the app after it is created.

## After the deploy

- Point the domain's DNS at the server, or use the automatic hostname under your apps domain. See [Domains and certificates](/concepts/domains).
- Follow the setup notes and post-deploy checks. If the app page warns that the admin account is unclaimed, open the app and create it right away (see [Admin accounts](#admin-accounts)).
- The app's **Overview** says which template and version it was deployed from, with a link to the template's upgrade notes.

The app is an ordinary Docker Compose app from here on. Change its environment variables, compose file and service domains like any other (see [Docker Compose apps](/guides/sources#docker-compose-apps)).

### Upgrade to a newer template version

When a Tond release ships a newer version of the template, the app's **Overview** shows **Update available** with the old and new version and the new version's upgrade notes. Read them, then choose **Upgrade to …**.

![An app's overview with Update available, its upgrade notes and a reminder that a setting still uses the old address](/img/templates/update-available.light.webp)

Tond:

1. takes a snapshot of the app's data first, and changes nothing if the snapshot fails;
2. replaces the compose file with the new version's;
3. keeps every environment variable and generated password; settings the new version adds are generated or take their default, and new first-login credentials are shown once;
4. starts a deploy.

If the new version adds setup commands (`[[post_deploy]]`), that deploy runs only the new ones, and the app stays routed while they run. Tond remembers which commands an app has already run, so an admin account it created is never created again. If the admin came from a first visitor instead, Tond doesn't run the new commands at all, because the account may already exist; it records them as done.

If the new version misbehaves, roll back to the previous deployment on the **Deployments** tab: that restores the previous compose file and images (and the app shows the older template version again). Restore the snapshot too if the new version already changed the data.

Tond records the compose file it deployed from the template. If you edited the file since, the one-click upgrade is refused, because it would throw your edits away: apply the new version by hand with the upgrade notes, usually by changing an image tag and deploying, after taking a snapshot. Upgrading needs the admin scopes for compose files (`settings:write`) and `apps:deploy`. The API is `GET /api/v1/apps/{id}/template` (status) and `POST /api/v1/apps/{id}/template/upgrade`.

### Change the domain later

Some templates store the app's address in an environment variable, for example a site URL or a trusted origin. Tond remembers which ones. When you change the app's primary domain, it offers to update those values to the new address and redeploy, so links, logins and redirects follow. It only updates values that are still what the template set; a value you edited by hand is left alone. If you skip the offer, the **Overview** keeps a reminder with **Update and redeploy…**. The API is `POST /api/v1/apps/{id}/template/domain-env` with `{"redeploy": true}`.

## The template format

Each template is a directory `templates/<id>/` in the Tond source tree with two files: `compose.yml`, the compose file Tond deploys, and `template.toml`, the manifest. A trimmed example:

```toml
id = "example"
name = "Example"
description = "One line for the catalog"
category = "productivity"       # analytics, automation, cms, communication, developer,
                                # documents, finance, media, monitoring, productivity,
                                # security or storage
tags = ["notes"]
version = "1.4"                 # the upstream version the images are pinned to
ram_mb = 200                    # typical memory of all services together
architectures = ["amd64", "arm64"]   # the default; list one to restrict it
setup_notes = "Open the app and sign in with the admin email and password."
upgrade_notes = "Change the image tag and redeploy."
post_deploy_checks = ["Sign in as the admin"]

[public]                        # the service that gets the app's domain
service = "web"
port = 8080
health_path = "/health"         # optional: without it, readiness is a TCP check

[[inputs]]
key = "APP_URL"
label = "Public URL"
value = "{app_url}"             # fixed: also {app_host} and {app_name}

[[inputs]]
key = "ADMIN_EMAIL"
label = "Admin email"
kind = "email"                  # text, email, password or url

[[inputs]]
key = "ADMIN_PASSWORD"
label = "Admin password"
kind = "password"
generate = "password"           # password, hex or base64
credential = true               # shown once after the deploy

[[volumes]]
service = "web"
path = "./data"                 # a bind mount in compose.yml
description = "Notes and uploads"
owner = "1000:1000"             # optional: for images that don't run as root

# Optional: create the admin before the app is routed (once, on the first deploy)
[[post_deploy]]
service = "web"
user = "1000:1000"              # optional exec user
description = "Create the administrator account"
env = ["ADMIN_EMAIL", "ADMIN_PASSWORD"]   # inputs passed as environment variables
command = ["sh", "-c", "example-cli admin create --email \"$ADMIN_EMAIL\" --password \"$ADMIN_PASSWORD\""]
timeout_secs = 120              # retried every 3 seconds until it succeeds (at most 600)
```

A `[[post_deploy]]` command runs with `docker exec` in the named service's container. It gets the inputs listed in `env` as environment variables, never on its command line, and its output is logged with those values hidden. Make it safe to run twice: a retry may follow an attempt that worked but didn't report it. A template whose app hands the admin account to its first visitor and can't be pre-seeded sets `first_visitor_admin = true` instead, which puts the warning on the app page.

Apart from these commands, templates are declarative. Tond validates every template against these rules, and its tests check each shipped one:

- every image is pinned to a version tag or digest, never `latest` or another moving tag, and nothing is built;
- every service sets `restart` and `mem_limit`;
- only the public service publishes a port, and only as `"<port>"`, so Tond picks the loopback host port;
- data lives in `./…` bind mounts declared under `[[volumes]]`, so snapshots include it;
- every `${VAR}` in the compose file is an input, and every input is used by the compose file or a `[[post_deploy]]` command;
- a `[[post_deploy]]` command names a compose service, a non-empty command and only inputs in `env`.

The compose file also goes through the same [compose policy](/guides/sources#what-the-compose-policy-rejects) as any compose app you paste.

## Next steps

- [Docker Compose apps](/guides/sources#docker-compose-apps) explains services, service domains and compose deploys.
- [Snapshots and backups](/concepts/backups) covers backing up a template app's data.
