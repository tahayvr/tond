# Deploy from Git

This guide connects an app to a Git repository and makes it deploy when you push. You need a running Tond server with a dashboard domain, and a repository on GitHub, GitLab, Gitea, Forgejo or any other Git host.

Tond clones the repository, builds it (see [Build sources](/guides/sources)) and runs the result. There are three ways to give Tond access:

| Repository | How Tond reads it | How pushes reach Tond |
| --- | --- | --- |
| Public, any host | Plain HTTPS clone | A per-app webhook |
| Private on GitHub | The Tond GitHub App | The GitHub App (no per-repo webhook) |
| Private, any host | An SSH deploy key (or a GitLab access token) | A per-app webhook |

## Deploy a public repository

1. Open **Apps → New app** and enter an **Application Name**.
2. Under **Source**, choose **Git**, then **Public Repo**, and paste the **Git Repository URL** (`https://…` or `git@…`).
3. On the **Build** step, pick the **Branch**. Tond lists the repository's branches and preselects its default branch. Choose a **Build Method** (TBuild or Dockerfile) and, for a monorepo, a **Base Directory**.
4. Turn on **Auto Deploy** if pushes should deploy the app. You still need a webhook (see Deploy on push below).
5. Finish the wizard and click **Deploy Application**. The first deploy starts right away.

From the CLI, on the server:

```sh
tond apps create web --source https://github.com/you/web.git --branch main --port 3000
tond apps deploy web
```

`tond apps deploy` follows the build and exits non-zero if the deploy fails.

## Private GitHub repositories: the GitHub App

The GitHub App gives Tond short-lived access tokens for the repositories you choose, delivers push and pull request events, and posts preview status back to pull requests. You create the App from the dashboard; it belongs to your GitHub account and to this Tond server.

Before you start, set a **Dashboard Domain** under **Settings** and make sure it serves HTTPS. GitHub sends its callbacks and webhooks there.

1. Open **Integrations → GitHub** and click **Create GitHub App**. You must be signed in as an admin in this browser.
2. GitHub opens with a prefilled App named `tond-<your-dashboard-domain>`. Confirm it. The App asks for read access to contents and metadata, write access to pull requests and commit statuses, and subscribes to `push` and `pull_request` events.
3. GitHub sends you back to Tond. The App now shows **Step 2 of 2 — Install on repositories**. Click **Install on Repos** and choose the repositories Tond may deploy.
4. Back in Tond, the App card shows its repositories.

The setup link is single-use and expires after an hour. If it fails, start again from the same page.

To deploy a repository through the App:

1. Open **Apps → New app**, choose **Git**, then **Private Repo (GitHub App)**.
2. Pick the **GitHub App**, the **Repository** and the **Branch**.
3. Continue as for a public repository.

To switch an existing app to the App, open the app's **Settings → Git**, use **Link a GitHub App (optional)** to pick the App, repository and branch, and save. The branch you pick there becomes the watch branch; the branch Tond builds stays the one the app was created with.

When the App receives a push, Tond deploys every app linked to that App whose Git URL matches the pushed repository, subject to **Auto-deploy** and the **Watch Branch**. No per-repository webhook is needed.

> [!TIP]
> Disconnecting a GitHub App in Tond removes it from Tond only. Delete it on GitHub as well if you no longer need it.

### Pull request previews

With the GitHub App linked, an app can get a preview deployment for every pull request. Tond posts a `tond/preview` commit status (pending, then success with the preview URL) and a comment on the pull request with the link. Previews need a separate preview domain and must be turned on per app under **Settings → Previews**. See [Previews](/concepts/previews).

## Private repositories on any host: SSH deploy key

1. Create the app with the SSH form of the URL, for example `git@github.com:you/web.git` or `git@git.example.com:team/web.git`.
2. Open the app's **Settings → Git**, find **SSH Deploy Key** and click **Generate Key**. From the CLI: `tond apps git-key web --generate`.
3. Copy the Ed25519 public key and add it to the repository as a read-only deploy key.
4. Deploy.

