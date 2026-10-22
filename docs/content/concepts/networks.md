# Networks and database links

Tond puts every app, database, cron job and preview on a Docker network of its own choosing. What shares a network can talk; what doesn't, can't. This page explains which network each workload lands on, how apps reach each other and databases by name, and how a database link grants one app access to one database.

## No shared default network

Nothing Tond starts joins Docker's default `bridge` network, where every container can reach every other one. Instead, each workload has one **home network**:

| Workload | Home network | Who else is on it |
| --- | --- | --- |
| App in a project | `tond-<project id>` | All members of the project |
| App without a project | `tond-net-app-<app>` | The app and the databases linked to it |
| Database in a project | `tond-<project id>` | All members of the project |
| Database without a project | `tond-net-db-<database>` | Only the database (plus the networks of linked apps) |
| Cron job in a project | `tond-<project id>` | All members of the project |
| Cron job without a project | `tond-net-cron-<job id>` | Only the job |
| Preview | `tond-net-preview-<slug>` | Only the preview, or the other apps of the same project preview |

Docker keeps bridge networks apart, so a container reaches only containers on its own networks. Networks that hold a single database or cron job also turn off container-to-container traffic.

tond-proxy isn't on any of these networks. It reaches apps through the port each container publishes on `127.0.0.1`, so network isolation never affects routing.

## Internal hostnames

On a shared network, every container is reachable by its name:

| Container | Hostname | Port |
| --- | --- | --- |
| App | The app name, e.g. `api` | The app's container port |
| Database | The database name, e.g. `shop-db` | The engine's port: 5432, 3306, 6379 or 27017 |

So an app in the same project as the PostgreSQL database `shop-db` connects to `shop-db:5432`. The database's **internal connection string** already uses this hostname. See [Databases](/concepts/databases).

## Database links

A database link gives one app access to one database outside its project. When you link database `D` to app `A`, Tond attaches `D`'s container to `A`'s home network, with `D`'s name as its hostname there. `A` then reaches it as `D:<port>`.

- The database becomes multi-homed: it stays on its own network and joins the network of each linked app.
- Two apps linked to the same database still can't reach each other. They are on different networks.
- If the linked app is in a project, the database joins the project network, so every member of that project can reach it.

Manage links in either place:

- the app's **Settings → Databases**, which lists the databases the app can reach;
- the database's page, which lists the apps with access: project members and linked apps.

Linking and unlinking are idempotent and recorded in the [audit log](/concepts/audit-log). A link changes what both sides can reach, so it needs write access to both: `db:write` and `apps:write`. Linking doesn't set environment variables for you. Copy the database's internal connection string into the app's environment, then redeploy. See [Environment and secrets](/concepts/secrets).

Tond keeps networks in line with the links. When you link or unlink, it attaches or detaches the database container at once. After a deploy, which may recreate the app's network, it attaches the linked databases again.

## Changing projects

Moving an app into or out of a project moves its running container to the new home network and re-attaches its linked databases there. Because this changes which databases the app can reach, it needs `db:write` as well as `apps:write`. Deleting a project moves its members onto their own networks before the project network is removed. See [Projects](/concepts/projects).

## Previews

Previews run pull-request code, so they never join a project network or a linked database's network. A preview can't reach production apps or databases. Point a preview at a staging database with a preview override instead. See [Previews](/concepts/previews).

## Outbound connections

Networks decide which containers an app can reach on the server. What it can reach on the internet is its egress policy: open by default, an allowlist of hosts and addresses, or none. An app's own networks always stay reachable under any policy. Previews get no internet by default. See [Outbound connections](/security/isolation#outbound-connections).

## Docker Compose apps

A compose app runs on its own compose network (`<app>_default`), where its services reach each other by service name. Tond refuses compose files that join external networks or the default bridge. Compose apps can't be linked to managed databases.

## Cleanup

Deleting an app or database removes its own network. Every hour, the janitor also removes Tond-labelled networks that no container uses and that are more than an hour old.

## Related

- [Projects](/concepts/projects)
- [Databases](/concepts/databases)
- [Isolation](/security/isolation)
