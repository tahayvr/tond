# AGENTS.md — Tond Repository Guide

- project state: pre-production

Tond is a fast & lightweight self-hosted Platform as a Service (PaaS) for a single cheap VPS. It consists of a Rust workspace
(API server, reverse proxy, CLI, shared library) and a SvelteKit dashboard SPA.

---

Use `just check` for full project formatting, checks, and linting.
Use `just test` to run all Rust tests (including doc tests). See `justfile` for more targets.
Use `just audit` for the supply-chain gates (`cargo deny` per `deny.toml`, `pnpm audit --prod`).
Linux builds/bundles/VMs default to x86_64; add `arch=arm64` for ARM64, e.g. `just arch=arm64 vm-e2e`.

### Dev environment variables (`dashboard/.env.local`)

```sh
VITE_DEV_AUTH_BYPASS=true   # skip auth API check, use a hardcoded dev user
VITE_MOCK_API=true          # start MSW service worker to intercept /api requests
```

---

## Rust Code Style

### General

- **Error propagation**: use `anyhow::Result` and `?` for fallible functions.
  Reserve `unwrap()` only for values guaranteed at compile time (e.g., regex literals).
  Never use `unwrap()` on runtime I/O or database results.
- **Handler errors**: handlers return `Result<impl IntoResponse, AppError>`. `AppError`
  is a newtype over `TondError` defined in `crates/tond-server/src/error.rs`. Use `?`
  freely in handlers — `sqlx::Error` auto-converts via `From` impls.
- **New error variants**: add to `TondError` in `crates/tond-common/src/errors.rs`
  (derives `thiserror::Error`). Add a matching `From` impl in `error.rs` if needed.
- **Discarded results**: use `let _ = expr;` for intentionally discarded non-critical
  results (e.g., Docker cleanup on a best-effort basis).

### Naming

- `PascalCase` — structs, enums, traits.
- `snake_case` — functions, variables, modules, file names.
- `SCREAMING_SNAKE_CASE` — constants and statics.
- API response DTOs use a `Response` suffix (e.g., `AppResponse`, `ProjectResponse`).
- Axum extractor structs use a descriptive noun (`RequireAuth`, `RateLimit`).

### Module layout

- Each handler resource gets its own file in `src/handlers/` (one file per route group).

### Imports

- Group: `std` → external crates → internal crates (`tond-common`, sibling modules).
- Use `use` at the top of the file; avoid inline `crate::` references inside function
  bodies unless disambiguation is required.

### Async & database

- All async runtime: Tokio. Use `tokio::spawn` for background tasks.
- Database: SQLx with `sqlx::query_as` / `sqlx::query!` macros where possible.
- Migration files live in `crates/tond-server/migrations/` numbered `001_…`, `002_…`.

---

## Dashboard Code Style

### TypeScript

- `strict: true` — no implicit `any`. Caught exceptions may use `e: any` since
  `unknown` requires extra narrowing boilerplate.
- `moduleResolution: bundler` — use bare specifiers and the `$lib/…` path alias.
- `checkJs: true` — `.js` files in `src/` are also type-checked.
- Prefer `interface` over `type` for object shapes; use `type` for unions/primitives.
- Use the `cn()` helper from `$lib/utils` (wraps `clsx` + `tailwind-merge`) whenever
  conditionally combining Tailwind classes.

### Naming

- `PascalCase` — Svelte components, TypeScript interfaces/types.
- `camelCase` — functions, variables, reactive state variables.
- `handle` prefix — event handler functions (`handleSubmit`, `handleDelete`).
- File names: Svelte components `PascalCase.svelte`; utility modules `camelCase.ts`;
  SvelteKit route files follow framework convention (`+page.svelte`, `+layout.svelte`).

### Svelte 5 Runes — always use runes, never legacy APIs

- Reactive state: `let x = $state(value)`
- Derived values: `let y = $derived(expr)` or `$derived.by(() => { … })` for multi-line
- Side effects / lifecycle: `$effect(() => { …; return () => { /* cleanup */ }; })`
- Component props: `let { prop1, prop2 } = $props()`
- **Never** use `$:` reactive statements, `writable`/`readable` stores, `onMount`, or
  `onDestroy`.

### Module-level shared state (`.svelte.ts` files)

- Shared reactive state lives in `src/lib/*.svelte.ts` using `$state` at module scope.
- Expose state via getter functions (`getStatus()`, `getUiContext()`), never by
  exporting the `$state` variable directly.
- See `auth.svelte.ts` and `ui-context.svelte.ts` for the canonical pattern.

### UI Context system (`ui-context.svelte.ts`)

- `UiContext` is a discriminated union (`{ type: 'idle' } | { type: 'projects' } | …`).
- Pages call `setUiContext(…)` on mount and `clearUiContext()` in `$effect` cleanup.
- `CommandDeck` and `ActionButton` read context via `$derived(getUiContext())` and
  switch on `ctx.type` to render context-aware controls.
