# Projects

A project groups related apps, databases and cron jobs, for example a frontend, an API and the PostgreSQL database behind them. Projects matter for two reasons: members share a private Docker network, so they can reach each other by name, and you can deploy or preview the whole group at once. Projects are optional. An app that isn't in a project runs on its own network.

## What a project holds

A project has a unique **name**, an optional **description** and optional **shared environment variables** that every member app and cron job gets, under its own (a member's variable with the same name wins; see [Project variables](/concepts/secrets#project-variables)). It can contain:

- **apps**: each app belongs to at most one project.
- **managed databases**: you choose the project when you create the database.
- **cron jobs**: you choose the project when you create the job.

The **Projects** page in the dashboard lists every project with its member apps and active preview environments. The CLI works with projects by name or ID:

```sh
tond projects create shop --description "Storefront and API"
tond projects assign web --project shop
tond projects info shop
```

To take an app out of its project, run `tond projects assign` without `--project`.

## The project network

Every project has one Docker bridge network, `tond-<project-id>`. All member apps, databases and cron jobs join it, and each is reachable there by its name:

- an app named `api` answers at `api:<container port>`;
- a database named `shop-db` answers at `shop-db:<engine port>`, for example `shop-db:5432` for PostgreSQL.

Containers outside the project can't reach the project network. Docker isolates bridge networks from each other.

Moving an app into or out of a project moves its container to the new network at once, and that changes which databases it can reach. So it needs the `db:write` scope as well as `apps:write`. Previews never join the project network: they run pull-request code and must not reach production databases.

To give an app outside the project access to one database, link them instead. See [Networks and database links](/concepts/networks).

## Deploy a whole project

**Deploy** on a project, or `tond projects deploy`, queues a deploy for every member app:

```sh
tond projects deploy shop
```

Each app gets its own deployment in the [build queue](/concepts/deploys). Builds follow the global concurrency limit, so with the default of one build slot the apps build one after another. Health checks and traffic cutover don't wait for a build slot. Each app's deploy succeeds or fails on its own; one failure doesn't roll back the others.

## Project previews

A project preview starts one [preview deployment](/concepts/previews) for every member app, under a shared slug, so the previews belong together:

```text
abc123-frontend.<preview domain>
abc123-api.<preview domain>
```

Create one from the project page (**Preview Environments**) or with the CLI:

```sh
tond projects preview create shop --ttl 48
tond projects preview list shop
tond projects preview delete shop <preview-id>
```

The time to live is 1 to 168 hours. The default is 24. Project previews need a preview domain, like any other preview. The preview apps of one project preview share their own preview network, so they can reach each other by app name. They can't reach the production project network or its databases. Compose apps can't be previewed.

Deleting a project preview stops every child container and marks its previews expired. Tond also cleans up a project preview once all its child previews have expired.

## Delete a project

Deleting a project doesn't delete its members:

- member apps and databases stay, lose their project and move to their own networks;
- cron jobs are detached and run on their own networks;
- the project network is removed.

Nothing is stopped. Deleting a project needs the `apps:delete` scope.

```sh
tond projects delete shop
```

## Permissions

| Action | Scope |
| --- | --- |
| See projects | `apps:read` |
| Create or edit a project | `apps:write` |
| Move an app between projects | `apps:write` and `db:write` |
| Deploy all apps in a project | `apps:deploy` |
| Create or delete project previews | `previews:write` |
| Delete a project | `apps:delete` |

API tokens can be limited to the apps of one or more projects. See [Users, roles and tokens](/concepts/access).

## Related

- [Apps](/concepts/apps)
- [Networks and database links](/concepts/networks)
- [Previews](/concepts/previews)
- [CLI reference](/reference/cli)
