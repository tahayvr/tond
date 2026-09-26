<script lang="ts">
	import {
		getWebhookConfig,
		revealWebhookSecret,
		rotateWebhookSecret,
		setWebhookSecret,
		type WebhookConfig,
		type WebhookUrls
	} from '$lib/api';
	import { isAdmin } from '$lib/auth.svelte';
	import RevealableSecret from '$lib/components/secrets/revealable-secret.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { SecretInput } from '$lib/components/ui/secret-input/index.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import Check from '~icons/ri/check-line';
	import Copy from '~icons/ri/file-copy-line';
	import Refresh from '~icons/ri/refresh-line';
	import ShieldCheck from '~icons/ri/shield-check-line';

	interface Props {
		appId: string;
	}

	let { appId }: Props = $props();

	type Provider = keyof WebhookUrls;

	const MIN_SECRET_LENGTH = 16;

	let config = $state<WebhookConfig | null>(null);
	let loading = $state(true);
	let error = $state('');
	let success = $state('');
	let provider = $state<Provider>('github');
	let copied = $state<string | null>(null);
	let confirmingRotate = $state(false);
	let rotating = $state(false);
	let customSecret = $state('');
	let savingCustom = $state(false);
	/** Plaintext secret, only after an explicit reveal or a rotation */
	let revealedSecret = $state<string | null>(null);

	async function handleReveal(): Promise<string> {
		return (await revealWebhookSecret(appId)).secret;
	}

	let origin = $derived(typeof window !== 'undefined' ? window.location.origin : '');

	/** Absolute URL for a provider: server-built when the dashboard domain is set. */
	function webhookUrl(cfg: WebhookConfig, p: Provider): string {
		return cfg.urls?.[p] ?? `${origin}${cfg.paths[p]}`;
	}

	let currentUrl = $derived(config ? webhookUrl(config, provider) : '');

	async function load() {
		loading = true;
		error = '';
		try {
			config = await getWebhookConfig(appId);
		} catch (e: any) {
			error = e.message ?? 'Failed to load webhook settings';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void appId;
		void load();
	});

	async function handleCopy(key: string, text: string) {
		try {
			await navigator.clipboard.writeText(text);
			copied = key;
			setTimeout(() => {
				if (copied === key) copied = null;
			}, 2000);
		} catch {
			copied = null;
		}
	}

	async function handleRotate() {
		error = '';
		success = '';
		rotating = true;
		try {
			revealedSecret = (await rotateWebhookSecret(appId)).secret;
			success =
				'New secret generated and shown below. Copy it into your git provider now — the old one no longer works.';
			confirmingRotate = false;
		} catch (e: any) {
			error = e.message ?? 'Failed to rotate the webhook secret';
		} finally {
			rotating = false;
		}
	}

	async function handleSetCustomSecret() {
		error = '';
		success = '';
		const secret = customSecret.trim();
		if (secret.length < MIN_SECRET_LENGTH) {
			error = `The secret must be at least ${MIN_SECRET_LENGTH} characters.`;
			return;
		}
		savingCustom = true;
		try {
			config = await setWebhookSecret(appId, secret);
			customSecret = '';
			revealedSecret = null;
			success = 'Webhook secret updated.';
		} catch (e: any) {
			error = e.message ?? 'Failed to update the webhook secret';
		} finally {
			savingCustom = false;
		}
	}

	const providers: { value: Provider; label: string }[] = [
		{ value: 'github', label: 'GitHub' },
		{ value: 'gitlab', label: 'GitLab' },
		{ value: 'gitea', label: 'Gitea / Forgejo' },
		{ value: 'generic', label: 'CI / Generic' }
	];

	let curlExample = $derived(
		`curl -fsS -X POST "${currentUrl}" \\\n` +
			`  -H "Authorization: Bearer $TOND_WEBHOOK_SECRET" \\\n` +
			`  -H "X-Tond-Delivery: $GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT"`
	);
</script>