- To add a new context: add a union member to `UiContext`, add a case in `CommandDeck`
  and `ActionButton`, and call `setUiContext` from the relevant page.

### API layer

- All API calls go through the `request<T>()` helper in `src/lib/api.ts`.
- Each endpoint is a named async function exported from `api.ts`; never use `fetch`
  directly in component files.
- The base URL is `/api` (proxied to the Axum server in dev via `vite.config.ts`).
- Add new interfaces alongside new API functions in `api.ts` — no separate type files.

### Status, feedback and confirmations (Dashboard)

One system for all of these — never hand-roll a pill, banner or `confirm()`:

- **Status**: `<StatusBadge status={app.status} kind="app" />`
  (`$lib/components/feedback/status-badge.svelte`). Colour + icon + text from
  `describeStatus()` in `$lib/status.ts`; `kind` (`app`, `database`, `cron`, `run`,
  `deployment`, `certificate`, `preview`, `snapshot`) decides whether `running` means
  "up" or "in progress". Override with `tone`/`label` for non-API states. Add new
  status slugs to `$lib/status.ts`, not to call sites.
- **Errors**: `<InlineError message={error} ondismiss={() => (error = '')} />` next to
  the form or action that failed (`role="alert"`; renders nothing when empty; optional
  `title` and `actions` snippet). Error copy says what to do next.
- **Success**: `<SuccessAlert message={saved} ondismiss={…} timeout={4000} />`
  (`role="status"`, always mounted so screen readers announce it).
- **Pending + live sync**: wrap actions in `useAction()` from `$lib/action.svelte`:
  `const deploy = useAction(() => deployApp(id), { invalidate: true })`, then
  `onclick={() => deploy.run()} disabled={deploy.pending}` and a pending label
  ("Deploying…"). `run()` ignores calls while one is in flight (no double deploys),
  records `error`, and `invalidate: true` re-runs the page's `load` so lists never go
  stale. Per-row actions use `key` + `isPending(id)`.
- **Confirmations**: every destructive or disruptive action goes through
  `ConfirmDialog` (`$lib/components/dialogs/confirm-dialog.svelte`) or the imperative
  `await confirmAction({ title, description, details, confirmLabel, onconfirm })`
  (`$lib/confirm.svelte`, rendered by the `ConfirmHost` in the root layout). Use
  `typedConfirmation={name}` only for irreversible data loss (delete an app or
  database, restore a snapshot, roll back volumes); a simple confirm for stop, restart,
  rollback, revoke and deleting config (cron jobs, alerts, channels, providers,
  previews). `onconfirm` runs inside the dialog (pending label, inline error); make it
  throw on failure. `details` lists exactly what is removed or overwritten.