Tond keeps the private key on the server, root-only, and uses it only for this app. Host keys of the large providers are pinned; other hosts are trusted on first connection and then verified. Regenerating the key replaces it, so update the deploy key in your Git host too.

## Private GitLab repositories: access token

For GitLab (gitlab.com or self-hosted), Tond can clone with a personal access token instead of a deploy key. This is available through the API only:

1. Store the token (admin, `settings:write`). Tond checks it against GitLab before saving and encrypts it at rest.

   ```sh
   curl -X POST https://tond.example.com/api/v1/gitlab-tokens \
     -H "Authorization: Bearer $TOND_TOKEN" -H "Content-Type: application/json" \
     -d '{"name":"gitlab","token":"glpat-…","gitlab_url":"https://gitlab.com"}'
   ```

2. Link it to the app with `PUT /api/v1/apps/{id}/gitlab-token` and the body `{"gitlab_token_id":"<id>"}`.

An app linked to a GitLab token also gets merge request previews from the GitLab webhook (below). See the [HTTP API](/reference/api/) for the full request shapes.

## Deploy on push

Two settings decide whether a push deploys:

- **Auto-deploy** must be on. Verified pushes to an app with auto-deploy off are accepted and ignored.
- **Watch Branch** filters pushes. If it is set, only pushes to that branch deploy. If it is empty, a push to any branch deploys.

Both live under the app's **Settings → Git**. The new-app wizard sets the watch branch to the branch you deploy. From the CLI:

```sh
tond apps git-webhook web --enable --branch main
```

> [!WARNING]
> The branch Tond builds is the one you chose when you created the app. The watch branch only filters which pushes trigger a deploy. Keep the two the same.

### Add a webhook (without the GitHub App)

Every app has its own webhook URLs and a secret. Open **Settings → Git → Webhooks** and pick the tab for your provider: **GitHub**, **GitLab**, **Gitea / Forgejo** or **CI / Generic**. The tab shows the URL, the secret (click to reveal) and the exact steps for that provider:

| Provider | Where | What to send |
| --- | --- | --- |
| GitHub | Repository → Settings → Webhooks | Content type `application/json`, the secret, "Just the push event" |
| GitLab | Project → Settings → Webhooks | The secret as **Secret token**, **Push events** (and **Merge request events** for previews) |
| Gitea / Forgejo | Repository → Settings → Webhooks | Content type `application/json`, the secret, push events |

The CLI prints the same URLs:

```sh
tond apps git-webhook web --show-secret
```

Webhook URLs contain the app's ID, never its name. They are built from the dashboard domain, so set that first.

### How Tond treats webhooks

- Every request must be signed with the app's secret (GitHub and Gitea HMAC signatures, GitLab's token header). Unsigned or wrongly signed requests get `401`. Verification can't be turned off.
- Each delivery is accepted once. A repeated delivery ID, including your provider's "Redeliver" button, is rejected with `409`. Push a new commit or deploy from the dashboard instead.
- Webhook deploys are rate-limited per app: a burst of 10, then one every 30 seconds. Pushes in quick succession collapse in the build queue: the newest replaces a deploy that hasn't started yet. See [Deploys and rollback](/concepts/deploys).
- Rotate the secret with **Rotate secret** (or `tond apps git-webhook web --rotate-secret`). The old secret stops working at once. You can also set your own secret of at least 16 characters with `--secret`.

To trigger deploys from a CI pipeline instead of a Git host, use the generic webhook or an API token. See [CI with a deployer token](/guides/ci).

## Next steps

- [Build sources](/guides/sources) explains TBuild, Dockerfiles and the other source types.
- [Environment and secrets](/concepts/secrets) covers configuration for the app.
- [Domains and certificates](/concepts/domains) gives the app its own domain.