<div class="grid gap-4" data-testid="app-webhooks">
	<div class="flex items-start justify-between gap-4">
		<div>
			<h3 class="flex items-center gap-2 text-sm font-medium">
				<ShieldCheck class="size-4 text-success" />
				Webhooks
			</h3>
			<p class="mt-1 text-sm text-muted-foreground">
				Every webhook must be signed with this app's secret. Unsigned or wrongly signed requests are
				rejected, and each delivery is accepted only once.
			</p>
		</div>
	</div>

	{#if error}
		<div
			class="flex items-start justify-between gap-3 rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive"
		>
			<span>{error}</span>
			<button type="button" class="text-xs underline" onclick={() => (error = '')}>Dismiss</button>
		</div>
	{/if}
	{#if success}
		<p class="text-sm text-success">{success}</p>
	{/if}

	{#if loading}
		<p class="text-xs text-muted-foreground">Loading webhook settings…</p>
	{:else if config}
		<Tabs.Root bind:value={provider}>
			<Tabs.List class="flex-wrap">
				{#each providers as p (p.value)}
					<Tabs.Trigger value={p.value}>{p.label}</Tabs.Trigger>
				{/each}
			</Tabs.List>
		</Tabs.Root>

		<!-- URL + secret for the selected provider -->
		<div class="grid gap-3 rounded-md border p-4">
			<div class="grid gap-2">
				<Label for="webhook-url">
					{provider === 'gitlab' ? 'URL' : provider === 'gitea' ? 'Target URL' : 'Payload URL'}
				</Label>
				<div class="flex gap-2">
					<Input id="webhook-url" readonly value={currentUrl} class="font-mono text-xs" />
					<Button
						variant="outline"
						size="sm"
						class="h-9 shrink-0"
						onclick={() => handleCopy('url', currentUrl)}
						aria-label="Copy webhook URL"
					>
						{#if copied === 'url'}
							<Check class="size-4" />
						{:else}
							<Copy class="size-4" />
						{/if}
					</Button>
				</div>
				{#if !config.urls}
					<p class="text-xs text-muted-foreground">
						Built from this browser's address. Set a dashboard domain in Settings so the URL is
						reachable from your git provider.
					</p>
				{/if}
			</div>

			<div class="grid gap-2">
				<Label for="webhook-secret-value">
					{provider === 'gitlab' ? 'Secret token' : 'Secret'}
				</Label>
				<RevealableSecret
					id="webhook-secret-value"
					reveal={handleReveal}
					canReveal={isAdmin()}
					bind:value={revealedSecret}
					lockedHint="Only admins can reveal the secret. Rotate it to get a new one you can copy."
				/>
			</div>
		</div>

		<!-- Provider-specific setup steps -->
		<div class="rounded-md border p-4 text-sm text-muted-foreground">
			{#if provider === 'github'}
				<ol class="list-decimal space-y-1 pl-5">
					<li>Open your repository → <strong>Settings → Webhooks → Add webhook</strong>.</li>
					<li>Paste the <strong>Payload URL</strong> above.</li>
					<li>
						Set <strong>Content type</strong> to
						<code class="font-mono text-xs">application/json</code>.
					</li>
					<li>Paste the <strong>Secret</strong> above.</li>
					<li>
						Choose <strong>Just the push event</strong> and click <strong>Add webhook</strong>.
					</li>
				</ol>
				<p class="mt-3 text-xs">
					Using the Tond GitHub App (Integrations → GitHub)? It delivers push and pull request
					events for linked apps on its own — no per-repository webhook needed.
				</p>
			{:else if provider === 'gitlab'}
				<ol class="list-decimal space-y-1 pl-5">
					<li>Open your project → <strong>Settings → Webhooks → Add new webhook</strong>.</li>
					<li>Paste the <strong>URL</strong> above.</li>
					<li>Paste the secret above into <strong>Secret token</strong>.</li>
					<li>
						Under <strong>Trigger</strong>, select <strong>Push events</strong> (and
						<strong>Merge request events</strong> for preview deployments).
					</li>
					<li>
						Keep <strong>Enable SSL verification</strong> on and click <strong>Add webhook</strong>.
					</li>
				</ol>
			{:else if provider === 'gitea'}
				<ol class="list-decimal space-y-1 pl-5">
					<li>
						Open your repository → <strong>Settings → Webhooks → Add webhook</strong> and pick
						<strong>Gitea</strong> (or <strong>Forgejo</strong>).
					</li>
					<li>
						Paste the <strong>Target URL</strong> above; keep the method <strong>POST</strong>.
					</li>
					<li>
						Set <strong>POST content type</strong> to
						<code class="font-mono text-xs">application/json</code>.
					</li>
					<li>Paste the <strong>Secret</strong> above.</li>
					<li>Trigger on <strong>Push events</strong> and click <strong>Add webhook</strong>.</li>
				</ol>
			{:else}
				<p>
					Call the URL from a CI job after your pipeline passes. Store the secret as a CI secret
					(e.g. <code class="font-mono text-xs">TOND_WEBHOOK_SECRET</code>) and send it as a bearer
					token — or sign the body with HMAC-SHA256 in
					<code class="font-mono text-xs">X-Tond-Signature-256: sha256=&lt;hex&gt;</code>. A unique
					<code class="font-mono text-xs">X-Tond-Delivery</code> per run blocks replays.
				</p>
				<div class="relative mt-3">
					<pre
						class="overflow-x-auto rounded-md bg-muted p-3 pr-10 font-mono text-xs leading-relaxed whitespace-pre">{curlExample}</pre>
					<Button
						variant="ghost"
						size="sm"
						class="absolute top-1 right-1 h-7 px-2"
						onclick={() => handleCopy('curl', curlExample)}
						aria-label="Copy command"
					>
						{#if copied === 'curl'}
							<Check class="size-4" />
						{:else}
							<Copy class="size-4" />
						{/if}
					</Button>
				</div>
			{/if}
			<p class="mt-3 text-xs">
				Deploys only run when <strong>Auto-deploy</strong> is enabled below. Webhook deploys are rate-limited
				per app.
			</p>
		</div>

		<!-- Rotate / custom secret -->
		<div class="grid gap-3">
			<div class="flex flex-wrap items-center gap-2">
				{#if confirmingRotate}
					<span class="text-sm text-muted-foreground">
						The current secret stops working immediately.
					</span>
					<Button size="sm" variant="destructive" disabled={rotating} onclick={handleRotate}>
						{rotating ? 'Rotating…' : 'Rotate now'}
					</Button>
					<Button size="sm" variant="outline" onclick={() => (confirmingRotate = false)}>
						Cancel
					</Button>
				{:else}
					<Button size="sm" variant="outline" onclick={() => (confirmingRotate = true)}>
						<Refresh class="size-4" />
						Rotate secret
					</Button>
				{/if}
			</div>

			<details class="text-sm">
				<summary class="cursor-pointer text-muted-foreground">Use your own secret</summary>
				<div class="mt-3 flex gap-3">
					<div class="grid flex-1 gap-2">
						<Label for="webhook-custom-secret">Custom secret</Label>
						<SecretInput
							id="webhook-custom-secret"
							bind:value={customSecret}
							placeholder="At least {MIN_SECRET_LENGTH} characters"
							copyable={false}
						/>
					</div>
					<Button
						class="self-end"
						disabled={savingCustom || customSecret.trim().length === 0}
						onclick={handleSetCustomSecret}
					>
						{savingCustom ? 'Saving…' : 'Save'}
					</Button>
				</div>
			</details>
		</div>
	{/if}
</div>
