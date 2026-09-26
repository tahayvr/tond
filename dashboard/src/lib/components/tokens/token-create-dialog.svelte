<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import {
		createToken,
		getTokenCatalog,
		listApps,
		listProjects,
		type ApiToken,
		type App,
		type Project,
		type Scope,
		type TokenCatalog,
		type TokenKind
	} from '$lib/api';
	import { TOKEN_KINDS, defaultExpiry, expiryOptions } from '$lib/access';
	import {
		canTick,
		matchingPreset,
		tokenFormProblem,
		tokenInput,
		type TokenFormState
	} from '$lib/token-form';
	import { cn } from '$lib/utils';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import CopyIcon from '~icons/ri/file-copy-line';
	import CheckIcon from '~icons/ri/check-line';
	import AlertIcon from '~icons/ri/error-warning-line';

	interface Props {
		open: boolean;
		onCreated: (token: ApiToken) => void;
	}

	let { open = $bindable(false), onCreated }: Props = $props();

	let catalog = $state<TokenCatalog | null>(null);
	let apps = $state<App[]>([]);
	let projects = $state<Project[]>([]);
	let loadError = $state('');

	const blank = (): TokenFormState => ({
		name: '',
		kind: 'personal',
		preset: 'readonly',
		scopes: [],
		reach: 'all',
		appIds: [],
		projectIds: [],
		allowedIps: '',
		expiry: defaultExpiry('personal')
	});

	let form = $state<TokenFormState>(blank());
	let showScopes = $state(false);
	let error = $state('');
	let creating = $state(false);
	let secret = $state('');
	let created = $state<ApiToken | null>(null);
	let copied = $state(false);

	async function loadCatalog() {
		loadError = '';
		try {
			const [c, a, p] = await Promise.all([getTokenCatalog(), listApps(), listProjects()]);
			catalog = c;
			apps = a;
			projects = p;
			applyPreset(form.preset);
		} catch (e: any) {
			loadError = e.message;
		}
	}

	$effect(() => {
		if (open && !catalog) void loadCatalog();
	});

	function reset() {
		form = blank();
		if (catalog) applyPreset('readonly');
		showScopes = false;
		error = '';
		secret = '';
		created = null;
		copied = false;
	}

	function applyPreset(id: string) {
		form.preset = id;
		const preset = catalog?.presets.find((p) => p.id === id);
		if (preset && catalog) {
			form.scopes = preset.scopes.filter((s) => canTick(s, form.kind, catalog!));
		}
	}

	function setKind(kind: TokenKind) {
		form.kind = kind;
		form.expiry = defaultExpiry(kind);
		if (catalog) form.scopes = form.scopes.filter((s) => canTick(s, kind, catalog!));
	}

	function toggleScope(scope: Scope, on: boolean) {
		form.scopes = on ? [...form.scopes, scope] : form.scopes.filter((s) => s !== scope);
		if (catalog) form.preset = matchingPreset(form.scopes, catalog);
	}

	function toggleId(list: 'appIds' | 'projectIds', id: string, on: boolean) {
		form[list] = on ? [...form[list], id] : form[list].filter((x) => x !== id);
	}

	const problem = $derived(catalog ? tokenFormProblem(form, catalog) : 'Loading…');

	async function handleSubmit(e: Event) {
		e.preventDefault();
		if (!catalog) return;
		error = problem ?? '';
		if (problem) return;
		creating = true;
		try {
			const result = await createToken(tokenInput(form));
			secret = result.secret;
			created = result.token;
			onCreated(result.token);
		} catch (err: any) {
			error = err.message;
		} finally {
			creating = false;
		}
	}

	async function handleCopy() {
		await navigator.clipboard.writeText(secret);
		copied = true;
		setTimeout(() => (copied = false), 2000);
	}

	const origin = typeof window === 'undefined' ? 'https://your-server' : window.location.origin;
</script>

<Dialog.Root
	bind:open
	onOpenChange={(isOpen) => {
		if (!isOpen) reset();
	}}