- Destructive actions never sit next to primary ones: put delete in a "Danger zone"
  section (e.g. the app's Settings → Danger zone), not the page header.
- Do not use global toast/notification stores for errors — keep error state local to
  the component.
- Tests: render `ConfirmHost` with `renderConfirmHost()` and drive dialogs with
  `confirmDialog(user, { typed })` from `src/test/confirm.ts`.

### Navigation, tabs and page structure (Dashboard)

- Navigation is always a real link (`<a href>` or `<Button href>`), never
  `onclick={() => goto(...)}`, so middle-click and "open in new tab" work. `goto` is for
  redirects after an action (e.g. after delete, with `{ invalidateAll: true }`).
- Every page sets a document title: `<PageHeader title=… section=…>` (renders the `<h1>`,
  meta/actions snippets and the title) or `<PageTitle title=… />` for pages with a
  custom header. Headings follow order (h1 → h2 → h3). User-provided names are never
  re-cased (no `uppercase`/`capitalize` on names, headings or labels).
- Tabs live in the URL (`?tab=`, plus `?section=` for the app's Settings): use
  `<LinkTabs>` with the definitions and helpers in `$lib/tabs.ts` (`APP_TABS`,
  `appTabHref`, `tabFromUrl`, `tabHref`) and render panels with
  `{#if tab === '…'}`. Load a tab's data in an `$effect` on the active tab, not in a
  click handler. Tests change the URL with `setPageUrl()` from
  `src/test/mocks/page-state.svelte.ts`.
- Colours: use the semantic tokens (`text-success`, `text-warn`, `text-destructive`,
  `text-info`, `text-muted-foreground`); never raw palette colours or opacity-dimmed
  text (`text-muted-foreground/60`). `src/test/contrast.test.ts` keeps every token at
  WCAG AA (text 4.5:1, focus ring 3:1) in light and dark.
- The global shell provides the Cmd/Ctrl+K palette (`$lib/command-palette.ts`), the `?`
  shortcut overlay and `g` chords (`$lib/dashboard-shortcuts.ts` — keep `SHORTCUTS` in
  sync), a navigation progress bar and a skip link.

### Styling

- Use **shadcn-svelte** components for all UI primitives.
  Add components: `pnpm dlx shadcn-svelte@latest add <component-name>`
- Icons: **Remix Icon** via `unplugin-icons` + `@iconify-json/ri`, imported per icon as
  `import SearchIcon from '~icons/ri/search-line';` (browse names at remixicon.com; prefer
  `-line` variants). Size with Tailwind classes (`class="size-4"`). `shadcn-svelte add`
  generates `@lucide/svelte` imports — replace them with the Remix equivalent.
- Tailwind CSS v4 (CSS-first config). Theme customisation goes in
  `src/routes/layout.css` inside `@theme { … }` — there is no `tailwind.config.*`.
- Use Tailwind utility classes directly on elements. No scoped `<style>` blocks or
  CSS-in-JS unless utilities are genuinely insufficient.

### Mock API (local dev)

- Handlers live in `src/mocks/handlers.ts`; fixtures in `src/mocks/fixtures.ts`.
- MSW is started in the `init` hook in `src/hooks.client.ts` (awaited before any `load`)
  only when `VITE_MOCK_API=true`.
- When adding a new API endpoint, add a corresponding MSW handler so the dashboard
  can be developed without a running backend.

---

## Key Technical Constraints

- Production target: a **single Linux VPS, x86_64 or aarch64 (ARM64)**. Both are built,
  packaged (`dist/tond-linux-<arch>`) and end-to-end tested. Keep resource usage minimal.
- `tond-proxy` runs as the unprivileged `tond-proxy` user (only `CAP_NET_BIND_SERVICE`);
  `tond-server` runs as root. The proxy reads `tond.db` through the `tond-proxy` group
  (`/opt/tond/data` is `root:tond-proxy 3770`, `tond.db*` `0640`) and opens it `mode=ro`:
  **the proxy never writes the database** — what it has to report (certificate results)
  goes to tond-server over its control socket `proxy.sock` (owned by tond-proxy, 0600,
  root/own-uid peers only; protocol in `tond_common::proxy_control`). tond-server keeps
  the WAL persistent (`SQLITE_FCNTL_PERSIST_WAL`) because a read-only reader cannot
  create `-wal`/`-shm`. Anything else under `/opt/tond/data` must stay root-only; never
  make the proxy depend on Docker or on files outside `tond.db*`, `certs/`,
  `acme_account.json` and `proxy.sock` (see `secure_data_dir` in `release/install.sh`).
- Previews run pull-request code and are served only as `<id>-<app>.<preview_domain>`, a
  separate registrable domain from the dashboard (`preview_domain` setting, validated with
  the Public Suffix List in `crates/tond-server/src/domains.rs`). tond-proxy never serves
  the dashboard/API on the preview domain. Without a preview domain, previews are refused.
- `tond-server` and `tond-proxy` share the same SQLite database file; avoid long
  transactions or WAL-mode changes that could block both processes.
- The dashboard is a **fully static SPA** (`ssr = false`, `prerender = false` in
  `+layout.ts`). There is no SSR — never add server-side load functions.
- **Never trust a request because it comes from 127.0.0.1.** `tond-proxy` forwards internet
  traffic to `tond-server` over loopback TCP, so a loopback peer can be anyone. The only
  credential-free path is the root-only Unix socket (`TOND_SOCKET_PATH`), marked by the
  `LocalSocket` request extension that only the socket listener adds (see
  `crates/tond-server/src/auth/extractor.rs`). Never derive trust from a header either.
- Routing is event-driven: SQL triggers bump `route_state.version` whenever anything the
  proxy routes on changes; tond-server's SQLite update hook sees that and nudges the
  proxy over `proxy.sock`, and a deploy stops the old container only after the proxy
  acknowledges the new version (`proxy_link::sync_routes`). The proxy keeps one
  persistent read-only connection and a 60 s fallback check. If you add a table or
  column the proxy routes on, extend the `route_*` triggers (baseline schema) in a new migration.
- Every production deploy goes through the build queue (`deploy/queue.rs`: one deploy
  per app, newest supersedes a queued one, global build slots). Never spawn
  `deploy::deploy` directly. Long-running work (deploys, cron runs, snapshots) is spawned
  with `runtime::shutdown::spawn_tracked` so SIGTERM waits for it; `reconcile.rs` fixes
  up whatever a crash left behind at startup.
- Idle budget: background loops sleep until their next deadline or a table change
  (`runtime::changes::Watch`), never on a short fixed tick; register them in
  `runtime::loops` so `/api/system/self` shows their health. `e2e/budget.sh` checks RSS,
  wakeups, DB queries and journal lines at idle.
- Spawn child processes with `crate::process::command(..)`, not `tokio::process::Command::new`:
  tond-server runs with `OOMScoreAdjust=-900`, and the helper resets children (git, docker CLI,
  tar) to a neutral OOM score so they can't hide behind Tond's protection.
