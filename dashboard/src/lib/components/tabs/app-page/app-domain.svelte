<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import CertificateStatus from '$lib/components/domains/certificate-status.svelte';
	import AlertIcon from '~icons/ri/alert-line';

	interface Props {
		domainInput: string;
		/** The domain currently saved on the app (drives the certificate status). */
		savedDomain?: string | null;
		/** Server warning for the saved domain (same site as the dashboard). */
		domainWarning?: string | null;
		/** Settings' apps domain: apps without a custom domain get <app>.<appsDomain>. */
		appsDomain?: string;
		appName: string;
		/** Bumped after a save so the certificate status re-fetches. */
		certRefreshKey?: number;
		/** Save the domain; rejects with the server's message. */
		onUpdate: () => Promise<void>;
	}

	let {
		domainInput = $bindable(''),
		savedDomain = null,
		domainWarning = null,
		appsDomain = '',
		appName,
		certRefreshKey = 0,
		onUpdate
	}: Props = $props();

	let saving = $state(false);
	let error = $state('');
	let saved = $state(false);

	let unchanged = $derived(
		domainInput.trim().toLowerCase().replace(/\.$/, '') === (savedDomain ?? '')
	);
	let automaticHost = $derived(appsDomain ? `${appName}.${appsDomain}` : '');

	async function handleSubmit(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		error = '';
		saved = false;
		try {
			await onUpdate();
			saved = true;
			setTimeout(() => (saved = false), 3000);
		} catch (err: any) {
			error = err.message ?? 'Could not update the domain';
		} finally {
			saving = false;
		}
	}
</script>

<div class="space-y-4 pt-4">
	<p class="max-w-2xl text-sm text-muted-foreground">
		Set a custom domain for this app. Point its A (and AAAA, if you use IPv6) record at this server;
		Tond then requests a Let's Encrypt certificate automatically.
		{#if automaticHost}
			Without one, the app is served at <span class="font-mono text-xs text-foreground"
				>{automaticHost}</span
			>.
		{:else}
			No apps domain is set in Settings, so the app has no address until you give it one.
		{/if}
	</p>
	<form class="flex flex-col gap-3 sm:flex-row" onsubmit={handleSubmit}>
		<div class="grid flex-1 gap-2">
			<Label for="domain-input">Domain</Label>
			<Input
				id="domain-input"
				type="text"
				bind:value={domainInput}
				placeholder="app.example.com"
				disabled={saving}
			/>
		</div>
		<Button type="submit" class="sm:self-end" disabled={saving || !domainInput.trim() || unchanged}>
			{saving ? 'Saving…' : 'Update Domain'}
		</Button>
	</form>

	{#if error}
		<div
			class="flex items-start justify-between gap-3 rounded bg-destructive/10 px-3 py-2 text-sm text-destructive"
			role="alert"
		>
			<p>{error}</p>
			<button
				type="button"
				class="shrink-0 text-xs underline-offset-2 hover:underline"
				onclick={() => (error = '')}
			>
				Dismiss
			</button>
		</div>
	{:else if saved}
		<p class="text-xs text-success" role="status">Domain saved. The certificate is on its way.</p>
	{/if}

	{#if domainWarning}
		<div
			class="flex max-w-2xl items-start gap-2 rounded border border-warn/40 bg-warn/10 px-3 py-2 text-sm"
			role="note"
		>
			<AlertIcon class="mt-0.5 size-4 shrink-0 text-warn" />
			<div>
				<p class="font-medium text-warn">Same site as your dashboard</p>
				<p class="text-xs text-muted-foreground">{domainWarning}</p>
			</div>
		</div>
	{/if}

	<CertificateStatus domain={savedDomain} refreshKey={certRefreshKey} />
</div>