>
	<Dialog.Content class="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
		{#if secret && created}
			<Dialog.Header>
				<Dialog.Title>Token created: {created.name}</Dialog.Title>
				<Dialog.Description>
					Copy it into your CI secrets or password manager now.
				</Dialog.Description>
			</Dialog.Header>
			<div
				class="flex items-start gap-2 rounded-md border border-warn/40 bg-warn/10 p-3 text-sm"
				role="alert"
			>
				<AlertIcon class="mt-0.5 size-4 shrink-0 text-warn" />
				<p>
					<strong>Store it now.</strong> This is the only time Tond shows the token. If you lose it, revoke
					it and create a new one.
				</p>
			</div>
			<div class="flex items-center gap-2">
				<code
					class="min-w-0 flex-1 rounded-md border border-border bg-muted/40 px-3 py-2 font-mono text-xs break-all select-all"
					data-testid="token-secret">{secret}</code
				>
				<Button variant="outline" size="sm" onclick={handleCopy} aria-label="Copy token">
					{#if copied}<CheckIcon class="size-4" /> Copied{:else}<CopyIcon class="size-4" /> Copy{/if}
				</Button>
			</div>
			<div class="space-y-1 text-xs text-muted-foreground">
				<p>Use it as a bearer token:</p>
				<pre
					class="rounded-md border border-border bg-muted/40 px-3 py-2 font-mono break-all whitespace-pre-wrap">curl -H "Authorization: Bearer $TOND_TOKEN" {origin}/api/apps</pre>
			</div>
			<Dialog.Footer>
				<Button onclick={() => (open = false)}>I've stored it</Button>
			</Dialog.Footer>
		{:else}
			<Dialog.Header>
				<Dialog.Title>Create API token</Dialog.Title>
				<Dialog.Description>
					Tokens act on your behalf with the access you choose, never more than your own.
				</Dialog.Description>
			</Dialog.Header>

			{#if loadError}
				<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
					Couldn't load the scope list: {loadError}
					<Button variant="link" size="sm" onclick={loadCatalog}>Try again</Button>
				</div>
			{:else if !catalog}
				<p class="py-8 text-center text-sm text-muted-foreground">Loading…</p>
			{:else}
				<form class="space-y-5" onsubmit={handleSubmit} novalidate>
					<div class="grid gap-1.5">
						<Label for="token-name">Name</Label>
						<Input
							id="token-name"
							bind:value={form.name}
							placeholder="e.g. github-actions-web"
							maxlength={64}
							autocomplete="off"
						/>
					</div>

					<fieldset class="grid gap-1.5">
						<legend class="mb-1.5 text-sm font-medium">Used by</legend>
						<div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
							{#each TOKEN_KINDS as k (k.id)}
								<button
									type="button"
									aria-pressed={form.kind === k.id}
									onclick={() => setKind(k.id)}
									class={cn(
										'flex flex-col items-start justify-start rounded-md border p-2 text-left text-sm transition-colors',
										form.kind === k.id
											? 'border-primary bg-primary/5'
											: 'border-border hover:bg-muted/50'
									)}
								>
									<span class="font-medium">{k.label}</span>
									<span class="block text-xs text-muted-foreground">{k.hint}</span>
								</button>
							{/each}
						</div>
					</fieldset>

					<fieldset class="grid gap-1.5">
						<legend class="mb-1.5 text-sm font-medium">Access</legend>
						<div class="grid gap-2 sm:grid-cols-3">
							{#each catalog.presets as p (p.id)}
								<button
									type="button"
									aria-pressed={form.preset === p.id}
									onclick={() => applyPreset(p.id)}
									class={cn(
										'flex flex-col items-start justify-start rounded-md border p-2 text-left text-sm transition-colors',
										form.preset === p.id
											? 'border-primary bg-primary/5'
											: 'border-border hover:bg-muted/50'
									)}
								>
									<span class="font-medium">{p.label}</span>
									<span class="block text-xs text-muted-foreground">{p.description}</span>
								</button>
							{/each}
						</div>
						<div class="flex items-center justify-between text-sm">
							<span class="text-muted-foreground">
								{form.scopes.length} scope{form.scopes.length === 1 ? '' : 's'}
								{form.preset === 'custom' ? '(custom)' : ''}
							</span>
							<Button
								type="button"
								variant="link"
								size="sm"
								onclick={() => (showScopes = !showScopes)}
								aria-expanded={showScopes}
							>
								{showScopes ? 'Hide scopes' : 'Customize scopes'}
							</Button>
						</div>
						{#if showScopes}
							<div class="grid gap-4 rounded-md border border-border p-3 sm:grid-cols-2">
								{#each catalog.groups as g (g.id)}
									<div class="space-y-2">
										<p class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
											{g.label}
											{#if g.id === 'admin'}<span class="font-normal normal-case">
													(never for agents)</span
												>{/if}
										</p>
										{#each g.scopes as s (s.id)}
											{@const allowed = canTick(s.id, form.kind, catalog)}
											<label
												class={cn(
													'flex items-start gap-2 text-sm',
													!allowed && 'cursor-not-allowed opacity-50'
												)}
												title={allowed ? '' : "Your access doesn't include this"}
											>
												<Checkbox
													checked={form.scopes.includes(s.id)}
													disabled={!allowed}
													onCheckedChange={(v) => toggleScope(s.id, v === true)}
													class="mt-0.5"
												/>
												<span>
													<code class="font-mono text-xs">{s.id}</code>
													<span class="block text-xs text-muted-foreground">{s.description}</span>
												</span>
											</label>
										{/each}
									</div>
								{/each}
							</div>
						{/if}
					</fieldset>

					<fieldset class="grid gap-2">
						<legend class="mb-1 text-sm font-medium">Apps</legend>
						<div class="flex flex-wrap gap-4 text-sm">
							<label class="flex items-center gap-2">
								<input type="radio" bind:group={form.reach} value="all" />
								All apps and projects
							</label>
							<label class="flex items-center gap-2">
								<input type="radio" bind:group={form.reach} value="some" />
								Only some
							</label>
						</div>
						{#if form.reach === 'some'}
							<div
								class="grid max-h-48 gap-3 overflow-y-auto rounded-md border border-border p-3 sm:grid-cols-2"
							>
								<div class="space-y-1.5">
									<p class="text-xs font-semibold text-muted-foreground uppercase">Apps</p>
									{#each apps as a (a.id)}
										<label class="flex items-center gap-2 text-sm">
											<Checkbox
												checked={form.appIds.includes(a.id)}
												onCheckedChange={(v) => toggleId('appIds', a.id, v === true)}
											/>
											{a.name}
										</label>
									{:else}
										<p class="text-xs text-muted-foreground">No apps yet.</p>
									{/each}
								</div>
								<div class="space-y-1.5">
									<p class="text-xs font-semibold text-muted-foreground uppercase">
										Projects (all their apps)
									</p>
									{#each projects as p (p.id)}
										<label class="flex items-center gap-2 text-sm">
											<Checkbox
												checked={form.projectIds.includes(p.id)}
												onCheckedChange={(v) => toggleId('projectIds', p.id, v === true)}
											/>
											{p.name}
										</label>
									{:else}
										<p class="text-xs text-muted-foreground">No projects yet.</p>
									{/each}
								</div>
							</div>
							<p class="text-xs text-muted-foreground">
								A limited token can only reach these apps; everything else is refused.
							</p>
						{/if}
					</fieldset>

					<div class="grid gap-4 sm:grid-cols-2">
						<div class="grid gap-1.5">
							<Label for="token-ips"
								>Allowed IPs <span class="text-muted-foreground">(optional)</span></Label
							>
							<Textarea
								id="token-ips"
								bind:value={form.allowedIps}
								rows={2}
								placeholder="203.0.113.4&#10;10.0.0.0/8"
								class="font-mono text-xs"
							/>
							<p class="text-xs text-muted-foreground">Leave empty to allow any address.</p>
						</div>
						<div class="grid content-start gap-1.5">
							<Label for="token-expiry">Expires</Label>
							<Select.Root type="single" bind:value={form.expiry}>
								<Select.Trigger id="token-expiry" class="w-full">
									{expiryOptions(form.kind).find((o) => o.id === form.expiry)?.label ?? 'Choose…'}
								</Select.Trigger>
								<Select.Content>
									{#each expiryOptions(form.kind) as o (o.id)}
										<Select.Item value={o.id}>{o.label}</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
							{#if form.kind === 'agent'}
								<p class="text-xs text-muted-foreground">Agent tokens expire within 30 days.</p>
							{:else if form.expiry === 'never'}
								<p class="text-xs text-warn">
									A token that never expires keeps working until you revoke it.
								</p>
							{/if}
						</div>
					</div>

					<InlineError message={error} />

					<Dialog.Footer>
						<Button type="button" variant="outline" onclick={() => (open = false)}>Cancel</Button>
						<Button type="submit" disabled={creating}>
							{creating ? 'Creating…' : 'Create token'}
						</Button>
					</Dialog.Footer>
				</form>
			{/if}
		{/if}
	</Dialog.Content>
</Dialog.Root>
