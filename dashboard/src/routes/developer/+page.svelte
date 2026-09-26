<script lang="ts">
	import { OPENAPI_URL, BASE } from '$lib/api';
	import { filterReference, type ApiReference } from '$lib/api-reference';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import ApiOperationRow from '$lib/components/api/ApiOperationRow.svelte';
	import InlineCodeText from '$lib/components/api/InlineCodeText.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import DownloadIcon from '~icons/ri/download-2-line';
	import KeyIcon from '~icons/ri/key-2-line';
	import SearchIcon from '~icons/ri/search-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let reference = $derived(data.reference as ApiReference);
	let query = $state('');
	let tag = $state('');
	let groups = $derived(filterReference(reference.groups, query, tag || null));
	let shownCount = $derived(groups.reduce((n, g) => n + g.operations.length, 0));
	let origin = $derived(typeof window === 'undefined' ? '' : window.location.origin);

	const errorExample = `{ "success": false, "data": null,
  "error": { "code": "APP_NOT_FOUND",
             "message": "App not found: web",
             "hint": "List apps with \`tond apps list\`." } }`;

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-8">
	<PageHeader
		title="API"
		description="Automate Tond from scripts, CI and agents with the HTTP API or the tond CLI. Every endpoint, the permissions it needs and the errors it can return."
	>
		{#snippet actions()}
			<Button variant="outline" href="/profile?tab=tokens">
				<KeyIcon class="size-4" aria-hidden="true" />
				Create a token
			</Button>
			<Button variant="outline" href={OPENAPI_URL} download="tond-openapi.json">
				<DownloadIcon class="size-4" aria-hidden="true" />
				OpenAPI spec
			</Button>
		{/snippet}
	</PageHeader>

	<section aria-labelledby="start-heading" class="space-y-3">
		<h2 id="start-heading" class="text-lg font-bold">Getting started</h2>
		<div class="grid gap-3 md:grid-cols-2">
			<div class="min-w-0 space-y-2 rounded-md border p-4">
				<h3 class="text-sm font-semibold">Base URL and authentication</h3>
				<p class="text-sm text-muted-foreground">
					Every endpoint is under <code class="font-mono text-foreground">{BASE}</code>. Send an API
					token as a bearer token; each endpoint below lists the scopes the token needs.
				</p>
				<pre
					class="overflow-x-auto rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs break-all whitespace-pre-wrap">curl -H "Authorization: Bearer $TOND_TOKEN" {origin}{BASE}/apps</pre>
			</div>
			<div class="min-w-0 space-y-2 rounded-md border p-4">
				<h3 class="text-sm font-semibold">Errors</h3>
				<p class="text-sm text-muted-foreground">
					Failures carry a stable <code class="font-mono text-foreground">error.code</code> (listed
					below) and often a <code class="font-mono text-foreground">hint</code>. Switch on the
					code, not the message.
				</p>
				<pre
					class="overflow-x-auto rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs">{errorExample}</pre>
			</div>
			<div class="min-w-0 space-y-2 rounded-md border p-4">
				<h3 class="text-sm font-semibold">Safe retries</h3>
				<p class="text-sm text-muted-foreground">
					Send an <code class="font-mono text-foreground">Idempotency-Key</code> header with a deploy,
					create or other change. Repeating the request with the same key within 24 hours returns the
					first result instead of acting twice.
				</p>
			</div>
			<div class="min-w-0 space-y-2 rounded-md border p-4">
				<h3 class="text-sm font-semibold">The tond CLI</h3>
				<p class="text-sm text-muted-foreground">
					<code class="font-mono text-foreground">tond --json</code> prints machine-readable output,
					<code class="font-mono text-foreground">tond apps deploy --wait</code>
					streams the build and exits non-zero if it fails, and
					<code class="font-mono text-foreground">tond help exit-codes</code> lists what each exit code
					means.
				</p>
			</div>
		</div>
	</section>

	<section aria-labelledby="endpoints-heading" class="space-y-4">
		<div class="flex flex-col gap-1 sm:flex-row sm:items-baseline sm:justify-between">
			<h2 id="endpoints-heading" class="text-lg font-bold">Endpoints</h2>
			<p class="text-sm text-muted-foreground" aria-live="polite">
				{shownCount === reference.operationCount
					? `${reference.operationCount} endpoints`
					: `${shownCount} of ${reference.operationCount} endpoints`} · API version {reference.version}
			</p>
		</div>
		<div class="flex flex-col gap-2 sm:flex-row">
			<label class="relative flex-1">
				<span class="sr-only">Search endpoints</span>
				<SearchIcon
					class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
					aria-hidden="true"
				/>
				<Input
					type="search"
					placeholder="Search by path, summary or scope (e.g. deploy, db:read)"
					bind:value={query}
					class="pl-8"
				/>
			</label>
			<label class="sm:w-56">
				<span class="sr-only">Filter by area</span>
				<select
					bind:value={tag}
					class="h-9 w-full rounded-md border bg-background px-2 text-sm focus-visible:outline-2 focus-visible:outline-ring"
				>
					<option value="">All areas</option>
					{#each reference.groups as group (group.name)}
						<option value={group.name}>{group.name} ({group.operations.length})</option>
					{/each}
				</select>
			</label>
		</div>

		{#if groups.length === 0}
			<div class="rounded-md border border-dashed p-8 text-center text-sm text-muted-foreground">
				No endpoint matches “{query}”. Try a shorter word, or clear the area filter.
			</div>
		{/if}

		{#each groups as group (group.name)}
			<section aria-labelledby="tag-{group.name}" class="space-y-2">
				<div>
					<h3 id="tag-{group.name}" class="text-base font-semibold">{group.name}</h3>
					{#if group.description}
						<p class="text-sm text-muted-foreground"><InlineCodeText text={group.description} /></p>
					{/if}
				</div>
				<div class="rounded-md border">
					{#each group.operations as op (op.method + op.path)}
						<ApiOperationRow {op} {origin} />
					{/each}
				</div>
			</section>
		{/each}
	</section>

	{#if reference.errorCodes.length > 0}
		<section aria-labelledby="errors-heading" class="space-y-3">
			<div>
				<h2 id="errors-heading" class="text-lg font-bold">Error codes</h2>
				<p class="text-sm text-muted-foreground">
					Each code always comes with the same HTTP status and makes the CLI exit with the same
					code. New codes can be added; treat an unknown one by its HTTP status.
				</p>
			</div>
			<div class="overflow-x-auto rounded-md border">
				<table class="w-full text-left text-sm">
					<thead class="border-b bg-muted/40 text-xs text-muted-foreground">
						<tr>
							<th scope="col" class="px-3 py-2 font-medium">Code</th>
							<th scope="col" class="px-3 py-2 font-medium">HTTP</th>
							<th scope="col" class="px-3 py-2 font-medium">CLI exit</th>
							<th scope="col" class="hidden px-3 py-2 font-medium sm:table-cell">Meaning</th>
						</tr>
					</thead>
					<tbody>
						{#each reference.errorCodes as row (row.code)}
							<tr class="border-b align-top last:border-b-0">
								<td class="px-3 py-2">
									<span class="font-mono text-xs break-all">{row.code}</span>
									<span class="mt-1 block text-xs text-muted-foreground sm:hidden"
										><InlineCodeText text={row.description} /></span
									>
								</td>
								<td class="px-3 py-2 font-mono text-xs">{row.status}</td>
								<td class="px-3 py-2 font-mono text-xs">{row.exitCode}</td>
								<td class="hidden px-3 py-2 text-muted-foreground sm:table-cell"
									><InlineCodeText text={row.description} /></td
								>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</section>
	{/if}

	{#if reference.exitCodes.length > 0}
		<section aria-labelledby="exit-heading" class="space-y-3">
			<h2 id="exit-heading" class="text-lg font-bold">CLI exit codes</h2>
			<dl class="divide-y rounded-md border">
				{#each reference.exitCodes as row (row.exitCode)}
					<div class="flex gap-3 px-3 py-2 text-sm">
						<dt class="w-36 shrink-0 font-mono text-xs leading-5 sm:w-44">
							{row.exitCode} <span class="text-muted-foreground">{row.name}</span>
						</dt>
						<dd class="text-muted-foreground"><InlineCodeText text={row.meaning} /></dd>
					</div>
				{/each}
			</dl>
		</section>
	{/if}
</div>
