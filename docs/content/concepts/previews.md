# Previews

A preview is a short-lived copy of an app, built from its source and served at its own URL, usually for a pull request. Previews let you and your reviewers try a change before it reaches production. Because they run code that hasn't been reviewed yet, Tond keeps them apart from your production apps, data and dashboard.

## The preview domain

Previews are served only under a **preview domain**, one hostname per preview:

```text
<slug>-<app>.<preview domain>
```

The slug is 12 random hexadecimal characters, for example `3f9a1c0b7d2e-web.example-previews.com`.

Tond refuses to create previews until a preview domain is set. The domain must meet these rules:

- it is a domain you own, with a wildcard DNS record `*.<preview domain>` pointing at your server;
- it is not the dashboard domain, a parent of it or a subdomain of it;
- it is a **different registrable domain** (site) from the dashboard and from the apps domain.

Tond works out the registrable domain with the Public Suffix List. For `dash.example.co.uk` the site is `example.co.uk`, so `previews.example.co.uk` is refused but `example-previews.co.uk` is accepted.

### Why a separate site

Browsers treat pages on the same site as related. A page on the dashboard's site can set cookies for the parent domain, sends `SameSite` cookies along with requests to the dashboard API, and passes same-site CSRF checks. Pull-request code can come from people who otherwise have no access to your server. On a separate site it gets none of that. For the same reason, the preview domain must not share a site with your production apps: a preview could otherwise set cookies for them. tond-proxy reserves the preview domain for previews and never serves the dashboard, the API or apps under it.

Set the preview domain in **Settings**, or on the server:

```sh
sudo tond settings set-preview-domain example-previews.com
```

Each preview hostname gets its own Let's Encrypt certificate, like any other domain. See [Domains and certificates](/concepts/domains).

## Creating previews

### By hand

![An app's Previews settings](/img/previews/previews.light.webp)

Create a preview from the app's **Settings → Previews**, or with the CLI:

```sh
tond apps previews create web --ttl 48
tond apps previews list web
tond apps previews delete web <preview-id>
```

The time to live is 1 to 168 hours (7 days). The default is 24. To preview every app of a project together, see [project previews](/concepts/projects).

### From pull requests

Turn on previews in the app's **Settings → Previews** to create them automatically:

| Provider | Event | What Tond does |
| --- | --- | --- |
| GitHub (through a GitHub App) | Pull request opened, synchronized or reopened | Replaces any preview of that pull request with a new one, posts a commit status and comments the preview URL on the pull request |
| GitHub (through a GitHub App) | Pull request closed | Expires the pull request's previews |
| GitLab (the app's GitLab webhook) | Merge request opened, updated or reopened | Replaces any preview of that merge request with a new one and posts a commit status when the app uses a GitLab token |
| GitLab | Merge request closed or merged | Expires the merge request's previews |

Pull-request previews live for 24 hours. A new push to the pull request replaces the preview, so the timer starts again.

Webhook-created previews are limited per app. Each app may have at most 10 live previews. Webhook-triggered deploys and previews share a per-app budget of 10 at once, refilled at one every 30 seconds.

See [Deploy from Git](/guides/deploy-from-git) to connect a GitHub App or GitLab.

## What a preview gets

A preview is built from the same source and build settings as the app. It then runs in isolation:

| | Production app | Preview |
| --- | --- | --- |
| Environment | All app variables | Only the variables you share, plus preview overrides |
| Volumes | `<data>/volumes/<app>/…` | Its own empty directories under `<data>/volumes/_preview/<slug>/` |
| Network | Its project network or its own network | Its own preview network: no project members, no databases |
| Internet | The app's [egress policy](/security/isolation#outbound-connections) (open by default) | **None** by default: DNS and its own preview network only |
| Memory and CPU | App limits | The same limits as the app |
| Builds | Build queue | Shares the same global build slots |

**Environment.** Previews don't inherit the app's environment: production variables such as database URLs and API keys are withheld. In the app's **Settings → Environment** you can:

- share chosen app variables with previews, for example `NODE_ENV`;
- add **preview overrides**: values that only preview containers see, such as a staging database URL.

Preview overrides are encrypted like any other secret. Reading their values needs the `env:read-values` scope.

**Internet.** Pull-request code gets no outbound connections by default, so it can't send out what it reads or misuse the server's address. If previews need an outside service, such as a staging database or an API, list it in the app's **Settings → Network** allowlist and set **Previews** to **Allowlist only**, or **Open**. A preview whose policy can't be loaded into the firewall doesn't start. See [Outbound connections](/security/isolation#outbound-connections).

Docker Compose apps can't be previewed.

## Expiry and cleanup

A preview has one of these statuses:

| Status | Meaning |
| --- | --- |
| `building` | The image is being built or pulled |
| `running` | The container is up and routed |
| `failed` | The build or start failed |
| `expired` | The preview was deleted, its pull request closed, or its time ran out |

When a preview's time runs out, a background task stops and removes its container and marks it `expired`. Deleting a preview does the same at once. Later, the janitor removes the preview's volume directories. It deletes expired and failed preview records a week after they expired.

## Metrics

Each preview has CPU, memory and request metrics, like an app, in the app's **Settings → Previews**. See [Metrics, uptime and alerts](/concepts/observability).

## Permissions

| Action | Scope |
| --- | --- |
| See previews | `apps:read` |
| Create or delete previews | `previews:write` |
| Turn automatic previews on or off | `apps:write` |
| Change which variables previews get | `env:write` |

## Related

- [Domains and certificates](/concepts/domains)
- [Projects](/concepts/projects)
- [Environment and secrets](/concepts/secrets)
- [CLI reference](/reference/cli)
