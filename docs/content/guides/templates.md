# Templates

A template is a ready-made app definition: where the code or image comes from, which port it listens on, the volumes and environment variables it needs, and the databases it depends on. Creating an app from a template sets all of that up in one step. You need a running Tond server and, for templates with databases, permission to create databases and read their credentials.

![The New app wizard with the Templates source selected](/img/templates/templates.light.webp)

## What a template creates

When you create an app from a template, Tond:

1. creates the template's project, if it declares one;
2. creates each database the template depends on, inside that project;
3. fills environment variables that point at those databases, such as `DATABASE_URL`, with the new database's connection string;
4. asks you for the remaining required variables;
5. creates the app with the template's source, port, health check path and build method, and deploys it.

In the dashboard, if any step fails, Tond deletes what it had already created, so you can fix the problem and try again.

The catalog is served at `GET /api/v1/templates`. It currently includes **Plausible Analytics** (`plausible`): the Plausible community edition image, a PostgreSQL database named `plausible-db` in a project named `plausible`, and a volume for `/var/lib/plausible`.

## Create an app from a template in the dashboard

1. Open **Apps → New app**.
2. Under **Source**, choose **Template** and pick one. The wizard fills in the name (the template's ID), the source, the port, the health check path, the volumes and, if the template suggests one, a domain. Change any of them.
3. The **Selected Template** panel lists the **Dependencies** Tond will create and the **Required Template Inputs**. Fill in every input. For Plausible these are `BASE_URL` (the public URL of the instance) and `SECRET_KEY_BASE` (a long random secret).
4. Step through the wizard. Git-based templates show the **Build** step; image templates skip it.
5. Click **Deploy Application**. The button shows each stage (creating the project, each database, the app, the first deploy) and then opens the live deploy view.

## Create an app from a template with the CLI

Run on the server as root, or with `--server` and a token that may create apps, projects and databases and read database credentials (`db:credentials`):

```sh
tond apps create analytics --template plausible \
  --env BASE_URL=https://stats.example.com \
  --env SECRET_KEY_BASE="$(openssl rand -hex 48)"
```

The CLI creates the project and databases, fills the database variables, and prompts for any required variable you didn't pass with `--env`. Add `--yes` to fail instead of prompting, for scripts. Afterwards it prints the template's setup notes and post-deploy checks.

`--template` can't be combined with `--source-type`, `--source`, `--branch` or `--build-method`; the template decides those. You can still pass `--domain`, `--memory-mb`, `--cpus` and the health check options.

The CLI doesn't create the template's volumes. Add them before the first deploy, then deploy:

```sh
tond apps volume add analytics /var/lib/plausible
tond apps deploy analytics
```

## After the first deploy

- Point the domain's DNS at the server, or use a hostname under your apps domain. See [Domains and certificates](/concepts/domains).
- Work through the template's post-deploy checks. For Plausible: open `/`, then add your site inside Plausible after the first login.
- The generated database password is shown only once, at creation. Reveal it later from the database page or with `tond db credentials plausible-db`.

The app is an ordinary app from here on. Change its environment, volumes and resources like any other. See [Apps](/concepts/apps).

## The template format

Templates are TOML files in the `templates/` directory of the Tond source tree, one file per template. They are declarative: no scripts or hooks run. A trimmed example:

```toml
id = "example-app"
name = "Example App"
description = "Example self-hosted application"
category = "productivity"
tags = ["example"]
recommended_domains = ["app.example.com"]
setup_notes = "Set the required environment variables before the first deploy."
post_deploy_checks = ["Open /health and verify it returns 200"]

[project]
name = "example"

[source]
kind = "git"                    # git or image
url = "https://github.com/example/app.git"
ref = "refs/tags/v1.0.0"        # branch or tag for git, tag for images

[deploy]
method = "tbuild"               # tbuild, dockerfile or image
default_port = 8080
health_check_path = "/health"

[[deploy.volumes]]
container_path = "/var/lib/example-app"
description = "Persistent application data"

[[required_env]]
key = "APP_URL"
description = "Public base URL for the application"
required = true

[[required_env]]
key = "DATABASE_URL"
description = "PostgreSQL connection string"
required = true
value = "${db.example-db.internal_connection_string}"

[[dependencies]]
name = "example-db"
kind = "database"
engine = "postgres"
required = true
```

A required variable with a `value` is filled in by Tond. The only placeholders are `${db.<name>.internal_connection_string}` (how apps with access reach the database over its network, which is what an app should use) and `${db.<name>.connection_string}` (the address from the server itself). A variable without a `value` becomes a required input. See [Networks and database links](/concepts/networks) for how the app reaches the database.

## Next steps

- [Databases](/concepts/databases) explains the managed databases a template creates.
- [Build sources](/guides/sources) covers the build methods a template can use.
